#!/usr/bin/env python3
"""The systemd bridge unit resolves one operator root across config, runtime and writes."""
import importlib.machinery
import importlib.util
import os
import pathlib
import stat
import tempfile
import unittest


ROOT = pathlib.Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "multi/serve/render-bridge-service.py"


def load_renderer():
    loader = importlib.machinery.SourceFileLoader("render_bridge_service", str(SCRIPT))
    spec = importlib.util.spec_from_file_location("render_bridge_service", SCRIPT, loader=loader)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


renderer = load_renderer()


class BridgeServiceUnit(unittest.TestCase):
    def assert_root(self, unit, root):
        self.assertIn(f"EnvironmentFile={root}/bridge.env", unit)
        self.assertIn(f"ExecStart=/usr/bin/env AGENCY_DIR={root} ", unit)
        self.assertIn(f"BindPaths={root}", unit)
        self.assertIn(f"ReadWritePaths={root}", unit)
        self.assertIn("ProtectSystem=strict", unit)
        self.assertIn("ProtectHome=tmpfs", unit)

    def test_default_rendering_keeps_the_multi_users_agency_directory(self):
        saved = os.environ.pop("AGENCY_DIR", None)
        try:
            unit = renderer.render()
        finally:
            if saved is not None:
                os.environ["AGENCY_DIR"] = saved
        self.assert_root(unit, pathlib.Path("/home/multi/.agency"))

    def test_custom_agency_dir_moves_env_runtime_and_writable_bind_together(self):
        custom = pathlib.Path("/srv/ironworks/operator-state")
        unit = renderer.render(custom)
        self.assert_root(unit, custom)
        self.assertNotIn("/home/multi/.agency", unit)

    def test_install_writes_the_same_custom_rendering_atomically(self):
        with tempfile.TemporaryDirectory() as tmp:
            target = pathlib.Path(tmp) / "systemd/bridge.service"
            custom = pathlib.Path("/var/lib/ironworks-agency")
            renderer.install(target, custom)
            self.assertEqual(target.read_text(), renderer.render(custom))
            self.assertEqual(stat.S_IMODE(target.stat().st_mode), 0o644)

    def test_relative_or_systemd_ambiguous_paths_are_rejected(self):
        for bad in ("relative/state", "/srv/has space", "/srv/has:colon"):
            with self.assertRaises(ValueError, msg=bad):
                renderer.render(bad)
class ServeUnitSandboxing(unittest.TestCase):
    """The OTHER two serve units, which nothing verified at all.

    `bridge.service` has this suite. `multi-backup.service` and `multi-watchdog.service` had no
    check of any kind — and they hold real authority: the backup unit runs `pg_dump` over every
    tenant's database and carries RESTIC_PASSWORD, the watchdog carries a Telegram bot token. They
    ran with zero sandboxing beside a sibling that goes to some length, which was never a decision.

    A source-level assertion, deliberately, and the limit is worth stating: this proves the
    directives are DECLARED, not that systemd accepts them. `systemd-analyze verify` on the serve
    host is the other half and cannot run on a developer machine. Declared-but-wrong would still
    reach the host — but silently REMOVED is the regression this catches, and it is the one that
    happens during an unrelated edit.
    """

    UNITS = ("multi/serve/multi-backup.service", "multi/serve/multi-watchdog.service")

    @staticmethod
    def directives(rel):
        """The unit's actual directives — COMMENTS EXCLUDED.

        A first draft matched raw text and failed on these very units, because their comments
        explain WHY `ProtectSystem=strict` is not used and the word appears in the prose. Same
        shape as a `# noqa` written inside an explanation: a checker that reads the file rather
        than the syntax cannot tell an instruction from a sentence about one."""
        return [ln.strip() for ln in (ROOT / rel).read_text().splitlines()
                if ln.strip() and not ln.lstrip().startswith("#")]

    # Present on all three serve units. NOT `ProtectSystem=strict` or `ProtectHome`: these two
    # need the docker socket under /run and a writable ~/.agency, so the bridge's stricter pair
    # would break them. `full` still makes /usr, /boot and /etc read-only.
    REQUIRED = ("NoNewPrivileges=true", "ProtectSystem=full", "PrivateTmp=true",
                "ProtectKernelTunables=true", "ProtectKernelModules=true",
                "ProtectControlGroups=true", "RestrictSUIDSGID=true")

    def test_both_units_carry_the_sandboxing_they_were_given(self):
        for rel in self.UNITS:
            lines = self.directives(rel)
            for directive in self.REQUIRED:
                self.assertIn(directive, lines, f"{rel} lost {directive}")

    def test_neither_unit_claims_confinement_that_would_break_it(self):
        """`ProtectSystem=strict` remounts everything read-only and `ProtectHome` hides the
        directory these read their own configuration from. Copying them across from
        `bridge.service` — the obvious tidy-up — would break both units on the next timer fire,
        with no failure until then. Asserted so the copy is caught here, not at 03:00."""
        for rel in self.UNITS:
            lines = self.directives(rel)
            self.assertNotIn("ProtectSystem=strict", lines,
                             f"{rel} would remount /run read-only and lose the docker socket")
            self.assertFalse([ln for ln in lines if ln.startswith("ProtectHome=")],
                             f"{rel} reads its config from ~/.agency; ProtectHome hides it")

    def test_the_operator_root_is_pinned_the_way_bridge_service_pins_it(self):
        """systemd does not inherit a login shell's environment, so a unit and an operator shell
        that has relocated the state root disagree about where it is — silently, in the direction
        of backing up or watching the wrong tree."""
        for rel in self.UNITS:
            self.assertIn("Environment=AGENCY_DIR=/home/multi/.agency", self.directives(rel),
                          f"{rel} does not pin AGENCY_DIR")

    def test_bridge_service_is_the_stricter_sibling_and_stays_that_way(self):
        """The positive control. If `bridge.service` ever loses `strict`, the asymmetry above
        stops being a considered difference and becomes an inconsistency nobody decided."""
        bridge = self.directives("multi/serve/bridge.service")
        self.assertIn("ProtectSystem=strict", bridge)
        self.assertIn("ProtectHome=tmpfs", bridge)


if __name__ == "__main__":
    unittest.main(verbosity=2)
