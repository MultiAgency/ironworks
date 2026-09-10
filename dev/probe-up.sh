#!/usr/bin/env bash
# COST: one model turn per bring-up — a warm-up, discarded deliberately, because a
# stack's first turn is not a measurement. `--down` destroys the volumes and every thread on the
# instance with them. Needs a model key.
# Bring up a disposable IronClaw at the pinned revision, or tear one down.
#
#   dev/probe-up.sh              bring it up and print how to reach it
#   dev/probe-up.sh --sandboxed  the same, on the SERVED profile
#   dev/probe-up.sh --down       tear it down, volumes included
#   dev/probe-up.sh --logs       follow the runtime log
#
# Guardrails first, because the failure this exists to prevent is pointing a probe at something
# real. It refuses to run if its own containers are already up or its port is taken, mints every
# secret per run, and never reads operator state.
#
# `--sandboxed` swaps `production` for `hosted-single-tenant-volume-sandboxed` — the profile
# `deploy/ironworks-ironclaw@.service` sets and the one §L-10 and `dev/prove-workspace.sh` need. It
# costs a private Docker daemon, a worker image build, and outbound network for the egress proxy;
# `dev/docker-compose.probe-sandboxed.yml` explains each. **The two are not interchangeable**: a
# contract measured on one has not been measured on the other, which is the whole reason the flag
# exists rather than a default.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/.." && pwd)"
PROJECT=multiprobe
PORT="${PROBE_PORT:-3997}"
STORE_PORT="${PROBE_STORE_PORT:-3996}"
STORE_PID="${TMPDIR:-/tmp}/ironworks-devstore.$STORE_PORT.pid"
# Synthetic throughout. This credential authorises nothing anywhere else.
STORE_TOKEN="dev-token-eval"
STORE_ORG="eval"
COORD_TOKEN="dev-token-coordination"
COORD_ORG="coordination-eval"

die() { printf 'probe: %s\n' "$*" >&2; exit 1; }

# ── which stack ─────────────────────────────────────────────────────────────────────────────
# The overlay is included for EVERY teardown, whether or not this invocation asked for it: the two
# stacks share a project name, so a `--down` that omitted it would leave `dind` and the workspace
# volume behind for the next run to inherit. Naming a service that is not running is harmless;
# leaving a daemon and a populated volume behind is not.
SANDBOXED=0
ACTION=up
for arg in "$@"; do
  case "$arg" in
    --sandboxed) SANDBOXED=1 ;;
    --down|--logs) ACTION="${arg#--}" ;;
    up|"") ;;
    *) die "unknown argument ${arg@Q}; use --sandboxed, --down or --logs" ;;
  esac
done

COMPOSE=(docker compose -p "$PROJECT" -f "$HERE/docker-compose.probe.yml")
if [ "$SANDBOXED" -eq 1 ] || [ "$ACTION" = down ]; then
  COMPOSE+=(-f "$HERE/docker-compose.probe-sandboxed.yml")
fi

# Teardown must not depend on bring-up secrets. `docker compose` interpolates the compose file
# for EVERY subcommand, so the `:?` required-variable guards in it would otherwise make `--down`
# and `--logs` fail — exactly when you most need them, on a stack that is already broken. These
# placeholders satisfy interpolation; the `up` path overwrites them with minted values below.
export PROBE_IMAGE="${PROBE_IMAGE:-ironclaw:placeholder}"
export PROBE_PGPW="${PROBE_PGPW:-placeholder}"
export PROBE_MASTER_KEY="${PROBE_MASTER_KEY:-placeholder}"
export PROBE_WEBUI_TOKEN="${PROBE_WEBUI_TOKEN:-placeholder}"

case "$ACTION" in
  down)
    if [ -f "$STORE_PID" ]; then
      kill "$(cat "$STORE_PID")" 2>/dev/null || true
      rm -f "$STORE_PID"
      printf 'probe: record store stopped\n'
    fi
    "${COMPOSE[@]}" down -v --remove-orphans
    exit 0
    ;;
  logs)
    "${COMPOSE[@]}" logs -f ic
    exit 0
    ;;
  up) ;;
esac

# A stack's first turn is not a measurement. Degeneracy — the runtime emitting its own control
# tokens instead of prose — landed on 2 of 3 first turns after bring-up and 0 of the 47 turns that
# followed (docs/experiments/0018). Left alone that costs every probe its run 1, either as a
# BLOCKED that shrinks the denominator or, before the seam refused to deliver it, as token soup
# scored as a reasoning failure. So spend one turn here, where it is not being counted.
#
# It bills one turn per bring-up. It is not allowed to fail the script: a warm-up that degenerates
# is the warm-up doing its job, and a runtime too broken to answer is the probe's finding to make,
# not this script's.
warm_runtime() {
  local journal; journal="$(mktemp -d)/warm.db"
  # The driver builds this too, but it runs after bring-up, so it may not exist yet.
  ( cd "$ROOT" && cargo build -q -p ironworks-cli ) || {
    printf 'probe: cannot build ironworks; skipping the warm-up turn\n'; return 0; }
  printf 'probe: warming the runtime (one billed turn, discarded) ... '
  if IRONWORKS_BASE="http://127.0.0.1:$PORT" IRONWORKS_TOKEN="$PROBE_WEBUI_TOKEN" \
     IRONWORKS_RECORDS="http://127.0.0.1:$STORE_PORT" IRONWORKS_RECORDS_TOKEN="$COORD_TOKEN" \
     "$ROOT/target/debug/ironworks" coordinate \
       --slug coordination-eval \
       --guidance "$ROOT/fixtures/account-coordination/guidance.md" \
       --journal "$journal" \
       --ask "Reply with the single word: ready." >/dev/null 2>&1
  then printf 'done\n'
  else printf 'the warm-up turn did not complete; the stack is still up\n'
  fi
  rm -rf "$(dirname "$journal")"
}

# ── the pin is the only source of the image tag ──────────────────────────────────────────
# Same rule as the Rust reader in crates/domain/src/pins.rs::parse: first line, then
# everything before the first '#', trimmed.
[ -f "$ROOT/IRONCLAW_PIN" ] || die "no IRONCLAW_PIN at $ROOT — cannot choose an image"
REV="$(head -n1 "$ROOT/IRONCLAW_PIN" | cut -d'#' -f1 | tr -d '[:space:]')"
[ -n "$REV" ] || die "IRONCLAW_PIN names nothing on its first line"
TAG="ironclaw:${REV:0:9}"

docker image inspect "$TAG" >/dev/null 2>&1 \
  || die "image $TAG is not present. Build it from the pinned revision in the IronClaw checkout:
    git -C ~/agency/ironclaw checkout $REV
    docker build --label ironclaw.rev=$REV -t $TAG ~/agency/ironclaw"

# The tag is a name; the label is the claim. A tag can be moved onto any image.
LABEL="$(docker inspect --format '{{index .Config.Labels "ironclaw.rev"}}' "$TAG" 2>/dev/null || true)"
case "$LABEL" in
  "$REV") ;;
  ""|"<no value>") die "$TAG carries no ironclaw.rev label — its provenance is unknown, so a
    measurement taken against it would not be attributable to $REV. Rebuild it with
    --label ironclaw.rev=$REV." ;;
  *) die "$TAG is labelled $LABEL but IRONCLAW_PIN names $REV — refusing to probe a runtime
    that is not the one we build against." ;;
esac

# ── the disposable record store ──────────────────────────────────────────────────────────
# A proof fixture, not a deployment: synthetic records, synthetic credentials, in memory, gone
# when the process exits. It exists so the chain credential -> server-resolved org -> verified
# instance -> scoped records can be demonstrated rather than asserted.
start_store() {
  if nc -z 127.0.0.1 "$STORE_PORT" 2>/dev/null; then
    die "127.0.0.1:$STORE_PORT is already in use. Set PROBE_STORE_PORT to a free port."
  fi
  printf 'probe: building the record store\n'
  ( cd "$ROOT" && cargo build -q -p ironworks-devstore ) || die "cannot build the record store"
  # Detached deliberately: without `disown` the store stays in this shell's job table and any
  # caller that waits on the script's children hangs on a process designed to outlive it.
  ( cd "$ROOT" && exec ./target/debug/ironworks-devstore \
      --port "$STORE_PORT" --book "$STORE_ORG=fixtures/book" \
      --book "$COORD_ORG=fixtures/account-coordination/book" \
      --identity "$STORE_TOKEN=$STORE_ORG" \
      --identity "$COORD_TOKEN=$COORD_ORG" ) >/dev/null 2>&1 </dev/null &
  echo $! > "$STORE_PID"
  disown 2>/dev/null || true
  # 60s, and the reason is contention rather than the store. Measured 2026-09-07: loading both
  # fixture books and answering /health takes ~0.12s on an idle machine, and 0.16s of that is
  # `load_book` for either book — the store's own work is not the variable. What it competes with
  # is: this runs the instant the runtime reports healthy, while IronClaw is still assembling and
  # the private daemon is still settling, and the same start measured 11s, 23s and over 30s under
  # that load. A 6s window failed there every time and blamed the store.
  #
  # Waiting longer must not become the way a dead store is missed, so the two are distinguished
  # by the process rather than by the clock: an exit is fatal immediately, and only a live process
  # is given the full window.
  for _ in $(seq 1 300); do
    if curl -fsS --max-time 1 "http://127.0.0.1:$STORE_PORT/health" >/dev/null 2>&1; then
      return 0
    fi
    if ! kill -0 "$(cat "$STORE_PID" 2>/dev/null)" 2>/dev/null; then
      die "the record store exited during startup. Run it in the foreground to see why:
    ./target/debug/ironworks-devstore --port $STORE_PORT --book $STORE_ORG=fixtures/book \\
      --book $COORD_ORG=fixtures/account-coordination/book \\
      --identity $STORE_TOKEN=$STORE_ORG --identity $COORD_TOKEN=$COORD_ORG"
    fi
    sleep 0.2
  done
  die "the record store is still running but did not answer /health within 60s"
}

# ── refuse to collide with anything ──────────────────────────────────────────────────────
if [ -n "$("${COMPOSE[@]}" ps -q 2>/dev/null)" ]; then
  die "a $PROJECT stack is already up. Tear it down first: dev/probe-up.sh --down"
fi
if nc -z 127.0.0.1 "$PORT" 2>/dev/null; then
  die "127.0.0.1:$PORT is already in use. Set PROBE_PORT to a free port."
fi
command -v openssl >/dev/null || die "openssl is required to mint disposable secrets"

# ── disposable identities, minted per run ────────────────────────────────────────────────
export PROBE_IMAGE="$TAG"
export PROBE_PORT="$PORT"
# Assigned before export, and that is not style. `export X="$(cmd)"` reports *export's* status, not
# the command's, so an `openssl` that failed left the variable empty and the stack came up with a
# blank master key and a blank web-UI token — a probe that looks healthy and is not secured.
PROBE_PGPW="$(openssl rand -hex 16)" || die "openssl could not mint a database password"
PROBE_MASTER_KEY="$(openssl rand -hex 32)" || die "openssl could not mint a master key"
PROBE_WEBUI_TOKEN="$(openssl rand -hex 32)" || die "openssl could not mint a web-UI token"
[ -n "$PROBE_PGPW" ] && [ -n "$PROBE_MASTER_KEY" ] && [ -n "$PROBE_WEBUI_TOKEN" ] \
  || die "openssl returned an empty secret"
export PROBE_PGPW PROBE_MASTER_KEY PROBE_WEBUI_TOKEN

# ── the model key ────────────────────────────────────────────────────────────────────────
# An explicit export always wins. Otherwise read the one file this repo owns, which lives
# OUTSIDE the checkout on purpose: a secret inside a git repo is one `git add -f` or one
# careless .gitignore edit away from being committed, and history is not un-publishable.
PROBE_ENV="${MULTI_PROBE_ENV:-$HOME/.config/multi/probe.env}"
if [ -z "${NEARAI_API_KEY:-}" ] && [ -f "$PROBE_ENV" ]; then
  mode="$(stat -f '%OLp' "$PROBE_ENV" 2>/dev/null || stat -c '%a' "$PROBE_ENV" 2>/dev/null || true)"
  case "$mode" in
    600|400|"") ;;
    # Advisory, not fatal: refusing to run over a mode bit trades a confidentiality risk for a
    # certain outage, and this is a probe, not a deployment.
    *) printf 'probe: %s is mode %s — it holds a model provider key and should be 0600.\n' \
         "$PROBE_ENV" "$mode" >&2 ;;
  esac
  set -a
  # shellcheck source=/dev/null
  . "$PROBE_ENV"
  set +a
  if [ -n "${NEARAI_API_KEY:-}" ]; then
    printf 'probe: model key loaded from %s\n' "$PROBE_ENV"
  fi
fi

if [ -z "${NEARAI_API_KEY:-}" ]; then
  printf 'probe: no NEARAI_API_KEY (checked the environment and %s) — using a placeholder.\n' "$PROBE_ENV"
  printf 'probe: the runtime will start and answer contract questions, but no turn will COMPLETE.\n'
fi

printf 'probe: starting %s at %s on 127.0.0.1:%s\n' "$PROJECT" "$TAG" "$PORT"
"${COMPOSE[@]}" up -d --wait db

# ── the served profile's worker image ───────────────────────────────────────────────────────
# Built from the PINNED revision, not from whatever the IronClaw checkout is currently on: the
# worker is part of the runtime under test, so building it from another tree would measure a
# combination nobody ships. It is loaded into `dind`, because each daemon has its own image store
# and a tag on the host daemon is invisible there — the same fact `deploy/README.md` records for
# the per-client rootless daemons.
if [ "$SANDBOXED" -eq 1 ]; then
  command -v git >/dev/null || die "git is required to build the worker image from the pin"
  IC_SRC="${IRONCLAW_SRC:-$HOME/agency/ironclaw}"
  git -C "$IC_SRC" cat-file -e "$REV^{commit}" 2>/dev/null \
    || die "$IC_SRC does not contain $REV; set IRONCLAW_SRC to an IronClaw checkout that does"
  ctx="$(mktemp -d "${TMPDIR:-/tmp}/ironclaw-worker.XXXXXX")"
  trap 'rm -rf "$ctx"' EXIT
  git -C "$IC_SRC" archive "$REV" Dockerfile.sandbox-worker docker/sandbox | tar -x -C "$ctx" \
    || die "could not extract the worker build context from $REV"
  printf 'probe: building ironclaw-worker:latest from %s ... ' "${REV:0:9}"
  docker build -q -f "$ctx/Dockerfile.sandbox-worker" --label "ironclaw.rev=$REV" \
    -t ironclaw-worker:latest "$ctx" >/dev/null || die "the worker image did not build"
  printf 'done\n'

  "${COMPOSE[@]}" up -d --wait dind
  printf 'probe: loading the worker image into the probe daemon ... '
  docker save ironclaw-worker:latest \
    | "${COMPOSE[@]}" exec -T dind docker load >/dev/null \
    || die "could not load the worker image into dind"
  printf 'done\n'
fi

"${COMPOSE[@]}" up -d ic

# ── wait for it to actually serve ────────────────────────────────────────────────────────
printf 'probe: waiting for /api/health'
for _ in $(seq 1 60); do
  if curl -fsS --max-time 2 "http://127.0.0.1:$PORT/api/health" >/dev/null 2>&1; then
    start_store
    warm_runtime
    printf '\nprobe: up.\n\n'
    printf '  base url      http://127.0.0.1:%s\n' "$PORT"
    printf '  record store  http://127.0.0.1:%s  (org %s)\n' "$STORE_PORT" "$STORE_ORG"
    printf '  coordination  org %s, token %s (synthetic)\n' "$COORD_ORG" "$COORD_TOKEN"
    printf '  admin token   %s\n' "$PROBE_WEBUI_TOKEN"
    printf '  revision      %s\n' "$REV"
    if [ "$SANDBOXED" -eq 1 ]; then
      printf '  profile       hosted-single-tenant-volume-sandboxed (private daemon, /workspace-root)\n'
    else
      printf '  profile       production\n'
    fi
    printf '\n  export IRONWORKS_BASE=http://127.0.0.1:%s IRONWORKS_TOKEN=%s \\\n' "$PORT" "$PROBE_WEBUI_TOKEN"
    printf '         IRONWORKS_RECORDS=http://127.0.0.1:%s IRONWORKS_RECORDS_TOKEN=%s\n' \
      "$STORE_PORT" "$STORE_TOKEN"
    printf '\n  tear down     dev/probe-up.sh --down\n'
    exit 0
  fi
  printf '.'
  sleep 2
done

# A runtime that never becomes healthy is a RESULT, not a script failure. Say so, and show why.
printf '\nprobe: the runtime never became healthy within 120s. Its log follows.\n\n' >&2
"${COMPOSE[@]}" logs --tail 60 ic >&2
die "runtime did not come up; the stack is left running for inspection (--down to remove it)"
