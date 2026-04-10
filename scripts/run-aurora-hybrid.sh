#!/usr/bin/env bash
set -euo pipefail

usage() {
    cat <<'EOF'
Usage:
  run-aurora-hybrid.sh [--print-only]

Environment overrides:
  AURORA_SDK_ROOT   Aurora SDK root
  EMULATOR_RELEASE  Emulator release directory under $AURORA_SDK_ROOT/emulator
  EMULATOR_NAME     QEMU window/name
  QEMU_BIN          Path to qemu-system-x86_64
  QEMU_IMG_BIN      Path to qemu-img
  BASE_IMAGE        Base qcow2 image
  OVERLAY_IMAGE     Overlay qcow2 image for manual runs
  SSH_PORT          Host SSH port forwarded to guest 22
  QMP_SOCKET        Unix socket path for QMP
  PIDFILE           QEMU pid file
  VM_MAC            VM MAC address
  VM_MEMORY_MB      Guest RAM size in MiB
  VM_CPUS           Guest CPU count
  VIEW_WIDTH        Guest visible width in px
  VIEW_HEIGHT       Guest visible height in px
  VGA_DEVICE        Display device prefix, default virtio-vga-gl
  DISPLAY_OPTS      Value passed to -display
  LIBGL_ALWAYS_SOFTWARE
                    Defaults to 1 on this machine
  QEMU_EXTRA_ARGS   Extra arguments appended to the QEMU command

Notes:
  - Hybrid mode starts both virtio tablet and virtio multitouch.
  - Manual mouse use stays on the tablet device.
  - Automated touch/swipe should target QMP mtt events via $QMP_SOCKET.
EOF
}

PRINT_ONLY=0

while (($#)); do
    case "$1" in
        --print-only)
            PRINT_ONLY=1
            shift
            ;;
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
done

: "${AURORA_SDK_ROOT:=/home/kotdath/AuroraOS}"
: "${EMULATOR_RELEASE:=AuroraOS-5.2.0.180}"
: "${EMULATOR_NAME:=AuroraOS-5.2.0.180-hybrid}"
: "${QEMU_BIN:=$AURORA_SDK_ROOT/share/qemu/bin/qemu-system-x86_64}"
: "${QEMU_IMG_BIN:=$AURORA_SDK_ROOT/share/qemu/bin/qemu-img}"
: "${BASE_IMAGE:=$AURORA_SDK_ROOT/emulator/$EMULATOR_RELEASE/image.qcow2}"
: "${OVERLAY_IMAGE:=$AURORA_SDK_ROOT/emulator/overlay_test.qcow2}"
: "${SSH_PORT:=32223}"
: "${QMP_SOCKET:=/tmp/aurora-hybrid.qmp}"
: "${PIDFILE:=/tmp/aurora-hybrid.pid}"
: "${VM_MAC:=52:54:00:12:34:57}"
: "${VM_MEMORY_MB:=4096}"
: "${VM_CPUS:=2}"
: "${VIEW_WIDTH:=360}"
: "${VIEW_HEIGHT:=800}"
: "${VGA_DEVICE:=virtio-vga-gl}"
: "${DISPLAY_OPTS:=sdl,gl=on,show-cursor=off}"
: "${LIBGL_ALWAYS_SOFTWARE:=1}"
: "${QEMU_EXTRA_ARGS:=}"

VM_DIR="$AURORA_SDK_ROOT/emulator/$EMULATOR_RELEASE"
CONFIGS_DIR="$VM_DIR/vmshare"
MEDIA_DIR="$AURORA_SDK_ROOT/emulator/media"
SSH_DIR="$VM_DIR/ssh"

for required in "$QEMU_BIN" "$QEMU_IMG_BIN" "$BASE_IMAGE" "$CONFIGS_DIR" "$MEDIA_DIR" "$SSH_DIR"; do
    if [[ ! -e "$required" ]]; then
        printf 'Required path is missing: %s\n' "$required" >&2
        exit 1
    fi
done

if [[ ! -e "$OVERLAY_IMAGE" ]]; then
    printf 'Creating overlay image: %s\n' "$OVERLAY_IMAGE"
    "$QEMU_IMG_BIN" create -f qcow2 -F qcow2 -b "$BASE_IMAGE" "$OVERLAY_IMAGE"
fi

rm -f "$QMP_SOCKET" "$PIDFILE"

QEMU_ARGS=(
    -M q35,i8042=off
    -cpu host
    -m "${VM_MEMORY_MB}M"
    -smp "$VM_CPUS"
    --enable-kvm
    -name "$EMULATOR_NAME"
    -device "${VGA_DEVICE},xres=${VIEW_WIDTH},yres=${VIEW_HEIGHT}"
    -display "${DISPLAY_OPTS}"
    -device virtio-tablet-pci
    -device virtio-multitouch-pci
    -device ahci,id=ahci
    -device ide-hd,drive=disk,bus=ahci.0
    -audiodev sdl,id=audiodev0
    -device intel-hda
    -device hda-output,audiodev=audiodev0
    -nodefaults
    -drive "id=disk,file=${OVERLAY_IMAGE},if=none"
    -nic "user,mac=${VM_MAC},hostfwd=tcp::${SSH_PORT}-:22"
    -virtfs "local,path=${CONFIGS_DIR},mount_tag=configs,security_model=mapped,readonly=on"
    -virtfs "local,path=${MEDIA_DIR},mount_tag=media,security_model=mapped,readonly=on"
    -virtfs "local,path=${SSH_DIR},mount_tag=ssh,security_model=mapped,readonly=on"
    -qmp "unix:${QMP_SOCKET},server=on,wait=off"
    -pidfile "$PIDFILE"
)

if [[ -n "$QEMU_EXTRA_ARGS" ]]; then
    # Intentionally split on shell words to make ad-hoc extra args convenient.
    # Example: QEMU_EXTRA_ARGS='-nic none -serial mon:stdio'
    read -r -a extra_args <<<"$QEMU_EXTRA_ARGS"
    QEMU_ARGS+=("${extra_args[@]}")
fi

printf 'Aurora SDK root: %s\n' "$AURORA_SDK_ROOT"
printf 'Base image:       %s\n' "$BASE_IMAGE"
printf 'Overlay image:    %s\n' "$OVERLAY_IMAGE"
printf 'SSH port:         %s\n' "$SSH_PORT"
printf 'QMP socket:       %s\n' "$QMP_SOCKET"
printf 'PID file:         %s\n' "$PIDFILE"
printf 'VM name:          %s\n' "$EMULATOR_NAME"
printf 'Touch mode:       hybrid (tablet + multitouch)\n'
printf '\nQEMU command:\n'
printf '  %q' "$QEMU_BIN"
printf ' %q' "${QEMU_ARGS[@]}"
printf '\n\n'

if (( PRINT_ONLY )); then
    exit 0
fi

exec env LIBGL_ALWAYS_SOFTWARE="$LIBGL_ALWAYS_SOFTWARE" "$QEMU_BIN" "${QEMU_ARGS[@]}"
