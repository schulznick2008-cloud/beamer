//! Firewall-Pruefung (SCHNITTSTELLEN.md §7, Nachtrag C03): Das Kabelnetz stuft Windows als
//! "Oeffentlich" ein. Der Helfer aendert die Firewall nicht selbst; er prueft nur, ob eine Regel UDP
//! fuer dieses Programm im Profil "Oeffentlich" erlaubt, und schreibt sonst einen klaren Hinweis ins Log.

use windows::core::Interface;
use windows::Win32::NetworkManagement::WindowsFirewall::*;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, IDispatch, CLSCTX_INPROC_SERVER,
    COINIT_MULTITHREADED,
};
use windows::Win32::System::Ole::IEnumVARIANT;
use windows::Win32::System::Variant::{VariantClear, VARIANT, VT_DISPATCH};

pub const CABLE_REMOTES: &str = "169.254.0.0/16,192.168.77.0/24";

#[derive(Debug, Default, PartialEq)]
pub struct Findings {
    pub public_firewall_on: bool,
    /// Erlaubnis-Regel fuer dieses Programm (oder alle Programme), UDP/alle, eingehend, Profil Oeffentlich.
    pub public_allow: bool,
    /// Block-Regel fuer dieses Programm (entsteht z. B., wenn der Nutzer den Firewall-Dialog abbricht).
    pub block_rule: bool,
}

const NET_FW_IP_PROTOCOL_UDP_I: i32 = 17;
const NET_FW_IP_PROTOCOL_ANY_I: i32 = 256;

fn same_program(rule_app: &str, exe: &str) -> bool {
    rule_app.eq_ignore_ascii_case(exe)
}

/// Deckt die Regel die Kabel-Adressen ab? `*` oder beide Netze (in Windows-Schreibweise mit Maske).
pub fn covers_cable(remote: &str) -> bool {
    let r = remote.to_ascii_lowercase();
    if r.trim() == "*" || r.is_empty() {
        return true;
    }
    let has = |net: &str, masks: &[&str]| masks.iter().any(|m| r.contains(&format!("{net}/{m}")));
    has("169.254.0.0", &["255.255.0.0", "16"]) && has("192.168.77.0", &["255.255.255.0", "24"])
}

pub fn check(exe: &str) -> windows::core::Result<Findings> {
    unsafe {
        let policy: INetFwPolicy2 = CoCreateInstance(&NetFwPolicy2, None, CLSCTX_INPROC_SERVER)?;
        let mut f = Findings {
            public_firewall_on: policy
                .get_FirewallEnabled(NET_FW_PROFILE2_PUBLIC)?
                .as_bool(),
            ..Default::default()
        };
        let rules = policy.Rules()?;
        let en: IEnumVARIANT = rules._NewEnum()?.cast()?;
        loop {
            let mut v = [VARIANT::default()];
            let mut n = 0u32;
            if en.Next(&mut v, &mut n).is_err() || n == 0 {
                break;
            }
            let mut var = std::mem::take(&mut v[0]);
            let rule: Option<INetFwRule> = if var.vt() == VT_DISPATCH {
                IDispatch::try_from(&var).ok().and_then(|d| d.cast().ok())
            } else {
                None
            };
            let _ = VariantClear(&mut var);
            let Some(rule) = rule else { continue };
            if !rule.Enabled()?.as_bool() || rule.Direction()? != NET_FW_RULE_DIR_IN {
                continue;
            }
            let app = rule
                .ApplicationName()
                .map(|b| b.to_string())
                .unwrap_or_default();
            let mine = same_program(&app, exe);
            let proto = rule.Protocol()?;
            if proto != NET_FW_IP_PROTOCOL_UDP_I && proto != NET_FW_IP_PROTOCOL_ANY_I {
                continue;
            }
            let action = rule.Action()?;
            if mine && action == NET_FW_ACTION_BLOCK {
                f.block_rule = true;
            }
            let profiles = rule.Profiles()?;
            if (mine || app.is_empty())
                && action == NET_FW_ACTION_ALLOW
                && profiles & NET_FW_PROFILE2_PUBLIC.0 != 0
            {
                let remote = rule
                    .RemoteAddresses()
                    .map(|b| b.to_string())
                    .unwrap_or_default();
                if mine && covers_cable(&remote) {
                    f.public_allow = true;
                }
            }
        }
        Ok(f)
    }
}

/// Text fuer das Log; leer, wenn alles passt.
pub fn advice(f: &Findings, exe: &str, port: u16) -> Vec<String> {
    let mut v = Vec::new();
    if f.block_rule {
        v.push(format!(
            "Firewall: eine Block-Regel fuer {exe} ist aktiv; UDP-Port {port} ist dann gesperrt. \
             Regel in \"Windows Defender Firewall mit erweiterter Sicherheit\" loeschen oder deaktivieren."
        ));
    }
    if f.public_firewall_on && !f.public_allow {
        v.push(format!(
            "Firewall: keine Regel erlaubt UDP fuer {exe} im Profil \"Oeffentlich\". Ein Direktkabel \
             (Nicht identifiziertes Netzwerk) bekommt dann keine Medienpakete. Entweder das Netzwerkprofil \
             auf \"Privat\" stellen oder (als Administrator):"
        ));
        v.push(format!(
            "  New-NetFirewallRule -DisplayName \"Beamer Teleport (UDP, Kabel)\" -Direction Inbound \
             -Program \"{exe}\" -Protocol UDP -Action Allow -Profile Public -RemoteAddress {CABLE_REMOTES}"
        ));
    }
    v
}

/// Im Hintergrund pruefen, damit `transport.open` nicht wartet.
pub fn check_in_background(port: u16) {
    let _ = std::thread::Builder::new()
        .name("firewall".into())
        .spawn(move || {
            let exe = std::env::current_exe()
                .map(|p| p.display().to_string())
                .unwrap_or_default();
            unsafe {
                let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            }
            match check(&exe) {
                Ok(f) => {
                    for line in advice(&f, &exe, port) {
                        eprintln!("beamer-teleport: {line}");
                    }
                }
                Err(e) => eprintln!(
                    "beamer-teleport: Firewall-Regeln nicht lesbar ({e}). Fuer das Kabel muss UDP im Profil \
                     \"Oeffentlich\" fuer {CABLE_REMOTES} erlaubt sein."
                ),
            }
            unsafe { CoUninitialize() };
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cable_coverage() {
        assert!(covers_cable("*"));
        assert!(covers_cable(
            "169.254.0.0/255.255.0.0,192.168.77.0/255.255.255.0"
        ));
        assert!(covers_cable("LocalSubnet,169.254.0.0/16,192.168.77.0/24"));
        assert!(!covers_cable("169.254.0.0/255.255.0.0"));
        assert!(!covers_cable("LocalSubnet"));
    }

    #[test]
    fn advice_text() {
        let ok = Findings {
            public_firewall_on: true,
            public_allow: true,
            block_rule: false,
        };
        assert!(advice(&ok, "x.exe", 1).is_empty());
        let off = Findings::default();
        assert!(advice(&off, "x.exe", 1).is_empty());
        let missing = Findings {
            public_firewall_on: true,
            ..Default::default()
        };
        let a = advice(&missing, "C:\\b\\x.exe", 47001);
        assert!(a.iter().any(|l| l.contains("-Profile Public")));
        assert!(a.iter().any(|l| l.contains(CABLE_REMOTES)));
    }

    #[test]
    fn reads_rules_without_admin() {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        }
        // Darf ohne Admin-Rechte lesen; das Ergebnis haengt vom Rechner ab.
        let r = check("C:\\gibt\\es\\nicht.exe");
        assert!(r.is_ok(), "{r:?}");
        assert!(!r.unwrap().public_allow);
    }
}
