#!/usr/bin/env python3
"""Live-Abnahme mit echten Apps (Paket C04a): TextEdit, Safari (und Finanzen mit Screenshots), je Fenster-Rueckfall
und virtueller Monitor. Schickt Eingaben ueber den Helfer (`inject`) und prueft die Wirkung unabhaengig in der App
(AppleScript bzw. Seitentitel in Safari).

  tools/teleport/live_accept.py textedit [--virtual]
  tools/teleport/live_accept.py safari [--virtual]

Voraussetzung: Bildschirmaufnahme und Bedienungshilfen fuer das startende Programm. Die Zwischenablage wird gesichert und
am Ende zurueckgesetzt. TextEdit und Safari bekommen ein EIGENES neues Dokument bzw. Fenster, das am Ende wieder
geschlossen wird; bestehende Dokumente werden nicht angefasst.
"""
import argparse, json, os, subprocess, sys, time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import mac_inject_demo as m  # noqa: E402

MENU = m.MENU_ALLOWANCE


def osa(script):
    r = subprocess.run(["osascript", "-e", script], capture_output=True, text=True)
    return r.stdout.strip() if r.returncode == 0 else "ERR:" + r.stderr.strip()


def cgwins():
    out = subprocess.run(["/tmp/cgwin"], capture_output=True, text=True).stdout.splitlines()
    return [json.loads(l) for l in out if l.strip()]


def menu_open(owner):
    return [w for w in cgwins() if w["owner"] == owner and w["layer"] >= 101 and w["w"] > 40 and w["h"] > 40]


class Live:
    def __init__(self, wid, virtual, app, port=47040):
        self.virtual, self.app = virtual, app
        subprocess.run(["swiftc", "-O", os.path.join(os.path.dirname(os.path.abspath(__file__)), "cgwin.swift"), "-o", "/tmp/cgwin"], check=True)
        self.recv = subprocess.Popen([m.HELPER, "--selftest-recv", str(port), "--seconds", "300"],
                                     stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        self.h = m.Helper(m.HELPER)
        self.h.call("hello", beamer_version="live")
        assert self.h.call("transport.open", local_port=0, peer_host="127.0.0.1", peer_port=port)["ok"]
        self.wid = wid
        self.orig = [w for w in self.h.call("windows.list")["windows"] if w["wid"] == wid][0]
        r = self.h.call("send.start", tid=1, mode="window", wid=wid, codec="h264", max_fps=30, bitrate_kbps=20000,
                        keys=m.TEST_KEYS, virtual_display=virtual)
        if not r.get("ok"):
            raise SystemExit(f"send.start: {r}")
        self.W, self.H, self.scale = r["width"], r["height"], r["scale"]
        time.sleep(1.5)

    def px(self, x, y):
        off = MENU if self.virtual else 0
        return x * self.scale, (y + off) * self.scale

    def inj(self, **inp):
        r = self.h.call("inject", tid=1, input=inp)
        if not r.get("ok"):
            raise RuntimeError(f"inject {inp}: {r.get('error')}")
        time.sleep(0.05)

    def click(self, x, y, button="left", n=1):
        px, py = self.px(x, y)
        for i in range(1, n + 1):
            self.inj(k="down", x=px, y=py, b=button, n=i)
            self.inj(k="up", x=px, y=py, b=button)

    def move(self, x, y):
        px, py = self.px(x, y)
        self.inj(k="move", x=px, y=py)

    def scroll(self, x, y, dy, unit="line"):
        px, py = self.px(x, y)
        self.move(x, y)
        self.inj(k="scroll", x=px, y=py, dx=0.0, dy=float(dy), unit=unit)

    def key(self, name, *mods):
        for md in mods:
            self.inj(k="key", key=md, down=True)
        self.inj(k="key", key=name, down=True)
        self.inj(k="key", key=name, down=False)
        for md in reversed(mods):
            self.inj(k="key", key=md, down=False)

    def text(self, s):
        self.inj(k="text", text=s)

    def resize(self, w, h):
        t0 = time.time()
        r = self.h.call("send.resize", tid=1, w=float(w), h=float(h))
        return r, (time.time() - t0) * 1000

    def stop(self):
        r = self.h.call("send.stop", tid=1, restore=False)
        time.sleep(0.8)
        now = [w for w in self.h.call("windows.list")["windows"] if w["wid"] == self.wid]
        try:
            self.h.close()
        except Exception:
            pass
        self.recv.terminate()
        return r, now[0] if now else None


class Results:
    def __init__(self):
        self.rows = []

    def check(self, name, ok, detail=""):
        self.rows.append((name, bool(ok), detail))
        print(f"  {'✅' if ok else '❌'} {name}" + (f"  [{detail}]" if detail else ""), flush=True)


def clip_get():
    return subprocess.run(["pbpaste"], capture_output=True, text=True).stdout


def clip_set(s):
    subprocess.run(["pbcopy"], input=s, text=True)


# ---------------------------------------------------------------- TextEdit
def run_textedit(virtual):
    res = Results()
    saved_clip = clip_get()
    long_text = "\\n".join(f"Zeile {i:03d} Test" for i in range(1, 301))
    wid = osa(f'''tell application "TextEdit"
        activate
        set d to make new document
        set text of d to "{long_text}"
        set bounds of window 1 to {{200, 120, 1000, 720}}
        return id of window 1
    end tell''')
    if wid.startswith("ERR") or not wid.isdigit():
        print("TextEdit-Dokument nicht anlegbar:", wid)
        return res
    wid = int(wid)
    time.sleep(1.0)
    DOC = f"document of (first window whose id is {wid})"
    WIN = f"(first window whose id is {wid})"
    L = Live(wid, virtual, "TextEdit")
    try:
        win = L.orig
        print(f"Fenster {win['w']}x{win['h']} pt bei ({win['x']},{win['y']}), Stream {L.W}x{L.H} px, scale {L.scale}")
        cx = win["w"] / 2
        # Klick setzt den Cursor: zwei Klicks in verschiedener Hoehe, Marker tippen, Zeilen vergleichen.
        def marker_line(y):
            L.click(cx, y)
            L.text("#")
            time.sleep(0.25)
            t = osa(f'tell application "TextEdit" to get text of {DOC}')
            idx = t.find("#")
            line = t[:idx].count("\n") if idx >= 0 else -1
            L.key("backspace")
            time.sleep(0.15)
            return line
        l1, l2 = marker_line(120), marker_line(320)
        h_est = 200.0 / (l2 - l1) if l2 > l1 else 0
        res.check("Linksklick setzt den Cursor an die angeklickte Stelle", l2 > l1 and 12 <= h_est <= 22,
                  f"Zeile {l1} bei y=120, Zeile {l2} bei y=320 (Zeilenhoehe ~{h_est:.1f} pt)")
        # Doppelklick: Wort markieren, Cmd+C, Zwischenablage lesen
        clip_set("LEER")
        L.click(cx - 300 if cx > 350 else 60, 200, n=2)
        time.sleep(0.2)
        L.key("c", "cmd")
        time.sleep(0.3)
        c = clip_get().strip()
        res.check("Doppelklick markiert ein Wort", c in ("Zeile", "Test") or c.startswith("Zeile") and len(c) < 8, repr(c))
        # Rechtsklick -> Kontextmenue
        L.click(cx, 250, button="right")
        time.sleep(0.6)
        menus = menu_open("TextEdit")
        res.check("Rechtsklick oeffnet das Kontextmenue", bool(menus), f"{len(menus)} Menuefenster")
        L.key("esc")
        time.sleep(0.4)
        res.check("Escape schliesst das Kontextmenue", not menu_open("TextEdit"))
        # Scrollen
        def sb():
            wn = osa(f'tell application "TextEdit" to get name of {WIN}')
            v = osa(f'tell application "System Events" to tell process "TextEdit" to get value of scroll bar 2 of scroll area 1 of window "{wn}"')
            if v.startswith("ERR"):
                print("   (Scrollleiste:", v[:120], ")")
            try:
                return float(v.replace(",", "."))
            except ValueError:
                return None
        s0 = sb()
        L.scroll(cx, 300, -5)
        time.sleep(0.5)
        s1 = sb()
        L.scroll(cx, 300, 5)
        time.sleep(0.5)
        s2 = sb()
        res.check("Scrollen bewegt den Inhalt (dy=-5 Zeilen, dann +5 zurueck)",
                  s0 is not None and s1 is not None and s1 != s0 and s2 is not None and abs(s2 - s0) < abs(s1 - s0),
                  f"Scrollleiste {s0} -> {s1} -> {s2} (dy=-5 {'nach unten' if (s1 or 0) > (s0 or 0) else 'nach oben'})")
        # Tippen + Umlaute
        osa(f'tell application "TextEdit" to set text of {DOC} to ""')
        L.click(cx, 200)
        for ch in "hallo ":
            L.key(ch if ch != " " else "space")
        L.text("äöü ß €")
        time.sleep(0.4)
        t = osa(f'tell application "TextEdit" to get text of {DOC}')
        res.check("Tippen inkl. Umlaute und Euro", t.lower() == "hallo äöü ß €", repr(t))
        # Cmd+A / Cmd+C / Cmd+V
        L.key("a", "cmd")
        L.key("c", "cmd")
        L.key("right")
        L.key("v", "cmd")
        time.sleep(0.4)
        t = osa(f'tell application "TextEdit" to get text of {DOC}')
        res.check("Cmd+A, Cmd+C, Cmd+V", t.lower() == "hallo äöü ß €hallo äöü ß €", repr(t))
        # Resize
        r, ms = L.resize(900, 650)
        time.sleep(0.5)
        b = osa(f'tell application "TextEdit" to get bounds of {WIN}')
        nums = [int(float(x)) for x in b.replace(" ", "").split(",")] if "," in b else []
        wh = (nums[2] - nums[0], nums[3] - nums[1]) if len(nums) == 4 else None
        res.check("Resize auf 900x650 pt", r.get("ok") and wh and abs(wh[0] - 900) <= 2 and abs(wh[1] - 650) <= 2,
                  f"{ms:.0f} ms, Fenster {wh}, Antwort {r.get('width')}x{r.get('height')} px")
        # Eingabe nach Resize noch richtig?
        L.click(cx, 200)
        L.text("Z")
        time.sleep(0.3)
        t = osa(f'tell application "TextEdit" to get text of {DOC}')
        res.check("Eingabe nach dem Resize funktioniert", "Z" in t)
    finally:
        r, back = L.stop()
        res.check("send.stop beendet sauber", r.get("ok"), f"{r.get('_ms', 0):.0f} ms")
        if virtual and back:
            res.check("Fenster zurueck an die alte Stelle", abs(back["x"] - win["x"]) < 4 and abs(back["y"] - win["y"]) < 4
                      and abs(back["w"] - win["w"]) < 4 and abs(back["h"] - win["h"]) < 4,
                      f"{back['w']}x{back['h']} bei ({back['x']},{back['y']}) vs {win['w']}x{win['h']} bei ({win['x']},{win['y']})")
        osa(f'tell application "TextEdit" to close {WIN} saving no')
        clip_set(saved_clip)
    return res


# ---------------------------------------------------------------- Safari
PAGE = """<!doctype html><meta charset=utf-8><title>start</title>
<body style="margin:0;font:16px sans-serif"><div id=a style="height:3000px;background:linear-gradient(#eef,#fee)">
<textarea id=t style="position:absolute;left:40px;top:140px;width:400px;height:80px"></textarea>
<script>
var c=0,d=0,r=0,w=0,px=-1,py=-1;
function u(){document.title='c='+c+' d='+d+' r='+r+' w='+Math.round(w)+' y='+Math.round(scrollY)+' p='+px+','+py+' v='+document.getElementById('t').value}
function pos(e){px=Math.round(e.clientX);py=Math.round(e.clientY)}
var a=document.getElementById('a');
a.addEventListener('click',function(e){c++;pos(e);u()});
a.addEventListener('dblclick',function(e){d++;pos(e);u()});
document.addEventListener('contextmenu',function(e){r++;pos(e);u()});
addEventListener('wheel',function(e){w+=e.deltaY;setTimeout(u,50)},{passive:true});
addEventListener('scroll',u);document.getElementById('t').addEventListener('input',u);u();
</script></div>"""


def run_safari(virtual):
    res = Results()
    saved_clip = clip_get()
    # Safari laedt file:// per AppleScript nicht; die Seite kommt von einem lokalen Server (nur 127.0.0.1).
    os.makedirs("/tmp/teleport_live", exist_ok=True)
    open("/tmp/teleport_live/page.html", "w").write(PAGE)
    srv = subprocess.Popen([sys.executable, "-m", "http.server", "47099", "--bind", "127.0.0.1", "--directory", "/tmp/teleport_live"],
                           stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    time.sleep(0.8)
    wid = osa('''tell application "Safari"
        activate
        open location "http://127.0.0.1:47099/page.html"
        delay 3
        set bounds of window 1 to {200, 120, 1100, 800}
        return id of window 1
    end tell''')
    if wid.startswith("ERR") or not wid.isdigit():
        print("Safari-Fenster nicht anlegbar:", wid)
        return res
    wid = int(wid)
    time.sleep(1.0)
    title = lambda: osa('tell application "Safari" to get name of current tab of window 1')
    for _ in range(60):
        if title().startswith("c="):
            break
        time.sleep(0.25)
    L = Live(wid, virtual, "Safari")
    try:
        win = L.orig
        print(f"Fenster {win['w']}x{win['h']} pt, Stream {L.W}x{L.H} px, scale {L.scale}, Titel jetzt: {title()}")
        # Kalibrieren: die ganze Seite ist Klickflaeche und meldet die Position des Klicks (clientX/Y) im Titel.
        import re
        def last_pos():
            mm = re.search(r" p=(-?\d+),(-?\d+) ", title() + " ")
            return (int(mm.group(1)), int(mm.group(2))) if mm else None
        L.click(450, 400)
        time.sleep(0.4)
        p1 = last_pos()
        ox, oy = (450 - p1[0], 400 - p1[1]) if p1 else (0, 0)
        L.click(700, 520)
        time.sleep(0.4)
        p2 = last_pos()
        ok = bool(p1) and bool(p2) and abs(p2[0] - (700 - ox)) <= 1 and abs(p2[1] - (520 - oy)) <= 1
        res.check("Linksklick landet pixelgenau (zweiter Klick, nach Kalibrierung)", ok,
                  f"Seitenursprung ({ox},{oy}) pt, Klick 2 gemeldet {p2}, erwartet {(700 - ox, 520 - oy)}")
        off = oy
        cl = lambda x, y, **kw: L.click(ox + x, oy + y, **kw)
        cl(100, 100, n=2)
        time.sleep(0.3)
        res.check("Doppelklick (dblclick-Ereignis)", " d=1 " in title() + " ", title())
        cl(100, 100, button="right")
        time.sleep(0.7)
        menus = menu_open("Safari")  # solange das Menue offen ist, antwortet Safari keinen AppleEvents
        L.key("esc")
        time.sleep(0.5)
        res.check("Rechtsklick: Kontextmenue geoeffnet, Seite bekommt contextmenu", bool(menus) and " r=1 " in title() + " ",
                  f"{len(menus)} Menuefenster, {title()}")
        res.check("Escape schliesst das Kontextmenue", not menu_open("Safari"))
        L.scroll(500, 400, -6)
        time.sleep(0.8)
        t1 = title()
        res.check("Scrollen (Seite rollt, wheel-Ereignisse)", "y=0 " not in t1 and " y=" in t1 and "w=0 " not in t1, t1)
        L.scroll(500, 400, 6)
        time.sleep(0.8)
        L.scroll(500, 400, 20, unit="px")
        time.sleep(0.5)
        # Zurueck nach oben, damit das Textfeld sichtbar ist
        cl(120, 180)
        for ch in "hallo ":
            L.key(ch if ch != " " else "space")
        L.text("äöü ß €")
        time.sleep(0.5)
        res.check("Tippen inkl. Umlaute im Textfeld", "v=hallo äöü ß €" in title(), title())
        L.key("a", "cmd")
        L.key("c", "cmd")
        L.key("right")
        L.key("v", "cmd")
        time.sleep(0.5)
        res.check("Cmd+A, Cmd+C, Cmd+V", "v=hallo äöü ß €hallo äöü ß €" in title(), title())
        r, ms = L.resize(800, 600)
        time.sleep(0.6)
        b = osa('tell application "Safari" to get bounds of window 1')
        nums = [int(float(x)) for x in b.replace(" ", "").split(",")] if "," in b else []
        wh = (nums[2] - nums[0], nums[3] - nums[1]) if len(nums) == 4 else None
        res.check("Resize auf 800x600 pt", r.get("ok") and wh and abs(wh[0] - 800) <= 2 and abs(wh[1] - 600) <= 2,
                  f"{ms:.0f} ms, Fenster {wh}")
    finally:
        r, back = L.stop()
        res.check("send.stop beendet sauber", r.get("ok"), f"{r.get('_ms', 0):.0f} ms")
        if virtual and back:
            res.check("Fenster zurueck an die alte Stelle", abs(back["x"] - win["x"]) < 4 and abs(back["y"] - win["y"]) < 4
                      and abs(back["w"] - win["w"]) < 4 and abs(back["h"] - win["h"]) < 4,
                      f"{back['w']}x{back['h']} bei ({back['x']},{back['y']}) vs {win['w']}x{win['h']} bei ({win['x']},{win['y']})")
        osa('tell application "Safari" to close window 1')
        srv.terminate()
        clip_set(saved_clip)
    return res


if __name__ == "__main__":
    ap = argparse.ArgumentParser()
    ap.add_argument("app", choices=["textedit", "safari"])
    ap.add_argument("--virtual", action="store_true")
    a = ap.parse_args()
    print(f"== {a.app}, {'virtueller Monitor' if a.virtual else 'Fenster-Rueckfall'} ==")
    res = (run_textedit if a.app == "textedit" else run_safari)(a.virtual)
    bad = [r for r in res.rows if not r[1]]
    print(f"{len(res.rows) - len(bad)}/{len(res.rows)} ok")
    sys.exit(1 if bad else 0)
