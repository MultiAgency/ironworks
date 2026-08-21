# ironclaw:vidgen — ironclaw:main + the toolchain Multimediator's gates need
# (node >= 20 for tsc/npm test, git for branches/PRs). Derived image only;
# the base fleet image stays lean and the ironclaw tree stays untouched
# (config/tooling, per the one rule).
#
# Build:      docker build -t ironclaw:vidgen deploy/vidgen
#             (--build-arg BASE=ironclaw:pinned to base on a pinned rev)
# Provision:  IRONCLAW_IMAGE=ironclaw:vidgen ... ./deploy/provision-agent.sh "Multimediator"
ARG BASE=ironclaw:main
FROM ${BASE}

USER root
# SUPPLY-CHAIN NOTE: the nodesource line pipes a remote script into root's shell at build —
# major version is pinned (setup_22.x) but the script content is not. Left as the vendor's official
# method (a checksum-pinned Node tarball would be more reproducible but needs a build-test we run
# elsewhere). If hardening, fetch a specific node-vXX-linux-<arch>.tar.xz and verify its SHASUMS.
RUN apt-get update \
 && apt-get install -y --no-install-recommends git ca-certificates curl \
 && curl -fsSL https://deb.nodesource.com/setup_22.x | bash - \
 && apt-get install -y --no-install-recommends nodejs \
 && rm -rf /var/lib/apt/lists/*

# Run as uid 1000 directly. From 1.3.0 the base is USER root and its entrypoint drops to
# ironclaw via gosu (UPGRADE.md); starting as ironclaw skips that branch — no chown of
# /data + /workspace, no sshd path, no CHOWN/SETUID/SETGID caps needed. Verified against
# base ironclaw:70795c16e (v1.3.0) on 2026-08-20: boots healthy on a FRESH volume and
# answers the operator API 200, because the base image already ships /data/ironclaw-reborn
# and /workspace owned by ironclaw, and docker seeds a new named volume with that ownership.
USER ironclaw
WORKDIR /workspace
ENTRYPOINT ["ironclaw-reborn-entrypoint"]
