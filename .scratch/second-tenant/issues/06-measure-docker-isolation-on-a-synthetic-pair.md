# Measure Docker isolation on a synthetic pair

Type: task
Status: open
Blocked by: 02, 08

## Question

§23 step 4. Provision a synthetic pair of control planes on the deployment host and run the two
proofs that must pass **before** real client credentials, records or either relay are attached.

The repository already provisions separate rootless daemons and rejects default or mismatched
identities. What is unmeasured is the live behaviour:

- `dev/prove-deployment-isolation.sh <a> <b>` must show that each control plane **lacks authority
  over the other's Docker resources**, and that the same object name can exist independently in
  both daemons. The proof uses same-named sentinels in the two daemons as its positive controls —
  §B-3's obligation, and the reason both halves are required: unique naming does not narrow daemon
  authority, and daemon separation does not excuse duplicate names within one daemon.
- `dev/prove-workspace.sh` must pass through **each** client as the separate positive control that
  IronClaw's real worker and managed proxy operate on that rootless daemon.

**The synthetic pair is not the second tenant.** §23 is explicit: creating disposable synthetic
identities and runtimes is the instrument for testing a later step, carries no client credentials,
and makes no production claim by existing.

Host requirements that will bite: cgroup v2 with the systemd cgroup driver — otherwise rootless
Docker silently ignores IronClaw's worker memory and PID limits and the proof fails. Non-overlapping
subordinate UID/GID ranges per client. Each rootless daemon has a **separate image store**, so the
pinned worker image must be loaded into each.

Blocked by ticket 08 because `dev/prove-workspace.sh` is BLOCKED — not failing — until some service
declares `runtime_tools: "sandbox"`, and none does. A BLOCKED proof is never a pass.
