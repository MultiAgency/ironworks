"""The repo-root pin files, read one way.

WHAT THIS IS FOR. `MODEL_PIN` and `IRONCLAW_PIN` are the version of record for the model and
the runtime. The FILE was already single; the PARSER was not — the same eight lines were
written five times (the seam, the verify suite, the egress proof, the console, and fleet.sh),
in two languages, and they did not agree on what happens when the pin is unreadable. One of
the five was a bare `open()` with no error handling and no `MODEL` override, inside the proof
that certifies the seam. A parser that fails differently is a parser that can serve a turn on
a model nobody chose.

WHY NO FALLBACK LITERAL — the rule this module exists to hold in one place. A default here
would be the one value that can SILENTLY outrank the pin. `MODEL_PIN` is tracked, so an
unreadable pin means a broken checkout or a bad `PERSONA_ROOT`, and a literal would then serve
every client turn on whatever model it last named. That is not cosmetic drift: the pin's first
stated reason is that the model is TEE-hosted, so a stale literal can quietly move a tenant's
private book onto a model with weaker privacy guarantees, and nothing in the reply would say
so. Fail loud, at import, so the bridge refuses to start rather than dying mid-conversation.

TWO SHAPES, ONE RULE. Callers want different failure behaviour and that is a real difference,
not one to paper over:
  - `model_pin()` raises. The product, the proofs and provisioning all want a hard stop.
  - `pin_value()` returns `(value, why)` and never raises. The operator console must REPORT a
    broken pin as a failed check, because a console that crashes tells you less than one that
    says which check failed.
Both go through the same parse, so the two cannot drift on what a pin file means.

WHERE THIS LIVES, AND WHY NOT deploy/lib. `multi/` imports nothing from `deploy/`; the console
reaches INTO `multi/seam` and `deploy/lib` both. Putting a module the seam's hot path needs
under `deploy/lib` would invert that and make the product depend on operator tooling. The
product owns the pin; the tooling reads it.

STILL A SECOND ADAPTER, DELIBERATELY: `deploy/lib/fleet.sh::fleet_model_pin` is the shell-side
reader, because provisioning is shell and cannot import this. Two adapters at a real seam. Its
comment points here; keep the two in step by hand, and `test_pins.py` asserts they agree on the
current pin so a divergence fails a gate rather than a client turn.
"""
import os
import pathlib

try:
    from .resources import resource_root
except ImportError:  # direct-script compatibility
    from resources import resource_root

_ROOT = resource_root()


class PinError(RuntimeError):
    """A pin file is missing, unreadable, or names nothing."""


def repo_root(root=None):
    """Where the pin files live. Explicit argument wins, then PERSONA_ROOT, then this file's
    own location — file-relative, so a caller in deploy/egress/proof resolves the same root as
    one in multi/seam without passing anything."""
    return pathlib.Path(root) if root is not None else resource_root()


def pin_value(name, root=None):
    """`(value, why)` for a repo-root pin file. Never raises — the console's shape.

    The parse rule, in one place: everything before the first `#` on the file, stripped. Both
    pin files carry a trailing comment explaining the choice, and that comment is not the value.
    """
    p = repo_root(root) / name
    try:
        raw = p.read_text()
    except OSError as e:
        return None, f"{p} unreadable: {e}"
    value = raw.split("#", 1)[0].strip()
    return (value, None) if value else (None, f"{p} names nothing on its first line")


def require_pin(name, root=None):
    """The pin, or `PinError`. Use when there is no sensible way to carry on without it."""
    value, why = pin_value(name, root)
    if value is None:
        raise PinError(f"cannot read the {name} at {repo_root(root) / name}: {why}. {name} is "
                       "tracked — an unreadable pin means a broken checkout or a bad "
                       "PERSONA_ROOT. Fix the checkout; do not hardcode a value here.")
    return value


def model_pin(root=None):
    """The model pin with an explicit one-off environment override for adjunct tests/tools.

    Canonical multi-tenant serving intentionally does not call this function: its registry
    loader reads the literal MODEL_PIN and rejects process or tenant attempts to change it.
    """
    return os.environ.get("MODEL") or require_pin("MODEL_PIN", root)


def ironclaw_pin(root=None):
    """The runtime rev of record — the rev every image is built from."""
    return require_pin("IRONCLAW_PIN", root)
