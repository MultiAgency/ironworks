#!/usr/bin/env bash
# Refresh one cell's short-lived confinement evidence. Drift removes; outage ages naturally.
set -uo pipefail

[ "$#" -eq 1 ] || { echo "usage: refresh-attestation.sh CLIENT" >&2; exit 64; }
client="$1"
case "$client" in
  ""|*[!a-z0-9-]*|-*|*-) echo "invalid client slug" >&2; exit 1 ;;
esac

directory="/run/ironworks-attest/$client"
current="$directory/current.json"
store=/var/lib/ironworks/lifecycle.store
bin=/opt/ironworks/target/release/ironworks
install -d -o root -g root -m 0755 "$directory" || exit 3
temporary="$(mktemp "$directory/.current.XXXXXX")" || exit 3
trap 'rm -f "$temporary"' EXIT
chmod 0644 "$temporary" || exit 3

"$bin" lifecycle attest "$client" --store "$store" >"$temporary"
rc=$?
case "$rc" in
  0)
    sync -f "$temporary" || exit 3
    mv -f "$temporary" "$current" || exit 3
    trap - EXIT
    sync -f "$directory" || exit 3
    ;;
  2)
    rm -f "$current"
    sync -f "$directory" || true
    exit 2
    ;;
  *)
    # An unavailable measurement leaves the last good file to expire on its own.
    exit 3
    ;;
esac
