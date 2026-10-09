#!/bin/sh
set -eu
exec docker exec --user mersdk --workdir "$PWD" "${AUDB_SDK_CONTAINER:-aurora-os-build-engine-5.2.1.200-mb2_kotdath}" \
    sb2 -t "${AUDB_SDK_TARGET:-AuroraOS-5.2.1.200-aarch64}" -m sdk-build gcc "$@"
