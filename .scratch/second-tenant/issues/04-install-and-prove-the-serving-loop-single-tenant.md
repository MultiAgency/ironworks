# Install and prove the serving loop, single-tenant

Type: task
Status: open
Blocked by: 02

## Question

§23 step 1, and the shortest path to moving §21's first success criterion. `dev/relay-loop.sh` and
`deploy/rustworks-relay.service` are written and committed; what is missing is an install and a
live proof. Nothing runs the serving loop anywhere — `README.md` says so, and `deploy/README.md`
says the whole install procedure is untested.

Do the install per `deploy/README.md` and **record what the procedure got wrong**, because it was
written as though performed and was not. In particular the container-build step, the systemd unit
install, and the `ironworks compose` digest check on the host — that last one is a real property
(the alloy digest must be architecture-independent) and the README warns its stated value is
historical: re-verify against whatever this tree reports at build time.

**Single-tenant only, and that constraint is load-bearing.** The cursor is shared across every
admitted tenant (`crates/channel/src/lib.rs::safe_cursor`), so this step is safe for one tenant and
unsafe for two client relays. Synthetic relays used to measure ticket 06 are not prohibited.

**What it unblocks:** the per-tenant loop mechanism cannot be chosen honestly against a relay
nobody has ever watched run continuously. §23: "Nothing about multi-tenancy matters while nothing
serves continuously."

Expect the 30s cliff. `DEFAULT_RESPONSES_WAIT_TIMEOUT` is 30s and a slow provider returns
`service_unavailable` — measured at eleven refusals in seventeen asks
(`docs/experiments/0036-the-demo-question-lands-on-the-thirty-second-cliff.md`), every one recovered
by byte-identical replay. `degraded()` now recognises it. A run that hits it has not failed.
