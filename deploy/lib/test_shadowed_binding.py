#!/usr/bin/env python3
"""A name that is both an import and a data binding in one scope. Offline, stdlib only.

Run: python3 deploy/lib/test_shadowed_binding.py

THE DEFECT THIS EXISTS FOR. deploy/egress/proof/service_path_checks.py bound `reg` to a
tempdir Path, then rebound it with `import registry as reg` forty lines on, and a later
`reg / "state.db"` raised `TypeError: unsupported operand type(s) for /: 'module' and 'str'`.
Both bindings were deliberate; the collision was not. It sat in the bridge crash-recovery leg of
a proof reached only after two real model turns, so it had never executed.

WHY NO EXISTING GATE SAW IT. Ruff does not catch this shape, and not because of how this repo
configures it — measured against the exact statement sequence under F, E9, B, C90, F811, PLR,
PLW, A and `--select ALL`, the only findings were a missing docstring, unsorted imports and two
`print`s. F811 cannot fire: it reports redefinition of an unused *import/def/class*, and here the
first binding is an assignment which is used before the import lands. Nor could a test have
caught it — the file executes on import and requires a provisioned stack, so nothing can import
it, which is also why it has no test of its own.

SCOPE-AWARE, because the innocent version is common: `multi/eval/run_eval.py` imports
`context_ingress as ing` INSIDE build_thread() and unpacks `ing, thread = build_thread()` at
main scope. Two scopes, no collision. A flat walk reports it and the gate gets ignored.

Scanned as `git ls-files --cached --others --exclude-standard` (CONTRIBUTING § the file set CI
uses), plus the two extensionless Python entry points ruff has to be told about by name for the
same reason: discovery by extension misses them.
"""
import ast
import pathlib
import subprocess
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[2]
EXTENSIONLESS = ("deploy/ironworks", "deploy/lib/compose-persona")

# `except E as e` is excluded: Python unbinds it at the end of the block, so the collision the
# name suggests cannot outlive the handler. Comprehension variables have their own scope.
BINDING_STATEMENTS = (ast.Assign, ast.AnnAssign, ast.AugAssign, ast.For, ast.AsyncFor,
                      ast.With, ast.AsyncWith, ast.NamedExpr)
SCOPES = (ast.FunctionDef, ast.AsyncFunctionDef, ast.Lambda, ast.ClassDef)


def _scope_nodes(node):
    """Every node in `node`'s own scope, stopping at each nested scope's boundary."""
    body = getattr(node, "body", [])
    # A Lambda's `body` is a single EXPRESSION; every other scope node's is a list of statements.
    # Walking a lambda can never yield a finding — an import is a statement, so no collision can
    # live inside one. Lambda is in SCOPES for the boundary it draws in the PARENT: without it a
    # walrus inside a lambda counts as a binding in the enclosing scope, and `import json` beside
    # `f = lambda: (json := 1)` reads as a collision. Asserted below, both ways.
    out, stack = [], list(body) if isinstance(body, list) else [body]
    while stack:
        n = stack.pop()
        out.append(n)
        if isinstance(n, SCOPES):
            continue                      # its contents belong to ITS scope, not this one
        stack.extend(c for c in ast.iter_child_nodes(n) if not isinstance(c, SCOPES))
    return out


def _bindings(node):
    """({name: [lineno]} imported, {name: [lineno]} assigned) for one scope."""
    imported, assigned = {}, {}
    for n in _scope_nodes(node):
        if isinstance(n, (ast.Import, ast.ImportFrom)):
            for a in n.names:
                if a.name != "*":
                    imported.setdefault(a.asname or a.name.split(".")[0], []).append(n.lineno)
        elif isinstance(n, BINDING_STATEMENTS):
            if isinstance(n, ast.Assign):
                targets = n.targets
            elif isinstance(n, (ast.With, ast.AsyncWith)):
                targets = [i.optional_vars for i in n.items]
            else:
                targets = [n.target]
            for t in targets:
                for name in (ast.walk(t) if t is not None else ()):
                    if isinstance(name, ast.Name) and isinstance(name.ctx, ast.Store):
                        assigned.setdefault(name.id, []).append(name.lineno)
    return imported, assigned


def shadowed(source, label="<src>"):
    """['label:LINE: name imported at [..], assigned at [..]'], one per collided name."""
    tree = ast.parse(source, label)
    found = []
    for node in [tree] + [n for n in ast.walk(tree) if isinstance(n, SCOPES)]:
        imported, assigned = _bindings(node)
        for name in sorted(set(imported) & set(assigned)):
            i, a = sorted(imported[name]), sorted(assigned[name])
            found.append(f"{label}:{min(i + a)}: {name!r} imported at {i}, assigned at {a}")
    return found


def python_files():
    listed = subprocess.run(
        ["git", "ls-files", "--cached", "--others", "--exclude-standard", "--", "*.py"],
        cwd=ROOT, capture_output=True, text=True, check=True).stdout.split()
    return [ROOT / p for p in listed] + [ROOT / p for p in EXTENSIONLESS]


class TheShapeItWasBuiltFor(unittest.TestCase):
    """Fixtures, so the check keeps proving it still catches the thing it was written for after
    the file that motivated it is fixed and forgotten."""

    PRE_FIX = ("import pathlib\n"
               "with open('x') as f:\n"
               "    reg = pathlib.Path('d')\n"
               "    import registry as reg\n"
               "    db = reg / 'state.db'\n")

    TWO_SCOPES = ("def build_thread():\n"
                  "    import context_ingress as ing\n"
                  "    return ing, None\n"
                  "ing, thread = build_thread()\n")

    def test_the_service_path_collision_is_reported(self):
        found = shadowed(self.PRE_FIX, "service_path_checks.py")
        self.assertEqual(len(found), 1, found)
        self.assertIn("'reg'", found[0])

    def test_order_does_not_matter(self):
        """The import came SECOND in the real defect, but either order is the same collision and
        either half can be the surprising one."""
        self.assertEqual(len(shadowed("import json as reg\nreg = 1\n")), 1)

    def test_an_import_and_an_assignment_in_DIFFERENT_scopes_are_not_reported(self):
        """The multi/eval/run_eval.py shape. Reporting it is how this gate would get ignored."""
        self.assertEqual(shadowed(self.TWO_SCOPES, "run_eval.py"), [])

    def test_a_handler_variable_is_not_a_collision(self):
        """`except ... as e` is unbound at the end of the block; the name cannot survive to
        collide with anything."""
        self.assertEqual(shadowed("import errno as e\ntry:\n    pass\n"
                                  "except OSError as e:\n    pass\n"), [])

    def test_a_star_import_is_not_treated_as_binding_one_name(self):
        self.assertEqual(shadowed("from os.path import *\njoin = 1\n"), [])

    def test_a_walrus_inside_a_lambda_does_not_bind_in_the_enclosing_scope(self):
        """Why Lambda is in SCOPES. Nothing inside a lambda can collide — an import is a
        statement — so its whole contribution is the boundary it draws in the parent. Drop
        Lambda from SCOPES and this source reports 'json' imported at [1], assigned at [2]."""
        self.assertEqual(shadowed("import json\nf = lambda: (json := 1)\n"), [])


class TheTree(unittest.TestCase):
    def test_no_tracked_python_file_shadows_an_import(self):
        found = []
        for path in python_files():
            try:
                source = path.read_text()
            except OSError:
                continue                  # listed but not on disk: test_doc_refs owns that gap
            try:
                found += shadowed(source, str(path.relative_to(ROOT)))
            except SyntaxError as e:
                self.fail(f"{path.relative_to(ROOT)} does not parse: {e}")
        self.assertEqual(found, [], "\n" + "\n".join(found) + "\nA name bound both ways in one "
                         "scope: the second binding wins from its line on, and every use after it "
                         "silently means the other thing. Rename one — the import is usually the "
                         "later and more surprising half.")

    def test_the_scan_is_not_vacuous(self):
        """A `git ls-files` that returned nothing would make the gate above pass on an empty set,
        which is the failure mode this whole pack is about."""
        self.assertGreater(len(python_files()), 50)


if __name__ == "__main__":
    unittest.main(verbosity=2)
