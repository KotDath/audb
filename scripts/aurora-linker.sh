#!/bin/sh
set -eu
container="${AUDB_SDK_CONTAINER:-aurora-os-build-engine-5.2.1.200-mb2_kotdath}"
target="${AUDB_SDK_TARGET:-AuroraOS-5.2.1.200-aarch64}"
# A Cargo triple aimed at the wrong sb2 target would link against another platform's
# libc and produce an ELF with the wrong loader, so check the target's own compiler
# before linking. AUDB_EXPECT_MACHINE is the family prefix (armv7 or aarch64).
if [ -n "${AUDB_EXPECT_MACHINE:-}" ]; then
    machine=$(docker exec --user mersdk "$container" sb2 -t "$target" -m sdk-build gcc -dumpmachine)
    case "$machine" in
        "${AUDB_EXPECT_MACHINE}"*) ;;
        *)
            echo "aurora-linker: sb2 target $target reports $machine, expected ${AUDB_EXPECT_MACHINE}-*" >&2
            exit 1
            ;;
    esac
fi
exec docker exec --user mersdk --workdir "$PWD" "$container" \
    sb2 -t "$target" -m sdk-build gcc "$@"
