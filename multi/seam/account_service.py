#!/usr/bin/env python3
"""The private Account Service client — the tenant's own records, fetched as the tenant.

THE CREDENTIAL BOUNDARY, and why it is a module. Every call here carries the tenant's
`X-Service-Token` and reaches a private address. That token is never sent to IronClaw and never
reaches the model; `_svc` is the only place it is put on a request, so "where does the account
token go?" has a one-file answer. The IronClaw client and the model-facing envelope live
elsewhere and hold no Account Service credential.

THE SCOPE CHECK IS THE POINT, not the fetching. The Account Service resolves an org from the
token SERVER-SIDE, so a token that is hot-remapped resolves somewhere else without anything
locally changing. `_assert_account_scope` refuses any response whose `org` is not the one bound
at startup, and `resolve_account_scopes` is what binds it — deliberately separate from
`load_clients`, so registry inspection stays offline while conversation CONTINUATION fails
closed on a scope it cannot verify. An authenticated 404 is checked too: discarding that body
used to discard the only evidence of a remap and let the turn continue with no context.

FAKING THIS MODULE IN TESTS. `_svc` is the single seam the suites replace to run a turn with no
service — patch `account_service._svc`, which is where it is implemented and therefore where
this module's own callers read it. It is deliberately NOT re-exported through
`context_ingress`: patching a re-exported name would rebind the copy and leave every internal
caller on the real one, silently. `_catalog` and `_get_context` ARE re-exported, because
`context_ingress` calls those through its own globals and patching them there works.
"""
import json
import urllib.parse, urllib.request, urllib.error

import dataclasses

try:
    from .registry import _client, account_service_base_identity
except ImportError:  # direct-script compatibility during service-unit rollout
    from registry import _client, account_service_base_identity


def _svc(path, client=None):
    """Call the private Account Service AS one client's org (the client's token — never sent to
    IronClaw). The token/host live ONLY here."""
    c = _client(client)
    req = urllib.request.Request(c.account_base + path, headers={"X-Service-Token": c.account_token})
    with urllib.request.urlopen(req, timeout=30) as x:
        return json.loads(x.read())


class AccountScopeError(RuntimeError):
    """The bridge could not establish a tenant's trusted Account Service data scope."""


class AccountScopeChanged(AccountScopeError):
    """A live credential now resolves outside the scope bound at bridge startup."""


def _assert_account_scope(client, response):
    """Refuse an Account Service response outside the startup-bound organization."""
    if not client.organization_id:
        return
    got = response.get("org") if isinstance(response, dict) else None
    if got != client.organization_id:
        raise AccountScopeChanged(
            f"tenant {client.slug!r}: Account Service organization changed from the startup-bound "
            f"{client.organization_id!r} to {got!r}. Refusing conversation continuation. Stop and "
            "restart the bridge to resolve the new scope, then explicitly reset with "
            f"./deploy/ironworks tenant reset-thread {client.slug} --confirm {client.slug}.")


def resolve_account_scopes(clients):
    """Resolve every token's authoritative org at startup and return verified configs.

    This is deliberately separate from load_clients: fixture and operator registry inspection
    remain offline. Conversation continuation is different: unknown scope cannot be treated as
    compatible, so the serving bridge fails closed before `_load_threads` can attach a prior
    response id.

    AND IT IS WHERE THE ONE-TO-ONE INVARIANT IS ENFORCED. `registry.load_clients` refuses a
    reused ACCOUNT_TOKEN, and its comment says why that is not enough: the duplicate case that
    matters is two DIFFERENT tokens minted against one org (an operator running
    register-identity.sh a second time for the same org id), which no registry check can see
    because the tokens differ and the mapping is server-side. The Account Service reports that
    as a warning rather than refusing it (`test_service_guards.py:100`,
    `duplicate_orgs_are_reported_not_refused`), so nothing refused it anywhere — and two
    Telegram groups, i.e. two AUDIENCES, were served one org's book. D-091 says the audience of
    a context is the audience of every byte supplied to a turn in it, which rests on
    org <-> audience being one-to-one.

    This function is the only place every tenant's AUTHORITATIVE org is in hand at once, so it
    is the only place the collision is visible.
    """
    resolved = {}
    by_org = {}
    for key, c in clients.items():
        try:
            cat = _svc("/list_accounts", c)
        except Exception as e:
            raise AccountScopeError(
                f"tenant {c.slug!r}: cannot resolve Account Service organization at "
                f"{account_service_base_identity(c.account_base)} ({type(e).__name__}). "
                "Refusing to load persisted conversations with an unverified data scope.") from e
        org = cat.get("org") if isinstance(cat, dict) else None
        if not isinstance(org, str) or not org.strip():
            raise AccountScopeError(
                f"tenant {c.slug!r}: authenticated /list_accounts returned no usable org id. "
                "Refusing to load persisted conversations with an unverified data scope.")
        if org in by_org:
            raise AccountScopeError(
                f"tenants {by_org[org]!r} and {c.slug!r} resolve to the SAME Account Service "
                f"organization {org!r} through different credentials. Each tenant is a separate "
                "Telegram group — a separate audience — and one org's records must reach exactly "
                "one of them (D-091). Deprovision the duplicate, or point one tenant at its own "
                "org: multi/provision/deprovision.sh, then re-run provision.sh.")
        by_org[org] = c.slug
        verified = dataclasses.replace(c, organization_id=org, organization_verified=True)
        resolved[key] = verified
    return resolved


def _catalog(client):
    c = _client(client)
    cat = _svc("/list_accounts", c)
    _assert_account_scope(c, cat)
    return cat


def _get_context(account_id, client=None):
    c = _client(client)
    try:
        result = _svc("/get_account_context?account_id=" + urllib.parse.quote(account_id), c)
    except urllib.error.HTTPError as e:
        if e.code == 404:
            # A not-found response is still authenticated Account Service output. In
            # particular, it is the response produced when a token is hot-remapped after the
            # catalog read and the old account id does not exist in the new org. Discarding the
            # body here used to discard the only evidence of that scope change and let the turn
            # continue to IronClaw with no context.
            try:
                result = json.loads(e.read())
            except (ValueError, OSError) as parse_error:
                raise AccountScopeError(
                    f"tenant {c.slug!r}: Account Service returned an unreadable authenticated "
                    "404 response; organization scope cannot be verified") from parse_error
            org = result.get("org") if isinstance(result, dict) else None
            if not isinstance(org, str) or not org.strip():
                raise AccountScopeError(
                    f"tenant {c.slug!r}: Account Service 404 response carried no authenticated "
                    "organization; refusing to treat it as an ordinary missing account") from e
            _assert_account_scope(c, result)
            return None
        raise
    _assert_account_scope(c, result)
    return result
