#!/usr/bin/env bash
# Root preflight for the two explicit TCP ports owned by one serving cell.
set -euo pipefail

[ "$#" -eq 1 ] || { echo "usage: check-serving-cell.sh CLIENT" >&2; exit 64; }
client="$1"
case "$client" in
  ""|*[!a-z0-9-]*|-*|*-) echo "invalid client slug" >&2; exit 1 ;;
esac

config="/etc/ironworks/cell-$client.env"
[ -f "$config" ] && [ ! -L "$config" ] || { echo "missing regular cell config: $config" >&2; exit 1; }
owner="$(stat -c '%u' "$config" 2>/dev/null || stat -f '%u' "$config")"
mode="$(stat -c '%a' "$config" 2>/dev/null || stat -f '%Lp' "$config")"
if [ "$owner" != 0 ] || (( (8#$mode & 0077) != 0 )); then
  echo "cell config must be root-owned and mode 0600: $config" >&2
  exit 1
fi

port_value() {
  local file="$1" key="$2"
  awk -F= -v key="$key" '
    $1 == key { count++; value=$2 }
    END { if (count == 1 && value ~ /^[1-9][0-9]{0,4}$/ && value + 0 <= 65535) print value }
  ' "$file"
}

adapter="$(port_value "$config" ADAPTER_PORT)"
runtime="$(port_value "$config" IRONCLAW_PORT)"
[ -n "$adapter" ] && [ -n "$runtime" ] || {
  echo "cell config needs one literal ADAPTER_PORT and IRONCLAW_PORT" >&2; exit 1;
}
[ "$adapter" != "$runtime" ] || { echo "adapter and IronClaw ports collide in $client" >&2; exit 1; }

for other in /etc/ironworks/cell-*.env; do
  [ -e "$other" ] || continue
  [ "$other" = "$config" ] && continue
  for key in ADAPTER_PORT IRONCLAW_PORT; do
    value="$(port_value "$other" "$key")"
    [ -z "$value" ] || { [ "$value" != "$adapter" ] && [ "$value" != "$runtime" ]; } || {
      echo "$client port collides with $other ($key=$value)" >&2; exit 1;
    }
  done
done
