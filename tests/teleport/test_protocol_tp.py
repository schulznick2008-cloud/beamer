from core.teleport_protocol import choose_codec, close_msg, offer_msg, read_tp


def test_read_tp_offer():
    msg = offer_msg(
        42,
        app="App",
        title="T",
        icon_png="",
        w=100,
        h=200,
        scale=2.0,
        codec="hevc",
        mode="window",
        udp_port=47123,
        salt_b64="AAAAAAAAAAAAAAAAAAAAAA==",
    )
    parsed = read_tp(msg)
    assert parsed is not None
    assert parsed[0] == "tp_offer"
    assert parsed[1]["tid"] == 42


def test_codec_low_power():
    assert choose_codec(True) == "h264"
    assert choose_codec(False) == "hevc"


def test_close_reason():
    msg = close_msg(1, "return")
    assert read_tp(msg)[1]["reason"] == "return"
