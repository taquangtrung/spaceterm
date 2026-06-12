"""OSC 9001 framing and capability detection for TBP.

This module owns the wire form so the public API in :mod:`spaceterm` stays about
*what* to render, not *how* it is encoded. The framing mirrors
``crates/proto`` and ``docs/terminal-block-protocol-spec.md``:

    OSC 9001 ; emit ; v=1,id=N,trust=TIER ; base64(json bundle) ST
"""

from __future__ import annotations

import base64
import itertools
import json
import os
import sys
from typing import TextIO

# ========================================================================
# Constants
# ========================================================================

PROTOCOL_VERSION = 1
TEXT_PLAIN = "text/plain"

_OSC_START = "\x1b]"
_ST = "\x1b\\"
_OSC_NUMBER = "9001"
_VERB_EMIT = "emit"

_SPACETERM_ENV = "SPACETERM"
_TERM_PROGRAM_ENV = "TERM_PROGRAM"
_SPACETERM_NAME = "spaceterm"

_VALID_TIERS = ("isolated", "restricted", "trusted")

_block_ids = itertools.count(1)


# ========================================================================
# Capability detection
# ========================================================================


def supported() -> bool:
    """Whether the current terminal is known to understand TBP.

    For now this is environment-based: SpaceTerm exports ``SPACETERM`` (and sets
    ``TERM_PROGRAM=spaceterm``).
    """
    if os.environ.get(_SPACETERM_ENV):
        return True
    return os.environ.get(_TERM_PROGRAM_ENV) == _SPACETERM_NAME


# ========================================================================
# Emission
# ========================================================================


def emit(
    mime: dict[str, str],
    *,
    title: str | None = None,
    height_hint: int | None = None,
    trust: str = "restricted",
    stream: TextIO | None = None,
) -> None:
    """Write ``mime`` as a TBP block, or its ``text/plain`` fallback elsewhere.

    ``mime`` maps each MIME type to its representation: a UTF-8 string for text
    and SVG payloads, a base64 string for binary images.
    """
    if trust not in _VALID_TIERS:
        raise ValueError(f"trust must be one of {_VALID_TIERS}, got {trust!r}")

    out = stream if stream is not None else sys.stdout
    if not supported():
        out.write(mime.get(TEXT_PLAIN, ""))
        out.flush()
        return

    out.write(_frame_emit(mime, title=title, height_hint=height_hint, trust=trust))
    out.flush()


def _frame_emit(
    mime: dict[str, str],
    *,
    title: str | None,
    height_hint: int | None,
    trust: str,
) -> str:
    bundle: dict[str, object] = {"mime": mime}
    meta: dict[str, object] = {}
    if title is not None:
        meta["title"] = title
    if height_hint is not None:
        meta["height_hint"] = height_hint
    if meta:
        bundle["meta"] = meta

    payload = base64.standard_b64encode(json.dumps(bundle).encode("utf-8")).decode(
        "ascii"
    )
    params = f"v={PROTOCOL_VERSION},id={next(_block_ids)},trust={trust}"
    return f"{_OSC_START}{_OSC_NUMBER};{_VERB_EMIT};{params};{payload}{_ST}"
