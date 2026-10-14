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


def test_offer_chrome_fields():
    msg = offer_msg(
        1,
        app="A",
        title="T",
        icon_png="",
        w=100,
        h=80,
        scale=2.0,
        codec="hevc",
        mode="window",
        udp_port=1,
        salt_b64="AAAAAAAAAAAAAAAAAAAAAA==",
        title_bar=True,
        dark=False,
        lights=[[14.0, 14.0], [34.0, 14.0], [54.0, 14.0]],
    )
    data = msg["data"]
    assert data["title_bar"] is True
    assert data["dark"] is False
    assert data["lights"][0] == [14.0, 14.0]
    plain = offer_msg(
        2,
        app="A",
        title="T",
        icon_png="",
        w=1,
        h=1,
        scale=1,
        codec="h264",
        mode="window",
        udp_port=1,
        salt_b64="AAAAAAAAAAAAAAAAAAAAAA==",
    )
    assert "title_bar" not in plain["data"]
    assert plain["data"]["dark"] is True


def test_close_reason():
    msg = close_msg(1, "return")
    assert read_tp(msg)[1]["reason"] == "return"
