#!/usr/bin/env python3
"""Erzeugt teleport/win/src/keytable_gen.rs aus den Tastentabellen von Beamer, damit der Windows-Helfer
dieselben Namen benutzt wie die Python-Seite und nichts neu erfunden wird (Muster wie
tools/teleport/gen_swift_keycodes.py fuer den Mac).

Quellen (per `ast` gelesen, ohne Windows-Module zu importieren):
- `win_app/capture_win.py`: `VK_TO_NAME` (virtuelle Taste -> Beamer-Name) und die F-Tasten-Schleifen.
  Diese Tabelle setzt die Semantik schon beim Erfassen um (Strg -> "cmd", Win -> "ctrl").
- `core/keytable.py`: `_PC_SEMANTIC` (genau diese Umsetzung) und `SPEC_NAMES` (gueltige Namen).

Der Proxy meldet Tasten **roh** (SCHNITTSTELLEN.md §3: die Cmd/Strg-Umsetzung macht Python), also mit den
physischen Namen von `core/keytable.py`: "ctrl" ist die Strg-Taste, "cmd" die Windows-Taste. Dafuer wird
`_PC_SEMANTIC` (eine Vertauschung, also ihr eigenes Inverses) wieder angewendet.

  tools/teleport/gen_rust_keytable.py            # schreibt die Datei
  tools/teleport/gen_rust_keytable.py --check    # Exit 1, wenn die Datei veraltet ist (CI)
"""
import ast
import os
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
OUT = os.path.join(ROOT, "teleport", "win", "src", "keytable_gen.rs")


def module_literals(path, names):
    tree = ast.parse(open(path, encoding="utf-8").read())
    found = {}
    for node in tree.body:
        target = None
        if isinstance(node, ast.Assign) and len(node.targets) == 1 and isinstance(node.targets[0], ast.Name):
            target, value = node.targets[0].id, node.value
        elif isinstance(node, ast.AnnAssign) and isinstance(node.target, ast.Name) and node.value is not None:
            target, value = node.target.id, node.value
        if target in names:
            try:
                found[target] = ast.literal_eval(value)
            except ValueError:
                found[target] = value
    missing = set(names) - set(found)
    if missing:
        raise SystemExit(f"nicht gefunden in {path}: {sorted(missing)}")
    return found


def fkey_loops(path):
    """Die Schleifen `for _index in range(a, b): VK_TO_NAME[base + (_index - a)] = f"f{_index}"`."""
    tree = ast.parse(open(path, encoding="utf-8").read())
    extra = {}
    for node in tree.body:
        if not (isinstance(node, ast.For) and isinstance(node.iter, ast.Call)):
            continue
        if getattr(node.iter.func, "id", None) != "range" or len(node.body) != 1:
            continue
        stmt = node.body[0]
        if not (isinstance(stmt, ast.Assign) and isinstance(stmt.targets[0], ast.Subscript)):
            continue
        if getattr(stmt.targets[0].value, "id", None) != "VK_TO_NAME":
            continue
        start, stop = (ast.literal_eval(a) for a in node.iter.args)
        var = node.target.id
        for i in range(start, stop):
            vk = eval(compile(ast.Expression(stmt.targets[0].slice), path, "eval"), {var: i})
            name = eval(compile(ast.Expression(stmt.value), path, "eval"), {var: i})
            extra[vk] = name
    if len(extra) < 20:
        raise SystemExit(f"F-Tasten-Schleifen in {path} nicht erkannt ({len(extra)} Eintraege)")
    return extra


def tables():
    cap = os.path.join(ROOT, "win_app", "capture_win.py")
    kt = os.path.join(ROOT, "core", "keytable.py")
    vk_to_name = dict(module_literals(cap, ["VK_TO_NAME"])["VK_TO_NAME"])
    vk_to_name.update(fkey_loops(cap))
    lit = module_literals(kt, ["_PC_SEMANTIC", "SPEC_NAMES"])
    pc_semantic = lit["_PC_SEMANTIC"]
    spec = lit["SPEC_NAMES"]
    if not isinstance(spec, (set, frozenset)):
        # frozenset({...}) | {...}: die Namen stehen als Literale im Ausdruck, F-Tasten 1-24 ergaenzen
        names = set()
        for n in ast.walk(spec):
            if isinstance(n, ast.Constant) and isinstance(n.value, str) and not n.value.startswith("f"):
                names.add(n.value)
        names |= {f"f{i}" for i in range(1, 25)}
        spec = names
    # Vertauschung rueckgaengig machen: physischer Name (Strg = "ctrl", Win = "cmd").
    physical = {vk: pc_semantic.get(name, name) for vk, name in vk_to_name.items()}
    bad = sorted({n for n in physical.values() if n not in spec})
    if bad:
        raise SystemExit(f"Namen nicht in core/keytable.SPEC_NAMES: {bad}")
    return physical


def render(physical):
    o = [
        "// GENERIERT von tools/teleport/gen_rust_keytable.py aus win_app/capture_win.py (VK_TO_NAME) und",
        "// core/keytable.py (_PC_SEMANTIC, SPEC_NAMES). Nicht von Hand aendern; `--check` prueft, ob sie aktuell ist.",
        "",
        "/// Virtuelle Windows-Taste -> physischer Beamer-Tastenname (\"ctrl\" = Strg-Taste, \"cmd\" = Windows-Taste).",
        "/// Tasten, die hier fehlen, sind Zeichentasten: ihr Name ist das Zeichen (US-unabhaengig, siehe input.rs).",
        "pub const VK_NAMES: &[(u32, &str)] = &[",
    ]
    for vk in sorted(physical):
        o.append(f'    (0x{vk:02X}, "{physical[vk]}"),')
    o.append("];")
    o.append("")
    return "\n".join(o)


def main():
    text = render(tables())
    if "--check" in sys.argv:
        current = open(OUT, encoding="utf-8").read() if os.path.exists(OUT) else ""
        if current.replace("\r\n", "\n") != text:
            print("keytable_gen.rs ist veraltet: tools/teleport/gen_rust_keytable.py ausfuehren", file=sys.stderr)
            return 1
        return 0
    with open(OUT, "w", encoding="utf-8", newline="\n") as f:
        f.write(text)
    print("geschrieben:", OUT)
    return 0


if __name__ == "__main__":
    sys.exit(main())
