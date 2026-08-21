"""The one reader of a bearer's tool catalog, and the one statement of what egress means.

WHY THIS FILE EXISTS. Four places parsed `/api/webchat/v2/settings/tools` and decided whether a
surface was confined: multi/provision/confine-member.sh, deploy/broker/confine-actor.sh,
deploy/broker/eval/probe-confinement.sh, and multi/verify/test_egress_closed.py. (The two
deploy/broker/ ones no longer ship — that experiment was removed — so the live callers are the
other two. The count below is the history that produced this file, not today's caller list.)
Every one of them is a FAIL-OPEN risk — a parser that returns {} for a body it does not
understand reads as "nothing is callable", which is indistinguishable from "everything is
locked down". Four copies meant four chances for one to drift into that shape, and one had:

  * probe-confinement.sh had NEITHER the empty-catalog guard NOR the egress non-vacuity check
    that its two siblings carried. It would have certified an unrecognised surface.
  * two copies skipped any entry whose `value` was not a dict; the other two kept it. A tool
    present but oddly shaped was therefore INVISIBLE to half the fleet — and invisible reads as
    absent, which reads as safe. The inclusive form below is the one that ships.

Import from a shell heredoc with:
    sys.path.insert(0, os.environ["LIB_DIR"]); import tool_surface
"""

# The load-bearing egress tools. At least one MUST be observed disabled for a surface to be
# certifiable — "absent from the catalog" is not proof we read a real surface, which is exactly
# how a renamed tool would slip through.
EGRESS = ["builtin.http", "builtin.http.save", "builtin.outbound_deliver"]


def parse_catalog(doc, who=""):
    """{tool_id: state} from a parsed /settings/tools body. Fails closed, never returns {}.

    `doc["entries"]` is subscripted on purpose: a malformed or error body raises KeyError/
    TypeError and aborts the caller. Defaulting to [] here would turn every error response into
    a silent pass.
    """
    state = {}
    for e in doc["entries"]:
        k = e.get("key", "")
        if k.startswith("tool."):
            v = e.get("value", {})
            # Keep non-dict values rather than skipping them: an entry we cannot interpret must
            # stay VISIBLE, because an invisible tool reads as absent and absent reads as safe.
            state[k[5:]] = v.get("state") if isinstance(v, dict) else v   # k[5:] drops "tool."
    if not state:
        raise SystemExit(f"!! {who or 'tool-surface'}: tool catalog is empty — "
                         "refusing to certify (fail closed)")
    return state


def egress_observed_off(state, who=""):
    """The non-vacuity check: prove we read a real surface and acted on it.

    Returns the egress tools observed `disabled`; raises if none were. Without this a catalog
    that simply does not contain the egress tools — a rename, a different build, a truncated
    response — certifies as clean.
    """
    seen = [t for t in EGRESS if state.get(t) == "disabled"]
    if not seen:
        raise SystemExit(f"!! {who or 'tool-surface'}: none of the egress tools "
                         "(http/http.save/outbound_deliver) were seen disabled — surface "
                         "unrecognized, refusing to certify (fail closed)")
    return seen


def read_deny_list(path):
    """Deny-list file -> tool ids, blank lines and # comments stripped."""
    with open(path) as fh:
        return [ln.strip() for ln in fh if ln.strip() and not ln.lstrip().startswith("#")]


if __name__ == "__main__":
    ok = {"entries": [{"key": "tool.builtin.http", "value": {"state": "disabled"}},
                      {"key": "tool.builtin.echo", "value": {"state": "always_allow"}},
                      {"key": "tool.builtin.odd", "value": "raw-string-state"},
                      {"key": "skill.something", "value": {"state": "x"}}]}
    st = parse_catalog(ok, "selftest")
    assert st == {"builtin.http": "disabled", "builtin.echo": "always_allow",
                  "builtin.odd": "raw-string-state"}, st
    assert egress_observed_off(st, "selftest") == ["builtin.http"]
    for bad, label in (({"entries": []}, "empty catalog"),
                       ({"entries": [{"key": "skill.x", "value": {}}]}, "no tool.* entries")):
        try:
            parse_catalog(bad, "selftest"); raise AssertionError(f"{label} must fail closed")
        except SystemExit:
            pass
    try:
        egress_observed_off({"builtin.echo": "disabled"}, "selftest")
        raise AssertionError("no egress observed must fail closed")
    except SystemExit:
        pass
    try:
        parse_catalog({"error": "unauthorized"}, "selftest")
        raise AssertionError("malformed body must raise")
    except (KeyError, TypeError):
        pass
    print("tool_surface selftest: PASS (fail-closed on empty, non-tool-only, and malformed)")
