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


if __name__ == "__main__":
    unittest.main(verbosity=2)
