#!/usr/bin/env python3
"""One local/CI quality gate. Run through ``./deploy/ironworks test``.

EXIT CODES, and they are the console's, not this file's own invention. `CONTRIBUTING.md`
§ "Instrumenting a new subsystem" fixes the contract: PASS, FAIL, BLOCKED — where BLOCKED means
*could not evaluate* and exits 3, and "a check that could not run must never report PASS". The
inverse is equally the point and is what this file used to get wrong: it must never report FAIL
either. Every unevaluated check collapsed into `ok = False` and exited 2, so a laptop without
docker could not see this gate green, and a missing `shellcheck`, `ruff` or `node` read as a
broken repository rather than an incomplete toolchain.

    0   every evaluated check passed
    2   at least one check FAILED — something is actually wrong
    3   nothing failed, but at least one check could not be evaluated
"""
from __future__ import annotations

import importlib.util
import os
import pathlib
import re
import shutil
import subprocess
import sys


ROOT = pathlib.Path(__file__).resolve().parents[1]
PASS, FAIL, BLOCKED = "PASS", "FAIL", "BLOCKED"


def missing_module(command):
    """The module name of a `python -m NAME` command that is not importable, else None.

    A missing BINARY raises FileNotFoundError and is caught below; a missing MODULE does not —
    `python -m coverage` just exits 1 with "No module named coverage", which is indistinguishable
    from a real failure by return code alone. Both are the same fact ("the toolchain is
    incomplete") and both must be BLOCKED, so the module case is asked before the run rather than
    guessed at from stderr afterwards. `coverage` and `pytest` are in `requirements-dev.lock`;
    without them installed this gate used to report the repository broken.
    """
    if len(command) < 3 or command[0] != sys.executable or command[1] != "-m":
        return None
    try:
        return None if importlib.util.find_spec(command[2]) else command[2]
    except (ImportError, ValueError):
        return command[2]


def run(label, command, *, cwd=ROOT, env=None, stdin=None):
    """PASS / FAIL / BLOCKED for one command. A missing TOOL is blocked, not failed."""
    print(f"\n== {label} ==", flush=True)
    absent = missing_module(command)
    if absent:
        print(f"BLOCKED: no module named {absent} — install requirements-dev.lock",
              file=sys.stderr)
        return BLOCKED
    try:
        result = subprocess.run(command, cwd=cwd, env=env, input=stdin, text=True)
    except FileNotFoundError as error:
        print(f"BLOCKED: {error}", file=sys.stderr)
        return BLOCKED
    if result.returncode:
        print(f"FAILED: {label} exited {result.returncode}", file=sys.stderr)
        return FAIL
    return PASS


# `$(... grep ...)` and `$(... | grep ...)` under `set -euo pipefail` die SILENTLY when the
# pattern does not match: the command substitution exits non-zero, `set -e` aborts the script,
# and the caller sees a clean exit with nothing done. `bash -n` parses it happily and shellcheck
# does not flag it — this guard is the only thing that ever has. It was one of ~25 individually
# commented checks in the four-job workflow this file replaced, and it was the only check that
# consolidation dropped outright; ported here rather than left as a comment in git history.
#
# Two rules preserved exactly from the shell original. Only scripts that OPT IN to pipefail are
# at risk, so the file must contain `pipefail` before any line in it counts. And the `|| true`
# exemption is LINE-oriented: a script that handles the empty case correctly on one line has
# said so on that line, and a file-wide exemption would hide the next one.
PIPEFAIL_SUBST = re.compile(r"\$\( *(?:pgrep|grep)|\$\(.+\| *(?:pgrep|grep)")


def _rel(path):
    """Display path, never an exception. `relative_to` RAISES for a path outside ROOT, so
    using it to format an error message can throw from inside the handler that exists to
    stop this file from throwing."""
    try:
        return path.relative_to(ROOT)
    except ValueError:
        return path


def pipefail_substitution_guard(shell_files):
    """([(path, lineno, line)], [unreadable]) — every unguarded grep/pgrep substitution under
    pipefail, plus the files that could not be read.

    `tracked()` asks git, which lists an index-present WORKTREE-ABSENT file, so a staged
    deletion put a nonexistent path through an unguarded `read_text()`. The traceback escaped
    from between the per-file checks and `emit()`, so `main()` never returned: exit 1 — outside
    this gate's documented 0/2/3 — with no verdict summary and no BLOCKED listing at all. A
    condition this gate can encounter is a ROW, never a crash."""
    hits, unreadable = [], []
    for path in shell_files:
        try:
            text = path.read_text(errors="ignore")
        except OSError as e:
            unreadable.append((_rel(path), f"{type(e).__name__}: {e.strerror}"))
            continue
        if "pipefail" not in text:
            continue
        for n, line in enumerate(text.splitlines(), 1):
            if PIPEFAIL_SUBST.search(line) and "|| true" not in line:
                hits.append((_rel(path), n, line.strip()))
    return hits, unreadable


def blocked_no_file_list(record, label):
    """Record `label` as BLOCKED for the one condition all three sections can hit — `git
    ls-files` did not answer — and SAY SO on stderr.

    Three sibling sections handled this identically-shaped case three different ways: one printed
    a diagnostic, two recorded BLOCKED silently. A bare `BLOCKED  JavaScript syntax` row with no
    reason is exactly what `emit()`'s own comment argues against one screen further down ("a count
    of blocked checks that does not say WHICH reads as a rounding error") — naming the check but
    not the cause leaves the reader in the same place."""
    print(f"\nBLOCKED: {label}: `git ls-files` failed — the file list could not be obtained, so "
          "nothing was checked. Is this a git repository with a readable index?", file=sys.stderr)
    record(label, BLOCKED)


def _git_files(args, patterns):
    """`git ls-files` for one argument set, or None if git itself could not answer.

    `check=True` made a git failure (not a repository, a broken index) a CalledProcessError out
    of a helper every section calls, with the same consequence as an unguarded read: no verdict
    at all, where "the file list could not be obtained" is a perfectly reportable BLOCKED."""
    try:
        result = subprocess.run(["git", "ls-files", *args, "-z", "--", *patterns],
                                cwd=ROOT, capture_output=True, check=True)
    except (subprocess.CalledProcessError, FileNotFoundError):
        return None
    return [ROOT / p.decode() for p in result.stdout.split(b"\0") if p]


def tracked(*patterns):
    """What SHIPS: tracked plus untracked-and-not-ignored. The right set for repository hygiene."""
    return _git_files(["--cached", "--others", "--exclude-standard"], patterns)


def present(*patterns):
    """What RUNS ON THIS MACHINE: `tracked()` plus the ignored files that are actually here.

    THE TWO QUESTIONS ARE NOT THE SAME, and the linter was asking the wrong one. `.gitignore`
    excludes operator-workstation tooling on a stated principle — "Fleet operation ships
    (multi/serve/); tooling for one machine does not. The audience decides, not the content."
    That is a sound rule about DISTRIBUTION, and it had quietly decided something else as well:
    `shell_checks` enumerated with `tracked()`, so `deploy/repoint-hostname.sh` and
    `deploy/backup-laptop-agency.sh` — live scripts that source `deploy/lib`, hold a Cloudflare
    token and a bot token, and run against real infrastructure — were parsed, shellchecked and
    pipefail-guarded by nothing at all, on the one machine that runs them.
    "Do not ship it" and "do not check it" are different decisions and only the first was made.

    Costs nothing in CI: a fresh checkout has no ignored files, so this returns exactly what
    `tracked()` does there. The coverage appears precisely where the risk does.

    NOT used for the whitespace guard, deliberately — that one is about the bytes this
    repository publishes, which is `tracked()`'s question by definition.

    NOT used for `js_checks` either, and that is a judgement rather than an oversight: the
    ignored `*.sh` set is exactly two files, each named individually in `.gitignore`, while the
    ignored `*.js` set on a machine that has ever run `npm install` is `node_modules` — thousands
    of files nobody here wrote. `*.sh` is safe to widen because this repository ignores shell
    scripts one at a time and for a stated reason; nothing else it ignores has that property.
    """
    shipped = tracked(*patterns)
    ignored = _git_files(["--others", "--ignored", "--exclude-standard"], patterns)
    if shipped is None or ignored is None:
        return None
    seen = {p: None for p in shipped}          # order-preserving union; git can list a path twice
    seen.update({p: None for p in ignored})
    return list(seen)


# Placeholders, not secrets: `docker compose config` only has to interpolate, and every one of
# these is a `${VAR:?}` the files refuse to start without. The SET is derived from the files
# themselves (deploy/lib/compose_env.py) rather than listed twice — this list and
# `egress_status.overlay_configured`'s had already drifted by four variables.
sys.path.insert(0, str(ROOT / "deploy" / "lib"))
from compose_env import placeholder_env  # noqa: E402

COMPOSE_STACKS = (("multi/instance/docker-compose.yml",),
                  ("multi/instance/docker-compose.yml",
                   "deploy/egress/docker-compose.egress.yml"),
                  ("deploy/egress/proof/docker-compose.proof.yml",),
                  ("deploy/account-intel/data/docker-compose.yml",))


def compose_checks():
    """[(label, verdict)] for every compose stack — or one BLOCKED row if docker is absent.

    BLOCKED and not FAIL, which is the whole reason this file grew a third verdict: docker's
    absence says nothing about whether these files are valid. Reporting it as a failure made
    every laptop without docker look like a broken repository."""
    if not shutil.which("docker"):
        print("\n== compose configuration ==\nBLOCKED: docker is not installed", file=sys.stderr)
        return [("compose configuration", BLOCKED)]
    env = placeholder_env(*(ROOT / f for stack in COMPOSE_STACKS for f in stack),
                          base=os.environ)
    rows = []
    for files in COMPOSE_STACKS:
        command = ["docker", "compose"]
        for file in files:
            command += ["-f", file]
        label = "compose config: " + " + ".join(files)
        rows.append((label, run(label, command + ["config", "-q"], env=env)))
    return rows


def emit(results):
    """One place where the verdicts become the exit code, so it is derived and never chosen.

    Split out of `main` because reporting is a separate job from running, and because the two
    used to be tangled enough that "could not evaluate" got folded into "failed" without anyone
    having to decide it."""
    failed = [label for label, verdict in results if verdict == FAIL]
    blocked = [label for label, verdict in results if verdict == BLOCKED]
    print(f"\n{len(results) - len(failed) - len(blocked)} passed · {len(failed)} FAILED · "
          f"{len(blocked)} BLOCKED", flush=True)
    # Name them. A count of blocked checks that does not say WHICH reads as a rounding error.
    for label in failed:
        print(f"  FAILED   {label}", flush=True)
    for label in blocked:
        print(f"  BLOCKED  {label}", flush=True)
    if failed:
        print("\nQUALITY GATES FAILED", flush=True)
        return 2
    if blocked:
        print("\nQUALITY GATES INCOMPLETE — nothing failed, but the above was not evaluated",
              flush=True)
        return 3
    print("\nALL QUALITY GATES PASS", flush=True)
    return 0


def shell_checks(check, record):
    """bash -n, ShellCheck and the pipefail guard over every tracked shell file.

    Split out of `main` with `js_checks` below because handling "git could not answer" and "the
    file is not on disk" as VERDICTS rather than as tracebacks is several branches, and `main`
    is a list of sections, not a place for them."""
    # `present`, not `tracked`: a linter's question is "what runs here", not "what ships".
    # See that function for the two ignored operator scripts this brings into the gate.
    shell_files = present("*.sh")
    if shell_files is None:
        blocked_no_file_list(record, "shell checks")
        return
    for path in shell_files:
        check(f"bash syntax: {_rel(path)}", ["bash", "-n", str(path)])
    if shell_files:
        check("ShellCheck", ["shellcheck", *map(str, shell_files)])

    print("\n== pipefail substitution guard ==", flush=True)
    hits, unreadable = pipefail_substitution_guard(shell_files)
    for rel, n, line in hits:
        print(f"FAILED: {rel}:{n}: {line}\n"
              "        an unmatched grep/pgrep in $() aborts the script under `set -euo "
              "pipefail`; append '|| true' and test emptiness explicitly", file=sys.stderr)
    for rel, why in unreadable:
        print(f"BLOCKED: {rel}: {why}\n"
              "        git lists it but it is not on disk — restore it, or stage its deletion",
              file=sys.stderr)
    record("pipefail substitution guard",
           FAIL if hits else (BLOCKED if unreadable else PASS))


def js_checks(check, record):
    """`node --check` over every tracked JavaScript file, reading each through the same
    could-not-read rule as the shell files above."""
    js_files = tracked("*.js", "*.mjs")
    if js_files is None:
        blocked_no_file_list(record, "JavaScript syntax")
        return
    for path in js_files:
        try:
            source = path.read_text()
        except OSError as e:
            print(f"BLOCKED: {_rel(path)}: {type(e).__name__}", file=sys.stderr)
            record(f"JavaScript syntax: {_rel(path)}", BLOCKED)
            continue
        check(f"JavaScript syntax: {_rel(path)}",
              ["node", "--input-type=module", "--check"], stdin=source)


# More than two consecutive blank lines. PEP 8 tops out at two (between top-level definitions),
# so three is always residue — the gap left by deleting the thing that used to sit there.
MAX_BLANK_RUN = 2


def whitespace_residue_guard(record):
    """Blank-line residue over every tracked TEXT file: at EOF, at the end of a line, or in a
    run of three or more.

    WHY THIS IS NOT A RUFF RULE — AND WHY THE PREVIOUS ANSWER TO THAT WAS HALF RIGHT. This
    docstring used to say `ruff` "selects W now, which covers W291/W292/W293 and E303" and that
    only W391 was out of reach as a preview rule. Two things were wrong with that. E303 is not a
    W rule; and it is preview-only in the pinned ruff EXACTLY as W391 is, so selecting it in
    `pyproject.toml` did nothing at all. Ruff said so on every single run —

        warning: Selection `E303` has no effect because preview is not enabled.

    — and nothing read it. The triple blank line named above as the reason this gate exists was
    therefore still ungated after it was written, and by the time it was measured the tree held
    twelve of them, the worst being TWENTY-ONE consecutive blank lines in
    `multi/seam/test_turn.py`.

    So the blank-run check moved here, next to the EOF check it was always the twin of, and for
    the same two reasons: `--preview` would enable every other unstable rule to get these two,
    and the class is not a Python one — the identical edit leaves the identical residue in a
    shell script or a markdown file, where no linter is looking at all.

    THE SELECTION IS ALSO ASSERTED, in `deploy/lib/test_ci_contracts.py`: a rule that is selected
    and silently inert is the defect this paragraph describes, and prose cannot catch the next
    one.
    """
    files = tracked("*")
    if files is None:
        blocked_no_file_list(record, "whitespace residue")
        return
    hits, unreadable = [], []
    for path in files:
        try:
            raw = path.read_bytes()
        except OSError as e:
            unreadable.append((_rel(path), f"{type(e).__name__}: {e.strerror}"))
            continue
        hits += [(f"{_rel(path)}{at}", why) for at, why in _residue_in(raw)]
    for rel, why in hits:
        print(f"FAILED: {rel}: {why}", file=sys.stderr)
    for rel, why in unreadable:
        print(f"BLOCKED: {rel}: {why}", file=sys.stderr)
    record("whitespace residue", FAIL if hits else (BLOCKED if unreadable else PASS))


def _residue_in(raw):
    """[(location-suffix, why)] for one file's bytes. Empty for a file with nothing to say.

    Split out from the walk above so each has one job, and because it makes the rules directly
    testable on bytes — `deploy/lib/test_ci_contracts.py` calls this rather than building a git
    tree per case."""
    if not raw or b"\0" in raw[:8000]:
        return []                         # empty, or binary — neither has a "line ending"
    try:
        text = raw.decode()
    except UnicodeDecodeError:
        return []
    found = []
    if text.endswith("\n\n"):
        found.append(("", "blank line at end of file"))
    elif not text.endswith("\n"):
        found.append(("", "no newline at end of file"))
    run = start = 0
    for n, line in enumerate(text.splitlines(), 1):
        if line.strip():
            run = 0
            continue
        start = n if run == 0 else start
        run += 1
        if run == MAX_BLANK_RUN + 1:
            found.append((f":{start}", f"more than {MAX_BLANK_RUN} consecutive blank lines"))
    return found


def main():
    results = []

    def check(label, command, **kw):
        results.append((label, run(label, command, **kw)))

    def record(label, verdict):
        results.append((label, verdict))

    check("Python tests with diagnostic coverage",
          [sys.executable, "-m", "coverage", "run", "-m", "pytest", "-q"])
    check("coverage report (diagnostic; no threshold)",
          [sys.executable, "-m", "coverage", "report", "-m"])
    for proof in ([sys.executable, "multi/verify/test_adversarial_routing.py"],
                  [sys.executable, "multi/verify/test_fixtures_offline.py"],
                  [sys.executable, "multi/verify/test_freshness_lifecycle.py", "--offline"]):
        check("offline proof: " + proof[1], proof)

    check("Python static analysis",
          ["ruff", "check", ".", "deploy/lib/compose-persona", "deploy/ironworks"])
    shell_checks(check, record)
    js_checks(check, record)
    whitespace_residue_guard(record)
    check("Secretary unit tests",
          ["node", "--test", "deploy/secretary/worker/secretary-core.test.js"])
    check("service definitions",
          ["./deploy/ironworks", "--offline", "--json", "service", "validate"])

    results.extend(compose_checks())
    return emit(results)


if __name__ == "__main__":
    raise SystemExit(main())
