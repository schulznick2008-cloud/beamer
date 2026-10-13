#!/usr/bin/env python3
"""Live-Abnahme Tastenwiederholung (C04b): in TextEdit eine Taste 1 s und 2 s halten (ein `down`, nach der Zeit ein
`up`), Zeichenzahl gegen Verzoegerung/Rate des Systems (NSEvent.keyRepeatDelay/-Interval) vergleichen. Prueft auch den
aelteren Weg (wiederholte `down`s) und dass nach dem `up` nichts mehr kommt.

  tools/teleport/live_repeat.py [--virtual]
"""
import math, os, subprocess, sys, time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import live_accept as la

virtual = "--virtual" in sys.argv
open("/tmp/keyrep.swift", "w").write("import AppKit\nprint(NSEvent.keyRepeatDelay, NSEvent.keyRepeatInterval)\n")
subprocess.run(["swiftc", "/tmp/keyrep.swift", "-o", "/tmp/keyrep"], check=True)
delay, interval = map(float, subprocess.run(["/tmp/keyrep"], capture_output=True, text=True).stdout.split())
print(f"System: Verzoegerung {delay:.3f} s, Intervall {interval:.3f} s ({1 / interval:.1f}/s)")

res = la.Results()
wid = int(la.osa('''tell application "TextEdit"
 activate
 set d to make new document
 set bounds of window 1 to {200, 120, 1000, 720}
 return id of window 1
end tell'''))
time.sleep(1)
DOC = f"document of (first window whose id is {wid})"
text = lambda: la.osa(f'tell application "TextEdit" to get text of {DOC}')
L = la.Live(wid, virtual, "TextEdit")
try:
    L.click(300, 200)
    for hold in (1.0, 2.0):
        la.osa(f'tell application "TextEdit" to set text of {DOC} to ""')
        L.click(300, 200)
        time.sleep(0.3)
        L.inj(k="key", key="x", down=True)
        time.sleep(hold)
        L.inj(k="key", key="x", down=False)
        time.sleep(0.6)
        n = len(text().replace("\n", ""))
        want = 1 + (math.floor((hold - delay) / interval) + 1 if hold >= delay else 0)
        res.check(f"{hold:.0f} s halten: Zeichenzahl passt zur Systemrate", abs(n - want) <= 2, f"{n} Zeichen, erwartet ~{want}")
        n2 = len(text().replace("\n", ""))
        time.sleep(1.0)
        res.check("nach dem up kommt nichts mehr", len(text().replace("\n", "")) == n2)
    # aelterer Weg: Wiederholung als wiederholte downs vom Proxy: darf die Rate nicht verdoppeln
    la.osa(f'tell application "TextEdit" to set text of {DOC} to ""')
    L.click(300, 200)
    time.sleep(0.3)
    t0 = time.time()
    L.inj(k="key", key="y", down=True)
    while time.time() - t0 < 1.5:
        time.sleep(interval)
        L.inj(k="key", key="y", down=True)  # Auto-Repeat-down wie in Vertrag <= v1.3
    L.inj(k="key", key="y", down=False)
    time.sleep(0.6)
    n = len(text().replace("\n", ""))
    ticks = 1 + int(1.5 / interval)
    want = 1 + math.floor((1.5 - delay) / interval) + 1
    res.check("wiederholte downs (aelterer Weg): genau einfache Wiederholung", abs(n - want) <= 2,
              f"{n} Zeichen bei {ticks} gesendeten downs, erwartet ~{want}")
    # send.stop waehrend gehalten
    la.osa(f'tell application "TextEdit" to set text of {DOC} to ""')
    L.click(300, 200)
    L.inj(k="key", key="z", down=True)
    time.sleep(0.9)
finally:
    r, back = L.stop()
    time.sleep(1.0)
    n1 = len(text().replace("\n", ""))
    time.sleep(1.0)
    res.check("send.stop beendet die Wiederholung", len(text().replace("\n", "")) == n1, f"{n1} Zeichen")
    la.osa(f'tell application "TextEdit" to close (first window whose id is {wid}) saving no')
sys.exit(1 if [r for r in res.rows if not r[1]] else 0)
