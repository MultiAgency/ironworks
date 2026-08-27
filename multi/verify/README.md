# Verification index

Run scripts from the repository root unless a command says otherwise. Offline proofs need only a
clean checkout. Instance proofs need a local multi-tenant runtime and operator-provided test
credentials. Fixture proofs additionally need the synthetic `proof-a` and `proof-b` organizations
described in [`fixtures/README.md`](fixtures/README.md). Never run destructive proofs against real
tenant identities.

## Offline

```sh
python3 multi/verify/test_adversarial_routing.py
python3 multi/verify/test_fixtures_offline.py
python3 multi/verify/test_output_text_visibility.py
(cd multi/seam && python3 test_services.py && python3 test_client_guidance.py)
python3 deploy/lib/test_compose_persona.py
python3 deploy/lib/test_tail_parity.py
python3 multi/eval/test_graders.py
python3 deploy/lib/test_doc_refs.py
```

## Instance and fixture proofs

| Script | Purpose |
|---|---|
| `test_injection.py` | Observe one-time instruction behavior |
| `test_injection2.py` | Require instructions supplied every turn |
| `test_product_loop.py` | Sealed account, composed service, records, and read-only answer |
| `test_two_clients.py` | End-to-end tenant isolation |
| `test_adversarial_cross_org.py` | Cross-organization denial under adversarial requests |
| `test_adversarial_routing.py` | Deterministic channel routing and negative cases |
| `test_catalog_parity.py` | Catalog assumptions used by confinement |
| `test_client_guidance_live.py` | Live guidance binding and behavior |
| `test_egress_closed.py` | Model-visible no-egress outcome |
| `test_freshness_lifecycle.py` | Record refresh and thread freshness |
| `test_member_admin_negative.py` | Member denial on administrative/write surfaces |
| `test_registry_reconciliation.py` | Registry and live identity consistency |
| `test_responses_recovery.py` | Response retrieval and bridge recovery assumptions |
| `test_session_revocation.py` | Current session-revocation capability and residual authority |
| `test_surface_drift.py` | Effective tool-surface drift |
| `test_tenant_shared_mount_probe.py` | Absence of tenant-shared mounted data |
| `test_tenant_shared_secret_probe.py` | Absence of tenant-shared runtime credentials |

Create the local runtime from `multi/instance/.env.example`, start the Account Service, copy the
synthetic guidance and account fixtures into a private test registry/data directory, and provision
the two proof tenants. The `.env.template` files document their shape. Then source the local
instance environment and run the applicable scripts.

`test_session_revocation.py` may report a known upstream limitation with a distinct exit code;
read its result rather than treating every nonzero capability report as test failure. Live network
containment is separately verified with `./deploy/egress/egress-control.sh verify`.

This index covers the product path only. `deploy/secretary/test_aide_discovery.py` is a live test
of the Secretary, which is a separate application (`../../README.md` § What else is in this
repository); it needs that instance's own `WEBUI_TOKEN` and is deliberately outside this suite and
outside CI.
