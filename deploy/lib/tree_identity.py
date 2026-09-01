#!/usr/bin/env python3
"""One name for "which tree is this", computable with or without git.

WHY THIS EXISTS. `release.promotable` aggregates checks that no single environment can answer:
repository-hygiene gates need a git checkout, and the serve host is a file copy; the live legs
need a provisioned instance and a model gateway key, which CI has not. The host cannot answer
four and CI cannot answer six, so the verdict was unreachable everywhere and both environments
were told the release was bad.

Composing the two halves needs one thing first: proof that they are talking about the SAME code.
Without it, `--with-evidence` would happily combine CI's green repository gates with a host's
boundary proof taken from a different tree, which is worse than no composition at all — it would
manufacture a promotion decision that was never true of any single artifact.

TWO SOURCES FOR THE FILE LIST, ONE ALGORITHM FOR THE CONTENT. The fingerprint is sha256 over
`LC_ALL=C`-sorted "<path> <sha256-of-content>" lines, which is exactly the manifest shape
`deploy/sync-vm.sh` already writes. Only the FILE LIST differs by environment:

  * a git checkout   -> `git ls-files` (what ships)
  * a deployed copy  -> the paths named in DEPLOYED_MANIFEST.sha256 (what was pushed)

Both then re-hash the files ON DISK. That is deliberate: reading the manifest's digests directly
would fingerprint what sync-vm SAW at deploy time, so an edit made on the box afterwards would
be invisible — the artifact would attest to a tree that is no longer there. Re-hashing makes the
fingerprint self-measured in both environments and makes post-deploy drift change it.

Returns None rather than a guess when the tree cannot be identified. A caller must refuse to
compose evidence rather than compare against a fabricated key.
"""
import hashlib
import pathlib
import subprocess

MANIFEST = "DEPLOYED_MANIFEST.sha256"


def _digest(path):
    h = hashlib.sha256()
    with open(path, "rb") as fh:
        for chunk in iter(lambda: fh.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def _tracked_by_git(root):
    try:
        out = subprocess.run(["git", "ls-files", "-z"], cwd=root,
                             capture_output=True, check=True).stdout
    except (subprocess.CalledProcessError, FileNotFoundError, OSError):
        return None
    return [p for p in out.decode().split("\0") if p]


def _tracked_by_manifest(root):
    """The paths sync-vm recorded. The trailing `deployed <timestamp>` line is not a file and is
    dropped by the two-field shape test rather than by matching on the word, so a future line of
    a different shape cannot be silently read as a path."""
    manifest = root / MANIFEST
    if not manifest.is_file():
        return None
    paths = []
    for line in manifest.read_text().splitlines():
        parts = line.split()
        if len(parts) == 2 and len(parts[1]) == 64:
            paths.append(parts[0])
    return paths or None


def tree_fingerprint(root=None):
    """(fingerprint, source) — or (None, why) when the tree cannot be identified.

    `source` names WHICH list was used, because "git" and "manifest" answering the same is the
    interesting case and an operator comparing two artifacts should be able to see it.
    """
    root = pathlib.Path(root or pathlib.Path(__file__).resolve().parents[2])
    for source, lister in (("git", _tracked_by_git), ("manifest", _tracked_by_manifest)):
        paths = lister(root)
        if not paths:
            continue
        lines = []
        for rel in sorted(paths):                      # C collation: ASCII byte order
            f = root / rel
            if not f.is_file():
                # A path that is listed and absent changes what the tree IS. Refuse rather than
                # fingerprint a subset: a missing file is exactly the drift this must not hide.
                return None, f"{rel} is listed but missing, so this tree cannot be identified"
            lines.append(f"{rel} {_digest(f)}")
        h = hashlib.sha256("\n".join(lines).encode()).hexdigest()
        return h, source
    return None, ("no git checkout and no " + MANIFEST + " — this tree cannot be named, so "
                  "evidence from another environment cannot be matched to it")


if __name__ == "__main__":
    fp, why = tree_fingerprint()
    print(f"{fp}  ({why})" if fp else f"UNIDENTIFIED: {why}")
