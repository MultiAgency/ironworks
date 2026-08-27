"""Multi's persona, composed at seam startup — no rendered artifact.

The seam injects this via the `instructions` field EVERY turn (proven rule: a once-only
injection drifts; see multi/verify/test_injection2.py). This is the SAME composition proven
end-to-end in multi/verify/test_product_loop.py, imported by that test so proof and product
can't diverge.

Repo root defaults to <seam>/../.. (ironworks), overridable via PERSONA_ROOT.

There is no single resolved "Multi" persona file, and no longer a hard-coded pair of
compositions either. WHICH parts compose a tenant's persona is a SERVICE DEFINITION
(`multi/services/*.json`, loaded by services.py); this module knows only how to assemble
whatever a definition names, plus that tenant's mandatory slug-and-service-bound guidance,
plus the safety tail. That is what lets MultiAgency run its own book as a tenant on the same
path a client uses, rather than through a private branch.
"""
import hashlib
import re
from pathlib import Path

try:
    from . import services
    from .resources import resource_root
except ImportError:  # direct-script compatibility during service-unit rollout
    import services
    from resources import resource_root

_ROOT = resource_root()

# The internal composition's PARTS are no longer written here: they are the persona_parts of
# the `relationship-intelligence` service definition, so the composition MultiAgency runs as a
# tenant and the one this dev oracle builds cannot drift apart.
_INTERNAL_SERVICE = "relationship-intelligence"

# Safety rides along with EVERY seam composition (tool-free mirror of _operational-tail.md's
# Safety section — the full tail's Computation/Files sections assume tools the channel-injected
# personas don't have). Appended LAST, matching the install-time tail position.
#
# WHICH file that is belongs to the SERVICE DEFINITION (`safety_tail` in multi/services/*.json),
# not to a constant here: services.load_service validates the key and that the file is on disk,
# and both composition paths read it from there. There is no second place to change it.


def _strip_frontmatter(text):
    """Drop a leading YAML frontmatter block. Every part is injected VERBATIM as
    `instructions`, so anything left here is prompt.

    The skill parts (`skills/*/SKILL.md`) carry frontmatter; the persona parts do not. That
    frontmatter is build metadata for humans — `name`, `version`, a `description` and an
    `activation.keywords` list nothing in this repo reads. Injected, it is a context pointer
    with nothing to point at: the body it would gate is already loaded, unconditionally, every
    turn. So it pays full prompt cost for no work, and drops a run of bare keywords into the
    model's instructions attached to no sentence.

    Stripped HERE rather than deleted from the files, so a SKILL.md stays a legible standalone
    artifact and `version:` keeps meaning something to an operator reading it.

    Conservative by construction: strips only when line 1 is exactly `---` AND a closing `---`
    line exists. An unterminated block is returned untouched — a malformed header is worth
    seeing in the prompt, where it is obvious, rather than silently eating a whole part."""
    if not text.startswith("---\n"):
        return text
    end = text.find("\n---\n", 3)
    if end == -1:
        return text
    return text[end + 5:].lstrip("\n")


def _read_part(base, rel):
    p = base / rel
    if not p.is_file():
        raise FileNotFoundError(f"persona part missing: {p} (set PERSONA_ROOT?)")
    return _strip_frontmatter(p.read_text())


def compose_persona(root=None):
    """The internal composition WITHOUT guidance — the local dev/verification oracle only.

    NOT REACHABLE FROM THE REGISTRY, deliberately, and there is a test that says so
    (`test_services.py::internal_service_is_not_reachable_without_guidance`). MultiAgency
    running on its own record is a TENANT: it names the `relationship-intelligence` service in its
    registry entry and carries its own slug-bound guidance file like every other tenant. This
    function exists for `context_ingress.py`'s `__main__` hero-flow oracle, which has no
    registry entry to read guidance from — it is a harness, not a serving path.

    Raises FileNotFoundError with a clear path if a part is missing (fail loudly at startup,
    not silently mid-turn)."""
    base = resource_root(root)
    defn = services.load_service(_INTERNAL_SERVICE, base)
    parts = [_read_part(base, rel) for rel in defn["persona_parts"]]
    # The definition's own key, exactly as compose_service_persona does. This used to append a
    # module constant instead: the same value today, so nothing was wrong — but the dev oracle
    # and the tenant path would have silently diverged the moment a definition named a different
    # tail, and the oracle exists to predict what a tenant gets.
    parts.append(_read_part(base, defn["safety_tail"]))
    return "\n\n---\n\n".join(parts)


# ── Client-specific composition ─────────────────────────────────────────────────────
# Every registry tenant gets ITS SERVICE'S parts + THEIR OWN business guidance. The parts
# live in the service definitions (multi/services/*.json), not here. Guidance is mandatory for
# every service and FAILS CLOSED — there is no un-guided composition on the serving path.

# First line of every guidance file binds it to exactly one client slug, and optionally to
# exactly one SERVICE. The `service:` field is optional so every guidance file written before
# service definitions existed keeps validating unchanged — an absent field pins the default.
_GUIDANCE_MARKER = re.compile(
    r"<!--\s*client-guidance\s+v1\s+slug:\s*([a-z0-9-]+)"
    r"(?:\s+service:\s*([a-z0-9-]+))?\s*-->")


class GuidanceError(RuntimeError):
    """A client's business guidance is missing, unreadable, or bound to another slug."""


def load_guidance(path, slug, service=None):
    """Read and validate one client's guidance file. Fail closed on: missing file,
    unreadable file, empty/trivial content, a first-line slug that doesn't match, or a
    first-line service that doesn't match the service this tenant is configured to run —
    a client must never run with no guidance, with another client's guidance, or with a
    composition its own guidance was not written for."""
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
    # Service binding. A guidance file that names a service pins it; one that names none pins
    # the default. Either way the registry's SERVICE= must agree, so pointing a tenant at a
    # different composition takes a deliberate edit in BOTH files.
    if service is not None:
        declared = m.group(2) or services.DEFAULT_SERVICE
        if declared != service:
            raise GuidanceError(
                f"client {slug!r}: guidance at {p} is bound to service {declared!r} but the "
                f"registry configures service {service!r} — refusing to run a tenant on a "
                "composition its own guidance was not written for. Fix whichever is wrong; "
                "do not 'just make them match'.")
    # Strip HTML comments AFTER marker validation: the template's operator-instruction block
    # (and any authoring notes) must never become model-visible text the analyst can quote
    # back at the client. Length is checked on the stripped text — real guidance, not comments.
    stripped = re.sub(r"<!--.*?-->", "", text, flags=re.S).strip()
    if len(stripped) < 400:
        raise GuidanceError(f"client {slug!r}: guidance at {p} is too short to be real "
                            f"({len(stripped)} chars of content) — fill in the template")
    return stripped


def compose_service_persona(service, guidance_path, slug, root=None):
    """Return a TENANT's persona for one service: that service's persona parts + that
    tenant's validated, slug-and-service-bound guidance + the safety tail.

    THE ONE COMPOSITION PATH FOR EVERY REGISTRY TENANT — internal and external alike. There
    is no branch here on who the tenant is, and there must never be one: an internal tenant
    that composed differently would be exactly the founder-only path this design exists to
    remove. What differs between tenants is the service definition they name and the
    guidance file bound to them, both of which are configuration.

    `service` is a service name (str) or an already-loaded definition (dict)."""
    base = resource_root(root)
    defn = service if isinstance(service, dict) else services.load_service(service, base)
    parts = [_read_part(base, rel) for rel in defn["persona_parts"]]
    # Section title is model-visible and the analyst is told to cite it, so it must read
    # naturally to the people it serves: "your organization", never operator vocabulary.
    # (The guidance FILE's first-line marker stays `client-guidance v1` — that is a machine
    # format validated fail-closed against files already on disk; renaming it is a separate,
    # coordinated change, not a prose edit.)
    parts.append(defn["guidance_heading"] + "\n\n"
                 + load_guidance(guidance_path, slug, service=defn["service"]))
    parts.append(_read_part(base, defn["safety_tail"]))
    return "\n\n---\n\n".join(parts)


def compose_client_persona(guidance_path, slug, root=None):
    """Return an EXTERNAL client's persona — the default service, composed by the one path
    above. Kept as a name because proofs and the eval runner call it, and because "what every
    registry client gets by default" is worth being able to say in one word."""
    return compose_service_persona(services.DEFAULT_SERVICE, guidance_path, slug, root)


def persona_digest(text):
    """A short, non-secret fingerprint of a composed persona.

    The operator needs to answer "is this tenant running the persona I think it is?" without
    printing 13KB of prompt (which carries that tenant's guidance — their data). A digest is
    comparable across tenants and across a release, and reveals nothing."""
    return hashlib.sha256(text.encode()).hexdigest()[:16]


if __name__ == "__main__":
    s = compose_persona()
    base = resource_root()
    n = len(services.load_service(_INTERNAL_SERVICE, base)["persona_parts"])
    print(f"composed persona: {len(s)} chars from {n} parts (service {_INTERNAL_SERVICE}), "
          f"root={base}")
