"""Multi's persona, composed at seam startup — no rendered artifact.

The seam injects this via the `instructions` field EVERY turn (proven rule: a once-only
injection drifts; see multi/verify/test_injection2.py). This is the SAME composition proven
end-to-end in multi/verify/test_product_loop.py, imported by that test so proof and product
can't diverge.

Repo root defaults to <seam>/../.. (ironworks), overridable via PERSONA_ROOT.
There is no single resolved "Multi" persona file: the internal composition below is
ACCOUNT_INTELLIGENCE plus its two skills, and external clients get the client-generic
ANALYST composition with their own guidance (see compose_client_persona).
"""
import os
import re
from pathlib import Path

_ROOT = Path(os.environ.get("PERSONA_ROOT") or Path(__file__).resolve().parents[2])

# byte-for-byte the composition in test_product_loop.py — joined with "\n\n---\n\n"
_PARTS = (
    "agent/identity/ACCOUNT_INTELLIGENCE.md",
    "skills/company-knowledge/SKILL.md",
    "skills/account-intelligence/SKILL.md",
)

# Safety rides along with EVERY seam composition (tool-free mirror of _operational-tail.md's
# Safety section — the full tail's Computation/Files sections assume tools the channel-injected
# personas don't have). Appended LAST, matching the install-time tail position.
_SAFETY_TAIL = "agent/identity/_safety-tail.md"


def _read_part(base, rel):
    p = base / rel
    if not p.is_file():
        raise FileNotFoundError(f"persona part missing: {p} (set PERSONA_ROOT?)")
    return p.read_text()


def compose_persona(root=None):
    """Return the full Multi persona string. Raises FileNotFoundError with a clear path
    if a part is missing (fail loudly at startup, not silently mid-turn)."""
    base = Path(root) if root is not None else _ROOT
    parts = [_read_part(base, rel) for rel in _PARTS]
    parts.append(_read_part(base, _SAFETY_TAIL))
    return "\n\n---\n\n".join(parts)


# ── Client-specific composition ─────────────────────────────────────────────────────
# External clients get the client-GENERIC analyst parts + THEIR OWN business guidance.
# The internal composition above (MultiAgency's company knowledge) must never reach an
# external client: guidance is mandatory and FAILS CLOSED — no fallback to _PARTS.

_CLIENT_PARTS = (
    "agent/identity/ANALYST.md",
    "skills/account-analysis/SKILL.md",
)

# First line of every guidance file binds it to exactly one client slug.
_GUIDANCE_MARKER = re.compile(r"<!--\s*client-guidance\s+v1\s+slug:\s*([a-z0-9-]+)\s*-->")


class GuidanceError(RuntimeError):
    """A client's business guidance is missing, unreadable, or bound to another slug."""


def load_guidance(path, slug):
    """Read and validate one client's guidance file. Fail closed on: missing file,
    unreadable file, empty/trivial content, or a first-line slug that doesn't match —
    a client must never run with no guidance or with another client's guidance."""
    p = Path(path)
    try:
        text = p.read_text()
    except OSError as e:
        raise GuidanceError(f"client {slug!r}: guidance unreadable at {p}: {e}") from e
    m = _GUIDANCE_MARKER.match(text.lstrip())
    if not m:
        raise GuidanceError(f"client {slug!r}: {p} lacks the required first-line marker "
                            f"'<!-- client-guidance v1 slug: {slug} -->'")
    if m.group(1) != slug:
        raise GuidanceError(f"client {slug!r}: guidance at {p} is bound to slug "
                            f"{m.group(1)!r} — refusing to cross-wire guidance")
    # Strip HTML comments AFTER marker validation: the template's operator-instruction block
    # (and any authoring notes) must never become model-visible text the analyst can quote
    # back at the client. Length is checked on the stripped text — real guidance, not comments.
    stripped = re.sub(r"<!--.*?-->", "", text, flags=re.S).strip()
    if len(stripped) < 400:
        raise GuidanceError(f"client {slug!r}: guidance at {p} is too short to be real "
                            f"({len(stripped)} chars of content) — fill in the template")
    return stripped


def compose_client_persona(guidance_path, slug, root=None):
    """Return an EXTERNAL client's persona: the generic analyst parts + that client's
    validated guidance + the safety tail. Never includes MultiAgency's company knowledge."""
    base = Path(root) if root is not None else _ROOT
    parts = [_read_part(base, rel) for rel in _CLIENT_PARTS]
    # Section title is model-visible and the analyst is told to cite it, so it must read
    # naturally to the people it serves: "your organization", never operator vocabulary.
    # (The guidance FILE's first-line marker stays `client-guidance v1` — that is a machine
    # format validated fail-closed against files already on disk; renaming it is a separate,
    # coordinated change, not a prose edit.)
    parts.append("# ORGANIZATION GUIDANCE (scoped to this organization only)\n\n"
                 + load_guidance(guidance_path, slug))
    parts.append(_read_part(base, _SAFETY_TAIL))
    return "\n\n---\n\n".join(parts)


if __name__ == "__main__":
    s = compose_persona()
    print(f"composed persona: {len(s)} chars from {len(_PARTS)} parts, root={_ROOT}")
