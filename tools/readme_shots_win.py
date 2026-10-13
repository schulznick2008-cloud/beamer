"""The README's Windows screenshots: the real settings window, built offscreen from a synthetic
config (documentation addresses, no house detail) and grabbed page by page. Starts nothing: no
receiver, no hooks, no announcer.

APPEARANCE=light|dark (default dark) is the palette the window is built in. SWITCH_TO=light|dark,
set as well, has the window built in APPEARANCE and then live-switched with _apply_appearance
before anything is shot -- proving the live switch renders the same as building in the target
appearance directly, not just the initial build."""

import json
import os
import sys
import tempfile
import time
from pathlib import Path

if len(sys.argv) != 3 or (sys.argv[1] != "--linux" and not Path(sys.argv[1]).is_dir()):
    raise SystemExit("Usage: readme_shots_win.py <repository-dir> <outdir> | --linux <outdir>")

LINUX = sys.argv[1] == "--linux"
if LINUX:
    REPO = Path(__file__).resolve().parents[1]
    OUT = Path(sys.argv[2])
else:
    REPO = Path(sys.argv[1])
    OUT = Path(sys.argv[2])
sys.path.insert(0, str(REPO / "win_app"))

from PySide6.QtWidgets import QApplication  # noqa: E402

import theme  # noqa: E402

app = QApplication(sys.argv[:1])
theme.init_fonts()
appearance = os.environ.get("APPEARANCE", "dark").strip().lower()
app.setFont(theme.font(theme.TYPE["body"]))

import kvm_bridge_win  # noqa: E402
import app_config  # noqa: E402
from core.receiver import ServerState  # noqa: E402
from core import pairing, peerlist  # noqa: E402

kvm_bridge_win.VERSION = os.environ.get("SHOT_VERSION", kvm_bridge_win.VERSION)
# Drawn as the installed app is, not as a run from source, which cannot start at sign-in.
kvm_bridge_win.autostart_win.installed_exe = lambda: Path("Beamer.exe")
kvm_bridge_win.autostart_win.is_enabled = lambda: not LINUX
folder = Path(tempfile.mkdtemp())
fixture = {
    "host": "192.168.1.20", "port": 24820, "auth_token": "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8", "paired_with": "MacBook Pro",
    "mac_host": "192.168.1.10", "mac_return_edge": "left", "glow_colour": "signal",
    "ignored_inputs": ["button:back", "button:forward", "key:175", "key:174"],
    "appearance": appearance,
}
if LINUX:
    fixture["paired_with"] = ""
    fixture["peers"] = [pairing.peer_entry(
        bytes([1]) * 16, "MacBook Pro", "macos", fixture["port"], fixture["auth_token"],
        fixture["mac_host"], 1791321600,
    )]
if LINUX:
    settings = app_config.migrate(fixture)
    peerlist.add_peer(settings, fixture["peers"][0], settings["peers"][0])
    app_config.write_settings(folder / "settings.json", settings)
else:
    (folder / "config.json").write_text(json.dumps(fixture))
window = kvm_bridge_win.WindowsApplication(folder / "settings.json")
window._set_status(ServerState.CONNECTED, "Connected to 192.168.1.10")
if LINUX:
    window.sender.links.up = lambda _peer: True
# This PC's own link to the Mac, as the Mac tool's CONNECTED fakes its socket: without it Overview
# says "not connected" under "Mac connected".
window.sender._sock = object()
window.resize(1200, 675) if LINUX else window.resize(820, 700)
window.show()
# _set_status only takes effect on the next _refresh_window, whose status heading then cross-fades
# in: forced and settled here, before anything is timed, so that fade never lands mid-shot.
window._refresh_window()
started = time.monotonic()
while time.monotonic() - started < 0.25:
    app.processEvents()
    time.sleep(0.01)

switch_to = os.environ.get("SWITCH_TO", "").strip().lower()
if switch_to:
    started = time.monotonic()
    while time.monotonic() - started < 0.1:
        app.processEvents()
        time.sleep(0.01)
    window._apply_appearance(switch_to)
    # Past the 200ms cross-fade (motion.DURATION), so the switch has fully settled.
    started = time.monotonic()
    while time.monotonic() - started < 0.5:
        app.processEvents()
        time.sleep(0.01)

OUT.mkdir(parents=True, exist_ok=True)
# PAGES names pages, space-separated; the README's three by default.
pages = os.environ.get("PAGES", "overview crossing design" if LINUX else "overview design keyboard").split()
for key in pages:
    settle = 0.3
    window._select_page(key)
    window.stack.setFocus()
    started = time.monotonic()
    while time.monotonic() - started < settle:
        app.processEvents()
        time.sleep(0.01)
    window._refresh_window()
    app.processEvents()
    prefix = "linux" if LINUX else "windows"
    window.grab().save(str(OUT / f"{prefix}-{key}{os.environ.get('SUFFIX', '')}.png"))
print(OUT)
