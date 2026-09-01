#!/usr/bin/env python3
"""The service manifest's security-shaped fields must agree with what actually enforces them.

WHY THIS EXISTS. `multi/services/*.json` carries four fields that read like configuration and
configure nothing:

    capabilities   an assertion about the product; no code grants or denies from it
    tool_policy    names a confinement script; provisioning invokes that script BY PATH
    data_schema    names a schema; nothing validates a book against it
    evaluation     a validated path; the eval runner composes the default regardless

`multi/services/README.md` is unusually honest about this and says so in a dedicated section. But
disclosure is not enforcement: nothing would notice if `tool_policy` named a script that does not
exist, if `data_schema` named a schema the Account Service does not load, or if the enforced
allowlist grew a tool the `capabilities` block says is `none`. The fields would keep reading as
security configuration while meaning nothing — which is the exact misreading that README spends a
section preventing, left reachable.

WHAT THIS DOES INSTEAD OF DELETING THEM. The earlier recommendation was to delete `tool_policy`
and `data_schema` as dead weight. Binding them is better and costs the same: each becomes a
machine-checked claim about a real enforcement point, so the manifest stops being decorative
without anyone having to implement per-service capability configuration — which is explicitly not
wanted, because both services declare an identical block over one shared surface and a profile
system would configure a value that does not vary.

WHY IT LIVES IN `deploy/lib/` AND NOT `multi/seam/test_services.py`. Three of the four binds read
operator tooling — `confine-member.sh`, the Account Store compose file, `tool_surface.EGRESS`.
`CLAUDE.md` forbids the serving path importing `deploy/`, and `multi/verify/test_fixtures_offline.py`
pins that independence from the other side. Operator tooling may import product modules, so the
cross-layer assertion belongs on this side of the line. `test_services.py` keeps the checks that
are purely about the manifest.

EVERY ABSENCE CHECK HERE CARRIES A POSITIVE CONTROL. "No egress tool is in the allowlist" is
trivially true of an empty allowlist, and "no mutating route" is trivially true of a file with no
routes. Each test asserts it actually measured something before asserting what it did not find.

Run:  python3 deploy/lib/test_service_manifest_binding.py
"""
import json
import pathlib
import re
import sys
import unittest

HERE = pathlib.Path(__file__).resolve().parent
ROOT = HERE.parents[1]
sys.path.insert(0, str(HERE))

import tool_surface  # noqa: E402  path set above

SERVICES = ROOT / "multi" / "services"
PROVISION = ROOT / "multi" / "provision"
ACCOUNT_DATA = ROOT / "deploy" / "account-intel" / "data"


def manifests():
    out = {}
    for p in sorted(SERVICES.glob("*.json")):
        out[p.stem] = json.loads(p.read_text())
    return out


def confinement_invocations():
    """Repo-relative paths of the scripts provisioning hands a MEMBER TOKEN to.

    Anchored on `IRONCLAW_MEMBER_TOKEN` rather than on a script name: the member token is what
    confinement structurally requires (the settings route accepts only the member's own bearer),
    so "the script handed that token" is the confinement step by definition rather than by
    spelling. A rename is then not a false failure, and swapping in a different script is a real
    one.
    """
    found = set()
    for sh in sorted(PROVISION.glob("*.sh")):
        for line in sh.read_text().splitlines():
            if line.lstrip().startswith("#") or "IRONCLAW_MEMBER_TOKEN" not in line:
                continue
            for m in re.finditer(r"\./(\S+\.sh)", line):
                found.add(str((PROVISION / m.group(1)).resolve().relative_to(ROOT)))
    return found


def initdb_schema():
    """Repo-relative host path the Account Store compose mounts into Postgres' initdb hook."""
    compose = ACCOUNT_DATA / "docker-compose.yml"
    for line in compose.read_text().splitlines():
        stripped = line.strip()
        if not stripped.startswith("-") or "docker-entrypoint-initdb.d" not in stripped:
            continue
        host = stripped.lstrip("- ").split(":", 1)[0]
        return str((ACCOUNT_DATA / host).resolve().relative_to(ROOT))
    return None


def keep_allowlist():
    """The tool ids `confine-member.sh` keeps, from its `KEEP_DEFAULT=` assignment."""
    text = (PROVISION / "confine-member.sh").read_text()
    m = re.search(r'KEEP_DEFAULT="((?:[^"\\]|\\.)*)"', text, re.S)
    if not m:
        return None
    return set(m.group(1).replace("\\\n", " ").split())


def account_service_route_methods():
    """The HTTP methods the Account Service declares, from its Flask decorators."""
    text = (ACCOUNT_DATA / "service.py").read_text()
    generic = re.findall(r'@app\.route\([^)]*methods\s*=\s*\[([^\]]*)\]', text)
    methods = {m.strip().strip("\"'").lower() for group in generic for m in group.split(",")}
    methods |= {m.lower() for m in re.findall(r"@app\.(get|post|put|patch|delete)\(", text)}
    return methods


class ManifestBinding(unittest.TestCase):
    def setUp(self):
        self.manifests = manifests()
        self.assertTrue(self.manifests, "multi/services/ holds no definitions to bind")

    def test_tool_policy_names_the_script_provisioning_actually_runs(self):
        """The field reads like it selects a confinement policy. It must at least be true."""
        invoked = confinement_invocations()
        self.assertTrue(invoked, "no provisioning script hands a member token to a confinement "
                                 "script — either confinement stopped running or this parse broke")
        for name, d in self.manifests.items():
            policy = d.get("tool_policy")
            self.assertIsNotNone(policy, f"{name}: no tool_policy")
            self.assertTrue((ROOT / policy).is_file(), f"{name}: tool_policy {policy} is not a file")
            self.assertIn(policy, invoked,
                          f"{name}: tool_policy names {policy}, but provisioning confines with "
                          f"{sorted(invoked)} — the field describes a policy nothing applies")

    def test_data_schema_names_the_schema_the_account_service_loads(self):
        """The book every service reads is created from exactly one file. Name that one."""
        loaded = initdb_schema()
        self.assertIsNotNone(loaded, "the Account Store compose mounts no initdb schema — either "
                                     "the stack changed or this parse broke")
        for name, d in self.manifests.items():
            self.assertEqual(loaded, d.get("data_schema"),
                             f"{name}: data_schema names {d.get('data_schema')}, but the Account "
                             f"Service initialises from {loaded}")

    def test_a_declared_egress_of_none_means_no_egress_tool_is_kept(self):
        """Where the declaration and the enforcement meet.

        `capabilities.egress: "none"` is an assertion about the product; `confine-member.sh`'s
        allowlist is what actually holds it, together with the network boundary. Nothing compared
        them, so the allowlist could have grown `builtin.http` while every manifest went on
        declaring `none`.
        """
        keep = keep_allowlist()
        self.assertTrue(keep, "could not read KEEP_DEFAULT from confine-member.sh")
        # Positive controls: an empty allowlist or an empty egress vocabulary would make the
        # intersection below empty for reasons that prove nothing about the boundary.
        self.assertTrue(tool_surface.EGRESS, "tool_surface declares no egress tools to check for")
        self.assertIn("builtin.echo", keep,
                      "the allowlist parse produced something that is not the real KEEP list")
        for name, d in self.manifests.items():
            if d["capabilities"].get("egress") != "none":
                continue
            overlap = sorted(keep & set(tool_surface.EGRESS))
            self.assertEqual([], overlap,
                             f"{name} declares egress:none, but the enforced allowlist keeps "
                             f"{overlap}")

    def test_read_only_records_means_the_account_service_declares_no_mutating_route(self):
        """`account_records: read-only` and `writes: none` are claims about a real HTTP surface."""
        methods = account_service_route_methods()
        self.assertTrue(methods, "no routes found in service.py — the parse broke, and 'no "
                                 "mutating route' would pass vacuously")
        self.assertIn("get", methods, "no readable route found; this is not the Account Service")
        for name, d in self.manifests.items():
            caps = d["capabilities"]
            if caps.get("account_records") != "read-only" and caps.get("writes") != "none":
                continue
            mutating = sorted(methods - {"get", "head", "options"})
            self.assertEqual([], mutating,
                             f"{name} declares read-only records with writes:none, but the "
                             f"Account Service declares {mutating} route(s)")


if __name__ == "__main__":
    unittest.main(verbosity=2)
