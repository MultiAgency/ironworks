#!/usr/bin/env bash
# Back up and migrate the Account Store using the exact service image/dependencies.
set -euo pipefail
cd "$(dirname "$0")"
. ../../lib/fleet.sh

action=${1:-apply}
case "$action" in
  apply|status) ;;
  *) echo "usage: $0 [apply|status]" >&2; exit 64 ;;
esac

[ -f "$FLEET_AGENCY_DIR/account-db.env" ] || {
  echo "!! missing $FLEET_AGENCY_DIR/account-db.env" >&2; exit 2; }
# `set -a` IS REQUIRED (CONTRIBUTING.md, "Sourcing an env file"): docker compose reads
# ${ACCOUNT_DB_PASSWORD:?} from its own environment, and this file never names it.
set -a; . "$FLEET_AGENCY_DIR/account-db.env"; set +a

docker compose up -d account-db
docker compose build account-service
ready=0
for _attempt in $(seq 1 60); do
  if docker compose exec -T account-db pg_isready -U postgres -d accounts >/dev/null; then
    ready=1
    break
  fi
  sleep 1
done
[ "$ready" -eq 1 ] || { echo "!! Account Store database did not become ready" >&2; exit 2; }

run_migrations() {
  docker compose run --rm --no-deps account-service \
    python3 /app/deploy/account-intel/data/migrations.py "$1"
}

if [ "$action" = apply ]; then
  if run_migrations status >/dev/null; then
    echo "Account Store schema already current"
    exit 0
  fi
  backup_dir="$FLEET_AGENCY_DIR/account-db-migrations"
  mkdir -p "$backup_dir"
  chmod 700 "$backup_dir"
  stamp=$(date -u +%Y%m%dT%H%M%SZ)
  backup="$backup_dir/accounts-pre-migrate-$stamp.sql.gz"
  umask 077
  docker compose exec -T account-db pg_dump -U postgres -d accounts | gzip -9 > "$backup"
  gzip -t "$backup"
  echo "migration backup: $backup"
fi

run_migrations "$action"
