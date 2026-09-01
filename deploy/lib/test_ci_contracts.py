"""Contracts for the gates themselves — what CI promises, and what its verdicts mean."""
import ast
import importlib.util
import sys
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]


def _load(relative, name):
    """Import a module by PATH. `run-quality.py` has a hyphen, so `import` cannot reach it."""
    spec = importlib.util.spec_from_file_location(name, ROOT / relative)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def test_scheduled_production_exercise_installs_the_hash_locked_runtime():
    workflow = (ROOT / ".github/workflows/scheduled-integration.yml").read_text()
    assert "pip install --require-hashes -r deploy/account-intel/data/requirements.lock" in workflow
    assert "pip install flask==" not in workflow


def test_missing_live_proof_key_is_blocked_and_non_green():
    workflow = (ROOT / ".github/workflows/scheduled-integration.yml").read_text()
    blocked = workflow.split('if [ -z "$NEARAI_API_KEY" ]; then', 1)[1].split("\n          fi", 1)[0]
    assert "BLOCKED" in blocked
    assert "::error" in blocked
    assert "exit 0" not in blocked
    assert "exit 3" in blocked


def test_seam_ci_runs_the_locked_full_suite_and_wheel_smoke_on_every_supported_python():
    workflow = (ROOT / ".github/workflows/seam-ci.yml").read_text()
    assert 'python-version: ["3.12", "3.13"]' in workflow
    assert "python-version: ${{ matrix.python-version }}" in workflow
    assert "pip install --require-hashes -r requirements-dev.lock" in workflow
    assert "Smoke the installed package outside the checkout" in workflow
    assert "./deploy/ironworks test" in workflow
    assert "python-${{ matrix.python-version }}-coverage" in workflow


def test_canonical_pytest_discovery_includes_every_intended_offline_location():
    """Committed offline tests must be reachable through the CI command, without ignores."""
    config = tomllib.loads((ROOT / "pyproject.toml").read_text())["tool"]["pytest"][
        "ini_options"]
    testpaths = set(config["testpaths"])
    assert {
        "multi/seam",
        "deploy/lib",
        "deploy/account-intel/data",
        "multi/eval",
        "deploy/egress",
        "multi/verify/test_output_text_visibility.py",
    } <= testpaths
    assert "--ignore" not in config.get("addopts", "")
    assert (ROOT / "multi/verify/test_output_text_visibility.py").is_file()


def test_ironworks_test_delegates_to_pytests_canonical_configuration():
    quality = (ROOT / "deploy/run-quality.py").read_text()
    launcher = (ROOT / "deploy/ironworks").read_text()
    assert '[sys.executable, "-m", "coverage", "run", "-m", "pytest", "-q"]' in quality
    assert 'ROOT / "deploy" / "run-quality.py"' in launcher


def test_every_seam_suite_runs_when_invoked_the_way_CONTRIBUTING_documents():
    """`python3 test_x.py` from `multi/seam/` must actually execute that file's tests.

    THE THIRD OCCURRENCE THIS GUARD EXISTS TO PREVENT. Twice now a seam suite has been
    reachable only by pytest: `test_turn.py`'s own runner comment records a hand-maintained
    call list that drifted, and the four `test_bridge_*.py` files shipped with no `__main__`
    at all — 45 tests that printed nothing and exited 0. Neither the documented per-file
    loop nor `python3 -m unittest` collects a bare `def test_`, so for those four, nothing
    on any developer machine had ever run them.

    Exit 0 is not the signal — a file with no runner exits 0 too. The signal is the runner
    block itself, which is why this asserts on the source rather than on a return code."""
    suites = sorted((ROOT / "multi" / "seam").glob("test_*.py"))
    assert len(suites) >= 15, f"only {len(suites)} seam suites found — the glob is wrong"
    missing = [p.name for p in suites if '__main__' not in p.read_text()]
    assert not missing, (
        "seam suite(s) with no __main__ runner: " + ", ".join(missing) +
        " — `python3 <file>` exits 0 having run nothing, so the documented local gate is "
        "blind to them and only pytest collects them. Copy the block from test_registry.py.")


def test_quality_gate_blocked_exits_3_and_is_never_reported_as_a_failure():
    """BLOCKED means *could not evaluate*, and it is neither a pass nor a failure.

    `CONTRIBUTING.md` § "Instrumenting a new subsystem" fixes this for the console, and
    `run-quality.py` holds the same contract — but it did not always. Every unevaluated check
    collapsed into one boolean and exited 2, so a machine merely missing a TOOL reported the
    repository broken: no docker meant the compose stacks, no `shellcheck` or `node` meant those
    linters, and skipping the `requirements-dev.lock` install meant `coverage` and `pytest`, which
    blocks the entire test run. A laptop in that state could never see this gate green, and the
    honest verdict — "we did not check" — had nowhere to go.

    Asserted against `emit` rather than against source text, because the defect was in what the
    verdicts ADD UP TO, and a grep for `exit 3` would have passed on the broken version the
    moment the string appeared anywhere."""
    rq = _load("deploy/run-quality.py", "run_quality")
    assert rq.emit([("a", rq.PASS)]) == 0
    assert rq.emit([("a", rq.PASS), ("b", rq.BLOCKED)]) == 3, "blocked must not read as green"
    assert rq.emit([("a", rq.BLOCKED), ("b", rq.FAIL)]) == 2, "a real failure outranks blocked"
    assert rq.emit([("a", rq.FAIL)]) == 2


def test_a_missing_python_module_is_blocked_not_failed():
    """The half a `FileNotFoundError` handler cannot see.

    A missing BINARY raises; `python -m coverage` does not — it exits 1 with "No module named
    coverage", which is indistinguishable from a real failure by return code. Both mean the
    toolchain is incomplete, so both must be BLOCKED."""
    rq = _load("deploy/run-quality.py", "run_quality")
    assert rq.missing_module([sys.executable, "-m", "no_such_module_xyz"]) == "no_such_module_xyz"
    assert rq.missing_module([sys.executable, "-m", "json"]) is None
    assert rq.missing_module(["shellcheck", "-x"]) is None      # binaries are the caller's case


def test_a_tracked_but_deleted_file_is_a_verdict_not_a_traceback():
    """`tracked()` asks git, and git lists an index-present WORKTREE-ABSENT file — a staged
    deletion, or a file removed without staging it. That path went into an unguarded
    `read_text()`, and the traceback escaped between the per-file checks and `emit()`, so
    `main()` never returned: exit 1 — outside this gate's documented 0/2/3 — with no verdict
    summary and no BLOCKED listing. The same shape applied to any `git` failure, via
    `tracked()`'s own `check=True`.

    BLOCKED, not FAIL: "the file is not on disk" says nothing about whether its contents are
    correct, which is the distinction this whole file exists to hold."""
    from pathlib import Path
    rq = _load("deploy/run-quality.py", "run_quality")
    hits, unreadable = rq.pipefail_substitution_guard([ROOT / "deploy" / "does-not-exist.sh"])
    assert hits == [], hits
    assert len(unreadable) == 1 and "FileNotFoundError" in unreadable[0][1], unreadable
    # The error path may not itself throw on a path outside the repository.
    assert rq.pipefail_substitution_guard([Path("/nowhere/at/all.sh")])[1], "the reporter threw"
    assert rq.emit([("a", rq.PASS), ("b", rq.BLOCKED)]) == 3


# The directories `release verify` runs as scripts, and the minimum number of runnable
# suites each must hold. Read by two tests below; see the second for why they compare.
_GATED_DIRS = {"deploy/lib": 12, "deploy/account-intel/data": 2, "multi/eval": 1,
               "deploy/egress": 2}


def test_the_shell_gate_covers_what_runs_here_not_only_what_ships():
    """`.gitignore` decides DISTRIBUTION. It was also deciding what gets linted.

    Two live operator scripts — `deploy/repoint-hostname.sh` and `deploy/backup-laptop-agency.sh`
    — source `deploy/lib`, hold a Cloudflare token and a bot token, and run against real
    infrastructure. `shell_checks` enumerated with `tracked()`, which excludes ignored files, so
    on the one machine that runs them they were parsed, shellchecked and pipefail-guarded by
    nothing. "Do not ship it" and "do not check it" are separate decisions and only the first was
    ever made.

    ASSERTED IN BOTH ENVIRONMENTS, which is the awkward half. A fresh CI checkout has no ignored
    files, so `present()` and `tracked()` are equal there and any assertion about the difference
    would pass for the wrong reason. So: the superset relation and the enumerator in use are
    asserted always, and the disjointness of the ignored set is asserted only when there IS one —
    with the empty case reported rather than silently skipped."""
    import subprocess
    gate = _load("deploy/run-quality.py", "quality_gate")
    shipped, here = set(gate.tracked("*.sh")), set(gate.present("*.sh"))
    assert shipped and here, "no shell scripts found at all — the enumerator is looking wrong"
    assert shipped <= here, (
        f"`present()` lost files `tracked()` had: {sorted(p.name for p in shipped - here)}")

    src = (ROOT / "deploy" / "run-quality.py").read_text()
    assert 'shell_files = present("*.sh")' in src, (
        "the shell gate went back to `tracked()`, which does not see the ignored operator "
        "scripts that run on this machine")
    # js_checks stays on `tracked()` on purpose — see present()'s docstring (node_modules).
    assert 'js_files = tracked(' in src

    ignored = {ROOT / p for p in subprocess.run(
        ["git", "ls-files", "--others", "--ignored", "--exclude-standard", "--", "*.sh"],
        cwd=ROOT, capture_output=True, text=True).stdout.split()}
    if not ignored:
        print("  (no ignored *.sh present — the superset relation is all this can assert here)")
        return
    assert not (ignored & shipped), "a file is both ignored and shipped; git disagrees with itself"
    assert ignored <= here, (
        "these run here and no gate sees them: "
        f"{sorted(p.name for p in ignored - here)}")


def test_ruff_reports_no_warnings_of_its_own():
    """A CONFIGURED RULE THAT DOES NOTHING IS WORSE THAN AN ABSENT ONE, because the config reads
    as coverage. `pyproject.toml` selected `E303` for months; it is preview-only in the pinned
    ruff, so it never ran. Ruff announced this on EVERY invocation —

        warning: Selection `E303` has no effect because preview is not enabled

    — and every gate discarded it, because the check only ever looked at the exit code. Meanwhile
    the comment beside the selection went on citing a triple blank line as the reason it was
    there, and the tree accumulated twelve of them, the worst twenty-one lines long.

    THE ASSERTION IS ON STDERR, NOT ON A KNOWN STRING. A first draft matched only "has no effect"
    and would have missed the second instance found minutes later — `warning: Invalid # noqa
    directive`, raised because a comment mentioned the directive spelling in prose and ruff parses
    it wherever it appears. Both are the same defect: a suppression or selection that looks like
    configuration and does nothing. Ruff is quiet when it is happy, so anything on stderr is a
    finding and this needs no list of which rules are preview-only in this release."""
    import subprocess
    result = subprocess.run(
        ["ruff", "check", ".", "deploy/lib/compose-persona", "deploy/ironworks"],
        cwd=ROOT, capture_output=True, text=True)
    if result.returncode == 2 and "No such file" in (result.stderr + result.stdout):
        return                                     # ruff not installed; the gate blocks on that
    noise = [ln for ln in result.stderr.splitlines() if ln.strip()]
    assert not noise, (
        "ruff emitted warnings, which means part of this configuration is not doing what it "
        f"looks like it does: {noise}. A selection with no effect gates nothing; a malformed "
        "suppression suppresses nothing.")


def test_the_whitespace_residue_guard_catches_what_ruff_cannot():
    """The rules E303 and W391 used to nominally cover, asserted against real bytes.

    Watch the empty case first: `_residue_in(b"")` must be silent, because a guard that reports
    a finding for every file is as useless as one that reports none."""
    gate = _load("deploy/run-quality.py", "quality_gate")
    assert gate._residue_in(b"") == []
    assert gate._residue_in(b"\x00\x01binary") == []
    assert gate._residue_in(b"fine\n\nstill fine\n") == []
    assert gate._residue_in(b"a\n\n\n\nb\n") == [(":2", "more than 2 consecutive blank lines")]
    assert gate._residue_in(b"a\n\n") == [("", "blank line at end of file")]
    assert gate._residue_in(b"a") == [("", "no newline at end of file")]
    # Not Python-only: the class is an editing residue, not a language feature.
    assert gate._residue_in(b"#!/bin/sh\necho a\n\n\n\necho b\n")


def test_every_gated_suite_runs_its_own_tests():
    """`release verify` runs each gate suite as a SCRIPT, so a file with no `__main__` block
    exits 0 having run nothing and is scored PASS.

    `multi/seam/test_suite_contract.py` asserts exactly this property, and the seam check above
    asserts it again — but both glob SEAM alone, so the other directories `release verify`
    discovers were never covered. This file had no runner itself, which is how eight of its own
    assertions went unexecuted behind a green `gate.lib.ci_contracts` row. Seam is deliberately
    absent from the map below: it already has two guards, and a third would only drift from them.

    The floor is asserted PER DIRECTORY. A single total would let one populated directory satisfy
    it while another had silently emptied out.

    `deploy/egress` is in this map because it is in `offline_dirs` — and it was in NEITHER until
    the two were compared. pytest collected its suites (`pyproject.toml` testpaths) while the
    readiness artifact did not run them, so the boundary suites were gated by the aggregate run
    alone. `test_release_verify_discovers_the_same_directories_this_map_floors` below is what
    keeps the two lists from drifting again, since that is exactly what happened here."""
    gated = _GATED_DIRS
    no_runner, hand_listed = [], []
    for rel, floor in gated.items():
        checked = 0
        for path in sorted((ROOT / rel).glob("test_*.py")):
            tree = ast.parse(path.read_text(), filename=str(path))
            if not _defines_tests(tree):
                continue
            checked += 1
            block = _main_block(tree)
            if block is None:
                no_runner.append(f"{rel}/{path.name}")
            elif "globals()" not in ast.unparse(block) and "unittest.main" not in ast.unparse(block):
                hand_listed.append(f"{rel}/{path.name}")
        assert checked >= floor, (
            f"only {checked} suites found in {rel} (expected at least {floor}) — this check is "
            "looking in the wrong place, or the directory moved")

    assert not no_runner, (
        "these suites define tests but execute NOTHING when run as scripts, so `release verify` "
        f"scores them PASS while running none of them: {no_runner}")
    assert not hand_listed, (
        f"these suites run a HAND-MAINTAINED list instead of discovering: {hand_listed}")


def test_release_verify_discovers_the_same_directories_this_map_floors():
    """TWO HAND-MAINTAINED LISTS OF THE SAME DIRECTORIES, AND THEY HAD DIVERGED. `release
    verify`'s `offline_dirs` decides which suites run for the readiness artifact; the `gated` map
    above decides which are floor-checked for having a runner. `deploy/egress` was in neither
    while pytest collected it, so its two suites — including the one driving the real gateway over
    real sockets — never contributed to `release.promotable`.

    Adding it to both fixes today. Comparing them fixes tomorrow: neither list can gain or lose a
    directory alone. Seam is the one deliberate asymmetry (it has two guards of its own), so it is
    named here rather than silently tolerated."""
    source = (ROOT / "deploy" / "ironworks").read_text()
    block = source.split("offline_dirs = (", 1)[1].split("\n    gates = [", 1)[0]
    discovered = set()
    for name in ("seam", "deploy/lib", "deploy/account-intel/data", "deploy/egress",
                 "multi/eval", "multi/verify"):
        parts = name.split("/")
        needle = " / ".join(f'"{p}"' for p in parts)
        if needle in block or (name == "seam" and "seam," in block):
            discovered.add(name)
    floored = set(_GATED_DIRS) | {"seam"}          # seam: guarded by test_suite_contract instead
    # `multi/eval` is floored but NOT discovered — its one suite is named individually in the
    # `gates` list below `offline_dirs`, because the directory also holds non-test modules.
    assert discovered - floored == set(), (
        f"`release verify` discovers {sorted(discovered - floored)}, which nothing floors for "
        "having a runner — a suite there could score PASS while executing nothing")
    assert "deploy/egress" in discovered, (
        "deploy/egress fell out of offline_dirs; pytest still collects it, so the release "
        "artifact would go green without ever running the egress boundary suites")


def _defines_tests(tree):
    """Bare `def test_*` and `unittest.TestCase` methods both count. Only the first shape is
    used in `multi/seam`, which is why a name-prefix check written for seam finds nothing in
    `deploy/lib`, where the class shape dominates and the classes are named for their subject
    (`DocRefs`, `Lifecycle`) rather than `Test*`."""
    for node in tree.body:
        if isinstance(node, ast.FunctionDef) and node.name.startswith("test_"):
            return True
        if isinstance(node, ast.ClassDef) and any(
                isinstance(m, ast.FunctionDef) and m.name.startswith("test_")
                for m in node.body):
            return True
    return False


def _main_block(tree):
    for node in tree.body:
        if not isinstance(node, ast.If):
            continue
        test = node.test
        if (isinstance(test, ast.Compare) and isinstance(test.left, ast.Name)
                and test.left.id == "__name__"
                and any(isinstance(c, ast.Constant) and c.value == "__main__"
                        for c in test.comparators)):
            return node
    return None


if __name__ == "__main__":
    # Discovered, not listed. This file had no runner at all, so `release verify`'s
    # `gate.lib.ci_contracts` ran it as a script, got exit 0 with no output, and scored eight
    # unexecuted assertions as PASS.
    for _name, _fn in list(globals().items()):
        if _name.startswith("test_") and callable(_fn):
            _fn()
    print("ALL CI CONTRACT TESTS PASS")
