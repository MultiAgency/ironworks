#!/usr/bin/env python3
"""The one rule for publishing operator state. Offline, stdlib only, no network.

WHY THE OBVIOUS TEST IS NOT ENOUGH. `egress_status._record` carried this defect for its whole
life with a passing test beside it, because that test asserted the FINAL mode of the published
file — and write-then-chmod produces exactly the same final mode as fchmod-then-write. The
window is the defect, and only the temp file, mid-write, can show it.

So the assertions here are about the temp file: it must never exist group- or world-readable,
whatever the process umask is. `test_a_permissive_umask_cannot_widen_the_window` is the one that
fails on the old implementation; everything else passes on both.
"""
import json
import os
import pathlib
import stat
import sys
import tempfile
import unittest

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
from private_state import write_private


class WritePrivate(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.dir = pathlib.Path(self.tmp.name)
        self.umask = os.umask(0o022)
        self.addCleanup(self.tmp.cleanup)
        self.addCleanup(os.umask, self.umask)

    def test_the_document_round_trips(self):
        p = write_private(self.dir / "state.json", {"b": 2, "a": [1, {"c": None}]})
        self.assertEqual(json.loads(p.read_text()), {"b": 2, "a": [1, {"c": None}]})

    def test_the_published_file_is_private(self):
        p = write_private(self.dir / "state.json", {"token": "secret"})
        self.assertEqual(stat.S_IMODE(p.stat().st_mode), 0o600)

    def test_a_permissive_umask_cannot_widen_the_window(self):
        """THE DEFECT. Under `umask 000`, a write-then-chmod creates the temp file 0666 and
        narrows it afterwards — so every byte of a credential map is world-readable for as long
        as the write takes. Asserted on the TEMP file because the published file is 0600 either
        way, which is precisely why the previous test could not see this.

        The probe is a JSON-serialisable object whose `__getitem__` runs during `json.dump`, so
        it observes the directory at the one moment that matters."""
        os.umask(0o000)
        seen = []

        class Watcher(dict):
            def items(_self):                       # called by json.dump mid-write
                seen.extend(sorted(
                    (q.name, stat.S_IMODE(q.stat().st_mode)) for q in self.dir.iterdir()))
                return super().items()

        write_private(self.dir / "state.json", Watcher({"token": "secret"}))
        self.assertTrue(seen, "the probe never ran — json.dump no longer calls items()")
        for name, mode in seen:
            self.assertEqual(mode, 0o600,
                             f"{name} was mode {mode:o} while the document was being written")

    def test_the_temp_file_is_unique_so_two_writers_cannot_collide(self):
        """A fixed `.tmp` name means the second writer truncates the first one's temp file and
        both `os.replace` it, so one document is lost and the other may be a splice of the two.
        Every name mkstemp hands out is distinct; a fixed suffix hands out one."""
        names = set()

        class Watcher(dict):
            def items(_self):
                names.update(q.name for q in self.dir.iterdir() if q.name != "state.json")
                return super().items()

        for _ in range(3):
            write_private(self.dir / "state.json", Watcher({"n": 1}))
        self.assertEqual(len(names), 3, f"temp names were not unique across writers: {names}")

    def test_a_failed_write_leaves_no_temp_file_and_reports_its_own_error(self):
        """The cleanup handler must not raise over the exception that reached it — a bare
        `os.unlink` on an already-gone temp file replaces the real failure with a
        FileNotFoundError, which is how a disk-full write gets reported as a missing file."""
        class Boom(dict):
            def items(self):
                raise RuntimeError("the real failure")

        with self.assertRaises(RuntimeError) as caught:
            # Non-empty: json.dump short-circuits an empty dict without ever asking for items.
            write_private(self.dir / "state.json", Boom({"a": 1}))
        self.assertEqual(str(caught.exception), "the real failure")
        self.assertEqual(list(self.dir.iterdir()), [], "a temp file survived a failed write")

    def test_the_parent_directory_is_created(self):
        p = write_private(self.dir / "nested" / "deeper" / "state.json", {"a": 1})
        self.assertTrue(p.is_file())

    def test_every_operator_state_writer_uses_this(self):
        """Source-level, because the alternative is three integration fixtures. The defect this
        module exists for was ONE of three copies drifting; a fourth copy would drift the same
        way, and the drift is invisible until someone reads the file at the wrong moment."""
        lib = pathlib.Path(__file__).resolve().parent
        offenders = []
        for path in sorted(lib.glob("*.py")):
            if path.name in ("private_state.py", "test_private_state.py"):
                continue
            src = path.read_text()
            if "tempfile.mkstemp" in src or "os.chmod" in src:
                offenders.append(path.name)
        self.assertEqual(offenders, [],
                         "publish operator state through private_state.write_private")


if __name__ == "__main__":
    unittest.main(verbosity=2)
