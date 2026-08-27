# Who tenant two is, and whether one composition serves them

Type: grilling
Status: open
Blocked by: —

## Question

Nothing in the tree names the second tenant. `docs/PRD.md` §23 says only that one is "named and in
view (operator, 2026-09-02)", which is what makes the ordering worth getting right — but every
downstream decision about addressing, records and binding needs the identity.

Two things must come out of this ticket:

1. **Who the tenant is**, at the resolution the map needs: their slug, their org, and whether they
   are a client with real records or a friendly org standing in for one.
2. **Whether `relationship-intelligence@2` can serve them as composed.** It names MultiAgency
   deliberately, and §14 records that as correct rather than a leak. A tenant that is not
   MultiAgency either gets a composition that does not name MultiAgency — which re-opens
   everything §7 closed by deleting `account-analysis@1`, and fires
   `no_shipped_composition_is_organization_neutral`
   (`crates/services/src/service.rs`) — or the composition needs a per-tenant addressing story
   that does not reintroduce an organization-neutral slot.

**The second half is the one that can redraw the map.** §14 states plainly that adding a second
service re-opens what §7 closed, and that the guarding pair must be *rebuilt* rather than deleted
if a default returns. If the answer is "they need their own service", several later tickets change
shape and the fog around guidance authoring graduates immediately.

Watch the two-things-must-agree rule (§14, Must not break): the instance configuration selects the
service and the tenant's guidance marker declares it, deliberately separately. Matching them
blindly picks at random.
