# Terminal Block Protocol (TBP) — Specification

**Version:** 1 · **Status:** Draft · Reference codec: `crates/proto` (`spaceterm-proto`)

TBP is an OSC escape sequence carrying a MIME bundle, directly inspired by
Jupyter's `display_data` message. A tool emits a *block*; the terminal renders the
richest representation it supports and falls back toward `text/plain`. Terminals
that do not understand TBP ignore the escape, so output degrades gracefully into a
dumb terminal, `cat`, or a log file.

This document is normative. The `spaceterm-proto` crate is its reference encoder and
decoder; the two are kept in lockstep — a change here is an API change there.

---

## 1. Wire format

A TBP message is exactly one OSC escape:

```
OSC  9001 ; <verb> ; <params> ; <payload>  ST
```

| Token | Bytes | Notes |
|---|---|---|
| `OSC` | `ESC ]` (`0x1B 0x5D`) | OSC introducer |
| `9001` | ASCII digits | provisional private-use number; final number TBD |
| `ST` | `ESC \` (`0x1B 0x5C`) | string terminator |

- Fields are separated by `;`.
- `<params>` is a comma-separated list of `key=value` pairs. It may be empty.
- `<payload>`, when present, is **base64-encoded JSON**. Base64 keeps the payload
  free of control bytes so the whole message stays a single, opaque OSC.
- A terminal that does not recognize OSC 9001 discards the escape; tools targeting
  unknown terminals also print the `text/plain` form *outside* the escape (the
  client libraries do this via capability detection, §6).

> **Note on the proposal's illustrative `… ST <payload> ST` form.** The base64
> payload rides **inside** a single OSC, terminated by one `ST` (proposal §3.7).
> This spec is the authoritative framing; `spaceterm-proto::wire` implements it.

### Verbs

| Verb | Direction | Params | Payload | Meaning |
|---|---|---|---|---|
| `emit` | tool → term | `v`, `id`, `trust` | MIME bundle | one-shot block |
| `open` | tool → term | `id`, `mime` | initial spec | begin a live block |
| `patch` | tool → term | `id` | RFC 6902 patch | update a live block |
| `close` | tool → term | `id` | — | end a live block |
| `caps` | tool → term | — | — | query capabilities (§6) |

### Parameters

| Key | Type | Default | Used by |
|---|---|---|---|
| `v` | integer | — | `emit` (protocol version) |
| `id` | integer (u64) | — (required) | `emit`, `open`, `patch`, `close` |
| `trust` | `trusted` \| `restricted` \| `isolated` | `restricted` | `emit` |
| `mime` | string | — (required) | `open` |

---

## 2. The MIME bundle

`emit`'s payload decodes to a bundle of alternative representations plus optional
metadata:

```json
{
  "mime": {
    "text/plain":    "id   name   score\n1    alpha  0.92\n…",
    "text/html":     "<table>…</table>",
    "image/svg+xml": "<svg>…</svg>",
    "application/vnd.vega-lite+json": { "$schema": "…", "mark": "bar" }
  },
  "meta": { "title": "query results", "height_hint": 12 }
}
```

- `mime` maps a MIME type to its representation. A value is a JSON **string** for
  text payloads, a JSON **object** for structured specs, and a **base64 string**
  for binary payloads (raster images, audio, video, PDF) — the raw bytes
  base64-encoded, following Jupyter's `display_data` convention. UTF-8 formats
  (`text/*`, `image/svg+xml`) are sent as plain strings, not base64.
- `meta.title` (string) and `meta.height_hint` (integer rows) are optional and
  omitted from the wire form when absent.
- Tools **should** always include `text/plain` as the mandatory fallback.
- The terminal selects the richest type it can render; otherwise it walks down to
  `text/plain`.

### Target MIME set (v1)

`text/plain` · `text/html` · `text/markdown` · `text/latex` · `text/csv` ·
`image/{png,jpeg,webp,avif,gif,svg+xml}` · `video/{mp4,webm}` ·
`audio/{mpeg,wav}` · `application/pdf` · `application/json` ·
`application/vnd.vega-lite+json` · `application/vnd.plotly+json` ·
`model/gltf+json` · `application/wasm`.

Binary/native formats (`.xlsx`, Parquet, HDF5, DICOM) are out of scope: client
libraries or backends convert them to a web-native MIME type before emission.

---

## 3. Live blocks

A tool opens a block and streams incremental updates:

```
OSC 9001 ; open  ; id=7,mime=application/vnd.vega-lite+json ; <base64 initial spec>  ST
OSC 9001 ; patch ; id=7 ; <base64 RFC 6902 patch>                                    ST
OSC 9001 ; close ; id=7                                                              ST
```

`id` correlates the three. A `patch` payload is a JSON array of RFC 6902
operations applied to the block's current state.

---

## 4. Trust tiers

A block's content is granted capability according to its tier. The tool
*requests* a tier; the terminal *clamps* it by policy. Untrusted content never
executes in the main UI context.

| Tier | Source | Sandbox |
|---|---|---|
| `trusted` | first-party tools, user allowlist | full DOM, scripts |
| `restricted` | unknown local CLIs (**default**) | CSP, no network, no top-level nav |
| `isolated` | network / AI output | sandboxed iframe, unique origin, no scripts unless opted in |

---

## 5. Block boundaries

Two block kinds coexist:

- **Command blocks** — each command's output as a navigable, foldable,
  exit-code-tagged unit, derived from **OSC 133** prompt/command marks. These work
  for every command without the tool being TBP-aware.
- **Content blocks** — a rendered chart, table, PDF, or equation from an explicit
  TBP `emit`, nesting inside the surrounding command block's output region.

OSC 133 is the primary boundary mechanism, complemented by explicit TBP emission.
When shell integration is absent the terminal falls back to a heuristic (prompt
detection, else a single rolling block); explicit content emission never depends on
OSC 133.

---

## 6. Capability negotiation

```
OSC 9001 ; caps  ST
→ reply on the tool's stdin (JSON, not an escape):
{ "v": 1, "mime": ["text/html","image/svg+xml", …], "live": true,
  "tiers": ["trusted","restricted"], "side_channel": false }
```

Client libraries cache the reply and silently fall back to `text/plain` when no
TBP-capable terminal is detected, so a tool built for SpaceTerm still works under
tmux/ssh/CI.

---

## 7. Transport

- **In-band (mandatory, universal).** The base64 payload rides the normal PTY
  stream inside the OSC. Works over SSH, pipes, and tmux (with passthrough).
- **Local side channel (optional, negotiated).** When tool and terminal share a
  host *and* the terminal advertised `side_channel` via `caps`, large payloads
  (video, big PDFs, hi-res images) may be written to shared memory or a temp file,
  with the escape carrying only a small reference. Falls back to in-band base64
  automatically when unavailable (e.g. remote sessions). Implementing in-band is
  sufficient for any tool; the side channel is a transparent optimization.

---

## 8. Versioning

Every bundle declares `v`. A terminal accepts any version at or below the one it
implements (`Version::is_supported`). The `text/plain` fallback is never broken
across versions.

---

## 9. Relationship to legacy image protocols

TBP is the additive rich layer. Existing inline-image protocols — Kitty graphics,
iTerm2 `OSC 1337 ; File=`, and Sixel — remain the compatibility floor and are
normalized by the terminal into `image/*` blocks so they flow through the same
compositing path. OSC 8 hyperlinks, OSC 133 marks, and OSC 7 cwd reporting are
honored as described in the proposal (§3.4).
