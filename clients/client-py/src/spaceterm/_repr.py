"""Convert a Python object to a TBP MIME map via Jupyter's ``_repr_*_`` protocol.

Objects that already render in Jupyter (pandas DataFrames, matplotlib figures,
SymPy expressions, ...) expose ``_repr_html_`` / ``_repr_svg_`` / ``_repr_png_`` /
``_repr_mimebundle_`` and so on. Reusing that protocol means those objects render
in SpaceTerm with no extra work.
"""

from __future__ import annotations

import base64

from spaceterm._wire import TEXT_PLAIN

# ========================================================================
# Constants
# ========================================================================

# Jupyter repr method -> MIME type. A method returning ``bytes`` is base64-encoded;
# one returning ``str`` is used verbatim.
_REPR_METHODS = {
    "_repr_html_": "text/html",
    "_repr_jpeg_": "image/jpeg",
    "_repr_latex_": "text/latex",
    "_repr_markdown_": "text/markdown",
    "_repr_png_": "image/png",
    "_repr_svg_": "image/svg+xml",
}


# ========================================================================
# Object -> MIME map
# ========================================================================


def mime_map_from_object(obj: object) -> dict[str, str]:
    """Best available MIME representations of ``obj``, always including text/plain."""
    bundle = _from_mimebundle(obj)
    if bundle is None:
        bundle = _from_repr_methods(obj)
    bundle.setdefault(TEXT_PLAIN, obj if isinstance(obj, str) else str(obj))
    return bundle


def _from_mimebundle(obj: object) -> dict[str, str] | None:
    method = getattr(obj, "_repr_mimebundle_", None)
    if method is None:
        return None
    result = method()
    data = result[0] if isinstance(result, tuple) else result
    if not isinstance(data, dict):
        return None
    return {mime: _as_payload(value) for mime, value in data.items()}


def _from_repr_methods(obj: object) -> dict[str, str]:
    out: dict[str, str] = {}
    for method_name, mime in _REPR_METHODS.items():
        method = getattr(obj, method_name, None)
        if method is None:
            continue
        value = method()
        if value is not None:
            out[mime] = _as_payload(value)
    return out


def _as_payload(value: object) -> str:
    if isinstance(value, (bytes, bytearray)):
        return base64.standard_b64encode(bytes(value)).decode("ascii")
    return str(value)
