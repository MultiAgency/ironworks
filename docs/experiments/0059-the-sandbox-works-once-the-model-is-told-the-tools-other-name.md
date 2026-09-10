# 0059 — The sandbox works, once the model is told the tool's other name

**2026-09-08.** Continues `0057` on a fresh disposable probe at `IRONCLAW_PIN` (`4cb47cfaf3`),
profile `hosted-single-tenant-volume-sandboxed`, model `qwen/qwen3.5-397b-a17b` from `MODEL_PIN`,
`PROBE_LOG=debug`. No transcript: this measures dispatch, infrastructure and persistence, not
answers.

**Verdict: `dev/prove-workspace.sh` scores 3 PASS / 0 FAIL / 0 BLOCKED.** IronClaw's persistent
per-user sandbox workspace is **exercised for the first time**. §L-10's "the sandbox itself is still
unexercised" no longer holds, and `0057`'s BLOCKED is superseded.

**It passes on a workaround, and that is the load-bearing sentence.** The three asks now name the
tool's wire spelling, because without it the runtime refuses its own model's output (`0057` D10,
sharpened below). Remove the workaround and this proof goes back to BLOCKED.

---

## 1 — Naming the wire spelling is what dispatches the call

`0057` established that removing every dotted tool name from our prompt changed nothing. It did not
try the opposite. Told explicitly *"your shell tool is named `builtin__shell` — two underscores,
exactly as your tool definitions spell it, not `builtin.shell`"*, the model emitted
`tool_call_name_sample=["builtin__shell"]`, **no rejection followed**, and the turn completed and
delivered.

So D10 is not "no tool call can complete". It is: **the model answers in the capability-id spelling
by default, and the runtime rejects its own model's output unless the prompt supplies the wire name
for that specific tool.**

### It is not shell-specific

The remaining gap `0057` admitted — one tool tried — is now closed. Asked to report the time with
its clock tool and given no wire spelling, the model returned
`tool_call_name_sample=["builtin.time"]` and the same `rejected invalid provider tool name`
followed. This was on the persona that already carries the double-underscore instruction and names
`builtin__shell` as its example, so **the instruction generalised no further than the tool it
named**.

**A per-tool prompt workaround does not scale**, which is the sharp end of D10 for a maintainer: an
agent with 27 built-ins would need all 27 spelled out, and the failure is silent — a sanitized
`internal_error` at the API, four wasted model calls per turn.

## 2 — Two probe defects stood between the call and the sandbox, both ours

Dispatch working only moved the failure. Each of these was found by raising the log level, which is
what `0057`'s `PROBE_LOG` change bought.

**The private daemon was four major versions too old.** `docker:27-dind` (27.5.1, API 1.47) answered
network creation with `400 unknown gateway mode "isolated"`, and the shell tool reported *"a sandbox
infrastructure error … The file was not created."* IronClaw creates the worker's egress network with
`com.docker.network.bridge.gateway_mode_ipv4=isolated`
(`ironclaw@4cb47cfaf3/crates/lanes/ironclaw_sandbox/src/sandbox_process/managed_egress.rs:50`) and
then **verifies the network carries it**
(`ironclaw@4cb47cfaf3/crates/lanes/ironclaw_sandbox/src/sandbox_process/railway.rs:136`), so a
daemon that does not know the option cannot host a sandbox at all. Now `docker:29-dind` (29.8.0, API 1.56).

*Added 2026-09-09, because the first version of this paragraph inferred the floor from a version
number rather than measuring it:* a bare `docker:28-dind` (28.5.2) accepts the same network create
and reports `gateway_mode=isolated`, so the floor is between 27.5.1 and 28.5.2. **28.0.0 is
untested.** Upstream's own sandbox reports run 29.x.

**The proxy's config never reached it, for the reason the compose file already documents about
something else.** With the daemon new enough, the worker got as far as its managed egress proxy,
which then failed readiness — `sandbox proxy failed readiness (status RUNNING, exit code 0)`. The
proxy container waits for `/run/ironclaw-proxy/proxy.yaml`; its healthcheck failed 42 times with
`sed: /run/ironclaw-proxy/proxy.yaml: No such file or directory`. The bind IronClaw asked the daemon
for was

```
/data/ironclaw-reborn/hosted-single-tenant-volume-sandboxed/sandbox-workspaces/
  .managed-egress/<proxy>:/run/ironclaw-proxy:ro
```

— a path on the **`ic-data`** volume, which was mounted into `ic` and not into `dind`. The daemon
resolved it against a filesystem with no such path and mounted an empty directory. This is exactly
the failure `dev/docker-compose.probe-sandboxed.yml` already warns about for `/workspace-root`
("mounting one volume twice is what makes it mean one directory rather than two empty ones") applied
to a second path nobody had noticed the runtime uses. Fixed by mounting `ic-data:/data` into `dind`.

**Neither is upstream's defect.** Both are our probe modelling the served shape incompletely.

## 3 — What the proof then measured

```
PASS  workspace.create        worker wrote /workspace and had no Docker socket
PASS  workspace.read.first    99b322284c59c161d7f7b2cb16b42a753af4462e584156a0ba5641b86c157fdb
PASS  workspace.persistence   three fresh Responses threads shared one client workspace
```

Three fresh IronWorks journals, no `previous_response_id`, no conversation replay: the only product
path joining them is the runtime's per-user workspace. The first turn wrote a value it was told not
to read back; the next two read it independently and matched. 2:16 wall for the three turns.

**The per-user layout, which the tree did not describe and Phase 6 of the HoH plan needs.** The
artifact landed at

```
/data/ironclaw-reborn/hosted-single-tenant-volume-sandboxed/sandbox-workspaces/users/
  <64-hex user id>/ironworks-persistence-proof
```

so per-user workspaces live under the runtime's **data** directory, not under the configured
`IRONCLAW_REBORN_WORKSPACE_ROOT`. What that root is for on this profile is not established here.

## What this does not establish

- **Nothing about the served host.** Every measurement is on a laptop probe with a disposable
  private daemon. The served deployment's Docker version is unknown and `deploy/README.md` states no
  requirement — which §2 makes a real precondition rather than a detail.
- **Not that the workaround is general.** Naming `builtin__shell` worked; naming `builtin__time` was
  never tried. What is measured is that *not* naming a tool fails for two unrelated tools.
- **Nothing about confinement.** This proof does not confine its member; the prompt surface carried
  27 capabilities. A confined member's sandbox is unmeasured.
- **One sitting, one model.** The dotted-spelling behaviour is this model's; another may not do it,
  which would make D10 a compatibility defect rather than a universal one.

## What changed

- `dev/docker-compose.probe-sandboxed.yml` — `docker:29-dind`, and `ic-data:/data` shared with the
  daemon.
- `dev/prove-workspace.sh` — the three asks name the wire spelling, under protest.
- `scrap/personas/_qualification-operator.md` — the same, as guidance rather than as an example.
- `docs/UPSTREAM.md` D10 is rewritten against this record: the defect is sharper and narrower than
  `0057` could state, and it is no longer a total block.
