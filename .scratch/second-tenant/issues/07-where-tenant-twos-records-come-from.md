# Where tenant two's records come from

Type: grilling
Status: open
Blocked by: 01

## Question

Tenant one is served from the live Account Service — `coordinate` answered a real question over two
real records on 2026-09-01. The other record source in the tree is `devstore`, and it is **barred
from this by its own deployment note**: `deploy/README.md` calls it a deliberate exception that
serves the demo, whose records are invented, RFC 2606-reserved and marked `_synthetic`, and says it
"must never become the record store for a tenant with real records — that path needs the Account
Service, and this unit exists partly so nobody is tempted to point the demo at it."

Settle for tenant two:

- **The record source**, and the credential shape behind it. §B-1: org is resolved server-side from
  a credential, never asserted by a caller and never supplied by a model.
- **Whether they get an append path.** This is the unmodelled second authority axis §I names:
  `Access` has deliberately no `Write` variant (`crates/domain/src/capability.rs`), yet
  `crates/records/src/append.rs` performs `POST /append_activity` under a distinct `RecordAppend`
  credential, and the authority is carried by `TenantEntry::append()` returning
  `Option<&CredentialRef>` (`crates/domain/src/registry.rs::TenantEntry::append`) rather than by the
  capability profile. Deciding whether tenant two may promote means deciding it in the *second*
  place, and this ticket should say whether that stays acceptable at two tenants.
- **What the demo shows, and what it does not.** `docs/ACCOUNT-COORDINATION.md` records that
  nothing in an answer says whether its records were real: the scope line goes to stderr once, and
  the two endpoints a demo switches between are both on `127.0.0.1`. With two tenants that
  ambiguity gets worse, not better.
