#!/usr/bin/env python3
"""What a seam suite owes the LOCAL gate. Run: python3 test_suite_contract.py (from multi/seam)

THE DEFECT, TWICE. CONTRIBUTING.md documents the local gate as running each suite directly:

    (cd multi/seam && for f in test_*.py; do python3 "$f" || break; done)

A file with no `__main__` block satisfies that loop by printing nothing and exiting 0. The loop
cannot tell "passed" from "did nothing", so a counter built on it scores silence as success.

  1. `test_turn.py` once ran from a HAND-MAINTAINED call list. It drifted: two tests defined in
     the file were never added to it, so pytest ran them and the documented command silently
     skipped them. Its runner comment still records this.
  2. The four suites split out of the delivery monolith — concurrency, delivery, operations,
     recovery — carried no runner at all. `python3 test_bridge_recovery.py` printed nothing and
     exited 0 for 45 tests covering every crash boundary, the residual-window duplicate, the
     atomic DELIVERED/offset commit, and the 0600 store check. `unittest` collects none of them
     either (bare functions, not TestCase), so pytest was the only path anywhere that ran them —
     and pytest is not installed on every developer interpreter. Two separate reports of
     "17/17 seam suites pass" were made against a loop executing thirteen.

Both were found by accident, months apart, and the second only because a deliberate breaking
change to `bridge_state` failed to break the test that asserted the thing it broke.

SO THIS ASSERTS THE CLASS, not the instance. Adding the four missing blocks fixes the files that
exist today; a fifth suite split out next month reintroduces it. Two properties, matching the two
occurrences exactly:

  * every suite that defines tests has a `__main__` block — occurrence 2;
  * that block DISCOVERS via `globals()` rather than naming tests — occurrence 1, because a
    hand-maintained list is the other way to run a file and execute less than it contains.

WHAT THIS DELIBERATELY DOES NOT CHECK: that the tests pass. That is every other file's job. This
one only answers "would running this file run anything?", which is the question the local gate
silently assumes and cannot ask for itself.

Helper modules are exempt by NAMING, not by exception: shared fixtures live in
`_bridge_delivery_support.py`, whose leading underscore keeps it out of `test_*.py` and out of
pytest collection at the same time. A file with no test functions is also skipped — it has
nothing to run, and failing it would push people toward an empty runner that satisfies the
letter of this check while restoring the defect.
"""
import ast
import pathlib

# A SANITY FLOOR, NOT A CENSUS. It exists to catch a glob that has stopped matching — a moved
# directory, a renamed suffix — so it is set well BELOW the true count (18 at the time of
# writing) and should never need bumping as suites are added. Stated once because it was
# stated twice, in the file whose own thesis is that hand-maintained numbers drift.
_SUITE_FLOOR = 14

SEAM = pathlib.Path(__file__).resolve().parent


def _main_block(tree):
    """The `if __name__ == "__main__":` node, or None."""
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


def test_every_seam_suite_runs_its_own_tests():
    """A suite that defines tests must execute them when run as a script, by discovery."""
    no_runner, hand_listed, checked = [], [], 0
    for path in sorted(SEAM.glob("test_*.py")):
        tree = ast.parse(path.read_text(), filename=str(path))
        defines = any(isinstance(n, ast.FunctionDef) and n.name.startswith("test_")
                      for n in tree.body)
        if not defines:
            continue
        checked += 1
        block = _main_block(tree)
        if block is None:
            no_runner.append(path.name)
        elif "globals()" not in ast.unparse(block):
            hand_listed.append(path.name)

    assert checked >= _SUITE_FLOOR, (
        f"only {checked} suites found — this check is looking in the wrong place")
    assert not no_runner, (
        "these suites define tests but execute NOTHING when run as scripts, so the local gate "
        f"in CONTRIBUTING.md scores them as passing while running none of them: {no_runner}. "
        'Add the block every other suite carries: `if __name__ == "__main__":` iterating '
        "globals() for callables named test_*.")
    assert not hand_listed, (
        "these suites run a HAND-MAINTAINED list instead of discovering via globals(): "
        f"{hand_listed}. That list drifts — it already did once in test_turn.py, where two "
        "tests were defined but never called by the documented command.")
    print(f"  PASS all {checked} seam suites execute their own tests, by discovery")


SIBLINGS = {p.stem for p in SEAM.glob("*.py")
            if not p.name.startswith("test_") and p.stem != "__init__"}


def _bare_sibling_imports(tree):
    """Top-level `import <sibling>` / `from <sibling> import …` outside any try/except.

    Only module scope: a shim IS a try/except, and the `except ImportError:` arm legitimately
    contains exactly these bare forms. Walking the whole tree would flag every correct shim.
    """
    hits = set()
    for node in tree.body:
        if isinstance(node, ast.Import):
            hits |= {a.name.split(".")[0] for a in node.names} & SIBLINGS
        elif isinstance(node, ast.ImportFrom) and node.level == 0 and node.module:
            if node.module.split(".")[0] in SIBLINGS:
                hits.add(node.module.split(".")[0])
    return hits


def _seam_path_insert(tree):
    """True if the module puts its OWN directory on `sys.path` at module scope.

    Checked separately from the imports above because it is the half that outlives the module:
    the entry is never removed, so every later suite's `except ImportError:` arm can succeed on
    it and the loud ModuleNotFoundError stops being loud. A file can do this while its imports
    look shimmed, so finding no bare import is not evidence that this is absent.
    """
    for node in ast.walk(tree):
        if not isinstance(node, ast.Call) or not isinstance(node.func, ast.Attribute):
            continue
        if node.func.attr not in ("insert", "append"):
            continue
        target = node.func.value
        if (isinstance(target, ast.Attribute) and target.attr == "path"
                and isinstance(target.value, ast.Name) and target.value.id == "sys"):
            return True
    return False


def test_every_seam_suite_reaches_its_siblings_through_the_shim():
    """THE MATCHED PAIR, asserted. CONTRIBUTING.md § "Running the CI gates locally" says the
    `__init__.py` package markers and the `try: from . import X / except ImportError: import X`
    shims are two halves of one mechanism: the markers let pytest resolve these suites from the
    repository root, the shims keep them runnable as bare scripts from inside `multi/seam/`.

    `test_handoff_2b.py` (since retired with the unwired module it covered) had the markers and
    not the shim. The incident is kept because the MECHANISM is still live — every suite here
    depends on that pair — and a worked example is why the rule is followed. It reached its
    siblings with
    `sys.path.insert(<this dir>)` and bare imports, so under `pytest multi/seam` from the root it
    loaded a SECOND copy of context_ingress, handoff, registry, persona, services, envelope,
    account_service, responses and pins as top-level modules beside the `multi.seam.*` ones
    already imported — two ClientConfig classes, two sets of module globals. And because the path
    entry was never removed, every other suite's `except ImportError:` arm could succeed for the
    rest of the session, turning the loud `ModuleNotFoundError` that says the two halves have
    come apart into silence.

    Nothing failed, which is why it survived: both copies behave identically until one of them
    holds state the other does not.
    """
    offenders, path_hackers, checked = {}, [], 0
    for path in sorted(SEAM.glob("test_*.py")):
        checked += 1
        tree = ast.parse(path.read_text(), filename=str(path))
        bare = _bare_sibling_imports(tree)
        if bare:
            offenders[path.name] = sorted(bare)
        if _seam_path_insert(tree):
            path_hackers.append(path.name)

    assert SIBLINGS, "no seam modules found — this check is looking in the wrong place"
    assert checked >= _SUITE_FLOOR, (
        f"only {checked} suites found — this check is looking in the wrong place")
    assert not offenders, (
        "these suites import a seam sibling at module scope WITHOUT the try/except shim, so "
        f"pytest-from-the-root loads a second copy of it: {offenders}. Use the pair every other "
        "suite carries:\n"
        "    try:\n"
        "        from . import context_ingress as ing\n"
        "    except ImportError:\n"
        "        import context_ingress as ing")
    assert not path_hackers, (
        f"these suites mutate sys.path at module scope: {path_hackers}. The shim makes it "
        "unnecessary, and the entry OUTLIVES the module — every later suite's `except "
        "ImportError:` arm can then succeed on it, turning the ModuleNotFoundError that says "
        "the package markers and the shims have come apart into silence.")
    print(f"  PASS all {checked} seam suites reach their siblings through the shim")


if __name__ == "__main__":
    # Discovered, not listed — the property this file exists to enforce, applied to itself.
    for _name, _fn in list(globals().items()):
        if _name.startswith("test_") and callable(_fn):
            _fn()
    print("ALL SUITE CONTRACT TESTS PASS")
