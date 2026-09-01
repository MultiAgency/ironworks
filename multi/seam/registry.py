#!/usr/bin/env python3
"""The client registry — tenant configuration, loaded from disk and validated fail-closed.

WHAT A TENANT IS, as data. One `*.env` file under `CLIENTS_DIR` (default ~/.agency/clients)
names one tenant: its identity, its two credentials, its channel, its service, and the path to
its own guidance. `load_clients()` turns that directory into `{slug: ClientConfig}` and is the
ONLY way a servable tenant comes into existence — there is deliberately no ambient single-client
fallback, so a tenant that was not composed explicitly cannot be served.

WHY IT IS ITS OWN MODULE. Everything here answers "who may be served, and with what?" and
answers it BEFORE any turn runs. The serving path in `context_ingress.py` answers "what does
this turn get?" and runs per turn. The two were one file; splitting them makes the registry's
one hard contract visible and testable on its own: it must stay usable on a clean clone with no
instance and no network (`multi/verify/test_fixtures_offline.py` pins that), which is why every
check below is a local one. Runtime identity probes belong at bridge startup, not here — see
`context_ingress.assert_no_member_is_the_operator`.

FAIL CLOSED IS THE WHOLE DESIGN. Each validation below refuses the WHOLE registry rather than
skipping one file, because a partially loaded registry is a bridge serving some tenants with
another tenant's routing, credentials, or data scope. The failures it refuses are named at each
check: duplicate slug, duplicate group id, the operator token in a member slot, cross-wired
credentials, a shared member token, a shared account token, a missing or off-path guidance file,
an unknown service, and an off-pin model.

Credentials read here stay here: they are used on the tenant's own outbound requests and are
never sent to the model. Registry files are chmod 600 on disk and never in the repository.
"""
import os, json, pathlib, dataclasses, hashlib
import urllib.parse
try:
    from . import pins, services
    from .operator_paths import agency_dir
    from .persona import compose_service_persona, persona_digest
except ImportError:  # direct-script compatibility during service-unit rollout
    import pins
    import services
    from operator_paths import agency_dir
    from persona import compose_service_persona, persona_digest


# Canonical IronWorks serving is pinned to the repository model of record. `pins.model_pin()`
# deliberately retains an environment override for explicit adjunct/test callers, but the
# registry + bridge path must not inherit that escape hatch: an off-pin model can change both
# privacy properties and measured behavior. `load_clients()` checks the live environment too,
# so setting MODEL cannot silently start a production bridge on another model.
MODEL = pins.require_pin("MODEL_PIN")
ACCOUNT_BASE = os.environ.get("ACCOUNT_BASE", "http://127.0.0.1:8443").rstrip("/")
# Hosted multi-tenant IronClaw bakes no per-account persona; the seam supplies it via
# `instructions` EVERY turn (once-only injection drifts — multi/verify/test_injection*.py).
# The persona is always composed EXPLICITLY: registry tenants get compose_service_persona
# (service definition + guidance-validated, fail-closed); the dev oracle requests compose_persona().


@dataclasses.dataclass(frozen=True)
class ClientConfig:
    """One client's credentials + overrides. The seam is the trusted broker: these tokens live
    here and are used on the client's own requests ONLY — never sent to the model, never mixed."""
    slug: str
    ironclaw_token: str      # the client's sealed IronClaw member token
    account_token: str       # the client's Account Service org token (identity implies org)
    name: str = ""
    telegram_group_id: str = ""
    account_base: str = ACCOUNT_BASE
    # The Account Service resolves this from `account_token`, server-side. Registry ORG_ID is
    # only operator metadata; the bridge replaces it with the authenticated /list_accounts
    # result at startup before it is allowed to load a persisted conversation.
    organization_id: str = ""
    # FALSE IS THE DEFAULT BECAUSE FORGETTING MUST BE SAFE. Only `resolve_account_scopes` may set
    # this, and only from an org the Account Service authenticated; `telegram_bridge._load_threads`
    # refuses to load a conversation without it. That made the whole guarantee rest on ONE keyword
    # argument at the registry construction below remembering to be there — a fail-open default
    # held shut by a line nothing tested. Measured: deleting that kwarg left all 596 offline tests
    # green while `_load_threads` began ACCEPTING registry tenants, so the bridge would have served
    # on the `ORG_ID` metadata that SECURITY.md says is never authoritative.
    #
    # Inverted, the same mistake fails closed, and every caller that passes True is making the
    # trust claim explicitly and visibly — which is what a directly-constructed "trusted
    # test/adjunct input" always was, said out loud. `test_registry.py` pins the registry case.
    organization_verified: bool = False
    model: str = MODEL
    # Which `facts` keys THIS partner's book is expected to carry, in the order they should be
    # read. Declared per client (registry FACT_FIELDS) because every book is shaped differently
    # — funded lines, grantees, programmes — and a global list would report meaningless gaps.
    #
    # THREE STATES, and the difference between the last two is the whole point:
    #   None  -> UNDECLARED. The tenant said nothing, so the service's legacy sales-shaped
    #            `missing_legacy` list stands in. Every registry file written before this
    #            distinction existed is in this state and keeps its old behaviour exactly.
    #   ()    -> DECLARED EMPTY. The tenant states it has no gap shape, and NO gap line is
    #            rendered. This is not the same as saying nothing: it is the only way a book
    #            that is not a B2B sales pipeline can stop being told which sales columns it
    #            is missing. Silence in the config used to collapse into this and could not
    #            be requested; now it can.
    #   (a,b) -> DECLARED. Only these keys are read, in this order, and only these are gaps.
    fact_fields: tuple | None = None
    # Words too common in THIS book's domain to identify an account on their own
    # (resolve_targets derives most of these from the book itself; this is the rest).
    name_stopwords: tuple = ()
    # NO usable default: a hand-built config must supply its persona explicitly —
    # an empty persona refuses to serve (Thread / receiving_turn fail closed).
    persona: str = ""
    # Which SERVICE this tenant runs, and at which version. Operator-visible metadata: it is
    # what makes "which service and version is each tenant running?" answerable without
    # reading four files, and what a release readiness artifact records per tenant.
    service: str = services.DEFAULT_SERVICE
    service_version: int = 0

    @property
    def service_id(self):
        """`<service>@<version>` — the string the operator CLI and the release artifact print."""
        return f"{self.service}@{self.service_version}"

    @property
    def persona_sha(self):
        """Short digest of the composed persona. Non-secret, comparable, and it never prints
        the guidance the persona carries (which is the tenant's own data)."""
        return persona_digest(self.persona) if self.persona else ""

    @property
    def instructions_sha256(self):
        """Full behavioral fingerprint of exactly the model-visible `instructions` string.

        Unlike `persona_sha`, this is a safety identity rather than a compact operator label.
        Because `self.persona` is the final composed value, it includes ordered persona parts,
        the guidance heading, stripped/validated guidance body, and the safety tail. Authoring
        comments stripped before composition deliberately do not affect it.
        """
        return hashlib.sha256(self.persona.encode()).hexdigest()

    @property
    def context_policy_sha256(self):
        """Fingerprint context rendering that can make persisted `supplied` state stale.

        FACT_FIELDS is tri-state, so None, (), and a non-empty ordered tuple remain distinct.
        NAME_STOPWORDS affects future target resolution but does not change how an already
        supplied record was rendered, and is deliberately outside this first compatibility key.
        """
        value = None if self.fact_fields is None else list(self.fact_fields)
        canonical = json.dumps({"fact_fields": value}, sort_keys=True, separators=(",", ":"))
        return hashlib.sha256(canonical.encode()).hexdigest()

    @property
    def thread_identity(self):
        """Non-secret compatibility identity for a persisted IronClaw conversation."""
        return {"service": self.service,
                "service_version": self.service_version,
                "instructions_sha256": self.instructions_sha256,
                "model": self.model,
                "context_policy_sha256": self.context_policy_sha256,
                "organization_id": self.organization_id,
                "account_service_base": account_service_base_identity(self.account_base)}


def account_service_base_identity(value):
    """Canonical, non-secret identity of the configured Account Service trust endpoint.

    The service exposes no stable instance/data-set id. Binding its configured base is the
    narrow conservative substitute: two services may both contain an `acme` org, so org alone
    is not enough when ACCOUNT_BASE changes. Credentials in a URL are refused because this
    value is persisted; Account Service authentication belongs only in X-Service-Token.
    """
    p = urllib.parse.urlsplit((value or "").rstrip("/"))
    if p.scheme not in ("http", "https") or not p.hostname:
        raise ValueError(f"ACCOUNT_BASE must be an absolute http(s) URL, got {value!r}")
    if p.username is not None or p.password is not None:
        raise ValueError("ACCOUNT_BASE must not contain credentials; use ACCOUNT_TOKEN")
    if p.query or p.fragment:
        raise ValueError("ACCOUNT_BASE must not contain a query or fragment")
    try:
        port = p.port
    except ValueError as e:
        raise ValueError(f"ACCOUNT_BASE has an invalid port: {value!r}") from e
    default = (p.scheme == "http" and port == 80) or (p.scheme == "https" and port == 443)
    host = p.hostname.lower()
    if ":" in host and not host.startswith("["):
        host = f"[{host}]"
    netloc = host if port is None or default else f"{host}:{port}"
    path = p.path.rstrip("/")
    return urllib.parse.urlunsplit((p.scheme.lower(), netloc, path, "", ""))


def _client(client):
    if client is None:
        raise RuntimeError("no client: pass a ClientConfig (Thread(client=...)); "
                           "registry clients come from load_clients()")
    return client


def _parse_fact_fields(kv):
    """`FACT_FIELDS` as a tri-state: absent -> None, present-but-empty -> (), listed -> tuple.

    `kv` holds only the keys the file actually wrote, so PRESENCE survives parsing and the
    distinction is free. It used to be thrown away one line later — `kv.get(key, "")` mapped an
    absent key and an empty one onto the same empty tuple — and that collapse is why a tenant
    could not say "I have no gap shape". It could only stay silent, and silence meant the
    sales-shaped fallback: the default was the thing the mechanism existed to escape.
    """
    raw = kv.get("FACT_FIELDS")
    if raw is None:
        return None
    return tuple(f.strip() for f in raw.split(",") if f.strip())


def _canonical_guidance_path(registry_file, slug):
    """The one supported tenant-guidance location on the canonical product path."""
    return registry_file.with_name(f"{slug}.guidance.md")


# The env vars that may carry the OPERATOR identity's credential. Named once because two
# things read them for opposite reasons: this module REJECTS a tenant configured with one, and
# the operator console must REDACT one out of anything it prints. A list that drifts between
# those two readers is a token printed in full by the tool built not to print tokens.
OPERATOR_TOKEN_VARS = ("IRONCLAW_OPERATOR_TOKEN", "IRONCLAW_REBORN_WEBUI_TOKEN", "WEBUI_TOKEN")


def load_clients(dir=None):
    """Load the client registry: every *.env under CLIENTS_DIR (default ~/.agency/clients),
    one client per file, KEY=VALUE lines (see multi/clients/README.md for the schema).
    Returns {slug: ClientConfig}. Secrets stay on disk chmod 600 — never in the repo."""
    process_model = os.environ.get("MODEL")
    if process_model and process_model != MODEL:
        raise ValueError(
            f"canonical IronWorks serving is pinned to MODEL_PIN ({MODEL!r}); process MODEL "
            f"selects {process_model!r}. Remove MODEL before starting the registry/bridge. "
            "Explicit off-pin experiments must construct a non-serving ClientConfig directly.")

    d = pathlib.Path(dir or os.environ.get("CLIENTS_DIR") or agency_dir("clients"))
    clients = {}
    seen_groups = {}   # TELEGRAM_GROUP_ID -> slug: a group id MUST map to exactly one client
    seen_tokens = {}   # IRONCLAW_TOKEN -> slug: a member token MUST map to exactly one client
    seen_accounts = {} # ACCOUNT_TOKEN -> slug: a data scope MUST map to exactly one client
    # The operator/admin token, if this process has it: a client handed the operator token would
    # run as the OPERATOR identity (cross-account read) AND could re-enable its own egress tools,
    # voiding the member confinement (multi/provision/confine-member.sh). Reject it, fail closed.
    operator_tokens = {os.environ.get(k) for k in OPERATOR_TOKEN_VARS}
    operator_tokens.discard(None); operator_tokens.discard("")
    for f in sorted(d.glob("*.env")) if d.is_dir() else []:
        kv = {}
        for line in f.read_text().splitlines():
            line = line.strip()
            if line and not line.startswith("#") and "=" in line:
                k, v = line.split("=", 1)
                kv[k.strip()] = v.strip().strip('"').strip("'")
        if not (kv.get("IRONCLAW_TOKEN") and kv.get("ACCOUNT_TOKEN")):
            raise ValueError(f"{f}: IRONCLAW_TOKEN and ACCOUNT_TOKEN are required")
        tenant_model = kv.get("MODEL")
        if tenant_model and tenant_model != MODEL:
            raise ValueError(
                f"{f}: canonical IronWorks tenants are pinned to MODEL_PIN ({MODEL!r}); "
                f"MODEL selects {tenant_model!r}. Remove the tenant MODEL override.")
        slug = kv.get("CLIENT_SLUG") or f.stem
        if slug in clients:
            raise ValueError(f"{f}: duplicate client slug {slug!r} — each client's slug must be unique")
        gid = kv.get("TELEGRAM_GROUP_ID", "")
        # FAIL CLOSED on a duplicate group id: otherwise the bridge's {gid: client} dict would
        # silently keep the last file and serve that whole group with the WRONG client's tokens/data.
        if gid and gid in seen_groups:
            raise ValueError(
                f"{f}: TELEGRAM_GROUP_ID {gid!r} is already used by client {seen_groups[gid]!r} — "
                "one Telegram group must map to exactly ONE client (else messages misroute across clients)")
        if gid:
            seen_groups[gid] = slug
        # FAIL CLOSED on a member-identity that is not unique-and-sealed. The whole isolation model
        # (per-member threads/memory AND the egress confinement) assumes each client is a DISTINCT,
        # non-operator IronClaw member. These are the checks that keep that assumption true.
        itok = kv["IRONCLAW_TOKEN"]
        if itok in operator_tokens:
            raise ValueError(
                f"{f}: IRONCLAW_TOKEN is the operator/admin token — a client must be a sealed MEMBER, "
                "never the operator (it would read across accounts and could re-enable its own egress). "
                "Provision a member with multi/provision/provision-client.sh.")
        if itok == kv["ACCOUNT_TOKEN"]:
            raise ValueError(f"{f}: IRONCLAW_TOKEN equals ACCOUNT_TOKEN — cross-wired credentials")
        if itok in seen_tokens:
            raise ValueError(
                f"{f}: IRONCLAW_TOKEN is already used by client {seen_tokens[itok]!r} — two clients "
                "sharing one member token are the SAME identity and can read each other's threads")
        seen_tokens[itok] = slug
        # FAIL CLOSED on a shared data scope. The account token resolves to exactly one org
        # SERVER-SIDE, so two clients carrying the same one read the SAME records — and because a
        # client is a room, that is two rooms served one dataset. The audience rule (D-091) is that
        # the audience of a context is the audience of every byte supplied to a turn in it, and it
        # rests on org <-> audience being one-to-one. Nothing else enforces that: the Account
        # Service's duplicate-org warning fires on two DIFFERENT tokens mapping to one org and is
        # blind to one token reused, and the checks above cover identity and routing, not scope.
        # Without this the invariant held only because nobody had made the mistake.
        atok = kv["ACCOUNT_TOKEN"]
        if atok in seen_accounts:
            raise ValueError(
                f"{f}: ACCOUNT_TOKEN is already used by client {seen_accounts[atok]!r} — one account "
                "credential resolves to one org, so two clients sharing it are served the SAME "
                "records in two different rooms. Provision this client its own org token "
                "(deploy/account-intel/data/register-identity.sh).")
        seen_accounts[atok] = slug
        # Client-specific business guidance is MANDATORY for registry clients and FAILS
        # CLOSED: no guidance file -> the registry refuses to load. There is deliberately
        # no fallback to MultiAgency's internal company knowledge (that composition is
        # for the operator's own env-fallback/dev mode only). Default path: the guidance
        # sits beside the client's env file, slug-bound by its first-line marker.
        canonical_guidance = _canonical_guidance_path(f, slug)
        configured_guidance = kv.get("GUIDANCE_FILE")
        if configured_guidance:
            configured_path = pathlib.Path(configured_guidance).expanduser()
            try:
                same_path = configured_path.resolve() == canonical_guidance.resolve()
            except OSError:
                same_path = False
            if not same_path:
                raise ValueError(
                    f"{f}: GUIDANCE_FILE selects {configured_guidance!r}, but canonical "
                    f"IronWorks guidance lives at {canonical_guidance}. Move the file there "
                    "and remove GUIDANCE_FILE; per-tenant guidance-path lifecycle is not supported.")
        gfile = str(canonical_guidance)
        # WHICH SERVICE this tenant runs. Absent = the default, which composes byte-for-byte
        # what every registry client composed before service definitions existed. An unknown
        # name raises out of load_service, so the whole registry refuses to load rather than
        # silently serving one tenant on a composition nobody chose. The guidance file's own
        # `service:` binding is checked inside load_guidance — both must agree.
        svc = services.load_service(kv.get("SERVICE") or services.DEFAULT_SERVICE)
        clients[slug] = ClientConfig(
            slug=slug, ironclaw_token=kv["IRONCLAW_TOKEN"], account_token=kv["ACCOUNT_TOKEN"],
            name=kv.get("CLIENT_NAME", slug), telegram_group_id=gid,
            account_base=kv.get("ACCOUNT_BASE", ACCOUNT_BASE).rstrip("/"),
            # Metadata only until resolve_account_scopes authenticates to the service. Keeping
            # it here lets offline operator output remain useful, but it is never trusted by
            # the bridge's continuation check.
            organization_id=kv.get("ORG_ID", ""),
            organization_verified=False,
            # An explicit MODEL equal to the pin is tolerated for compatibility, but cannot
            # alter behavior. Off-pin values were rejected above before this tenant can serve.
            model=MODEL,
            fact_fields=_parse_fact_fields(kv),
            name_stopwords=tuple(w.strip().lower() for w in kv.get("NAME_STOPWORDS", "").split(",") if w.strip()),
            service=svc["service"], service_version=svc["version"],
            persona=compose_service_persona(svc, gfile, slug))
    return clients
