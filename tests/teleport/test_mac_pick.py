from core.teleport_mac_pick import pick_window_for_frontmost


def test_picks_by_pid_not_largest_other_app():
    windows = [
        {"wid": 1, "app": "Other", "pid": 9, "w": 2000, "h": 2000},
        {"wid": 2, "app": "Front", "pid": 42, "w": 400, "h": 300},
    ]
    win = pick_window_for_frontmost(windows, app_name="Front", pid=42)
    assert win["wid"] == 2
