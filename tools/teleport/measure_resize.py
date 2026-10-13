#!/usr/bin/env python3
"""Misst die Dauer von `send.resize` (Antwort und `send.size`) im Fenster- oder virtuellen Modus (Paket C04a).

  tools/teleport/measure_resize.py --virtual [--runs 10] [--app NAME --no-launch]

Wechselt zwischen grossen und kleinen Zielgroessen (jede zweite Messung vergroessert, jede andere verkleinert).
Gibt pro Messung Antwortzeit und `send.size`-Zeit aus, dazu Median und Maximum.
"""
import argparse, os, statistics, subprocess, sys, time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import mac_inject_demo as m  # noqa: E402


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--helper", default=m.HELPER)
    ap.add_argument("--virtual", action="store_true")
    ap.add_argument("--runs", type=int, default=10)
    ap.add_argument("--app", default="teleport_probe")
    ap.add_argument("--no-launch", action="store_true")
    ap.add_argument("--port", type=int, default=47031)
    a = ap.parse_args()
    probe_log = "/tmp/teleport_probe_rs.log"
    procs = []
    if not a.no_launch:
        subprocess.run(["swiftc", "-O", m.PROBE_SRC, "-o", "/tmp/teleport_probe"], check=True)
        procs.append(subprocess.Popen(["/tmp/teleport_probe", probe_log]))
        time.sleep(3.0)
    recv = subprocess.Popen([a.helper, "--selftest-recv", str(a.port), "--seconds", "120"],
                            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    procs.append(recv)
    h = m.Helper(a.helper)
    try:
        h.call("hello", beamer_version="measure")
        assert h.call("transport.open", local_port=0, peer_host="127.0.0.1", peer_port=a.port)["ok"]
        ws = h.call("windows.list")["windows"]
        cands = [w for w in ws if a.app.lower() in w["app"].lower() or "TeleportInputProbe" in w["title"]]
        win = max(cands, key=lambda w: w["w"] * w["h"])
        r = h.call("send.start", tid=1, mode="window", wid=win["wid"], codec="h264", max_fps=30,
                   bitrate_kbps=20000, keys=m.TEST_KEYS, virtual_display=a.virtual)
        assert r.get("ok"), r
        time.sleep(1.5)
        sizes = [(1200.0, 800.0), (800.0, 600.0)]
        replies, events = [], []
        for i in range(a.runs):
            w, hh = sizes[i % 2] if i else (1200.0, 800.0)
            t0 = time.time()
            r = h.call("send.resize", tid=1, w=w, h=hh)
            dt = (time.time() - t0) * 1000
            ev = h.events_named("send.size", since=t0)
            te = (ev[0]["_at"] - t0) * 1000 if ev else float("nan")
            ok = r.get("ok")
            print(f"{i + 1:2d}. {int(w)}x{int(hh)}: Antwort {dt:6.0f} ms, send.size {te:6.0f} ms, ok={ok}")
            if ok:
                replies.append(dt)
            time.sleep(1.0)
        print(f"Median {statistics.median(replies):.0f} ms, Max {max(replies):.0f} ms, Min {min(replies):.0f} ms ({len(replies)} Messungen)")
        h.call("send.stop", tid=1, restore=True)
    finally:
        try:
            h.close()
        except Exception:
            pass
        for p in procs:
            p.terminate()


main()
