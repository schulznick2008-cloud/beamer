"""Tests für core.teleport_ipc gegen tools/teleport/fake_helper.py."""
import logging
import os
import sys
import threading
import time

import pytest

from core.teleport_ipc import (
    TeleportHelper,
    TeleportHelperConfig,
    TeleportHelperCrashed,
    TeleportIPCTimeout,
    hello,
)

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
FAKE = os.path.join(ROOT, "tools", "teleport", "fake_helper.py")


def _fake_config(**env: str) -> TeleportHelperConfig:
    base_env = {"PYTHONUNBUFFERED": "1"}
    base_env.update(env)
    return TeleportHelperConfig(
        executable=[sys.executable, FAKE],
        extra_args=[],
        env=base_env,
        restart_on_crash=False,
    )


def test_hello_and_windows_list():
    h = TeleportHelper(_fake_config())
    h.start()
    r = hello(h, "test")
    assert r["ok"] is True
    assert r["platform"] == "mac"
    assert "send_window" in r["caps"]
    assert r.get("low_power") is False

    wl = h.request("windows.list")
    assert wl["ok"] is True
    assert wl["windows"][0]["app"] == "FakeApp"
    assert h.stop() == 0


def test_events_stats_and_send_gone():
    h = TeleportHelper(_fake_config())
    h.start()
    hello(h, "test")
    h.request("send.start", tid=7, mode="window", wid=1, codec="hevc", max_fps=30, bitrate_kbps=8000)
    evs = h.pop_events()
    assert any(e.get("ev") == "stats" and e.get("tid") == 7 for e in evs)

    h.request("emit.gone", tid=7)
    gone = [e for e in h.pop_events() if e.get("ev") == "send.gone"]
    assert len(gone) == 1
    assert gone[0]["reason"] == "window_closed"
    h.stop()


def test_encoder_slow_event_fields():
    h = TeleportHelper(_fake_config())
    h.start()
    hello(h, "test")
    h.request("emit.slow", tid=3)
    stats = [e for e in h.pop_events() if e.get("ev") == "stats"][-1]
    assert stats["encoder_slow"] is True
    assert stats["low_power"] is True
    h.stop()


def test_timeout_on_hang():
    h = TeleportHelper(
        TeleportHelperConfig(
            executable=[sys.executable, FAKE],
            extra_args=[],
            env={
                "BEAMER_FAKE_HELPER_MODE": "hang",
                "BEAMER_FAKE_HELPER_HANG_CMD": "transport.open",
            },
            restart_on_crash=False,
        )
    )
    h.start()
    hello(h, "test")
    with pytest.raises(TeleportIPCTimeout):
        h.request("transport.open", timeout=0.4)
    h.stop(send_shutdown=False)


def test_busy_error():
    h = TeleportHelper(
        _fake_config(
            BEAMER_FAKE_HELPER_MODE="busy",
            BEAMER_FAKE_HELPER_BUSY_CMD="transport.open",
        )
    )
    h.start()
    hello(h, "test")
    r = h.request("transport.open")
    assert r["ok"] is False
    assert r["error"]["code"] == "busy"
    h.stop()


def test_bad_line_from_helper_delivered_as_error_event():
    h = TeleportHelper(_fake_config(BEAMER_FAKE_HELPER_MODE="bad_line"))
    h.start()
    hello(h, "test")
    errs = [e for e in h.pop_events() if e.get("ev") == "error"]
    assert errs
    assert errs[0]["code"] == "bad_request"
    h.stop()


def test_eof_shutdown_within_five_seconds():
    h = TeleportHelper(_fake_config())
    h.start()
    hello(h, "test")
    t0 = time.time()
    code = h.stop(send_shutdown=False)
    assert code == 0
    assert time.time() - t0 < 5.0


def test_stderr_flood_does_not_block_requests():
    h = TeleportHelper(
        _fake_config(BEAMER_FAKE_HELPER_STDERR_FLOOD="1", BEAMER_FAKE_HELPER_STDERR_MB="1")
    )
    h.start()
    hello(h, "test")
    for _ in range(100):
        r = h.request("hello", beamer_version="loop", timeout=2.0)
        assert r["ok"] is True
    assert len(h.stderr_tail()) <= 200
    h.stop()


def test_unicode_window_title_roundtrip():
    h = TeleportHelper(_fake_config())
    h.start()
    hello(h, "test")
    wl = h.request("windows.list")
    assert wl["windows"][0]["title"] == "Fenster – Übersicht"
    h.stop()


def test_late_reply_after_timeout_is_discarded(caplog):
    h = TeleportHelper(
        TeleportHelperConfig(
            executable=[sys.executable, FAKE],
            extra_args=[],
            env={
                "BEAMER_FAKE_HELPER_MODE": "late",
                "BEAMER_FAKE_HELPER_LATE_CMD": "transport.open",
                "BEAMER_FAKE_HELPER_LATE_DELAY": "0.5",
            },
            restart_on_crash=False,
        )
    )
    h.start()
    hello(h, "test")
    with caplog.at_level(logging.WARNING):
        with pytest.raises(TeleportIPCTimeout):
            h.request("transport.open", timeout=0.15)
        time.sleep(0.6)
        assert any("verspätete" in r.message for r in caplog.records)
    r = h.request("transport.open", timeout=2.0)
    assert r["ok"] is True
    h.stop()


def test_request_retry_busy():
    h = TeleportHelper(
        _fake_config(
            BEAMER_FAKE_HELPER_MODE="busy",
            BEAMER_FAKE_HELPER_BUSY_CMD="transport.open",
            BEAMER_FAKE_HELPER_BUSY_COUNT="2",
        )
    )
    h.start()
    hello(h, "test")
    r = h.request_retry_busy("transport.open", tries=4, delay=0.05)
    assert r["ok"] is True
    h.stop()


def test_crash_restart_with_backoff():
    cfg = TeleportHelperConfig(
        executable=[sys.executable, FAKE],
        extra_args=[],
        env={
            "BEAMER_FAKE_HELPER_MODE": "crash",
            "BEAMER_FAKE_HELPER_CRASH_CMD": "crash.test",
        },
        restart_on_crash=True,
        restart_backoff=(0.15, 0.15),
    )
    h = TeleportHelper(cfg)
    h.start()
    hello(h, "test")
    with pytest.raises(TeleportHelperCrashed):
        h.request("crash.test", timeout=1.0)

    deadline = time.time() + 3.0
    restarted = False
    while time.time() < deadline:
        for ev in h.pop_events():
            if ev.get("ev") == "helper_restarted":
                assert ev.get("code") == 1
                restarted = True
        if h.running:
            try:
                r = hello(h, "test", timeout=2.0)
                if r.get("ok"):
                    break
            except TeleportHelperCrashed:
                pass
        time.sleep(0.05)
    assert restarted
    assert h.running
    hello(h, "after-restart")
    h._intentional_stop = True
    h._stop_requested = True
    h.stop()


def test_on_event_callback_thread_safe():
    received = []
    lock = threading.Lock()

    def on_event(ev):
        with lock:
            received.append(ev.get("ev"))

    h = TeleportHelper(_fake_config(), on_event=on_event)
    h.start()
    hello(h, "test")
    h.request("emit.slow", tid=1)
    time.sleep(0.2)
    with lock:
        assert "stats" in received
    h.stop()
