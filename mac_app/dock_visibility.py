"""Pure policy selection for the Mac app's Dock presence."""

DEFAULT_SHOW_IN_DOCK = True


def complete_settings(settings):
    settings.setdefault("show_in_dock", DEFAULT_SHOW_IN_DOCK)
    return settings


def validate_settings(settings):
    if not isinstance(settings.get("show_in_dock"), bool):
        raise ValueError("show_in_dock must be true or false")


def activation_policy(show_in_dock, window_open):
    return "regular" if show_in_dock or window_open else "accessory"
