# verify fixtures — the clean-clone reproduction kit

Committed so the isolation evidence is reproducible from a clean clone: two
**synthetic** proof clients — Alpine DevTools (`proof-a`) and Harbor Studio
Services (`proof-b`) — as `.env` templates plus their slug-bound business guidance.
The guidance files are the exact synthetic content the suite asserts on
(`test_client_guidance_live.py`, `test_fixtures_offline.py`); they contain no real
companies, credentials, or chat ids (group ids are the repo's synthetic
`-1009000xx` test ids).

Offline (no services): `python3 ../test_fixtures_offline.py` pins that these files
load through the real `load_clients()` via `CLIENTS_DIR`.

Live bring-up: see "Reproduce from a clean clone" in `../README.md`. Fill the two
`<REQUIRED …>` tokens per client from your own provisioning into copies named
`<slug>.env` in a private dir (chmod 600 — never commit filled copies), or point
`CLIENTS_DIR` at wherever `provision.sh` wrote the real registry entries.
