# Invoked as sh -c SCRIPT audb-rpm KEY_ID rpm ARGS... by root.
# This is an Aurora-specific developer install, not a global trust change.
set -eu
key_id=$1
shift
umask 077
trust_dir=$(mktemp -d /tmp/audb-rpm-trust.XXXXXX)
cleanup() {
    rm -f -- "$trust_dir/keyid" "$trust_dir/etc/rpmsign-external/rpmsign-external.conf"
    rmdir -- "$trust_dir/etc/rpmsign-external" "$trust_dir/etc" "$trust_dir" 2>/dev/null || :
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM HUP
mkdir -p "$trust_dir/etc/rpmsign-external"
printf '%s\n' "$key_id" > "$trust_dir/keyid"
# Preserve the OS database, CA, revocation and developer-mode policy. Refuse an
# unfamiliar configuration instead of falling back to disabling validation.
awk -v keyfile="$trust_dir/keyid" '
    /^[[:space:]]*\[/ { main = ($0 ~ /^[[:space:]]*\[main\][[:space:]]*$/) }
    main && /^[[:space:]]*system_developer_keyid_path[[:space:]]*=/ {
        print "system_developer_keyid_path = " keyfile
        found++
        next
    }
    { print }
    END { if (found != 1) exit 42 }
' /etc/rpmsign-external/rpmsign-external.conf > "$trust_dir/etc/rpmsign-external/rpmsign-external.conf"
RPMSIGN_EXTERNAL_ROOTFS="$trust_dir" "$@"
