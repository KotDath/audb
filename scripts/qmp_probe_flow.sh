#!/usr/bin/env bash
set -euo pipefail

usage() {
    cat <<'EOF'
Usage:
  qmp_probe_flow.sh

Environment overrides:
  SOCKET        QMP socket path
  OUT_DIR       Output directory for screenshots
  TAP_X         Tap X coordinate in px
  TAP_Y         Tap Y coordinate in px
  TAP_HOLD_MS   Tap hold duration in ms

This script runs the confirmed end-to-end probe flow:
  1. swipe up
  2. screendump after swipe
  3. tap known dock/icon coordinate
  4. screendump after tap
EOF
}

if (($#)); then
    case "$1" in
        -h|--help)
            usage
            exit 0
            ;;
        *)
            printf 'Unknown argument: %s\n\n' "$1" >&2
            usage >&2
            exit 2
            ;;
    esac
fi

: "${SOCKET:=/tmp/aurora-screendump.qmp}"
: "${OUT_DIR:=/tmp/aurora-probe-flow}"
: "${TAP_X:=134}"
: "${TAP_Y:=746}"
: "${TAP_HOLD_MS:=90}"

TOUCH_HELPER=/home/kotdath/omp/personal/rust/audb/scripts/qmp_touch.py
SCREEN_HELPER=/home/kotdath/omp/personal/rust/audb/scripts/qmp_screendump.py

mkdir -p "$OUT_DIR"

AFTER_SWIPE="$OUT_DIR/after-swipe.png"
AFTER_TAP="$OUT_DIR/after-tap.png"

printf 'QMP socket: %s\n' "$SOCKET"
printf 'Output dir: %s\n' "$OUT_DIR"
printf 'Tap point:  %s,%s\n' "$TAP_X" "$TAP_Y"
printf '\n[1/4] swipe-up\n'
python3 "$TOUCH_HELPER" --socket "$SOCKET" swipe-up

printf '\n[2/4] screendump after swipe\n'
python3 "$SCREEN_HELPER" --socket "$SOCKET" --output "$AFTER_SWIPE"

printf '\n[3/4] tap\n'
python3 "$TOUCH_HELPER" --socket "$SOCKET" tap --at "${TAP_X},${TAP_Y}" --hold-ms "$TAP_HOLD_MS"

printf '\n[4/4] screendump after tap\n'
python3 "$SCREEN_HELPER" --socket "$SOCKET" --output "$AFTER_TAP"

printf '\nArtifacts:\n'
printf '  %s\n' "$AFTER_SWIPE"
printf '  %s\n' "$AFTER_TAP"
