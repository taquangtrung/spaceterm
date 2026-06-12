"""SpaceTerm: emit rich Terminal Block Protocol (TBP) blocks from Python.

Quick use::

    import spaceterm
    spaceterm.display(dataframe)              # uses the object's _repr_*_ methods
    spaceterm.display_svg(open("plot.svg").read())
    spaceterm.display_image("chart.png")

Every block carries a ``text/plain`` fallback, and when SpaceTerm is not the active
terminal the fallback is printed instead, so scripts stay safe under tmux/ssh/CI.
"""

from __future__ import annotations

import base64
from pathlib import Path

from spaceterm._repr import mime_map_from_object
from spaceterm._wire import TEXT_PLAIN, emit, supported

__all__ = [
    "display",
    "display_html",
    "display_image",
    "display_latex",
    "display_markdown",
    "display_svg",
    "supported",
]

# ========================================================================
# Constants
# ========================================================================

_IMAGE_MIME_BY_SUFFIX = {
    ".gif": "image/gif",
    ".jpeg": "image/jpeg",
    ".jpg": "image/jpeg",
    ".png": "image/png",
    ".svg": "image/svg+xml",
    ".webp": "image/webp",
}

_SVG_MIME = "image/svg+xml"


# ========================================================================
# Public API
# ========================================================================


def display(
    obj: object,
    *,
    title: str | None = None,
    height_hint: int | None = None,
    trust: str = "restricted",
) -> None:
    """Render ``obj`` using its richest available representation."""
    emit(mime_map_from_object(obj), title=title, height_hint=height_hint, trust=trust)


def display_html(
    html: str,
    *,
    text: str | None = None,
    title: str | None = None,
    height_hint: int | None = None,
    trust: str = "restricted",
) -> None:
    """Render an HTML fragment inline."""
    emit(
        {"text/html": html, TEXT_PLAIN: text or "[html block]"},
        title=title,
        height_hint=height_hint,
        trust=trust,
    )


def display_svg(
    svg: str,
    *,
    text: str | None = None,
    title: str | None = None,
    height_hint: int | None = None,
    trust: str = "restricted",
) -> None:
    """Render an SVG document inline."""
    emit(
        {_SVG_MIME: svg, TEXT_PLAIN: text or "[svg image]"},
        title=title,
        height_hint=height_hint,
        trust=trust,
    )


def display_markdown(
    markdown: str,
    *,
    text: str | None = None,
    title: str | None = None,
    height_hint: int | None = None,
    trust: str = "restricted",
) -> None:
    """Render Markdown inline. The raw Markdown is the text fallback."""
    emit(
        {"text/markdown": markdown, TEXT_PLAIN: text or markdown},
        title=title,
        height_hint=height_hint,
        trust=trust,
    )


def display_latex(
    latex: str,
    *,
    text: str | None = None,
    title: str | None = None,
    height_hint: int | None = None,
    trust: str = "restricted",
) -> None:
    """Render a LaTeX expression inline. The raw source is the text fallback."""
    emit(
        {"text/latex": latex, TEXT_PLAIN: text or latex},
        title=title,
        height_hint=height_hint,
        trust=trust,
    )


def display_image(
    source: str | Path | bytes | bytearray,
    *,
    mime: str | None = None,
    text: str | None = None,
    title: str | None = None,
    height_hint: int | None = None,
    trust: str = "restricted",
) -> None:
    """Render an image from a file path or raw bytes.

    For a path the MIME type is inferred from the extension when not given; for
    bytes ``mime`` is required.
    """
    if isinstance(source, (bytes, bytearray)):
        if mime is None:
            raise ValueError("mime is required when source is bytes")
        data = bytes(source)
    else:
        path = Path(source)
        data = path.read_bytes()
        mime = mime or _IMAGE_MIME_BY_SUFFIX.get(path.suffix.lower())
        if mime is None:
            raise ValueError(f"cannot infer MIME type from suffix {path.suffix!r}")

    payload = (
        data.decode("utf-8")
        if mime == _SVG_MIME
        else base64.standard_b64encode(data).decode("ascii")
    )
    fallback = text or f"[{mime} image, {len(data)} bytes]"
    emit(
        {mime: payload, TEXT_PLAIN: fallback},
        title=title,
        height_hint=height_hint,
        trust=trust,
    )
