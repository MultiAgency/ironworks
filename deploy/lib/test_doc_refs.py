"""Every repo-relative path named in the docs must be a path the reader can actually open.

The docs name files constantly, and nothing else reads markdown — so a rename or a removal
leaves citations pointing at nothing, and a reader who greps for a promised path cannot tell
whether it moved, was retired, or never existed.

WHAT IT CHECKS. Tokens in tracked markdown that look like a repo-relative path must resolve,
from the repo root or relative to the citing file. Four places a path can sit, and all four
count: backtick-quoted (`TOKEN`), inside a fenced block, mid-way through a code span
(`EMBEDDED` over `_regions`), and a markdown link target (`LINK`). The last two are where the RUNNABLE commands live — a proof index
is a fenced list of them — and covering only the first meant the commands a reader is most
likely to paste were the ones nobody verified.

Bare filenames (`doctor.sh`, and `./migrate.sh` with its shorthand prefix) are deliberately NOT
checked: the docs use them as shorthand for a path given in full elsewhere, so requiring
uniqueness would forbid the shorthand rather than catch a defect.

AND NOT ONLY MARKDOWN. Service JSON, compose files, wrangler config and systemd units name
paths too, and no code followed those names either — `docker-compose.yml` pointed at
`extension/README.md`, a directory that has never existed here. Those file types get their own
narrow checks rather than a widened `TOKEN`, because half of what they name is not
documentation at all but a path the runtime resolves: break a compose bind mount and the
container starts with no schema. The bare-filename exemption above is a PROSE rule and does
not apply there — docker resolves `./schema.sql` itself, so it is a real path, not shorthand.

RESOLVED AGAINST THE INDEX, NOT THE DISK. A clone contains the tracked set and nothing else, so
the tracked set is exactly what the docs may promise a reader. Using Path.exists() instead would
pass on an author's laptop — where a gitignored file happens to sit — and fail in CI.

WHAT INDEX-RESOLUTION THEREFORE CANNOT DETECT, stated because a gate whose blind spot is unwritten
gets read as covering more than it does: a path that is IN THE INDEX but MISSING FROM THE WORKING
TREE resolves clean here while nobody on this machine can open it. That is the three-states-of-a-
file problem (worktree / index / running) landing inside the gate meant to catch it. The check is
still correct about the clone — which is the promise the docs make — so the resolution stays, and
`test_tracked_markdown_is_on_disk` names the one case that would otherwise pass as a stack trace.
It covers tracked MARKDOWN only; a tracked script or JSON deleted from the worktree is invisible
here, and `git status` is what surfaces that.

Run:  python3 deploy/lib/test_doc_refs.py
"""
import json
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
    # Lives in the separate MultiAgency/vidgen repo; MULTIMEDIATOR.md tells the agent to fetch it
    # over the GitHub extension at runtime. The citation names where it lives THERE, which is why
    # it cannot resolve here and must not be "fixed" into a local path.
    "skills/vidgen-contributor/SKILL.md": ("absent", "vidgen repo, read over GitHub at runtime"),
    # Upstream nearai/ironclaw's own tree, cited by UPGRADE.md so a bump can be checked against
    # the source. Never vendored here — this repo runs the official binary unmodified.
    "docker/reborn/entrypoint.sh": ("absent", "upstream ironclaw path, deliberately not vendored"),
    # Two gitignored operator scripts (.gitignore:55 and the deploy/hq/ rule's neighbours) that
    # CONTRIBUTING.md's "Sourcing an env file" section must name, because they are the worked
    # examples on both sides of that rule: the laptop backup script is a load-bearing `set -a`
    # (restic reads RESTIC_* from its environment) and repoint-hostname.sh is one of the two
    # comments arguing for plain source. The "untracked" kind is doing real work on these — they
    # carry operator host details, so the entry failing the day either becomes TRACKED is the
    # point, not a side effect.
    "deploy/backup-laptop-agency.sh": ("untracked", "gitignored operator script; cited as the restic set -a example"),
    "deploy/repoint-hostname.sh": ("untracked", "gitignored operator script; cited as a plain-source example"),
}
# Extensions worth checking. Deliberately excludes .rs and .toml: markdown here cites upstream
# Rust as a SOURCE citation, naming a file in the ironclaw checkout at IRONCLAW_PIN rather than
# promising a path in this tree. `multi/README.md` names
# `crates/substrates/ironclaw_network/src/policy.rs`; with .rs in the list that resolves as a
# dangling repo path. Note it is the full upstream PATH that needs the exclusion — a bare
# `channel_pairing.rs` is already out of scope by the same rule that ignores `doctor.sh`.
# Nothing cites a .toml today; it is excluded ahead of the first Cargo.toml citation, not
# because of one.
EXTS = ("md", "py", "sh", "js", "json", "yml", "yaml", "sql", "service", "timer", "wit", "txt")

# Two shapes, and the second is not optional. A file citation ends in a known extension; a
# DIRECTORY citation ends in `/` (`multi/verify/`, `deploy/egress/`). An extension-only regex
# misses every directory — which is most of what a removed subtree leaves behind, and therefore
# most of the incident this gate was written for. Mutation-testing caught that: deleting all of
# `multi/verify/fixtures/` left the file-only version completely silent.
#
# WHERE THE CITATION ENDS is the other half, and requiring the closing backtick got it wrong.
# The docs cite a path WITH ITS ARGUMENTS constantly — `data/migrate.sh apply`,
# `multi/provision/deprovision.sh <slug> --execute`, `./deploy/ironworks release verify` — and
# every one of those was silently exempt, because the backtick does not follow the extension.
# That is not a narrower check, it is an inverted one: the citations most likely to be followed
# by a reader (the runnable ones) were the only ones nobody verified, and one of them
# (`data/migrate.sh apply`, the documented bring-up step) had been pointing at an untracked file.
# So the terminator is a LOOKAHEAD at a backtick or whitespace: the path still has to start at
# the opening backtick — `bash deploy/foo.sh` is a command, not a citation — but what follows it
# inside the span no longer decides whether the path is checked.
TOKEN = re.compile(
    r"`([A-Za-z0-9_./-]+/[A-Za-z0-9_./-]+\.(?:" + "|".join(EXTS) + r")"     # a file
    r"|[A-Za-z0-9_.-]+(?:/[A-Za-z0-9_.-]+)*/)(?=`|\s)")                     # a directory


# THE OTHER HALF OF "WHERE THE CITATION ENDS": where it BEGINS. `TOKEN` anchors at the opening
# backtick, so it sees a path only when the path is the first thing in the span. Two whole
# regions of every document are therefore invisible to it, and they are where the runnable
# commands live:
#
#   FENCED BLOCKS. Inside ```sh … ``` nothing is backticked, so `python3
#   multi/verify/test_adversarial_routing.py` matches NOTHING. `multi/verify/README.md` is the
#   file `README.md` calls the runnable proof index, and all seven of its offline commands were
#   unchecked — a proof index reporting clean on paths it never inspected. Mutation-tested:
#   renaming one of those commands to a file that does not exist left the whole suite green.
#
#   MID-SPAN. `bash deploy/foo.sh`, `cd deploy/account-intel/data && ./migrate.sh status` — the
#   path is inside the span but not at its start, so the anchor misses it.
#
# FILE CITATIONS ONLY here, deliberately, unlike `TOKEN`. A directory citation is a bare word
# followed by `/`, and free text is full of those; requiring the backtick anchor is what keeps
# the directory rule from firing on prose. Files carry a known extension, which is enough
# signal to read them as citations wherever they appear.
EMBEDDED = re.compile(
    r"(?<![A-Za-z0-9_./~-])"                                   # not mid-path — and not `~/…`,
    r"((?:\.{1,2}/)*"                                          # which is a home path, not a repo one
    r"[A-Za-z0-9_.-]+(?:/[A-Za-z0-9_.-]+)+"
    r"\.(?:" + "|".join(EXTS) + r"))"
    r"(?![A-Za-z0-9_-])")
# A fence opens with ``` or ~~~ (3+), may be indented, and closes on the same marker. Group 3 is
# the body. Non-greedy, so consecutive fences do not merge into one region.
FENCE = re.compile(r"^([ \t]*)(`{3,}|~{3,})[^\n]*\n(.*?)^\1?\2[ \t]*$", re.M | re.S)
SPAN = re.compile(r"`([^`\n]+)`")


# THE FOURTH PLACE, and the one a reader is most likely to FOLLOW: a markdown link target.
# `TOKEN` needs the path to start at an opening backtick and `EMBEDDED` only runs over fenced
# and code-span regions, so `[prose text](docs/thing.md)` was scanned by neither. The doc map in
# `README.md` survived that only by accident — it writes the path as the link TEXT too
# (`[`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md)`), which `TOKEN` catches. Write the same link
# with prose for text and the target went unchecked: measured at 21 such targets across 9 docs,
# and deleting a link-only-cited file from the index produced ZERO findings while deleting a
# backticked one produced four.
#
# The bare-filename exemption deliberately does NOT apply here, for the reason the module
# docstring already gives for config files: a renderer RESOLVES a link target, so `SECURITY.md`
# in `](...)` is a real relative path a reader clicks, not prose shorthand for a path given in
# full elsewhere. `#fragment` is stripped; external schemes are not repo paths.
#
# An ABSOLUTE target is excluded on the same principle `EMBEDDED` already applies to `~/…`:
# a leading `/` is not repo-relative. This is not a courtesy exclusion — the first run of this
# class found `agent/identity/_operational-tail.md` linking `[report.csv](/workspace/report.csv)`,
# which is the CONTAINER workspace path the tail instructs the model to emit so the interface
# renders a download link (and which the tail tells it NOT to backtick, which is exactly why the
# older rules never saw it). Treating that as a repo citation would be the gate reporting a
# defect the docs do not have.
LINK = re.compile(r"\]\(([^)\s]+)\)")
_LINK_SKIP = ("http:", "https:", "mailto:", "#", "/")


def _link_targets(text):
    for raw in LINK.findall(text):
        target = raw.split("#", 1)[0]
        if target and not target.startswith(_LINK_SKIP):
            yield target


def _regions(text):
    """Every stretch of a doc where a path may appear without abutting a backtick."""
    for m in FENCE.finditer(text):
        yield m.group(3)
    for m in SPAN.finditer(text):
        yield m.group(1)


def _is_bare_shorthand(path):
    """`./migrate.sh` is the bare filename the module docstring says is deliberately unchecked.

    Stripping the leading `./` and `../` first, because a command in a fence writes the
    shorthand with a `./` on the front and that is still shorthand, not a repo-relative path."""
    return "/" not in re.sub(r"^(?:\.{1,2}/)+", "", path)


# NON-MARKDOWN FILES CITE PATHS TOO, and until this existed nothing followed those names. The
# module docstring says this gate covers markdown; that is a scope statement, not a claim the
# class is safe. `deploy/account-intel/data/docker-compose.yml` carried a comment pointing at
# `extension/README.md` — a directory that has never existed in this tree — and every check here
# was green the whole time.
#
# Two KINDS live in these files, and the second is why this is not merely tidiness:
#   - a comment citing a document, exactly like markdown prose does;
#   - a LIVE PATH the runtime reads: a compose bind mount (`./schema.sql` is the Account Store's
#     initdb script, `../connect-proxy.py` is the egress proxy the proof stack runs), or a
#     wrangler bundle input (`../PERSONA.md`). A rename there does not rot a doc — it starts a
#     container with no schema, or no proxy, and the failure surfaces at runtime on a VM.
#
# THE BARE-FILENAME EXEMPTION DOES NOT APPLY HERE. In prose, `doctor.sh` is shorthand for a path
# given in full elsewhere. In config, `./schema.sql` and `../connect-proxy.py` are resolved
# relative to the file by docker and wrangler themselves — they are real paths, not shorthand,
# so an explicit `./` or `../` prefix makes a citation checkable even with no directory in it.
# Reusing the markdown rule here skipped both live mounts on the first pass.
#
# Scoped to the file types that actually carry citations, following
# `test_service_definitions_cite_paths_this_repo_ships` rather than widening `TOKEN` to every
# extension: a general-purpose scanner over arbitrary files is a different gate with a different
# false-positive profile, and this repo's experience is that a noisy gate gets switched off.
CONFIG_GLOBS = ("*/docker-compose*.yml", "docker-compose*.yml", "*/wrangler*.jsonc",
                "*.service", "*.timer")
CONFIG_PATH = re.compile(
    r"(?<![A-Za-z0-9_./~-])("
    r"(?:\.{1,2}/)+[A-Za-z0-9_.-]+(?:/[A-Za-z0-9_.-]+)*"          # explicitly relative
    r"\.(?:" + "|".join(EXTS) + r")"
    r"|[A-Za-z0-9_.-]+(?:/[A-Za-z0-9_.-]+)+\.(?:" + "|".join(EXTS) + r")"   # or has a directory
    r")(?![A-Za-z0-9_-])")


def config_citations(index):
    """[(citing file, cited path, resolved index path or None)] across the config file types.

    Resolved against the INDEX for the same reason the markdown checks are: a clone is what the
    repo promises. The worktree half is `git status`'s job here, as the module docstring says of
    every non-markdown path."""
    out = []
    for pattern in CONFIG_GLOBS:
        for f in sorted(set(_git("ls-files", pattern))):
            here = os.path.dirname(f) or "."
            text = (REPO / f).read_text(errors="ignore")
            for raw in sorted(set(CONFIG_PATH.findall(text))):
                path = raw.split(":", 1)[0]         # `src.py:/dst.py:ro` mount syntax, and :123
                if path in ALLOWED_MISSING:
                    continue
                from_root = os.path.normpath(path)
                from_here = os.path.normpath(f"{here}/{path}") if here != "." else from_root
                out.append((f, path, next((c for c in (from_root, from_here) if c in index), None)))
    return out


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


def _read(doc):
    """A tracked doc missing from the worktree reads as empty here, deliberately.

    Raising instead buries `test_tracked_markdown_is_on_disk` — the check that names the
    condition and says why nothing else caught it — under three FileNotFoundError tracebacks
    from the checks that merely happened to read the file first. With nothing missing this
    changes nothing; with something missing, exactly one failure explains it."""
    try:
        return doc.read_text()
    except FileNotFoundError:
        return ""


def _checkable(path, roots):
    """Is this citation a claim about THIS repo's tree, or shorthand for something else?

    Same principle the file rule uses when it ignores a bare `doctor.sh`. A directory citation
    qualifies only if it is rooted at a real top-level directory of this repo — so
    `deploy/egress/` is checked, while `data/` (shorthand inside a section about one subtree)
    and `src/scenes/` (a path in the separate vidgen repo) are not. Without it the gate fires on
    prose it was never meant to police, and a noisy gate gets switched off.
    """
    if not path.endswith("/"):
        return True
    return path.split("/", 1)[0] in roots


def citations(index):
    """[(citing doc, cited path, resolved index path or None)] for every checkable citation.

    One walk, because the two questions a citation raises — does git track it, and can a reader
    open it — differ only in what they do with the answer, and two walks would drift on the
    normalization rules below."""
    roots = {p.split("/", 1)[0] for p in index if "/" in p}
    out = []
    for doc in tracked_markdown():
        text = _read(doc)
        here = doc.parent.relative_to(REPO).as_posix()
        # Backtick-anchored first, then the fenced and mid-span regions `TOKEN` cannot reach.
        # One pass, one dedup: a path cited both ways is one citation, and reporting it twice
        # would make a single rename read as two defects.
        anchored = sorted(set(TOKEN.findall(text)))
        embedded = sorted({p for body in _regions(text) for p in EMBEDDED.findall(body)
                           if not _is_bare_shorthand(p)})
        linked = sorted(set(_link_targets(text)))
        seen = set()
        for raw in anchored + embedded + linked:
            path = raw.split(":", 1)[0]                       # tolerate `file.py:123` citations
            if path in seen:
                continue
            seen.add(path)
            if path in ALLOWED_MISSING or not _checkable(path, roots):
                continue
            # Docs cite both ways: from the repo root (sometimes `./`-prefixed) and relative to
            # the citing file (sometimes with `../`). Normalize BOTH forms rather than
            # concatenating, or every upward or dot-prefixed citation reads as dangling.
            from_root = os.path.normpath(path)
            from_here = os.path.normpath(f"{here}/{path}") if here != "." else from_root
            resolved = next((c for c in (from_root, from_here) if c in index), None)
            out.append((doc.relative_to(REPO).as_posix(), path, resolved))
    return out


def dangling(index):
    """[(citing doc, cited path)] for every repo-relative citation git does not track."""
    return [(d, p) for d, p, resolved in citations(index) if resolved is None]


# A SOURCED SHELL LIBRARY IS A CITATION THAT EXECUTES. `. "$REPO_DIR/deploy/lib/fleet.sh"` names
# a file the same way prose does, except a stale one does not mislead a reader — it kills the
# script on its first line, on a VM, at runtime. Twenty-seven of these lines across the tree all
# point at five shared libraries, so renaming one breaks about fourteen scripts at once. That is
# the `deploy/lib` consolidation's exact shape, and `CONTRIBUTING.md` § "Retiring something"
# currently handles it by telling contributors to grep.
#
# MATCHED ON BASENAME, and that is the design, not a shortcut. The path half is almost always
# behind a variable — `$REPO_DIR`, `$REPO`, `$LIB_DIR`, `$_CF_LIB_DIR`, `$_FLEET_LIB_DIR` — each
# anchored at a different directory. Resolving those needs either a shell parser or a
# hand-maintained map of variable to root, and a map like that goes stale silently and reports
# defects that are not there. Measured before choosing: a resolver assuming repo-root anchors
# reported two `deploy/doctor.sh` lines as broken purely because `$LIB_DIR` is `deploy/lib`.
#
# So this asks the narrower question that needs no map, borrowing the uniqueness rule
# `symbol_citations` already uses: the basename must belong to exactly one tracked file.
# Basenames are unique in this tree. WHAT THAT COSTS, stated because an unwritten limit gets
# read as coverage: it catches a RENAME or a DELETION, not a MOVE — relocating `fleet.sh` with
# its name intact leaves every `source` line broken and this check silent. `bash -n` and
# shellcheck do not resolve `source` either, so nothing else would say so.
#
# Extension-filtered to EXTS, which is what keeps `$FLEET_AGENCY_DIR/watchdog.env` out with no
# special case: `.env` files are gitignored by this repo's own secrets rule, so they are runtime
# state and never a repo path. A variable-name exclusion list would have been the alternative.
SOURCED = re.compile(r"^\s*(?:\.|source)\s+(\S+)")


def sourced_libraries():
    """[(script, line, raw target, basename)] for every STATIC `source`/`.` line in tracked shell.

    Dynamic targets — `$(dirname "$0")/x.sh`, a bare `$CFG` — are skipped rather than guessed
    at: what they resolve to is decided at runtime, and a gate that pretends otherwise reports
    on a path nobody wrote."""
    out = []
    for f in _git("ls-files", "*.sh"):
        for n, line in enumerate((REPO / f).read_text(errors="ignore").splitlines(), 1):
            m = SOURCED.match(line)
            if not m:
                continue
            base = pathlib.PurePosixPath(m.group(1).strip("\"'")).name
            if "$" in base or "~" in base or base.rsplit(".", 1)[-1] not in EXTS:
                continue
            out.append((f, n, m.group(1), base))
    return out


# A citation to a SYMBOL rather than a path: `context_ingress.assert_no_member_is_the_operator`,
# `registry.py::load_clients`. The path half of such a citation resolves as long as the FILE
# exists, so a symbol that moved between modules leaves the whole citation reading clean — which
# is the one shape of doc rot a rename cannot make loud. Splitting `context_ingress.py` into
# `registry.py`/`envelope.py`/`responses.py` moved `load_clients` and `ClientConfig` out; every
# citation to them still resolved, and only a hand-check found the ones that needed rewriting.
#
# Extensions are excluded because `foo.py` is a filename, not `foo` dot a symbol named `py`.
SYMBOL = re.compile(r"`([A-Za-z_][A-Za-z0-9_]*)(?:\.py)?(?:::|\.)([A-Za-z_][A-Za-z0-9_]*)`")
_NOT_SYMBOLS = set(EXTS) | {"template", "example"}


def _blob(path):
    """Index content for a tracked path, matching this gate's resolve-against-the-index rule.

    Reading the worktree instead would pass on an author's laptop mid-refactor — exactly the
    state in which a symbol citation is most likely to be wrong — and fail in CI.
    """
    return subprocess.run(["git", "-C", str(REPO), "show", f":{path}"],
                          capture_output=True, text=True, check=True).stdout


def symbol_citations(index):
    """[(citing doc, module, symbol, ok)] for every `module.symbol` naming a tracked module.

    A citation is checkable only when its module half is the basename of exactly one tracked
    `.py` — which is what makes `sys.path`, `builtin.http` and `ironclaw.rev` fall out rather
    than needing an allowlist. Basenames are unique in this tree; if that ever stops being true
    the ambiguous name is skipped rather than guessed at.
    """
    modules = {}
    for p in _git("ls-files", "*.py"):
        modules.setdefault(pathlib.PurePosixPath(p).stem, []).append(p)
    out = []
    for doc in tracked_markdown():
        for mod, sym in sorted(set(SYMBOL.findall(_read(doc)))):
            if sym in _NOT_SYMBOLS or len(modules.get(mod, ())) != 1:
                continue
            body = _blob(modules[mod][0])
            ok = bool(re.search(rf"^\s*(?:async\s+)?(?:def|class)\s+{re.escape(sym)}\b", body, re.M)
                      or re.search(rf"^{re.escape(sym)}\s*[:=]", body, re.M))
            out.append((doc.relative_to(REPO).as_posix(), mod, sym, ok))
    return out


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

    def test_link_targets_are_checked_not_just_backticked_paths(self):
        """A markdown link target is a citation a reader FOLLOWS, so it must resolve.

        Regression test for a real blind spot: `TOKEN` anchors at an opening backtick and
        `EMBEDDED` runs only over fenced/code-span regions, so `[prose](docs/thing.md)` was
        scanned by neither.

        It asserts the walk REACHES a path cited ONLY as a link. An earlier draft picked any
        citation resolving to a link-cited file, which `TOKEN` also finds — so deleting the
        link class from the walk left the test green. Mutating the guard is what caught that;
        the victim has to be link-ONLY or this proves nothing."""
        index = tracked()
        link_only = []
        for doc in tracked_markdown():
            text = _read(doc)
            others = set(TOKEN.findall(text)) | {p for body in _regions(text)
                                                 for p in EMBEDDED.findall(body)}
            rel = doc.relative_to(REPO).as_posix()
            link_only += [(rel, p) for p in _link_targets(text)
                          if p not in others and _checkable(p, {q.split("/", 1)[0]
                                                                for q in index if "/" in q})]
        self.assertTrue(link_only,
                        "no path is cited ONLY as a markdown link — this test cannot prove the "
                        "walk reaches links; drop it or point it at a doc that has one.")
        found = {(d, c) for d, c, _ in citations(index)}
        missed = [pair for pair in link_only if pair not in found]
        self.assertEqual(
            missed, [],
            "\nThese paths are cited only as markdown link targets and the walk never saw "
            "them:\n" + "".join(f"  {d}  ->  {p}\n" for d, p in missed))

    def test_cited_symbols_exist_in_the_module_named(self):
        """A symbol citation whose module no longer defines it.

        `test_no_dangling_paths` asks only whether the FILE exists, so
        `context_ingress.load_clients` kept resolving after `load_clients` moved to
        `registry.py`. The reader who greps the module named finds nothing, and no gate says so.

        Deliberately narrow, and the corpus is SMALL — one citation at the time this was
        written. That is not a reason to skip it: the cost is a regex, and the defect it catches
        is invisible by construction and survives exactly the refactor most likely to cause it.
        It is not asserted non-empty, unlike `test_gate_sees_a_real_corpus`, because at this size
        an editor legitimately deleting the last symbol citation must not fail the build."""
        bad = [(d, m, s) for d, m, s, ok in symbol_citations(self.index) if not ok]
        self.assertEqual(
            bad, [],
            "\nDocs name symbols the module they cite does not define:\n"
            + "".join(f"  {d}  ->  {m}.{s}\n" for d, m, s in bad)
            + "The file resolves, so no path check would catch this. Point the citation at the "
              "module that defines it now, or drop the module half if it moved for good.")

    def test_service_definitions_cite_paths_this_repo_ships(self):
        """Service JSON names files, and nothing followed those names.

        `multi/services/*.json` carries `tool_policy`, `evaluation` and `data_schema`, each a
        repo-relative path — and no code reads any of the three (`services._REQUIRED_KEYS` does
        not include them). So they are documentation embedded in machine-loaded config, with
        none of the gating either half normally gets: this suite swept `*.md` only, so renaming
        `confine-member.sh` or `schema.sql` left both definitions pointing at nothing, silently.

        Gate them as the citations they are, rather than deleting them. They record which tool
        policy and which schema a service is built against, which is worth keeping — it just has
        to be true."""
        bad = []
        for defn in sorted((REPO / "multi" / "services").glob("*.json")):
            doc = json.loads(defn.read_text())
            for key in ("tool_policy", "evaluation", "data_schema"):
                cited = doc.get(key)
                if cited and cited not in self.index:
                    bad.append((defn.relative_to(REPO), key, cited))
        self.assertEqual(
            bad, [],
            "\nService definitions name paths this repo does not ship:\n"
            + "".join(f"  {d}  {k}  ->  {p}\n" for d, k, p in bad)
            + "These keys are read by no code, so nothing else would have caught the rename.")

    def test_config_files_cite_paths_this_repo_ships(self):
        """Compose files, wrangler config and unit files name paths, and nothing followed them.

        The sibling of `test_service_definitions_cite_paths_this_repo_ships`, for the file types
        that carry citations without being service JSON. Half of what it catches is documentation
        embedded in machine-read config — `docker-compose.yml` pointed a comment at
        `extension/README.md`, which has never existed here, with every other check green. The
        other half is not documentation at all: a compose bind mount and a wrangler bundle input
        are paths the runtime resolves, so a rename produces a container with no schema or no
        proxy rather than a stale sentence."""
        bad = [(f, p) for f, p, resolved in config_citations(self.index) if resolved is None]
        self.assertEqual(
            bad, [],
            "\nConfig files name paths this repo does not ship:\n"
            + "".join(f"  {f}  ->  {p}\n" for f, p in bad)
            + "This gate covers markdown; these files are read by docker, wrangler and systemd, "
              "so nothing else here would have caught it. Fix the path, or add it to "
              "ALLOWED_MISSING with its kind and reason.")

    def test_sourced_shell_libraries_are_tracked(self):
        """A `source`d library that stopped existing, which fails at RUNTIME and nowhere else.

        `bash -n` does not resolve `source`, and neither does shellcheck, so a renamed shared
        library leaves every script that sources it parsing perfectly and dying on line one.
        Twenty-seven of these lines point at five libraries; one rename breaks fourteen scripts.

        Basename-matched, so it catches a rename or a deletion and NOT a move — see the comment
        on `SOURCED` for why resolving the variable half would cost more correctness than it
        buys."""
        by_base = {}
        for path in _git("ls-files"):
            by_base.setdefault(pathlib.PurePosixPath(path).name, []).append(path)
        bad = [(f, n, raw) for f, n, raw, base in sourced_libraries() if not by_base.get(base)]
        self.assertEqual(
            bad, [],
            "\nShell scripts source libraries this repo does not ship:\n"
            + "".join(f"  {f}:{n}  ->  {raw}\n" for f, n, raw in bad)
            + "Nothing else can see this: bash -n and shellcheck do not resolve `source`, so "
              "the script parses clean and fails on its first line at runtime.")

    def test_allowlist_entries_are_still_needed(self):
        """An allowlist entry that outlives its reason silently stops gating that path."""
        for path, (kind, why) in ALLOWED_MISSING.items():
            self.assertNotIn(
                path, self.index,
                f"\n{path} is now TRACKED but still in ALLOWED_MISSING ({kind}: {why}). "
                f"Remove the entry so the real file is gated like every other path.")

    def test_allowlist_entries_are_still_cited(self):
        """And one nothing cites is dead weight — the prose that needed it is gone."""
        corpus = "\n".join(_read(d) for d in tracked_markdown())
        for path, (kind, why) in ALLOWED_MISSING.items():
            self.assertIn(
                path, corpus,
                f"\nNo doc cites {path} any more ({kind}: {why}). Drop the ALLOWED_MISSING entry.")

    def test_tracked_markdown_is_on_disk(self):
        """Index-resolution's blind spot, made loud for the case this gate can see.

        A doc staged but deleted from the worktree still satisfies every citation to it, because
        `dangling()` asks the index. Reading it then dies with a FileNotFoundError traceback that
        names a path and no reason. Fail with the reason instead."""
        missing = [p for p in _git("ls-files", "*.md") if not (REPO / p).exists()]
        self.assertEqual(
            missing, [],
            "\nTracked markdown is missing from the working tree:\n"
            + "".join(f"  {p}\n" for p in missing)
            + "Every citation to it still resolves — this gate asks the index, not the disk — so "
              "nothing else would have told you. Restore the file, or stage its deletion.")

    def test_every_cited_path_is_on_disk(self):
        """The same blind spot as `test_tracked_markdown_is_on_disk`, for what a doc CITES.

        That check sweeps tracked `*.md`, which is how a staged-but-deleted document gets
        caught. It says nothing about the scripts, modules and config files the docs name far
        more often: those resolve against the index too, so a citation to a path that is staged
        and deleted from the worktree passes every other check here while a reader following it
        finds nothing. This gate promises "a path the reader can actually open", and
        index-resolution alone does not deliver that half of it.

        Scoped to CITED paths rather than every tracked file on purpose. Whether the worktree
        matches the index in general is `git status`'s job, and restating it here would make
        this gate fire on work in progress it has no business policing."""
        missing = sorted({(d, r) for d, _p, r in citations(self.index)
                          if r and not (REPO / r).exists()})
        self.assertEqual(
            missing, [],
            "\nDocs cite tracked paths that are missing from the working tree:\n"
            + "".join(f"  {d}  ->  {r}\n" for d, r in missing)
            + "Each citation still resolves — this gate asks the index, not the disk — so "
              "nothing else would have told you. Restore the file, or stage its deletion.")

    def test_cited_sections_exist(self):
        """A `file.md § Heading` citation must name a heading that file actually has.

        The path half of these references is already gated; the section half was not, and it is
        the half that rots faster — renaming a heading is an ordinary edit, and nothing outside
        the citing document knows it happened. Every one of these references in the tree pointed
        at a heading that no longer existed when this check was written, across four documents,
        with every path resolving and every other check green.

        Where the section ENDS is the whole difficulty, because a citation runs straight on into
        the sentence around it. Three delimiters, in order: a backticked heading (`` § `ironclaw-1` ``)
        ends at its closing backtick, a quoted one ends at its closing quote, and a bare one ends
        at the first comma, period, semicolon or bracket. Reading past that swallows "and the
        network boundary is…" and reports a false break — which the first draft of this check did.

        Matching is containment either way rather than equality: a citation legitimately trims a
        long heading or trails off mid-clause, so demanding an exact string would fire on prose
        this has no business policing.

        Resolved against the WORKTREE, matching `test_every_cited_path_is_on_disk`: the question
        is whether a reader following the pointer arrives somewhere, and a reader has the
        worktree."""
        heading = re.compile(r"^#{1,6} +(.+?)\s*$", re.M)
        ref = re.compile(r"`([A-Za-z0-9_./-]+\.md)`\s*§\s*"
                         r"(?:`([^`\n]+)`|\"([^\"\n]+)\"|([^\n,.;:)]+))")
        bad = []
        for doc in tracked_markdown():
            here = doc.parent
            for target, tick, quoted, bare in ref.findall(_read(doc)):
                path = REPO / target if (REPO / target).exists() else here / target
                if not path.exists():
                    continue                      # the path gates above own that failure
                section = tick or quoted or bare
                want = section.replace("`", "").strip().rstrip(".,;:—-").strip().lower()
                if not want:
                    continue
                have = [h.replace("`", "").strip().lower()
                        for h in heading.findall(path.read_text(errors="ignore"))]
                if not any(want in h or h in want for h in have):
                    bad.append((doc.relative_to(REPO).as_posix(), target, section.strip()))
        self.assertEqual(
            sorted(bad), [],
            "\nDocs cite sections that do not exist in the file they name:\n"
            + "".join(f"  {d}  ->  {t} § {s}\n" for d, t, s in sorted(bad))
            + "The path resolves, so no other check here can see this. Repoint the reference at "
              "a heading the file actually has, or drop the section anchor.")

    def test_gate_sees_a_real_corpus(self):
        """Fail closed: a broken glob or regex would make every other check vacuously pass."""
        docs = tracked_markdown()
        self.assertGreater(len(docs), 20, "tracked markdown not found — is the git call working?")
        cited = {p for d in docs for p in TOKEN.findall(_read(d))}
        self.assertGreater(len(cited), 40,
                           f"only {len(cited)} repo-relative paths matched across {len(docs)} "
                           f"docs — the TOKEN regex has probably stopped matching.")
        # The same fail-closed rule for the regions TOKEN cannot reach — and the two regions are
        # counted SEPARATELY, which is not a stylistic choice. Code spans yield ~101 paths and
        # fenced blocks ~22, so one combined threshold is met by the spans alone: the first
        # version of this check passed with the FENCE regex deliberately broken, leaving every
        # fenced command — this repo's whole runnable proof index — silently unchecked while
        # claiming to cover it. That is the exact defect this gate exists to catch, committed
        # inside the gate. Each region asserts its own floor.
        fenced = {p for d in docs for m in FENCE.finditer(_read(d))
                  for p in EMBEDDED.findall(m.group(3)) if not _is_bare_shorthand(p)}
        self.assertGreater(len(fenced), 15,
                           f"only {len(fenced)} paths matched inside FENCED BLOCKS — the FENCE "
                           f"or EMBEDDED regex has stopped matching. The proof index is fenced.")
        spanned = {p for d in docs for m in SPAN.finditer(_read(d))
                   for p in EMBEDDED.findall(m.group(1)) if not _is_bare_shorthand(p)}
        self.assertGreater(len(spanned), 60,
                           f"only {len(spanned)} paths matched inside CODE SPANS — the SPAN or "
                           f"EMBEDDED regex has stopped matching.")
        # And the link corpus — TWICE, by shape, for the same reason the two regions above are
        # counted apart. 36 targets checkable today, but 25 of them carry a directory segment: a
        # `LINK` narrowed to those alone still clears any total-based floor while dropping every
        # bare `SECURITY.md`-style target. Verified, not assumed — that exact mutation passed a
        # single floor of 20. The bare subset is also the shape ONLY this class reaches, since
        # `_is_bare_shorthand` deliberately exempts it everywhere else, so it is the half whose
        # loss no other check would report.
        linked = {p for d in docs for p in _link_targets(_read(d))
                  if p.endswith(EXTS) or p.endswith("/")}
        self.assertGreater(len(linked), 20,
                           f"only {len(linked)} paths matched inside MARKDOWN LINK TARGETS — the "
                           f"LINK regex has stopped matching. The doc map is written as links.")
        bare_linked = {p for p in linked if _is_bare_shorthand(p)}
        self.assertGreater(len(bare_linked), 6,
                           f"only {len(bare_linked)} BARE link targets matched — LINK has "
                           f"narrowed to paths with a directory segment, dropping the "
                           f"sibling-doc links (`SECURITY.md`, `UPGRADE.md`) no other rule sees.")
        # And the config corpus, for the same reason and with the same mistake available: a glob
        # that stops matching, or a CONFIG_PATH that does, makes the check above pass on an empty
        # set. Counted on its own, never folded in with the markdown totals.
        config = config_citations(self.index)
        self.assertGreater(len(config), 6,
                           f"only {len(config)} paths matched across the config file types — a "
                           f"CONFIG_GLOBS entry or CONFIG_PATH has stopped matching.")
        # And the sourced-library corpus, counted on its own for the third time and the same
        # reason: a SOURCED regex that stops matching makes its check pass on an empty set.
        sourced = sourced_libraries()
        self.assertGreater(len(sourced), 20,
                           f"only {len(sourced)} static `source` lines matched across tracked "
                           f"shell — the SOURCED regex has stopped matching.")
        self.assertGreater(len(self.index), 200, "tracked file set looks empty")


if __name__ == "__main__":
    unittest.main(verbosity=2)
