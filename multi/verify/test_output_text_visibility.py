#!/usr/bin/env python3
"""Proof readers must see exactly the assistant text exposed by the product."""
import ast
import pathlib
import re
import sys
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "multi" / "seam"))
sys.path.insert(0, str(ROOT / "multi" / "verify"))

import context_ingress as ingress  # noqa: E402
import common  # noqa: E402
import responses  # noqa: E402


MARKER = "REASONING-ONLY-MARKER-7749"
VISIBLE = "The client can see this answer."


PROOF_CHECKS = ROOT / "deploy" / "egress" / "proof" / "proof_checks.py"


def proof_reader():
    """The extractor `proof_checks.py` uses, however it gets it.

    It cannot simply be imported: that file runs live assertions at module scope and needs a
    disposable stack. It used to define its own `text_of`, which this file lifted out by AST —
    the only way to reach a copy without running the module. It now imports the shared reader
    instead, so the copy is gone and there is nothing to lift.

    Resolving it from the SOURCE keeps the guarantee either way: whichever form the file is in,
    what is tested below is what that proof actually reads.
    """
    src = PROOF_CHECKS.read_text()
    if re.search(r"^from responses import output_text as text_of", src, re.M):
        return responses.output_text
    tree = ast.parse(src, filename=str(PROOF_CHECKS))
    node = next(n for n in tree.body if isinstance(n, ast.FunctionDef) and n.name == "text_of")
    namespace = {}
    exec(compile(ast.Module(body=[node], type_ignores=[]), str(PROOF_CHECKS), "exec"), namespace)
    return namespace["text_of"]


proof_text_of = proof_reader()


def response_with_reasoning_only_evidence():
    return {
        "output_text": MARKER,
        "output": [
            {"type": "reasoning", "content": [
                {"type": "reasoning_text", "text":
                 f"{MARKER} 500 Dana budget technical-fit FACT? next step discovery recommend"}
            ]},
            {"type": "message", "content": [
                {"type": "reasoning_text", "text": MARKER},
                {"type": "output_text", "text": VISIBLE},
            ]},
        ],
    }


def product_loop_score(reply):
    """The seven release-proof tells, applied to already-extracted visible text."""
    low = reply.lower()
    return sum({
        "pain": "500" in reply,
        "contact": "dana" in low,
        "missing": any(w in low for w in
                       ["budget", "timeline", "helpdesk", "decision process"]),
        "guidance": any(w in low for w in
                        ["technical-fit", "pilot", "discovery", "deprioriti", "alpine",
                         "subscription", "renewal"]),
        "evidence": any(t in reply for t in ["FACT", "UNKNOWN", "INFERENCE"]),
        "question": "?" in reply,
        "next_step": any(w in low for w in
                         ["next step", "discovery", "advance", "intake", "recommend"]),
    }.values())


class OutputTextVisibilityTests(unittest.TestCase):
    def test_reasoning_is_not_client_visible_to_either_proof_reader(self):
        doc = response_with_reasoning_only_evidence()
        self.assertEqual(common.text_of(doc), VISIBLE)
        self.assertEqual(proof_text_of(doc), VISIBLE)
        self.assertEqual(ingress.output_text(doc), VISIBLE)

    def test_reasoning_only_marker_cannot_pass_injection_proofs(self):
        doc = response_with_reasoning_only_evidence()
        self.assertNotIn(MARKER, common.text_of(doc))
        self.assertNotIn(MARKER, proof_text_of(doc))

    def test_reasoning_only_evidence_cannot_raise_product_loop_score(self):
        doc = response_with_reasoning_only_evidence()
        reasoning = doc["output"][0]["content"][0]["text"]
        self.assertGreaterEqual(product_loop_score(reasoning), 5)
        self.assertEqual(product_loop_score(common.text_of(doc)), 0)

    def test_proof_readers_remain_in_parity_with_product_extraction(self):
        cases = [
            response_with_reasoning_only_evidence(),
            {"output": [
                {"type": "message", "content": [
                    {"type": "text", "text": "first"},
                    {"type": "output_text", "text": "second"},
                ]},
                {"type": "tool_call", "content": [
                    {"type": "output_text", "text": "internal"},
                ]},
            ]},
            {"output_text": "flattened text is not product-visible", "output": []},
            {},
        ]
        for doc in cases:
            with self.subTest(doc=doc):
                product = ingress.output_text(doc)
                self.assertEqual(common.text_of(doc), product)
                self.assertEqual(proof_text_of(doc), product)


if __name__ == "__main__":
    unittest.main()
