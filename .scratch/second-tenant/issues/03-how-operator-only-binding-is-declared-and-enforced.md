# How operator-only binding is declared and enforced

Type: grilling
Status: open
Blocked by: 01

## Question

`docs/ACCOUNT-COORDINATION.md` records a deliberate POC deferral: the composition names MultiAgency,
but the code does not compare the verified record scope against a separately configured operator
org. **That deferral ends at a second tenant** — it is §23 step 3.

What it obligates is already stated and needs designing, not deciding from scratch: the service
definition must declare an operator-only binding, and instance construction must **fail closed**
unless the server-resolved org matches an operator org supplied by configuration — *no fallback
literal and no tenant registry required*.

Settle:

- **What declares it.** A field on the service definition; how it is spelled; whether it is
  refused at load when incoherent, the way `CapabilityProfile::incoherence`
  (`crates/domain/src/capability.rs`) already refuses `runtime_tools: sandbox` beside
  `egress: none`.
- **Where configuration supplies the operator org**, given the no-fallback-literal rule. A default
  is the one value that can silently outrank configuration, and this is an authority boundary.
- **Where the check fires.** §B-11 says registry org id is metadata until authenticated, and only
  the authenticated startup resolution sets it verified. The comparison cannot happen before that.
- **The positive control.** §B-3: an empty store passes every cross-org test. Whatever asserts the
  refusal needs a control that fails if the check becomes vacuous — the shape
  `a_composition_naming_the_operator_exists_or_the_default_check_is_vacuous` already uses.

Blocked by ticket 01 because whether tenant two shares a composition with the operator decides
whether this is one binding or a per-tenant one.
