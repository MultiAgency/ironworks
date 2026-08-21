# config/ — agent-level configuration

Agent config the harness loads as data. Currently thin by design: on the stock
ironclaw binary, most configuration does **not** live here.

- **Channel setup** (Telegram bot token, webhook secret, webhook URL, bot username)
  is entered through ironclaw's **Admin → Configuration** and stored in the encrypted
  secret store on the data volume — not in this repo. That encryption protects
  against reads of the database or a dump alone, not against captures that include
  the key: fleet instances keep the auto-generated master-key dotfile on the same
  volume, the MT instance's key lives in the host-side `.env`, and whole-box vendor
  images capture both.
- **Deployment config** (profile, serve host, operator identity + token, NEAR AI
  creds, log level) is passed as env vars at run time — see `../../deploy/README.md`.
- **Secrets never belong in this repo** (`.env` and key files are gitignored).

Put agent-level, non-secret config the harness reads as data here as it emerges.
