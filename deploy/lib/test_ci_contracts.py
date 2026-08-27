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
    assert (ROOT / "multi/seam/test_handoff_2b.py").is_file()
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


def test_every_gated_suite_runs_its_own_tests():
    """`release verify` runs each gate suite as a SCRIPT, so a file with no `__main__` block
    exits 0 having run nothing and is scored PASS.

    `multi/seam/test_suite_contract.py` asserts exactly this property, and the seam check above
    asserts it again — but both glob SEAM alone, so the three OTHER directories `release verify`
    discovers were never covered. This file had no runner itself, which is how eight of its own
    assertions went unexecuted behind a green `gate.lib.ci_contracts` row. Seam is deliberately
    absent from the map below: it already has two guards, and a third would only drift from them.

    The floor is asserted PER DIRECTORY. A single total would let one populated directory satisfy
    it while another had silently emptied out."""
    gated = {"deploy/lib": 12, "deploy/account-intel/data": 2, "multi/eval": 1}
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
