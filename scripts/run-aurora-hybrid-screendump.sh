#!/usr/bin/env bash
set -euo pipefail

# Experimental launch profile for testing QMP screendump without GL.
# Keeps the hybrid input setup but switches display/VGA defaults away from the
# working GL profile so screenshots can be tested independently.

export EMULATOR_NAME="${EMULATOR_NAME:-AuroraOS-5.2.0.180-hybrid-screendump}"
export OVERLAY_IMAGE="${OVERLAY_IMAGE:-/home/kotdath/AuroraOS/emulator/overlay_screendump.qcow2}"
export SSH_PORT="${SSH_PORT:-33223}"
export QMP_SOCKET="${QMP_SOCKET:-/tmp/aurora-screendump.qmp}"
export PIDFILE="${PIDFILE:-/tmp/aurora-screendump.pid}"
export VM_MAC="${VM_MAC:-52:54:00:12:34:58}"
export VGA_DEVICE="${VGA_DEVICE:-virtio-vga}"
export DISPLAY_OPTS="${DISPLAY_OPTS:-sdl,show-cursor=off}"

exec /home/kotdath/omp/personal/rust/audb/scripts/run-aurora-hybrid.sh "$@"
