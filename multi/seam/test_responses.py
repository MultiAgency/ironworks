#!/usr/bin/env python3
"""The one response reader, and the three disagreements it replaced.
Run: python3 test_responses.py   (from multi/seam — the suites import siblings by bare name)

WHAT THIS GUARDS. "What did the model say?" was three separate walks of the same document:
`context_ingress._output_text`, `multi/verify/common.text_of`, and
`deploy/egress/proof/proof_checks.text_of`. The product filtered on item and content TYPE; both
proof copies took any content entry carrying a `text` key. So a proof could assert on the
model's own REASONING, which the client never receives — and the two injection proofs decide
"did it refuse?" by looking for markers in exactly that string.

Each test below is one of the shapes on which the three gave different answers.
"""
import os
import pathlib
import subprocess
import sys
import tempfile

try:
    from . import responses
except ImportError:
    import responses


ROOT = pathlib.Path(__file__).resolve().parents[2]


def test_only_message_items_are_client_visible_text():
    """THE DIVERGENCE THAT MATTERED. A reasoning item sits beside the message and carries its
    own `text`. The product never delivered it; both proof copies read it."""
    doc = {"output": [
        {"type": "reasoning", "content": [{"type": "reasoning_text", "text": "INTERNAL SCRATCHPAD"}]},
        {"type": "message", "content": [{"type": "output_text", "text": "the real answer"}]},
    ]}
    got = responses.output_text(doc)
    assert got == "the real answer", got
    assert "SCRATCHPAD" not in got, "model reasoning reached the reader that proofs assert on"
    print("  PASS reasoning items are not client-visible text")


def test_non_text_content_inside_a_message_is_skipped():
    """The content-type filter is the second half of the same rule: a message item can carry
    annotations or tool payloads beside the prose."""
    doc = {"output": [{"type": "message", "content": [
        {"type": "tool_use", "text": "TOOL ARGUMENTS"},
        {"type": "output_text", "text": "visible"},
        {"type": "text", "text": "also visible"},
    ]}]}
    assert responses.output_text(doc) == "visible\nalso visible"
    print("  PASS only output_text/text content inside a message item is read")


def test_the_flattened_top_level_field_is_not_client_visible():
    """`common.text_of` read a top-level `output_text` and the product did not — the second of
    the three disagreements. It resolves toward the PRODUCT: adding the fallback would change
    what a client receives (an empty fetch becomes a delivered answer), and that is a serving
    change, not a proof fix. See the note in responses.py."""
    assert responses.output_text({"output": [], "output_text": "flattened"}) == ""
    both = {"output": [{"type": "message", "content": [{"type": "output_text", "text": "structured"}]}],
            "output_text": "flattened"}
    assert responses.output_text(both) == "structured"
    print("  PASS a flattened top-level field is not read as client-visible text")


def test_a_malformed_document_yields_empty_rather_than_raising():
    """Callers decide what empty means — telegram_bridge treats it as a failed fetch. A reader
    that raised would turn 'the model said nothing' into a traceback in the delivery path."""
    for bad in (None, {}, {"output": None}, {"output": [None, "junk"]},
                {"output": [{"type": "message", "content": None}]},
                {"output": [{"type": "message", "content": [{"type": "output_text"}]}]},
                {"output_text": 12345}):
        assert responses.output_text(bad) == "", bad
    print("  PASS a malformed or empty document reads as empty, never an exception")


def test_the_product_and_the_proof_suites_share_one_implementation():
    """The point of the module. If any of these stops being the same function, the proofs and
    the product can disagree about what a client was told."""
    try:
        from . import context_ingress
    except ImportError:
        import context_ingress
    assert context_ingress.output_text is responses.output_text
    print("  PASS context_ingress re-exports the one reader")


def test_removed_seam_helpers_have_no_executable_or_documented_callers():
    """The seam split moved transport and response parsing into their owning modules. A live
    proof and the upgrade runbook retained the removed names, which passed every offline test and
    then failed only at the target host."""
    paths = (ROOT / "deploy/egress/proof/service_path_checks.py", ROOT / "deploy/UPGRADE.md")
    # Construct the spellings so the final source-tree grep stays meaningful: any literal hit is
    # a real caller or documentation dependency, not this guard naming what it forbids.
    stale = tuple("ing." + name for name in ("_svc", "_output_text"))
    hits = [(str(path.relative_to(ROOT)), name) for path in paths for name in stale
            if name in path.read_text()]
    assert not hits, f"removed seam helper dependencies remain: {hits}"
    print("  PASS no executable/documented caller uses removed seam helpers")


def test_service_path_without_live_prerequisites_reports_blocked():
    """An isolated export has no tenant credentials. That is BLOCKED, not an import failure."""
    proof = ROOT / "deploy/egress/proof/service_path_checks.py"
    with tempfile.TemporaryDirectory() as home:
        env = {"HOME": home, "PATH": os.environ.get("PATH", ""),
               "PYTHONDONTWRITEBYTECODE": "1"}
        run = subprocess.run([sys.executable, str(proof)], cwd=ROOT, env=env,
                             capture_output=True, text=True)
    assert run.returncode == 2, (run.returncode, run.stdout, run.stderr)
    assert "BLOCKED" in run.stdout and "AttributeError" not in run.stdout + run.stderr
    print("  PASS service-path proof reports BLOCKED when live prerequisites are absent")


if __name__ == "__main__":
    for _name, _fn in list(globals().items()):
        if _name.startswith("test_") and callable(_fn):
            _fn()
    print("ALL RESPONSE READER TESTS PASS")
