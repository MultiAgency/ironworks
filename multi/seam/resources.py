"""Resolve committed seam resources in a checkout or an installed wheel."""
import os
from pathlib import Path

_CHECKOUT_ROOT = Path(__file__).resolve().parents[2]
_BUNDLE_ROOT = Path(__file__).resolve().parent / "_bundle"


def resource_root(root=None):
    """Explicit root, then PERSONA_ROOT, then checkout, then the wheel's resource bundle."""
    if root is not None:
        return Path(root)
    configured = os.environ.get("PERSONA_ROOT")
    if configured:
        return Path(configured)
    if (_CHECKOUT_ROOT / "MODEL_PIN").is_file() and (_CHECKOUT_ROOT / "multi/services").is_dir():
        return _CHECKOUT_ROOT
    if (_BUNDLE_ROOT / "MODEL_PIN").is_file() and (_BUNDLE_ROOT / "multi/services").is_dir():
        return _BUNDLE_ROOT
    return _CHECKOUT_ROOT
