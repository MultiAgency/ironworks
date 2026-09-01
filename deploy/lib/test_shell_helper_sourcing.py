#!/usr/bin/env python3
"""A script that CALLS a `deploy/lib` helper must SOURCE the file defining it.

THE DEFECT THIS PINS, measured on the serve host rather than reasoned about:

    multi-watchdog.sh[1925162]: line 120: tg_send: command not found

`tg_send` was extracted into `deploy/lib/telegram.sh` from the two scripts that had a copy of it
— `multi/serve/multi-watchdog.sh` and `multi/serve/multi-backup.sh`. The extraction rewrote both
CALL SITES and neither SOURCE LINE. Both scripts source `deploy/lib/fleet.sh`, which reaches
`curl-private.sh` and stops there, so from that commit onward the alert path in both was an
undefined command.

WHY IT SURVIVED, AND WHY THE GATE HAD TO BE THIS ONE. Nothing else could see it:

  * `bash -n` parses a call to an undefined function happily — it is a runtime lookup.
  * `shellcheck` does not follow `.` into another file, so it cannot know what is in scope.
  * systemd recorded `Result=success` / `ExecMainStatus=0`, because the scripts exit 0.
  * the watchdog kept DETECTING correctly; only the reporting leg was dead, and a monitor whose
    silence is indistinguishable from health is worse than no monitor at all.
  * `multi-backup.sh` calls it from an EXIT trap under `|| true` — correct, so a failed alert
    cannot overwrite the run's real exit code — which swallows a 127 exactly as it swallows a
    network error. That one never even logged.

`~/.agency/watchdog.state` carried `prev_alert=0`: no alert had ever sent on that host.

WHAT THIS CHECKS. For every tracked shell script, every `deploy/lib` helper name it calls in
command position must be defined by a lib the script sources, transitively. Not "does
multi-watchdog.sh source telegram.sh" — that is the instance, and the next extraction moves a
different function into a different lib.

DELIBERATELY CONSERVATIVE. Comment lines are dropped and a name counts only in command position,
because both scripts now carry long comments naming `tg_send`, and a gate that fired on prose
would be turned off. A script defining the helper itself is exempt: that is a copy, not a missing
source, and it is a different finding.

Tracked files only. A clone contains the tracked set and nothing else, so that is the scope CI
can actually check; a gitignored operator script that sources `deploy/lib` is covered by the
shell gate in `deploy/run-quality.py`, not here.

Run: python3 test_shell_helper_sourcing.py   (from deploy/lib/)
"""
import pathlib
import re
import subprocess
import unittest

HERE = pathlib.Path(__file__).resolve().parent
ROOT = HERE.parents[1]
LIB = ROOT / "deploy" / "lib"

# `name() {` or `function name {`, at the start of a line — the two forms this tree uses.
DEFINITION = re.compile(r"^\s*(?:function\s+)?([A-Za-z_][A-Za-z0-9_]*)\s*\(\)\s*\{|"
                        r"^\s*function\s+([A-Za-z_][A-Za-z0-9_]*)\s*\{", re.M)
# `. path` or `source path`. The REST of the line is captured and the lib basename pulled out of
# it, because a source line in this tree is rarely a bare path: `. "$REPO/deploy/lib/fleet.sh"`,
# `. "$(dirname "$0")/../../deploy/lib/fleet.sh"`, and `. ../../deploy/lib/fleet.sh   # comment`
# all appear. A `\S+`-shaped pattern matches none of the last two — the first draft of this gate
# used one and reported fourteen scripts as unsourced, every one of them a false positive.
SOURCING = re.compile(r"^\s*(?:\.|source)\s+(.+)$", re.M)
LIB_BASENAME = re.compile(r"([A-Za-z0-9_-]+\.sh)")
# Command position: start of a LINE, or after a separator/keyword that begins a command.
#
# `re.M` IS LOAD-BEARING AND WAS MISSING. Without it `^` anchors at the start of the whole file
# instead of each line, so the motivating call site — indented inside `alert() { … }` in
# multi-watchdog.sh — matched nothing. Mutation-tested: reverting the watchdog's source line
# reproduced the original production defect exactly, and this gate stayed GREEN. The backup case
# failed as intended, but for an incidental reason (a separator character earlier on its line),
# so one of the two mutations passing was not evidence the rule worked.
#
# That is the whole argument for mutating a new guard rather than trusting a green first run:
# this file would have shipped as a check that could not see the thing it was written for.
CALL_SITE = r"(?:^|[;|&(]|\|\||&&|\bthen\b|\belse\b|\bdo\b|\$\()\s*"


def _tracked_shell_scripts():
    out = subprocess.run(["git", "ls-files", "-z"], cwd=ROOT,
                         capture_output=True, text=True, check=True).stdout
    paths = []
    for rel in out.split("\0"):
        if not rel:
            continue
        path = ROOT / rel
        if rel.endswith(".sh"):
            paths.append(path)
            continue
        try:                                    # extensionless scripts, by shebang
            if path.is_file() and path.open("rb").readline().startswith(b"#!") \
               and b"sh" in path.open("rb").readline(0) or False:
                paths.append(path)
        except OSError:
            pass
    return paths


def _uncommented(text):
    """Drop whole-line comments. Enough for command-position matching, and it is what keeps this
    gate off the long block comments that now explain the defect in both repaired scripts."""
    return "\n".join(ln for ln in text.splitlines() if not ln.lstrip().startswith("#"))


def _definitions(text):
    return {m.group(1) or m.group(2) for m in DEFINITION.finditer(text)}


def _sourced_libs(path, text, seen=None):
    """The `deploy/lib/*.sh` files this script pulls in, following lib-to-lib sourcing."""
    seen = set() if seen is None else seen
    for raw in SOURCING.findall(_uncommented(text)):
        for name in LIB_BASENAME.findall(raw.split("#", 1)[0]):
            lib = LIB / name
            if lib.is_file() and lib not in seen:
                seen.add(lib)
                _sourced_libs(lib, lib.read_text(), seen)
    return seen


def _sourced_by_someone(scripts):
    """Basenames that some other tracked shell file sources.

    THE EXEMPTION, AND WHY IT IS NARROW. A file that is itself sourced is a LIBRARY, and its
    dependencies are the caller's to supply — `deploy/account-intel/data/smoke.sh` says so in
    its own header ("Sourced by prod-up.sh, dev-up.sh and seed-real.sh, all of which already
    source ../../lib/fleet.sh"), and all four of its callers do. Requiring a library to source
    what its callers already hold would be asking for the double-source this tree avoids.

    An exemption is where a gate goes to die, so it has a positive control:
    `test_the_repaired_scripts_are_not_exempt` asserts the two scripts that carried the defect
    are NOT in this set. They are systemd ExecStart targets, sourced by nothing."""
    sourced = set()
    for script in scripts:
        for raw in SOURCING.findall(_uncommented(script.read_text())):
            sourced.update(LIB_BASENAME.findall(raw.split("#", 1)[0]))
    return sourced


class ShellHelperSourcing(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.libs = {p: _definitions(p.read_text()) for p in sorted(LIB.glob("*.sh"))}
        cls.owner = {fn: p for p, fns in cls.libs.items() for fn in fns}
        cls.scripts = _tracked_shell_scripts()
        cls.exempt = _sourced_by_someone(cls.scripts)

    def test_the_repaired_scripts_are_not_exempt(self):
        """Without this, widening the library exemption would silently re-admit the exact defect
        this file exists for, and every test here would still pass."""
        for name in ("multi-watchdog.sh", "multi-backup.sh"):
            self.assertNotIn(name, self.exempt,
                             f"{name} is now sourced by something — it was an ExecStart target, "
                             "and the library exemption must not start covering it")

    def test_the_corpus_is_real(self):
        """Fail closed: a broken glob or a changed definition syntax would make the check below
        pass vacuously, which is the failure mode of every gate that enumerates."""
        self.assertGreaterEqual(len(self.libs), 3, sorted(p.name for p in self.libs))
        self.assertGreaterEqual(len(self.owner), 10, sorted(self.owner))
        self.assertIn("tg_send", self.owner, "telegram.sh no longer defines the extracted helper")

    def test_every_called_lib_helper_is_sourced(self):
        missing = []
        for script in self.scripts:
            if script.name in self.exempt:      # a library; its caller holds the dependency
                continue
            text = script.read_text()
            body = _uncommented(text)
            available = set()
            for lib in _sourced_libs(script, text):
                available |= self.libs.get(lib, set())
            local = _definitions(body)
            for helper, lib in self.owner.items():
                if helper in available or helper in local:
                    continue
                if re.search(CALL_SITE + re.escape(helper) + r"(?=\s|$|;|\))", body, re.M):
                    missing.append(f"{script.relative_to(ROOT)} calls {helper}() "
                                   f"but never sources {lib.relative_to(ROOT)}")
        self.assertEqual(missing, [], "\n  " + "\n  ".join(missing))


if __name__ == "__main__":
    unittest.main(verbosity=2)
