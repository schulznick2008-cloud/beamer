#!/usr/bin/env python3
"""Live-Abnahme mit Nicks Finanzen-App (C04a), nur lesende Eingaben: Seitenleiste, Liste scrollen, Rechtsklick (Menue
sofort mit Esc zu), Text in das Beraterfeld tippen und wieder loeschen (NICHT absenden). Schreibt Screenshots
nach /tmp/fin_<modus>_*.png, die Sicht auf das Fenster nach jedem Schritt.

  tools/teleport/live_finanzen.py [--virtual]
"""
import os, subprocess, sys, time
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import live_accept as la

virtual = "--virtual" in sys.argv
tag = "virt" if virtual else "win"
wins = [w for w in la.cgwins() if w["owner"] == "Finanzen" and w["layer"] == 0 and w["w"] > 400]
assert wins, "Finanzen-Fenster nicht auf dem Bildschirm"
wid = wins[0]["wid"]
res = la.Results()
saved = la.clip_get()
L = la.Live(wid, virtual, "Finanzen")
shot = lambda n: subprocess.run(["screencapture", "-x", "-o", "-l", str(wid), f"/tmp/fin_{tag}_{n}.png"])
title = lambda: next((w["name"] for w in la.cgwins() if w["wid"] == wid), "?")
try:
    w0 = L.orig
    print(f"Fenster {w0['w']}x{w0['h']} pt, Stream {L.W}x{L.H} px, scale {L.scale}")
    side = lambda y: (60, y)  # Seitenleiste in Fenster-Punkten (Fenster 1800x1050 pt)
    SB = dict(start=68, umsaetze=100, budget=132, vermoegen=164, berater=196)
    L.click(*side(SB["vermoegen"])); time.sleep(0.8)
    res.check("Linksklick: Seitenleiste 'Vermoegen'", title() == "Vermögen", title()); shot("1_vermoegen")
    L.click(*side(SB["umsaetze"]), n=2); time.sleep(0.8)
    res.check("Doppelklick: Seitenleiste 'Umsaetze'", title() == "Umsätze", title()); shot("2_umsaetze")
    shot("3_vorscroll")
    L.scroll(900, 500, -8); time.sleep(0.9); shot("4_nachscroll")
    L.scroll(900, 500, 8); time.sleep(0.5)
    L.click(1630, 26, button="right"); time.sleep(0.7)
    menus = la.menu_open("Finanzen"); shot("5_kontextmenue")
    L.key("esc"); time.sleep(0.5)
    res.check("Rechtsklick (Suchfeld): Kontextmenue", bool(menus), f"{len(menus)} Menuefenster")
    res.check("Escape schliesst es", not la.menu_open("Finanzen"))
    la.clip_set("LEER")
    L.click(1630, 26); time.sleep(0.4)
    L.text("mcdonald"); time.sleep(0.8); shot("6_gefiltert")
    L.key("a", "cmd"); L.key("c", "cmd"); time.sleep(0.3)
    res.check("Tippen + Cmd+A, Cmd+C im Suchfeld (Zwischenablage)", la.clip_get() == "mcdonald", repr(la.clip_get()[:30]))
    L.key("right"); L.key("v", "cmd"); time.sleep(0.6); shot("7_eingefuegt")
    L.key("a", "cmd"); L.text("äöü ß €"); time.sleep(0.6); shot("8_umlaute")
    L.key("a", "cmd"); L.key("backspace"); time.sleep(0.5); shot("9_geloescht")
    r, ms = L.resize(1400, 900); time.sleep(0.8)
    nw = next((w for w in la.cgwins() if w["wid"] == wid), None)
    res.check("Resize auf 1400x900 pt", r.get("ok") and nw and abs(nw["w"] - 1400) <= 2 and abs(nw["h"] - 900) <= 2,
              f"{ms:.0f} ms, Fenster {nw and (nw['w'], nw['h'])}"); shot("10_resize")
    L.click(*side(SB["budget"])); time.sleep(0.8)
    res.check("Eingabe nach dem Resize (Seitenleiste 'Budget')", title() == "Budget", title())
    L.click(*side(SB["umsaetze"])); time.sleep(0.5)
finally:
    r, back = L.stop()
    res.check("send.stop", r.get("ok"), f"{r.get('_ms', 0):.0f} ms")
    if back:
        print(f"   Fenster nach Ende: {back['w']}x{back['h']} bei ({back['x']},{back['y']}), vorher {w0['w']}x{w0['h']} bei ({w0['x']},{w0['y']})")
    la.clip_set(saved)
    subprocess.run(['osascript','-e','tell application "System Events" to tell process "Finanzen" to set size of window 1 to {%d, %d}' % (w0['w'], w0['h'])])
