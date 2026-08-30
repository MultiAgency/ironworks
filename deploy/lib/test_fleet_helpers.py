#!/usr/bin/env python3
"""The fleet.sh helpers whose rules more than one script depends on. Offline, stdlib only.

Run for real through `bash`, never reimplemented here — the same reasoning as
`test_teardown_contract.py` and `test_pins.py`. A Python mirror of a shell rule proves the mirror
works and says nothing about the function five scripts actually call.

WHAT THESE TWO WERE BEFORE. `fleet_slug` was a byte-identical pipeline in two provisioning
scripts and `fleet_slug_valid` was the same `case` in three more, with three messages and two
exit codes; `fleet_first_free_port` replaced an `lsof` in two places where the tool's ABSENCE
read as a passing answer. A slug names a container, a volume, a hostname, an env file and a
directory under the operator state root, so the two questions here — "what is this client
called" and "is this port free" — must have one answer each.
"""
import os
import pathlib
import socket
import subprocess
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[2]
FLEET = ROOT / "deploy" / "lib" / "fleet.sh"


def shell(snippet, env=None):
    """Run a snippet with fleet.sh sourced, under the callers' own `set -euo pipefail`."""
    e = dict(os.environ)
    e.update(env or {})
    r = subprocess.run(["bash", "-c", f'set -euo pipefail; . "{FLEET}"; {snippet}'],
                       capture_output=True, text=True, env=e)
    return r.returncode, r.stdout.strip()


class Slug(unittest.TestCase):
    def test_derivation_lowercases_and_collapses(self):
        for text, want in (("Acme Corp", "acme-corp"),
                           ("ALL-CAPS", "all-caps"),
                           ("  spaced  out  ", "spaced-out"),
                           ("Foo_Bar-9", "foo-bar-9"),
                           ("-leading-and-trailing-", "leading-and-trailing")):
            rc, got = shell(f'fleet_slug {shq(text)}')
            self.assertEqual((rc, got), (0, want), text)

    def test_whatever_the_deriver_produces_the_validator_accepts(self):
        """The two halves must agree or provisioning derives a slug its own guard rejects."""
        for text in ("Acme Corp", "ALL-CAPS", "Foo_Bar-9", "ünïcode ltd", "x", "9lives"):
            rc, slug = shell(f'fleet_slug {shq(text)}')
            self.assertEqual(rc, 0, text)
            if not slug:
                continue                     # nothing derivable; callers check for empty
            rc, _ = shell(f'fleet_slug_valid {shq(slug)}')
            self.assertEqual(rc, 0, f"derived {slug!r} from {text!r}, then rejected it")

    def test_uppercase_is_rejected_whatever_the_locale(self):
        """THE DEFECT THE THREE COPIES SHARED. A bracket RANGE is collation-dependent: under
        `en_US.UTF-8`, `[a-z]` matches `A`-`Z`, so `case Acme in *[!a-z0-9-]*)` does not match
        and every copy accepted `Acme` while its own message said "must be lowercase". That put
        `Acme.env` beside `acme.env` in the clients directory — the same file, under two names,
        on any case-insensitive filesystem. POSIX classes are not ranges and cannot widen.

        Asserted under BOTH locales, because passing under only `C` is exactly the state that
        hid this."""
        for loc in ("en_US.UTF-8", "C"):
            for bad in ("Acme", "ACME", "aCme"):
                rc, _ = shell(f'fleet_slug_valid {shq(bad)}',
                              env={"LC_ALL": loc, "LANG": loc})
                self.assertEqual(rc, 1, f"{bad!r} accepted as a lowercase slug under {loc}")

    def test_path_and_shell_metacharacters_are_rejected(self):
        """What seed-real.sh's guard is really for: `$SLUG` reaches a path and an in-container
        `sh -c \"rm -rf /tmp/real-$SLUG\"`."""
        for bad in ("../etc", "a/b", "a b", "a;b", "a$b", "a`b`", "a*", "a.b", ".", "..", ""):
            rc, _ = shell(f'fleet_slug_valid {shq(bad)}')
            self.assertEqual(rc, 1, f"{bad!r} was accepted as a slug")

    def test_ordinary_slugs_are_accepted(self):
        """The positive control: a validator that rejects everything also rejects every attack."""
        for good in ("acme", "acme-corp", "a", "9", "x-1-y", "northwind-2"):
            rc, _ = shell(f'fleet_slug_valid {shq(good)}')
            self.assertEqual(rc, 0, f"{good!r} was rejected")

    def test_every_caller_uses_the_shared_rule(self):
        """Source-level. Five scripts had their own copy and they had already drifted on the
        empty case; a sixth would drift the same way."""
        offenders = []
        for path in sorted(ROOT.glob("**/*.sh")):
            if ".git" in path.parts or path.name == "fleet.sh":
                continue
            for n, line in enumerate(path.read_text().splitlines(), 1):
                if "[!a-z0-9-]" in line or "tr -cs 'a-z0-9'" in line:
                    offenders.append(f"{path.relative_to(ROOT)}:{n}")
        self.assertEqual(offenders, [],
                         "use fleet_slug / fleet_slug_valid; a local copy is collation-dependent")


class FirstFreePort(unittest.TestCase):
    def test_a_free_port_is_reported_free(self):
        with socket.socket() as probe:
            probe.bind(("127.0.0.1", 0))
            free = probe.getsockname()[1]
        # The socket is closed, so `free` is almost certainly available again.
        rc, got = shell(f"fleet_first_free_port {free}")
        self.assertEqual((rc, got), (0, str(free)))

    def test_a_listening_port_is_reported_taken(self):
        """THE CASE `lsof` GOT WRONG BY BEING ABSENT. Bound AND listening, because a bind alone
        can be reused; a listener is what a later `docker run -p` would collide with."""
        with socket.socket() as srv:
            srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
            srv.bind(("0.0.0.0", 0))
            srv.listen(1)
            taken = srv.getsockname()[1]
            rc, got = shell(f"fleet_first_free_port {taken}")
            self.assertEqual(rc, 1, f"port {taken} was listening but reported free ({got!r})")

    def test_a_range_skips_the_taken_port_and_returns_the_next(self):
        with socket.socket() as srv:
            srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
            srv.bind(("0.0.0.0", 0))
            srv.listen(1)
            taken = srv.getsockname()[1]
            rc, got = shell(f"fleet_first_free_port {taken} {taken + 20}")
            self.assertEqual(rc, 0)
            self.assertNotEqual(got, str(taken), "the range returned the occupied port")
            self.assertTrue(taken < int(got) <= taken + 20, got)

    def test_no_caller_asks_lsof_this_question(self):
        """`if lsof …; then` reads exit 127 (not installed) as "the port is free", which is how
        run-proof.sh's collision guard was inert on `ubuntu-latest`."""
        offenders = []
        for path in sorted(ROOT.glob("**/*.sh")):
            if ".git" in path.parts:
                continue
            for n, line in enumerate(path.read_text().splitlines(), 1):
                if "lsof" in line and not line.lstrip().startswith("#"):
                    offenders.append(f"{path.relative_to(ROOT)}:{n}")
        self.assertEqual(offenders, [], "use fleet_first_free_port; lsof's absence reads as free")


def shq(s):
    """Single-quote for the bash snippet. Mirrors what the shell's own quoting would do."""
    return "'" + s.replace("'", "'\\''") + "'"


if __name__ == "__main__":
    unittest.main(verbosity=2)
