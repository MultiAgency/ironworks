"""Every repo-relative path named in the docs must be a path the reader can actually open.

The docs name files constantly, and nothing else reads markdown — so a rename or a removal
leaves citations pointing at nothing, and a reader who greps for a promised path cannot tell
whether it moved, was retired, or never existed.

WHAT IT CHECKS. Backtick-quoted tokens in tracked markdown that look like a repo-relative path
must resolve, from the repo root or relative to the citing file. Bare filenames (`doctor.sh`)
are deliberately NOT checked: the docs use them as shorthand for a path given in full
elsewhere, so requiring uniqueness would forbid the shorthand rather than catch a defect.

RESOLVED AGAINST THE INDEX, NOT THE DISK. A clone contains the tracked set and nothing else, so
the tracked set is exactly what the docs may promise a reader. Using Path.exists() instead would
pass on an author's laptop — where a gitignored file happens to sit — and fail in CI.

Run:  python3 deploy/lib/test_doc_refs.py
"""
import os
import pathlib
import re
import subprocess
import unittest

REPO = pathlib.Path(__file__).resolve().parents[2]

# Paths the docs name on purpose that this repo does not ship. Each needs a reason, because the
# value of the gate is that an unexplained dangling path fails. `tracked` says whether the file
# may nonetheless exist in a working tree — see ALLOWED_MISSING's two kinds below.
#   "absent"    — not here at all: retired, or living in another repo. Must not reappear tracked.
#   "untracked" — deliberately gitignored. May sit in an operator's tree; must never be tracked.
ALLOWED_MISSING = {
    # Retired with the Agent Relay PoC. Both citations describe it in the past tense; deleting
    # the sentences would delete the explanation of why intake no longer exists.
    "deploy/intake/provision-user.sh": ("absent", "retired PoC tooling, cited as retired"),
    # A second entry for the same file, `intake/provision-user.sh`, lived here for OPS.md's
    # RELATIVE citation of it. OPS.md is no longer published, so nothing cites the bare form
    # and the entry became dead weight. Note it would have kept passing
    # test_allowlist_entries_are_still_cited forever — that check is a substring match, and
    # the bare path is a substring of the full one above. An entry can outlive its reason
    # while still looking alive; removing it was a judgement, not a test result.
    # Upstream nearai/ironclaw's own tree, cited by UPGRADE.md so a bump can be checked against
    # the source. Never vendored here — this repo runs the official binary unmodified.
    "docker/reborn/entrypoint.sh": ("absent", "upstream ironclaw path, deliberately not vendored"),
    # Lives in the separate MultiAgency/vidgen repo; MULTIMEDIATOR.md tells the agent to fetch it
    # over the GitHub extension at runtime. The citation names where it lives THERE, which is why
    # it cannot resolve here and must not be "fixed" into a local path.
    "skills/vidgen-contributor/SKILL.md": ("absent", "vidgen repo, read over GitHub at runtime"),
}
# `agent/identity/IDEA_SCOUT.md` was here as an "untracked" entry while two docs cited it by full
# path. Both citations lived in the deploy/README.md block for the extension that persona drove;
# that subtree is no longer shipped, so the entry had nothing left to gate and this file's own
# test_allowlist_entries_are_still_cited failed it out. ARCHITECTURE.md § Personas still discusses
# the persona, but by bare filename like every other entry in that section, which this gate
# deliberately does not check. Re-add the entry if a doc ever names the full path again.

# Extensions worth checking. Deliberately excludes .rs and .toml: the docs cite upstream Rust and
# config files (`webui_serve.rs:855`, `handlers.rs`) as source citations, and those live in the
# ironclaw checkout rather than here.
EXTS = ("md", "py", "sh", "js", "json", "yml", "yaml", "sql", "service", "timer", "wit", "txt")

# Two shapes, and the second is not optional. A file citation ends in a known extension; a
# DIRECTORY citation ends in `/` (`multi/verify/`, `deploy/broker/`). An extension-only regex
# misses every directory — which is most of what a removed subtree leaves behind, and therefore
# most of the incident this gate was written for. Mutation-testing caught that: deleting all of
# `multi/verify/fixtures/` left the file-only version completely silent.
TOKEN = re.compile(
    r"`([A-Za-z0-9_./-]+/[A-Za-z0-9_./-]+\.(?:" + "|".join(EXTS) + r")"     # a file
    r"|[A-Za-z0-9_.-]+(?:/[A-Za-z0-9_.-]+)*/)`")                            # a directory


def _git(*args):
    return subprocess.run(["git", "-C", str(REPO), *args],
                          capture_output=True, text=True, check=True).stdout.split()


def tracked():
    """Every tracked path, plus every directory implied by one — docs cite dirs too."""
    paths = set(_git("ls-files"))
    for p in list(paths):
        parent = pathlib.PurePosixPath(p).parent
        while parent != pathlib.PurePosixPath("."):
            paths.add(str(parent))
            paths.add(str(parent) + "/")
            parent = parent.parent
    return paths


def tracked_markdown():
    return [REPO / p for p in _git("ls-files", "*.md")]


def _checkable(path, roots):
    """Is this citation a claim about THIS repo's tree, or shorthand for something else?

    Same principle the file rule uses when it ignores a bare `doctor.sh`. A directory citation
    qualifies only if it is rooted at a real top-level directory of this repo — so
    `deploy/broker/` is checked, while `data/` (shorthand inside a section about one subtree)
    and `src/scenes/` (a path in the separate vidgen repo) are not. Without it the gate fires on
    prose it was never meant to police, and a noisy gate gets switched off.
    """
    if not path.endswith("/"):
        return True
    return path.split("/", 1)[0] in roots


def dangling(index):
    """[(citing doc, cited path)] for every repo-relative citation git does not track."""
    roots = {p.split("/", 1)[0] for p in index if "/" in p}
    bad = []
    for doc in tracked_markdown():
        here = doc.parent.relative_to(REPO).as_posix()
        for raw in sorted(set(TOKEN.findall(doc.read_text()))):
            path = raw.split(":", 1)[0]                       # tolerate `file.py:123` citations
            if path in ALLOWED_MISSING or not _checkable(path, roots):
                continue
            # Docs cite both ways: from the repo root (sometimes `./`-prefixed) and relative to
            # the citing file (sometimes with `../`). Normalize BOTH forms rather than
            # concatenating, or every upward or dot-prefixed citation reads as dangling.
            from_root = os.path.normpath(path)
            from_here = os.path.normpath(f"{here}/{path}") if here != "." else from_root
            if from_root in index or from_here in index:
                continue
            bad.append((doc.relative_to(REPO).as_posix(), path))
    return bad


class DocRefs(unittest.TestCase):
    def setUp(self):
        self.index = tracked()

    def test_no_dangling_paths(self):
        bad = dangling(self.index)
        self.assertEqual(
            bad, [],
            "\nDocs name paths this repo does not ship:\n"
            + "".join(f"  {d}  ->  {p}\n" for d, p in bad)
            + "Fix the prose, restore the file, or — if the doc names something outside this "
              "repo on purpose — add it to ALLOWED_MISSING with its kind and reason.")

    def test_allowlist_entries_are_still_needed(self):
        """An allowlist entry that outlives its reason silently stops gating that path."""
        for path, (kind, why) in ALLOWED_MISSING.items():
            self.assertNotIn(
                path, self.index,
                f"\n{path} is now TRACKED but still in ALLOWED_MISSING ({kind}: {why}). "
                f"Remove the entry so the real file is gated like every other path.")

    def test_allowlist_entries_are_still_cited(self):
        """And one nothing cites is dead weight — the prose that needed it is gone."""
        corpus = "\n".join(d.read_text() for d in tracked_markdown())
        for path, (kind, why) in ALLOWED_MISSING.items():
            self.assertIn(
                path, corpus,
                f"\nNo doc cites {path} any more ({kind}: {why}). Drop the ALLOWED_MISSING entry.")

    def test_gate_sees_a_real_corpus(self):
        """Fail closed: a broken glob or regex would make every other check vacuously pass."""
        docs = tracked_markdown()
        self.assertGreater(len(docs), 20, "tracked markdown not found — is the git call working?")
        cited = {p for d in docs for p in TOKEN.findall(d.read_text())}
        self.assertGreater(len(cited), 40,
                           f"only {len(cited)} repo-relative paths matched across {len(docs)} "
                           f"docs — the TOKEN regex has probably stopped matching.")
        self.assertGreater(len(self.index), 200, "tracked file set looks empty")


if __name__ == "__main__":
    unittest.main(verbosity=2)
