"""Tests for the TBP escape the client produces and its text/plain fallback."""

from __future__ import annotations

import base64
import json

import pytest

import spaceterm


def _parse_escape(escape: str) -> tuple[str, str, dict[str, object]]:
    """Return (verb, params, decoded bundle) from one emit escape."""
    assert escape.startswith("\x1b]"), escape
    assert escape.endswith("\x1b\\"), escape
    osc, verb, params, payload = escape[2:-2].split(";", 3)
    assert osc == "9001"
    bundle = json.loads(base64.standard_b64decode(payload))
    return verb, params, bundle


def test_display_svg_emits_decodable_block(monkeypatch, capsys):
    monkeypatch.setenv("SPACETERM", "1")
    spaceterm.display_svg("<svg/>", text="alt")

    verb, params, bundle = _parse_escape(capsys.readouterr().out)
    assert verb == "emit"
    assert "trust=restricted" in params
    assert bundle["mime"]["image/svg+xml"] == "<svg/>"
    assert bundle["mime"]["text/plain"] == "alt"


def test_image_bytes_are_base64_encoded(monkeypatch, capsys):
    monkeypatch.setenv("SPACETERM", "1")
    raw = b"\x89PNG\r\n\x1a\n"
    spaceterm.display_image(raw, mime="image/png")

    _, _, bundle = _parse_escape(capsys.readouterr().out)
    assert bundle["mime"]["image/png"] == base64.standard_b64encode(raw).decode("ascii")


def test_repr_html_object_is_detected(monkeypatch, capsys):
    monkeypatch.setenv("SPACETERM", "1")

    class Table:
        def _repr_html_(self) -> str:
            return "<table></table>"

    spaceterm.display(Table())

    _, _, bundle = _parse_escape(capsys.readouterr().out)
    assert bundle["mime"]["text/html"] == "<table></table>"
    assert "text/plain" in bundle["mime"]


def test_fallback_prints_plain_text_when_not_spaceterm(monkeypatch, capsys):
    monkeypatch.delenv("SPACETERM", raising=False)
    monkeypatch.delenv("TERM_PROGRAM", raising=False)
    spaceterm.display_svg("<svg/>", text="just text")

    out = capsys.readouterr().out
    assert out == "just text"
    assert "\x1b]" not in out


def test_invalid_trust_is_rejected(monkeypatch):
    monkeypatch.setenv("SPACETERM", "1")
    with pytest.raises(ValueError, match="trust"):
        spaceterm.display("x", trust="admin")
