#!/usr/bin/env python3
"""Smoke-Test C02d: mehrmals hintereinander send.start/send.stop mit einem Fenster, H.264 und HEVC. Der Helfer darf
nie abstuerzen (Exit-Code, stderr ohne CGS_REQUIRE_INIT) und jeder Lauf muss ok sein.

  tools/teleport/smoke_start_stop.py --app TextEdit [--runs 5]
  tools/teleport/smoke_start_stop.py                  # startet das Test-Fenster anim_window.swift
"""
import argparse, base64, json, os, subprocess, sys, threading, time

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
HELPER = os.path.join(ROOT, "teleport", "mac", ".build", "release", "BeamerTeleport")
# NUR FUER TESTS (StreamKeys.testOnly)
KEYS = {
    "s2r_key": base64.b64encode(bytes([0x11] * 32)).decode(),
    "s2r_prefix": base64.b64encode(bytes([0xA1, 0xA2, 0xA3, 0xA4])).decode(),
    "r2s_key": base64.b64encode(bytes([0x22] * 32)).decode(),
    "r2s_prefix": base64.b64encode(bytes([0xB1, 0xB2, 0xB3, 0xB4])).decode(),
}
ANIM = os.path.join(ROOT, "tools", "teleport", "anim_window.swift")


class Helper:
    def __init__(self, path):
        self.p = subprocess.Popen([path, "--ipc", "stdio"], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                  stderr=subprocess.PIPE, text=True, bufsize=1)
        self.next_id, self.replies, self.cv = 1, {}, threading.Condition()
        self.err = []
        threading.Thread(target=self._read, daemon=True).start()
        threading.Thread(target=lambda: self.err.extend(self.p.stderr), daemon=True).start()

    def _read(self):
        for line in self.p.stdout:
            if line.strip():
                m = json.loads(line)
                with self.cv:
                    if "re" in m:
                        self.replies[m["re"]] = m
                    self.cv.notify_all()

    def call(self, cmd, timeout=20, **kw):
        i = self.next_id
        self.next_id += 1
        self.p.stdin.write(json.dumps({"v": 1, "id": i, "cmd": cmd, **kw}) + "\n")
        self.p.stdin.flush()
        end = time.time() + timeout
        with self.cv:
            while i not in self.replies:
                if self.p.poll() is not None:
                    raise RuntimeError(f"Helfer beendet (Exit {self.p.returncode}) bei {cmd}")
                left = end - time.time()
                if left <= 0:
                    raise TimeoutError(cmd)
                self.cv.wait(min(left, 0.5))
            return self.replies.pop(i)


class m:  # kleine Huelle, damit der Rest des Skripts lesbar bleibt
    Helper = Helper
    HELPER = HELPER
    TEST_KEYS = KEYS

ap = argparse.ArgumentParser()
ap.add_argument("--app", default=None, help="App-Name; ohne: Test-Fenster")
ap.add_argument("--runs", type=int, default=5)
a = ap.parse_args()

probe = None
app = a.app
if app is None:
    subprocess.run(["swiftc", "-O", ANIM, "-o", "/tmp/teleport_anim"], check=True)
    probe = subprocess.Popen(["/tmp/teleport_anim"])
    app = "teleport_anim"
    time.sleep(2.5)

failures = 0
for codec in ("h264", "hevc"):
    for n in range(1, a.runs + 1):
        h = m.Helper(m.HELPER)
        try:
            h.call("hello", beamer_version="smoke")
            h.call("transport.open", local_port=0, peer_host="127.0.0.1", peer_port=47098)
            wins = h.call("windows.list")["windows"]
            cands = [w for w in wins if app.lower() in w["app"].lower()]
            if not cands:
                print("kein Fenster fuer", app); failures += 1; break
            w = max(cands, key=lambda x: x["w"] * x["h"])
            r = h.call("send.start", tid=1, mode="window", wid=w["wid"], codec=codec, max_fps=30, bitrate_kbps=8000, keys=m.TEST_KEYS)
            ok = r.get("ok")
            time.sleep(0.8)
            s = h.call("send.stop", tid=1)
            h.p.stdin.close()
            h.p.wait(timeout=5)
            code = h.p.returncode
            print(f"{codec} Lauf {n}: send.start {'ok' if ok else r.get('error')}, send.stop {'ok' if s.get('ok') else s}, Exit {code}")
            if any("CGS_REQUIRE_INIT" in line for line in h.err):
                print("   CGS_REQUIRE_INIT in stderr!"); failures += 1
            if not ok or not s.get("ok") or code != 0:
                failures += 1
        except Exception as e:
            print(f"{codec} Lauf {n}: FEHLER {e} (Exit {h.p.poll()})")
            failures += 1
            try: h.p.kill()
            except Exception: pass
if probe:
    probe.terminate()
print("FEHLER:", failures) if failures else print("alle Laeufe ok")
sys.exit(1 if failures else 0)
