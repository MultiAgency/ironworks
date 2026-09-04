# User creation can outlive token minting

Pinned-source inspection on 2026-09-03 found a retry edge in the authority-owning member-creation
surface used by the isolated IronClaw control plane.

## What was inspected

- Runtime: IronClaw 1.4.0 at `4cb47cfaf3fbacd3a2f575a8da654a6542d5fdfe`.
- Creation route: `POST /api/webchat/v2/admin/users`.
- Product implementation:
  `crates/product/ironclaw_assistant/src/admin_user_directory.rs::create_user` at that pin.
- Client contract: the create response returns `api_token` exactly once; the same pin's frontend
  explicitly reports that token re-issue for an existing user is not supported.
- Installed control plane: an authenticated, read-only
  `GET /api/webchat/v2/admin/users` returned HTTP 200 on `multi-serve`. No POST was made, because
  proving the route by creating real authority would itself have changed production state.

## Finding

The implementation creates the user-directory record first and mints its signed session bearer
second. If minting fails, the request reports an internal error but the created user can remain.
At this pin there is no supported endpoint that issues a replacement bearer for that existing
user.

The failure is visible, but blindly retrying creation can accumulate another member while leaving
the first tokenless. The safe retry is reconciliation, not repetition: list users, identify the
new tokenless member created by the failed operation, delete that member through the same admin
authority, confirm its absence, and only then retry creation. Never infer the member id from the
failed response, and never record adoption until the returned bearer has authenticated through
`GET /api/webchat/v2/session`.

## Scope

This is a measured fact about the named upstream pin, not a claim about later IronClaw releases.
It is also not an IronWorks lifecycle defect: lifecycle adoption deliberately observes authority
and cannot mint or repair it. The consequence belongs in the operator procedure because the
authority-owning operation has a partial-success boundary before IronWorks receives evidence.
