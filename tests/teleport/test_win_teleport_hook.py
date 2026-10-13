"""Windows-Hook: bei Proxy-Fokus KVM nicht schlucken (return False), Maus unverändert."""


def kvm_key_hook_should_swallow_to_peer(redirecting: bool, proxy_focused: bool) -> bool:
    """Nach Fix: proxy_focused blockiert nur KVM-Weiterleitung, nicht systemweit."""
    if proxy_focused:
        return False
    return redirecting


def test_proxy_focus_does_not_swallow_keys():
    assert kvm_key_hook_should_swallow_to_peer(redirecting=True, proxy_focused=True) is False


def test_redirect_still_swallows_when_no_proxy():
    assert kvm_key_hook_should_swallow_to_peer(redirecting=True, proxy_focused=False) is True
