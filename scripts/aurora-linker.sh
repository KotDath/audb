#!/bin/sh
set -eu
container="${AUDB_SDK_CONTAINER:-aurora-os-build-engine-5.2.1.200-mb2_kotdath}"
target="${AUDB_SDK_TARGET:-AuroraOS-5.2.1.200-aarch64}"
# AUDB_CROSS_CC selects the platform's compiler driver inside the container: `gcc` for the
# native aarch64 build, /opt/cross/bin/armv7hl-meego-linux-gnueabi-gcc for 32-bit ARM.
cc="${AUDB_CROSS_CC:-gcc}"
# A Cargo triple aimed at the wrong platform would link against another libc and produce an
# ELF with the wrong loader, so check the driver's own machine before linking.
if [ -n "${AUDB_EXPECT_MACHINE:-}" ]; then
    machine=$(docker exec --user mersdk "$container" sb2 -t "$target" -m sdk-build "$cc" -dumpmachine)
    case "$machine" in
        "${AUDB_EXPECT_MACHINE}"*) ;;
        *)
            echo "aurora-linker: $cc reports $machine, expected ${AUDB_EXPECT_MACHINE}-*" >&2
            exit 1
            ;;
    esac
fi
exec docker exec --user mersdk --workdir "$PWD" "$container" \
    sb2 -t "$target" -m sdk-build "$cc" "$@"
