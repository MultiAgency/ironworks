# The catalogue has two axes, and one of them had never been stood up

Date: 2026-09-02. **`0037-what-the-seam-never-asked-for.md` closed by saying none of its checks had
been repeated on the served profile, because nothing in this repository could stand one up. This
built that stack, repeated them, and found the settings catalogue varies along two independent
axes — who reads it, and which profile the runtime booted with. Every other finding in 0037
reproduced unchanged.**

It also changed what `dev/prove-confinement.sh` proves: it now mints its own member instead of
confining the WebUI env bearer, which is a different caller with a different catalogue.

## What was built

`dev/docker-compose.probe-sandboxed.yml`, an overlay on the existing probe, and
`dev/probe-up.sh --sandboxed`. Two upstream details decided its shape, and both were found by
being wrong first:

- **The runtime reads `IRONCLAW_REBORN_DOCKER_HOST`, not `DOCKER_HOST`.** Setting the conventional
  name fails closed — `profile=hosted-single-tenant-volume-sandboxed requires a reachable Docker
  daemon for its user-sandbox process backend; refusing to boot with an unsandboxed fallback`.
  That refusal is the profile behaving correctly and is worth recording as such: there is no
  unsandboxed fallback, so a misconfigured served deployment does not quietly serve unsandboxed.
- **A non-loopback daemon needs a second, separately-named opt-in**,
  `IRONCLAW_REBORN_DOCKER_HOST_ALLOW_REMOTE`. Upstream's own comment names the case this exists
  for: "a DinD/CI sidecar reachable at a container-network IP, not `127.0.0.1`"
  (`ironclaw@4cb47cfaf3/crates/lanes/ironclaw_sandbox/src/sandbox_process/connect.rs:38-58`).

The daemon is a `dind` sidecar rather than the host socket. `deploy/README.md` says why the socket
is not an option — it "would give that container control of the host while making the deployment
look confined" — and a probe that modelled the served shape by breaking its central property would
be worse than no probe. The privilege sits in a container holding no credential and running no
model, and is deleted with the stack. **It is the disposable analogue of ADR 0008's per-client
rootless daemon, not a proof of it.**

The worker image is built from the pinned revision and loaded into that daemon, because each daemon
has its own image store — the same fact `deploy/README.md` records for the rootless case.

## Finding 1 — the catalogue varies along two axes, not one

`0037` check 3 found the row count depends on the caller. It also depends on the profile, and the
two are independent:

| `GET /settings/tools` | operator (WebUI env bearer) | minted member |
|---|---|---|
| `production` | **53** | **52** |
| `hosted-single-tenant-volume-sandboxed` | **54** | **53** |

- The **caller** axis is `nearai.web_search`, present for the operator and absent for a member, on
  both profiles.
- The **profile** axis is `builtin.shell`, present only on the sandboxed profile, for both callers.

Neither axis interacts with the other, and the locked pair is invariant across all four cells:
`builtin.operator_config_set_auto_approve` and `builtin.operator_config_set_tool_permission`, and
nothing else.

**This is the measured form of §L-10's claim.** That section says a client workspace is an explicit
different policy requiring the sandboxed deployment, and that "allowing shell on the old
`production` profile is not equivalent". It is stronger than that: on `production` there is no
`builtin.shell` row *to* allow. The profile decides whether the capability is in the catalogue at
all, so a `sandbox_workspace` policy on `production` would not be a weaker sandbox — it would be a
policy naming a row that does not exist, which `assess` reports as `AllowedToolAbsent`.

## Finding 2 — confinement closes the shell the profile opens

`dev/prove-confinement.sh`, run against a member minted on each profile:

```
production   6 PASS   52 catalogued, 2 reachable, approval off      (50 disabled)
sandboxed    6 PASS   53 catalogued, 2 reachable, approval off      (51 disabled)
```

`relationship-intelligence@2` declares `runtime_tools: none`, so `read_only_analyst` disables
`builtin.shell` along with everything else. The served profile grants the capability and the
service's own declaration takes it away, which is the layering §L-10 asserts, measured end to end
for the first time.

## Finding 3 — `prove-workspace.sh` was doubly blocked and is now singly blocked

`DESIGN.md` and PRD §13 record two independent reasons that proof had never run: no runtime on the
sandboxed profile, and no shipped service declaring `runtime_tools: "sandbox"`. With the stack above
the first is retired. The second is unchanged and is what the script now reports:

```
BLOCKED   service.runtime_tools   relationship-intelligence declares runtime_tools=none;
                                  a sandbox turn needs "sandbox". No shipped service declares it.
```

That remaining block is a product decision, not an engineering one — PRD §13 states it plainly:
"Whether the sandbox path deserves a service is a product decision nobody has taken." **It is still
BLOCKED and must not be read as closer to a pass than it is.** What changed is which of the two
reasons stands.

## Finding 4 — everything else in 0037 reproduced

Repeated on the served profile, same method, same controls:

| 0037 check | served-profile result |
|---|---|
| 4 — replay an in-flight key | id returned in **0.008s**, `status: "in_progress"` |
| 5 — compat cancel | **400** `invalid_request`, `param: null`, turn keeps running |
| 5 — webchat cancel | **200** `CancelRequested`; the blocked create returned `status: "cancelled"` |
| 6 — thread projection | timeline `200`; SSE carries the `run_id` and the cursor |
| 7 — cross-member read | `404` for the member, `200` for the owner (control) |
| 8 — model policy | member write `403`; off-policy model `400`, on-policy `200`; widening the policy serves the identical request `200` |

So the compat cancel defect is not profile-specific, and the model-selection policy governs
`/api/v1/responses` on the profile the seam will actually be served from.

## What this does not establish

The `dind` sidecar is not ADR 0008's rootless per-client daemon, and nothing here measured daemon
isolation between two clients — `dev/prove-deployment-isolation.sh` still needs a deployment host
and still has not run. This stack has one control plane and one workspace root.

No turn in this run used `builtin.shell`, so the sandbox was configured, booted and confined but
never *exercised*: that is exactly what `prove-workspace.sh` is for and it remains BLOCKED. The
managed egress proxy was never provisioned either, because provisioning is lazy and no sandboxed
command ran.

The `0037` caveat that its checks were unrepeated on the served profile is retired by this run.
Every other caveat in that write-up stands, including that the compat cancel's `400` is measured
and not explained.
