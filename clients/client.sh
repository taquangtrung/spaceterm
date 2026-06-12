#!/usr/bin/env bash
# client.sh — emit Terminal Block Protocol (TBP) blocks from the shell.
#
# Source it to use the spaceterm_emit_* functions, or run it as a CLI:
#   ./client.sh svg plot.svg
#   ./client.sh image chart.png
#   cat report.md | ./client.sh markdown -
#
# Every block carries a text/plain fallback, and when SpaceTerm is not the active
# terminal (no $SPACETERM, $TERM_PROGRAM != spaceterm) the fallback is printed instead,
# so pipelines stay safe under tmux/ssh/CI. Wire form mirrors
# docs/terminal-block-protocol-spec.md.

set -euo pipefail

# =============================================================================
# Constants
# =============================================================================

OSC_START=$'\e]'
ST=$'\e\\'
OSC_NUMBER="9001"
VERB_EMIT="emit"
PROTOCOL_VERSION="1"
DEFAULT_TRUST="restricted"
TEXT_PLAIN="text/plain"

SPACETERM_BLOCK_ID=0

die() { printf 'client.sh: %s\n' "$*" >&2; exit 1; }

# =============================================================================
# Capability detection
# =============================================================================

spaceterm_supported() {
    [[ -n "${SPACETERM:-}" ]] && return 0
    [[ "${TERM_PROGRAM:-}" == "spaceterm" ]]
}

# =============================================================================
# Emission
# =============================================================================

# Emit a UTF-8 text representation (svg/html/markdown/latex) with a fallback.
# Usage: spaceterm_emit_text <mime> <value> <fallback> [trust]
spaceterm_emit_text() {
    local mime="$1" value="$2" fallback="$3" trust="${4:-$DEFAULT_TRUST}"
    local bundle
    bundle="$(printf '{"mime":{"%s":"%s","%s":"%s"}}' \
        "$mime" "$(json_escape "$value")" \
        "$TEXT_PLAIN" "$(json_escape "$fallback")")"
    emit_bundle "$bundle" "$trust" "$fallback"
}

# Emit a base64 binary representation (images) with a fallback.
# Usage: spaceterm_emit_binary <mime> <base64> <fallback> [trust]
spaceterm_emit_binary() {
    local mime="$1" data_b64="$2" fallback="$3" trust="${4:-$DEFAULT_TRUST}"
    local bundle
    bundle="$(printf '{"mime":{"%s":"%s","%s":"%s"}}' \
        "$mime" "$data_b64" \
        "$TEXT_PLAIN" "$(json_escape "$fallback")")"
    emit_bundle "$bundle" "$trust" "$fallback"
}

# Frame a complete bundle JSON into an OSC 9001 escape, or print the fallback.
emit_bundle() {
    local bundle_json="$1" trust="$2" fallback="$3"
    if ! spaceterm_supported; then
        printf '%s' "$fallback"
        return 0
    fi
    local payload id
    payload="$(printf '%s' "$bundle_json" | base64 | tr -d '\n')"
    SPACETERM_BLOCK_ID=$((SPACETERM_BLOCK_ID + 1)); id="$SPACETERM_BLOCK_ID"
    printf '%s%s;%s;v=%s,id=%s,trust=%s;%s%s' \
        "$OSC_START" "$OSC_NUMBER" "$VERB_EMIT" \
        "$PROTOCOL_VERSION" "$id" "$trust" "$payload" "$ST"
}

# =============================================================================
# Typed helpers
# =============================================================================

spaceterm_emit_svg() {
    local content; content="$(read_source "${1:--}")"
    spaceterm_emit_text "image/svg+xml" "$content" "[svg image]"
}

spaceterm_emit_html() {
    local content; content="$(read_source "${1:--}")"
    spaceterm_emit_text "text/html" "$content" "[html block]"
}

spaceterm_emit_markdown() {
    local content; content="$(read_source "${1:--}")"
    # Raw Markdown reads fine as plain text, so it is its own fallback.
    spaceterm_emit_text "text/markdown" "$content" "$content"
}

spaceterm_emit_latex() {
    local content; content="$(read_source "${1:--}")"
    spaceterm_emit_text "text/latex" "$content" "$content"
}

spaceterm_emit_image() {
    local file="$1" mime data_b64
    [[ -r "$file" ]] || die "cannot read image: $file"
    mime="$(image_mime_for "$file")"
    data_b64="$(base64 < "$file" | tr -d '\n')"
    spaceterm_emit_binary "$mime" "$data_b64" "[$mime image]"
}

# =============================================================================
# Helpers
# =============================================================================

# JSON-escape a string for embedding inside a double-quoted JSON value. Covers
# the characters that appear in HTML/SVG/Markdown; binary data takes the base64
# path and never reaches here.
json_escape() {
    local s="$1" out="" i char code
    for (( i=0; i<${#s}; i++ )); do
        char="${s:i:1}"
        case "$char" in
            '"') out+='\"' ;;
            '\\') out+='\\' ;;
            $'\n') out+='\n' ;;
            $'\r') out+='\r' ;;
            $'\t') out+='\t' ;;
            $'\b') out+='\b' ;;
            $'\f') out+='\f' ;;
            *)
                code=$(printf '%d' "'$char")
                if (( code < 32 )); then
                    out+=$(printf '\\u%04x' "$code")
                else
                    out+="$char"
                fi
                ;;
        esac
    done
    printf '%s' "$out"
}

read_source() {
    local src="${1:--}"
    if [[ "$src" == "-" ]]; then
        cat
    else
        [[ -r "$src" ]] || die "cannot read: $src"
        cat "$src"
    fi
}

image_mime_for() {
    case "${1##*.}" in
        png) printf 'image/png' ;;
        jpg | jpeg) printf 'image/jpeg' ;;
        gif) printf 'image/gif' ;;
        webp) printf 'image/webp' ;;
        svg) printf 'image/svg+xml' ;;
        *) printf 'application/octet-stream' ;;
    esac
}

usage() {
    cat >&2 <<'EOF'
usage: client.sh <command> [args]
  svg       <file|->     emit an SVG document
  html      <file|->     emit an HTML fragment
  markdown  <file|->     emit Markdown
  latex     <file|->     emit a LaTeX expression
  image     <file>       emit a raster image (mime from extension)
  supported              print yes/no for TBP capability
EOF
}

# =============================================================================
# CLI
# =============================================================================

main() {
    local command="${1:-}"
    shift || true
    case "$command" in
        svg) spaceterm_emit_svg "$@" ;;
        html) spaceterm_emit_html "$@" ;;
        markdown | md) spaceterm_emit_markdown "$@" ;;
        latex) spaceterm_emit_latex "$@" ;;
        image) spaceterm_emit_image "$@" ;;
        supported) spaceterm_supported && echo yes || echo no ;;
        *) usage; return 2 ;;
    esac
}

if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
    main "$@"
fi
