#!/usr/bin/env bash
# COST: free — builds from a local IronClaw checkout at the pin. No runtime, no model key.
# Build deployable IronClaw artifacts from the exact Git object named by this repository's pin.
# The caller supplies a local official IronClaw checkout; its working tree is never read.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source_tree=""
output=""

while [ "$#" -gt 0 ]; do
  case "$1" in
    --source) source_tree="${2:-}"; shift 2 ;;
    --output) output="${2:-}"; shift 2 ;;
    *) echo "usage: $0 --source <official-ironclaw-checkout> --output <new-directory>" >&2; exit 64 ;;
  esac
done

[ -n "$source_tree" ] && [ -n "$output" ] || {
  echo "usage: $0 --source <official-ironclaw-checkout> --output <new-directory>" >&2
  exit 64
}
[ -f "$root/IRONCLAW_PIN" ] || { echo "IRONCLAW_PIN is absent" >&2; exit 1; }
revision="$(sed -n '1{s/#.*//;s/[[:space:]]//g;p;}' "$root/IRONCLAW_PIN")"
[[ "$revision" =~ ^[0-9a-f]{40}$ ]] || {
  echo "IRONCLAW_PIN must name one full 40-character lowercase Git revision" >&2
  exit 1
}

# No `cargo` here on purpose. The serving host deliberately carries no Rust toolchain — the
# IronWorks build in `deploy/README.md` runs in a container for exactly that reason — so requiring
# one made this script unrunnable on the host it documents. Measured 2026-09-07: `cargo is
# required`, on a host with docker and no rustc. The compile moved into a container below.
for command in git docker sha256sum tar; do
  command -v "$command" >/dev/null 2>&1 || { echo "$command is required" >&2; exit 1; }
done
[ ! -e "$output" ] || { echo "output already exists: $output" >&2; exit 1; }
git -C "$source_tree" cat-file -e "$revision^{commit}" 2>/dev/null || {
  echo "the supplied IronClaw checkout does not contain $revision" >&2
  exit 1
}
[ "$(git -C "$source_tree" rev-parse "$revision^{commit}")" = "$revision" ] || {
  echo "IRONCLAW_PIN did not resolve to its exact commit" >&2
  exit 1
}

work="$(mktemp -d "${TMPDIR:-/tmp}/ironclaw-pinned-build.XXXXXX")"
trap 'rm -rf "$work"' EXIT
mkdir "$work/source" "$work/output"
git -C "$source_tree" archive "$revision" | tar -x -C "$work/source"

# The build is not Rust alone: IronClaw compiles a frontend through pnpm, so the builder needs Node
# beside a toolchain. Measured 2026-09-07 — a plain `rust:1.97-bookworm` fails with `corepack was
# not found`, and the escape that error names, `SKIP_FRONTEND_BUILD=1`, would emit a **different
# artifact than upstream ships**. This script exists to reproduce the pinned build, so it installs
# the missing half rather than dropping it.
builder="ironclaw-pinned-builder:rust1.97-node"
# Node comes from the official image rather than Debian, because the pinned source pulls
# `pnpm@11.7.0`, which refuses bookworm's Node 18. Only Node's own files are copied: a blanket
# `/usr/local` copy would overwrite this image's `cargo` and `rustup`, which live there too.
docker build -q -t "$builder" - <<'DOCKERFILE' >/dev/null
FROM rust:1.97-bookworm
COPY --from=node:22-bookworm-slim /usr/local/bin/node /usr/local/bin/node
COPY --from=node:22-bookworm-slim /usr/local/lib/node_modules /usr/local/lib/node_modules
RUN ln -sf ../lib/node_modules/npm/bin/npm-cli.js /usr/local/bin/npm \
 && ln -sf ../lib/node_modules/npm/bin/npx-cli.js /usr/local/bin/npx \
 && ln -sf ../lib/node_modules/corepack/dist/corepack.js /usr/local/bin/corepack \
 && corepack enable \
 && node --version && corepack --version
DOCKERFILE

# The archived tree carries its own `rust-toolchain.toml`, and rustup inside the image resolves it,
# so the toolchain is the pinned source's choice rather than this script's or the host's. `--locked`
# keeps the lockfile authoritative. `-u` so the artifacts belong to the invoker rather than root,
# and `HOME=/src` because `npm exec` writes a cache and must not try the container's root home.
docker run --rm -u "$(id -u):$(id -g)" \
  -v "$work/source:/src" -w /src \
  -e CARGO_HOME=/src/.cargo-home -e HOME=/src -e npm_config_cache=/src/.npm \
  "$builder" cargo build --locked --release -p ironclaw
install -m 0755 "$work/source/target/release/ironclaw" "$work/output/ironclaw"

build_tag="ironclaw-worker-build:${revision}"
# `ironclaw.rev`, not `ironclaw.revision`. This is the one label name the tree already reads —
# `dev/probe-up.sh` refuses an image that lacks it — so a second spelling here produced an image
# whose provenance that check could not see.
#
# It does NOT make the image ID reproducible, and the attempt to show that it did is what measured
# the opposite. On 2026-09-07 three worker images — one built four days earlier, two built minutes
# apart from this same revision — agreed on all eight layer digests (`a7dc34d660dc…`), on
# `created`, on labels, and on entrypoint, env, workdir and user, and still carried three different
# image IDs. The ID hashes a config carrying per-build history, so it is not a function of anything
# observable in the image.
#
# So of the two worker fields in the manifest below, only one is a reproducible property. The layer
# set reproduces exactly; `IRONCLAW_REBORN_SANDBOX_IMAGE` identifies *these* bytes and cannot be
# re-derived by rebuilding — which is what the closing line of this script means, and why
# `deploy/check-ironclaw-artifacts.sh` verifies that the client daemon resolves this exact ID
# rather than that a rebuild would agree with it.
docker build -f "$work/source/Dockerfile.sandbox-worker" \
  --label "ironclaw.rev=$revision" -t "$build_tag" "$work/source"
image_id="$(docker image inspect --format '{{.Id}}' "$build_tag")"
[[ "$image_id" =~ ^sha256:[0-9a-f]{64}$ ]] || {
  echo "Docker returned no immutable worker image ID for $build_tag" >&2
  exit 1
}
docker save --output "$work/output/ironclaw-worker.tar" "$image_id"

binary_sha="$(sha256sum "$work/output/ironclaw" | awk '{print $1}')"
archive_sha="$(sha256sum "$work/output/ironclaw-worker.tar" | awk '{print $1}')"
{
  printf 'IRONCLAW_ARTIFACT_REVISION=%s\n' "$revision"
  printf 'IRONCLAW_BINARY_SHA256=%s\n' "$binary_sha"
  printf 'IRONCLAW_WORKER_ARCHIVE_SHA256=%s\n' "$archive_sha"
  # Use IronClaw's pinned configuration name directly. A second local name would need a wrapper
  # whose only job was to copy this value before exec.
  printf 'IRONCLAW_REBORN_SANDBOX_IMAGE=%s\n' "$image_id"
} > "$work/output/artifact-manifest.env"
chmod 0444 "$work/output/artifact-manifest.env"

mv "$work/output" "$output"
printf 'built IronClaw artifacts from %s in %s\n' "$revision" "$output"
printf 'the manifest identifies these bytes; it is not a reproducible-build claim\n'
