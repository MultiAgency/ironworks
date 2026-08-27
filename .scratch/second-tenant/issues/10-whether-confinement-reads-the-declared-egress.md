# Whether confinement reads the declared `egress`

Type: grilling
Status: open
Blocked by: 08, 09

## Question

`docs/INVARIANTS.md` §I: **the authority a service declares is not the authority confinement
enforces.** `"egress"` is parsed into the service's capability profile
(`crates/services/src/service.rs::WireCapabilities::egress`, carried into the profile by
`crates/services/src/service.rs::from_raw`) and `crates/lifecycle/src/confinement.rs` **never reads
it**. Two mechanisms that both exist, unconnected — so a service declaring `egress: none` is not
thereby confined.

This was **narrowed on 2026-09-02 and not closed**: `CapabilityProfile::incoherence` now refuses a
definition declaring `runtime_tools: sandbox` alongside `egress: none`
(`crates/domain/src/capability.rs::Incoherence::SandboxWithoutEgress`, controlled both ways by
`crates/services/src/service.rs::a_sandbox_service_must_admit_its_egress`). That closes the case
where a definition *lies about itself*. It does not connect the two mechanisms: a coherent
declaration and the policy actually applied to the member remain unrelated, and nothing compares
them.

Decide whether this milestone connects them, and if so how:

- **Does `confine` read the profile and derive the policy from it**, so the declaration is the
  single source? That is the "one mechanism" answer, and this repo's house rule says two mechanisms
  for one invariant is one mechanism plus something to keep in sync.
- **Or does something compare them and report disagreement**, leaving them separate?
- **What fires when they disagree** — a refusal at confine time, or a DRIFTED verdict.

Watch §L-3's ordering invariant: applying confinement in catalogue order leaves a window in which
the work is reversible by the thing being confined. Any derivation must survive automation.

Adjacent but distinct from ticket 09: that one is about whether a *verdict* can be vacuous; this one
is about whether the declaration and the applied policy are the same thing.
