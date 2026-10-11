//! Taskleisten-Identitaet eines Proxy-Fensters: eigene AppUserModelID je `app` (eigene Gruppe in der
//! Taskleiste, z. B. "Finanzen" getrennt von anderen Mac-Apps) und das App-Icon aus `icon_png`.

use windows::core::HSTRING;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::Storage::EnhancedStorage::PKEY_AppUserModel_ID;
use windows::Win32::System::Com::StructuredStorage::{PropVariantClear, PROPVARIANT};
use windows::Win32::System::Variant::VT_LPWSTR;
use windows::Win32::UI::Shell::PropertiesSystem::{IPropertyStore, SHGetPropertyStoreForWindow};
use windows::Win32::UI::Shell::SHStrDupW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateIconFromResourceEx, DestroyIcon, GetSystemMetrics, SendMessageW, HICON, ICON_BIG,
    ICON_SMALL, LR_DEFAULTCOLOR, SM_CXICON, SM_CXSMICON, WM_SETICON,
};

/// AppUserModelID fuer eine Mac-App: "Beamer.Teleport.<App>" (nur Buchstaben, Ziffern, Punkt; max. 128).
pub fn app_user_model_id(app: &str) -> String {
    let mut clean: String = app
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    if clean.trim_matches('_').is_empty() {
        clean = "App".into();
    }
    let mut id = format!("Beamer.Teleport.{clean}");
    id.truncate(128);
    id
}

/// Setzt die AppUserModelID des Fensters (Fehler werden nur geloggt: kosmetisch).
pub fn set_app_id(hwnd: HWND, app: &str) {
    let id = app_user_model_id(app);
    unsafe {
        let store: IPropertyStore = match SHGetPropertyStoreForWindow(hwnd) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("beamer-teleport: AppUserModelID: {e}");
                return;
            }
        };
        let Ok(text) = SHStrDupW(&HSTRING::from(id.as_str())) else {
            return;
        };
        let mut pv = PROPVARIANT::default();
        (*pv.Anonymous.Anonymous).vt = VT_LPWSTR;
        (*pv.Anonymous.Anonymous).Anonymous.pwszVal = text;
        if let Err(e) = store
            .SetValue(&PKEY_AppUserModel_ID, &pv)
            .and_then(|_| store.Commit())
        {
            eprintln!("beamer-teleport: AppUserModelID {id}: {e}");
        }
        let _ = PropVariantClear(&mut pv);
    }
}

/// Icons aus PNG-Daten (Vista+ versteht PNG direkt als Icon-Ressource), gross und klein.
pub struct Icons {
    big: HICON,
    small: HICON,
}

impl Icons {
    pub fn from_png(png: &[u8]) -> Option<Icons> {
        if png.len() < 8 || &png[..8] != b"\x89PNG\r\n\x1a\n" {
            return None;
        }
        unsafe {
            let make = |size: i32| {
                CreateIconFromResourceEx(png, true, 0x0003_0000, size, size, LR_DEFAULTCOLOR).ok()
            };
            let big = make(GetSystemMetrics(SM_CXICON))?;
            let small = make(GetSystemMetrics(SM_CXSMICON)).unwrap_or(big);
            Some(Icons { big, small })
        }
    }

    pub fn apply(&self, hwnd: HWND) {
        unsafe {
            SendMessageW(
                hwnd,
                WM_SETICON,
                Some(WPARAM(ICON_BIG as usize)),
                Some(LPARAM(self.big.0 as isize)),
            );
            SendMessageW(
                hwnd,
                WM_SETICON,
                Some(WPARAM(ICON_SMALL as usize)),
                Some(LPARAM(self.small.0 as isize)),
            );
        }
    }
}

impl Drop for Icons {
    fn drop(&mut self) {
        unsafe {
            if self.small != self.big {
                let _ = DestroyIcon(self.small);
            }
            let _ = DestroyIcon(self.big);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_clean_and_distinct() {
        assert_eq!(app_user_model_id("Finanzen"), "Beamer.Teleport.Finanzen");
        assert_eq!(
            app_user_model_id("Mail & Co."),
            "Beamer.Teleport.Mail___Co_"
        );
        assert_eq!(app_user_model_id(""), "Beamer.Teleport.App");
        assert_ne!(app_user_model_id("Safari"), app_user_model_id("Finanzen"));
        assert!(app_user_model_id(&"x".repeat(500)).len() <= 128);
    }

    #[test]
    fn icon_from_png() {
        // 1x1-PNG (rot): wird zu einem Icon; kein PNG -> None.
        let png: &[u8] = &[
            0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48,
            0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00,
            0x00, 0x90, 0x77, 0x53, 0xDE, 0x00, 0x00, 0x00, 0x0C, 0x49, 0x44, 0x41, 0x54, 0x08,
            0xD7, 0x63, 0xF8, 0xCF, 0xC0, 0x00, 0x00, 0x03, 0x01, 0x01, 0x00, 0x18, 0xDD, 0x8D,
            0xB0, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
        ];
        assert!(Icons::from_png(png).is_some());
        assert!(Icons::from_png(b"kein png").is_none());
    }
}
