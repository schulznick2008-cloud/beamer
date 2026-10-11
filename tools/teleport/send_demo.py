#!/usr/bin/env python3
"""Demo/Test fuer den Mac-Sender-Helfer `BeamerTeleport --ipc stdio` (SCHNITTSTELLEN.md §1).

Startet den Helfer, listet Fenster, startet `send.start` auf einen Empfaenger (z. B.
`BeamerTeleport --selftest-recv 47000`) und gibt die Ereignisse aus. Die Schluessel sind feste
TEST-Schluessel (identisch zu `StreamKeys.testOnly` im Swift-Paket), NUR fuer diesen Selbsttest.

  tools/teleport/send_demo.py --list
  tools/teleport/send_demo.py --app TextEdit --peer 127.0.0.1:47000 --seconds 20
  tools/teleport/send_demo.py --display 0 --seconds 10
"""
import argparse
import base64
import json
import os
import subprocess
import sys
import threading
import time

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
DEFAULT_HELPER = os.path.join(ROOT, "teleport", "mac", ".build", "release", "BeamerTeleport")

# NUR FUER TESTS (siehe StreamKeys.testOnly):
TEST_KEYS = {
    "s2r_key": base64.b64encode(bytes([0x11] * 32)).decode(),
    "s2r_prefix": base64.b64encode(bytes([0xA1, 0xA2, 0xA3, 0xA4])).decode(),
    "r2s_key": base64.b64encode(bytes([0x22] * 32)).decode(),
    "r2s_prefix": base64.b64encode(bytes([0xB1, 0xB2, 0xB3, 0xB4])).decode(),
}


class Helper:
    def __init__(self, path):
        self.p = subprocess.Popen([path, "--ipc", "stdio"], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                  stderr=sys.stderr, text=True, bufsize=1)
        self.next_id = 1
        self.replies = {}
        self.events = []
        self.cv = threading.Condition()
        threading.Thread(target=self._read, daemon=True).start()

    def _read(self):
        for line in self.p.stdout:
            line = line.strip()
            if not line:
                continue
            m = json.loads(line)
            with self.cv:
                if "re" in m:
                    self.replies[m["re"]] = m
                else:
                    self.events.append(m)
                self.cv.notify_all()

    def call(self, cmd, timeout=15, **kw):
        i = self.next_id
        self.next_id += 1
        self.p.stdin.write(json.dumps({"v": 1, "id": i, "cmd": cmd, **kw}) + "\n")
        self.p.stdin.flush()
        end = time.time() + timeout
        with self.cv:
            while i not in self.replies:
                left = end - time.time()
                if left <= 0:
                    raise TimeoutError(cmd)
                self.cv.wait(left)
            return self.replies.pop(i)

    def drain_events(self):
        with self.cv:
            ev, self.events = self.events, []
        return ev


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--helper", default=DEFAULT_HELPER)
    ap.add_argument("--list", action="store_true", help="nur Fenster auflisten")
    ap.add_argument("--app", help="Teil des App-Namens (groesstes Fenster)")
    ap.add_argument("--wid", type=int)
    ap.add_argument("--display", type=int)
    ap.add_argument("--peer", default="127.0.0.1:47000")
    ap.add_argument("--codec", default="hevc", choices=["hevc", "h264"])
    ap.add_argument("--fps", type=int, default=60)
    ap.add_argument("--kbps", type=int, default=20000)
    ap.add_argument("--seconds", type=float, default=15)
    ap.add_argument("--tid", type=int, default=1)
    a = ap.parse_args()

    h = Helper(a.helper)
    r = h.call("hello", beamer_version="demo")
    print("hello:", r, "(Energiesparmodus: %s)" % r.get("low_power"))
    host, port = a.peer.rsplit(":", 1)
    r = h.call("transport.open", local_port=0, peer_host=host, peer_port=int(port))
    print("transport.open:", r)

    r = h.call("windows.list")
    if not r.get("ok"):
        print("windows.list:", r)
        h.p.stdin.close()
        return 1
    wins = r["windows"]
    if a.list:
        for w in wins:
            print(f'{w["wid"]:>7} {w["app"][:22]:<22} {w["w"]:.0f}x{w["h"]:.0f} scale={w["scale"]} {w["title"][:50]}')
        h.call("shutdown")
        return 0

    start = {"tid": a.tid, "codec": a.codec, "max_fps": a.fps, "bitrate_kbps": a.kbps,
             "keys": TEST_KEYS, "virtual_display": False}
    if a.display is not None:
        start.update(mode="display", display=a.display)
    else:
        wid = a.wid
        if wid is None:
            cands = [w for w in wins if a.app and a.app.lower() in w["app"].lower()]
            if not cands:
                print("kein Fenster gefunden (--list zeigt die Fenster)")
                return 1
            wid = max(cands, key=lambda w: w["w"] * w["h"])["wid"]
        start.update(mode="window", wid=wid)
    r = h.call("send.start", **start)
    print("send.start:", r)
    if not r.get("ok"):
        h.call("shutdown")
        return 1

    end = time.time() + a.seconds
    while time.time() < end:
        time.sleep(1)
        for e in h.drain_events():
            if e.get("ev") == "stats":
                print("stats: fps=%s kbps=%s capture_fps=%s encode_ms=%s cpu=%s%% state=%s target=%s rtt=%s loss=%s "
                      "eff_fps=%s slow=%s lowpower=%s drops=%s" % (
                    e["fps"], e["kbps"], e.get("capture_fps"), e.get("encode_ms"), e.get("cpu_pct"),
                    e.get("state"), e.get("target_kbps"), e["rtt_ms"], e["loss_pct"],
                    e.get("effective_fps"), e.get("encoder_slow"), e.get("low_power"), e.get("backlog_drops")))
            else:
                print("event:", e)
                if e.get("ev") == "send.gone":
                    end = 0

    t0 = time.time()
    r = h.call("send.stop", tid=a.tid)
    print("send.stop:", r, f"({time.time() - t0:.2f} s)")
    t0 = time.time()
    h.p.stdin.close()
    h.p.wait(timeout=5)
    print(f"EOF -> Prozessende nach {time.time() - t0:.2f} s, Code {h.p.returncode}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
