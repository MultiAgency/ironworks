"""Service definitions — what a MultiAgency-operated agent service IS, as data.

A *service definition* names the reusable half of a service: which persona parts compose it,
whether tenant guidance is mandatory, which tool policy confines it, whether an evaluation suite
measures it, and whether the model is pinned. A *tenant* (a registry `.env` under
`CLIENTS_DIR`) names the other half: identity, credentials, channel, and that tenant's own
guidance file.

WHY THIS EXISTS — it is the difference between MultiAgency being a first-class tenant and
being a special case. Before this file there were exactly two persona compositions and the
choice between them was hard-coded: `load_clients()` always composed the client-generic one,
so MultiAgency's own book could only be served through `context_ingress.py`'s `__main__`
dev-oracle — a path with no registry, no routing, no mandatory guidance, no provisioning, no
confinement gate and no deprovisioning. "We use what our clients use" was not true, and no
test could have caught that, because the internal path had no tests.

Now both compositions are service definitions, both are selected the same way, and BOTH run
through `load_clients -> ClientConfig -> Thread -> turn`. The internal service gets no
privilege: guidance is mandatory for it too.

TWO INDEPENDENT EDITS, DELIBERATELY. A service is bound to a tenant in two places that must
agree: the registry's `SERVICE=` key, and an optional `service:` field in the guidance file's
first-line marker. A guidance file that declares a service pins it; a registry that names a
different one FAILS CLOSED. Guidance with no `service:` field pins the default. So serving
MultiAgency's internal composition to an external client takes two deliberate edits to two
files in two places, not one mistyped key — the same slug-binding trick that already stops
one client's guidance reaching another.

Definitions are COMMITTED (`multi/services/*.json`) because they are product decisions, not
operator state. Tenant configuration and tenant guidance stay outside the repo, as before.
"""
import json
import pathlib

try:
    from .resources import resource_root
except ImportError:  # direct-script compatibility
    from resources import resource_root


# The service every tenant gets unless it says otherwise. Existing registry files predate
# service definitions and must keep composing byte-for-byte what they composed before, so the
# default IS the old client path.
DEFAULT_SERVICE = "account-analysis"

_REQUIRED_KEYS = ("service", "version", "audience", "persona_parts", "guidance",
                  "guidance_heading", "safety_tail", "capabilities", "model_policy",
                  "evaluation", "responsibility")
_AUDIENCES = ("internal", "external")

_cache = {}


class ServiceError(RuntimeError):
    """A service definition is missing, malformed, or names a file that is not there."""


def services_dir(root=None):
    return resource_root(root) / "multi" / "services"


def available(root=None):
    """Every service name with a definition on disk, sorted. Used by the operator CLI and by
    validation; never by the serving path, which always names one service explicitly."""
    d = services_dir(root)
    return sorted(p.stem for p in d.glob("*.json")) if d.is_dir() else []


def load_service(name, root=None):
    """Read and validate one service definition. Fails closed on: unknown name, malformed
    JSON, a missing required key, an unknown audience, a persona part that is not on disk, or
    an unsupported guidance/model policy. Evaluation is required metadata but may be null; a
    declared suite must be a repo-relative path that exists."""
    base = resource_root(root)
    key = (str(base), name)
    if key in _cache:
        return _cache[key]
    if not name or any(ch not in "abcdefghijklmnopqrstuvwxyz0123456789-" for ch in name):
        raise ServiceError(f"service name {name!r} must be lowercase [a-z0-9-]")
    p = services_dir(base) / f"{name}.json"
    try:
        d = json.loads(p.read_text())
    except FileNotFoundError:
        known = ", ".join(available(base)) or "(none)"
        raise ServiceError(f"unknown service {name!r}: no definition at {p}. Known: {known}") from None
    except ValueError as e:
        raise ServiceError(f"service {name!r}: {p} is not valid JSON: {e}") from e
    missing = [k for k in _REQUIRED_KEYS if k not in d]
    if missing:
        raise ServiceError(f"service {name!r}: {p} is missing required key(s): {', '.join(missing)}")
    if d["service"] != name:
        raise ServiceError(f"service {name!r}: {p} declares service {d['service']!r} — "
                           "the filename is the service id and they must agree")
    if d["audience"] not in _AUDIENCES:
        raise ServiceError(f"service {name!r}: audience {d['audience']!r} is not one of {_AUDIENCES}")
    if d["guidance"] != "required":
        raise ServiceError(f"service {name!r}: guidance mode {d['guidance']!r} is not supported — "
                           "'required' is the only mode; anything else would be a fail-open path")
    if d["model_policy"] != "pin":
        raise ServiceError(f"service {name!r}: model_policy {d['model_policy']!r} is not "
                           "supported — canonical IronWorks has one policy: 'pin'")
    evaluation = d["evaluation"]
    if evaluation is not None:
        if not isinstance(evaluation, str) or not evaluation.strip():
            raise ServiceError(f"service {name!r}: evaluation must be a repo-relative path or null")
        evaluation_path = pathlib.Path(evaluation)
        if evaluation_path.is_absolute() or ".." in evaluation_path.parts:
            raise ServiceError(f"service {name!r}: evaluation must stay under the repository root")
        if not (base / evaluation_path).exists():
            raise ServiceError(f"service {name!r}: evaluation {evaluation!r} does not exist under {base}")
    if not isinstance(d["persona_parts"], list) or not d["persona_parts"]:
        raise ServiceError(f"service {name!r}: persona_parts must be a non-empty list")
    for rel in list(d["persona_parts"]) + [d["safety_tail"]]:
        if not (base / rel).is_file():
            raise ServiceError(f"service {name!r}: persona part {rel!r} is not a file under {base}")
    if not isinstance(d["version"], int) or d["version"] < 1:
        raise ServiceError(f"service {name!r}: version must be a positive integer")
    # PRESENT, A STRING, NON-EMPTY — AND DELIBERATELY NOTHING MORE. `responsibility` states what
    # this service is answerable for, in the organization's vocabulary rather than the
    # composition's. It exists because three separate things reached around the definition to
    # find that: the structural test grepped persona prose for it, the evaluation claim is bound
    # to persona wording, and this README described it as "a reasoning objective" that the data
    # did not carry.
    #
    # Resist making it structured. A responsibility that is PARSED becomes a specification, and a
    # specification for one service is a workflow engine for two — which is on the explicit
    # non-goal list. Nothing branches on this value and nothing should; it is a declaration a
    # human reviews and a test can anchor on.
    if not isinstance(d["responsibility"], str) or not d["responsibility"].strip():
        raise ServiceError(f"service {name!r}: responsibility must be a non-empty string stating "
                           "what this service is answerable for, in the organization's vocabulary")
    _cache[key] = d
    return d


def service_id(defn):
    """The operator-visible version string for a service: `<name>@<version>`."""
    return f"{defn['service']}@{defn['version']}"
