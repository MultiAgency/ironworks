# Whether anything declares `runtime_tools: sandbox`

Type: grilling
Status: open
Blocked by: —

## Question

`dev/prove-workspace.sh` is one of the proofs §23 step 4 requires, and it is **BLOCKED rather than
failing**: it needs a runtime on the `hosted-single-tenant-volume-sandboxed` profile exercising a
service that declares `capabilities.runtime_tools: "sandbox"`, and **no shipped service declares
one**. `deploy/README.md` states it plainly: "A workspace deployment needs a service definition that
asks for one."

The tier that would satisfy it already exists as a type —
`CapabilityProfile::SANDBOX_WORKSPACE`: `egress: read-only`, records read-only, writes and outreach
absent — and nothing uses it. There is also no `--workspace` flag any more, deliberately: the policy
is whatever the recorded instance's service declares, so `confine` cannot be told to grant a shell
to a service that does not ask for one.

Decide:

- **Does a service declare it, and which?** Adding a service re-opens what §7 closed by deletion,
  so this collides with ticket 01 if the answer there is "tenant two needs their own service".
- **Or is the workspace proof out of scope for this milestone?** That is a legitimate answer — but
  it must be argued rather than assumed, because §23 step 4 currently requires it, and dropping it
  means the rootless daemon is proven isolated without being proven to *work* for IronClaw's real
  worker and proxy path.

Note the trap `deploy/README.md` describes: IronClaw 1.4.0's sandbox shell always carries its
built-in managed allowlist for package registries and GitHub, and the only operator setting *adds*
domains. So `runtime_tools: sandbox` beside `egress: none` would make the declaration false — and
now does not load at all (`Incoherence::SandboxWithoutEgress`). Any service declaring sandbox must
admit its egress.

Unblocked: this can be decided against the tree as it stands.
