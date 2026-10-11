#!/usr/bin/env python3
"""Erzeugt teleport/mac/Sources/TeleportCore/KeyTables.swift aus den Tastentabellen von Beamer
(`mac_app/key_codes.py` und den Konstanten in `mac_app/input_injector_mac.py`), damit der Mac-Helfer dieselben
Namen und Tastencodes benutzt wie die Python-Seite und nichts neu erfunden wird.

  tools/teleport/gen_swift_keycodes.py            # schreibt die Datei
  tools/teleport/gen_swift_keycodes.py --check    # Exit 1, wenn die Datei veraltet ist (CI)

Zusaetzlich vergleicht ein Swift-Test (`KeyTablesTests`) die Swift-Tabellen mit dem Python-Quelltext.
"""
import ast
import os
import runpy
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
OUT = os.path.join(ROOT, "teleport", "mac", "Sources", "TeleportCore", "KeyTables.swift")


def literal_assignments(path, names):
    tree = ast.parse(open(path, encoding="utf-8").read())
    found = {}
    for node in tree.body:
        if isinstance(node, ast.Assign) and len(node.targets) == 1 and isinstance(node.targets[0], ast.Name):
            if node.targets[0].id in names:
                found[node.targets[0].id] = ast.literal_eval(node.value)
    missing = set(names) - set(found)
    if missing:
        raise SystemExit(f"nicht gefunden in {path}: {sorted(missing)}")
    return found


def swift_str(s):
    return '"' + s.replace("\\", "\\\\").replace('"', '\\"') + '"'


def render():
    kc = runpy.run_path(os.path.join(ROOT, "mac_app", "key_codes.py"))
    special = kc["SPECIAL_KEY_NAMES"]
    printable = kc["PRINTABLE_KEY_FALLBACKS"]
    modifier_codes = kc["MODIFIER_KEY_CODES"]
    inj = literal_assignments(
        os.path.join(ROOT, "mac_app", "input_injector_mac.py"),
        ["MODIFIER_FLAG_NAMES", "WINDOWS_ONLY_KEY_CODES", "MEDIA_NX_KEYS", "BROWSER_KEY_CHORDS", "CHORD_MODIFIERS"],
    )
    o = []
    o.append("// GENERIERT von tools/teleport/gen_swift_keycodes.py aus mac_app/key_codes.py und")
    o.append("// mac_app/input_injector_mac.py. Nicht von Hand aendern; `--check` prueft, ob sie aktuell ist.")
    o.append("")
    o.append("/// Beamers Tastennamen und macOS-Tastencodes (dieselben Tabellen wie die Python-Seite).")
    o.append("public enum KeyTables {")
    o.append("    /// Sondertasten: Tastencode zu Beamer-Name (`SPECIAL_KEY_NAMES`). Mehrere Codes koennen denselben Namen haben.")
    o.append("    public static let specialKeyNames: [UInt16: String] = [")
    for code in sorted(special):
        o.append(f"        0x{code:02X}: {swift_str(special[code])},")
    o.append("    ]")
    o.append("")
    o.append("    /// Name zu Tastencode; bei doppelten Namen gilt der kleinste Code (`KEY_NAME_TO_CODE.setdefault`).")
    o.append("    public static let keyNameToCode: [String: UInt16] = [")
    seen = {}
    for code in sorted(special):
        seen.setdefault(special[code], code)
    for name in sorted(seen):
        o.append(f"        {swift_str(name)}: 0x{seen[name]:02X},")
    o.append("    ]")
    o.append("")
    o.append("    public static let modifierKeyCodes: Set<UInt16> = [" + ", ".join(f"0x{c:02X}" for c in sorted(modifier_codes)) + "]")
    o.append("")
    o.append("    /// US-Tastatur: Tastencode zu unverschobenem Zeichen (`PRINTABLE_KEY_FALLBACKS`).")
    o.append("    public static let printableFallbacks: [UInt16: String] = [")
    for code in sorted(printable):
        o.append(f"        0x{code:02X}: {swift_str(printable[code])},")
    o.append("    ]")
    o.append("")
    o.append("    /// Tasten, die ein PC hat und ein Mac nicht (`WINDOWS_ONLY_KEY_CODES`).")
    o.append("    public static let windowsOnlyKeyCodes: [String: UInt16] = [")
    for name in sorted(inj["WINDOWS_ONLY_KEY_CODES"]):
        o.append(f"        {swift_str(name)}: 0x{inj['WINDOWS_ONLY_KEY_CODES'][name]:02X},")
    o.append("    ]")
    o.append("")
    o.append("    /// Modifier-Namen zum Flag (`MODIFIER_FLAG_NAMES`, ohne das Praefix kCGEventFlagMask).")
    o.append("    public static let modifierFlagNames: [String: String] = [")
    for name in sorted(inj["MODIFIER_FLAG_NAMES"]):
        flag = inj["MODIFIER_FLAG_NAMES"][name].replace("kCGEventFlagMask", "")
        o.append(f"        {swift_str(name)}: {swift_str(flag)},")
    o.append("    ]")
    o.append("")
    o.append("    /// Modifier, die einen Tastendruck zum Kuerzel machen (`CHORD_MODIFIERS`).")
    o.append("    public static let chordModifiers: Set<String> = [" + ", ".join(swift_str(n) for n in sorted(inj["CHORD_MODIFIERS"])) + "]")
    o.append("")
    o.append("    /// Medientasten (NX-Tastencodes aus IOKit `ev_keymap.h`, `MEDIA_NX_KEYS`).")
    o.append("    public static let mediaNXKeys: [String: Int] = [")
    for name in sorted(inj["MEDIA_NX_KEYS"]):
        o.append(f"        {swift_str(name)}: {inj['MEDIA_NX_KEYS'][name]},")
    o.append("    ]")
    o.append("")
    o.append("    /// Browser-Vor/Zurueck als Kuerzel (`BROWSER_KEY_CHORDS`).")
    o.append("    public static let browserKeyChords: [String: String] = [")
    for name in sorted(inj["BROWSER_KEY_CHORDS"]):
        o.append(f"        {swift_str(name)}: {swift_str(inj['BROWSER_KEY_CHORDS'][name])},")
    o.append("    ]")
    o.append("}")
    o.append("")
    return "\n".join(o)


def main():
    text = render()
    if "--check" in sys.argv:
        current = open(OUT, encoding="utf-8").read() if os.path.exists(OUT) else ""
        if current != text:
            print("KeyTables.swift ist veraltet: tools/teleport/gen_swift_keycodes.py ausfuehren", file=sys.stderr)
            return 1
        return 0
    with open(OUT, "w", encoding="utf-8") as f:
        f.write(text)
    print("geschrieben:", OUT)
    return 0


if __name__ == "__main__":
    sys.exit(main())
