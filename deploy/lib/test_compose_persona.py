"""Unit checks for deploy/lib/compose-persona.

Every reproduced sed failure input from the templating review is pinned here:
PURPOSE with '/', PURPOSE with '&', slot value with a newline, a persona with
a mid-file one-line HTML comment, a persona with a mid-file '# ' line — plus
the unresolved-slot and >64KiB refusals and the verify exit-code contract.

Run:  python3 deploy/lib/test_compose_persona.py
"""
import pathlib
import subprocess
import sys
import tempfile
import unittest

LIB = pathlib.Path(__file__).resolve().parent
TOOL = LIB / "compose-persona"
REPO = LIB.parents[1]

PERSONA = """<!--
header comment: never installed
-->

# {{AGENT_NAME}}

You are {{AGENT_NAME}}, focused on {{PURPOSE}}.

## Style
Be direct.
"""
TAIL = """<!--
tail header
-->
## Operational tail

Shared rules.
"""


def run(args, stdin=None):
    return subprocess.run(
        [sys.executable, str(TOOL)] + args,
        input=stdin, capture_output=True, text=True,
    )


class ComposeTests(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.TemporaryDirectory()
        base = pathlib.Path(self.dir.name)
        self.persona = base / "persona.md"
        self.tail = base / "tail.md"
        self.persona.write_text(PERSONA)
        self.tail.write_text(TAIL)

    def tearDown(self):
        self.dir.cleanup()

    def compose(self, slots=(), persona_text=None, slug="acme"):
        if persona_text is not None:
            self.persona.write_text(persona_text)
        args = ["compose", "--persona", str(self.persona),
                "--tail", str(self.tail), "--slug", slug]
        for s in slots:
            args += ["--slot", s]
        return run(args)

    def test_slash_in_purpose_is_literal(self):
        # sed died with 'bad flag in substitute command' mid-provision here
        r = self.compose(["AGENT_NAME=Multi", "PURPOSE=research w/ partners"])
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertIn("focused on research w/ partners.", r.stdout)

    def test_ampersand_in_purpose_is_literal(self):
        # sed emitted 'M{{PURPOSE}}A intake' (& = whole match)
        r = self.compose(["AGENT_NAME=Multi", "PURPOSE=M&A intake"])
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertIn("focused on M&A intake.", r.stdout)
        self.assertNotIn("{{", r.stdout)

    def test_newline_in_slot_refused(self):
        r = self.compose(["AGENT_NAME=Multi\nAgent", "PURPOSE=x"])
        self.assertEqual(r.returncode, 1)
        self.assertIn("newline", r.stderr)

    def test_midfile_oneline_comment_survives(self):
        # sed's /<!--/,/-->/d deleted from here to EOF
        text = PERSONA + "\n<!-- inline note -->\nAfter the note.\n"
        r = self.compose(["AGENT_NAME=Multi", "PURPOSE=x"], persona_text=text)
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertIn("<!-- inline note -->", r.stdout)
        self.assertIn("After the note.", r.stdout)

    def test_midfile_h1_and_fence_comment_survive(self):
        # sed's /^# /d ate every such line, including inside code fences
        text = PERSONA + "\n# Mid Heading\n\n```sh\n# a shell comment\n```\n"
        r = self.compose(["AGENT_NAME=Multi", "PURPOSE=x"], persona_text=text)
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertIn("# Mid Heading", r.stdout)
        self.assertIn("# a shell comment", r.stdout)

    def test_leading_header_and_title_h1_stripped(self):
        r = self.compose(["AGENT_NAME=Multi", "PURPOSE=x"])
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertNotIn("never installed", r.stdout)
        self.assertNotIn("tail header", r.stdout)
        self.assertNotIn("# Multi\n", r.stdout)  # title H1 gone, name filled elsewhere
        self.assertIn("## Operational tail", r.stdout)

    def test_unresolved_slot_refused(self):
        # update-persona.sh used to ship literal {{AGENT_NAME}} in exactly this case
        r = self.compose(["PURPOSE=x"])
        self.assertEqual(r.returncode, 1)
        self.assertIn("{{AGENT_NAME}}", r.stderr)

    def test_over_64k_refused(self):
        r = self.compose(["AGENT_NAME=Multi", "PURPOSE=x"],
                         persona_text=PERSONA + "z" * (64 * 1024))
        self.assertEqual(r.returncode, 1)
        self.assertIn("64 KiB", r.stderr)

    def test_unclosed_leading_comment_refused(self):
        r = self.compose(["AGENT_NAME=Multi", "PURPOSE=x"],
                         persona_text="<!--\nnever closed\nbody\n")
        self.assertEqual(r.returncode, 1)
        self.assertIn("unclosed", r.stderr)

    def test_repo_template_composes_and_verifies(self):
        # the real provisioning inputs, end to end
        r = run(["compose",
                 "--persona", str(REPO / "agent/identity/MULTI.template.md"),
                 "--tail", str(REPO / "agent/identity/_operational-tail.md"),
                 "--slug", "acme",
                 "--slot", "AGENT_NAME=Multi", "--slot", "PURPOSE=onboarding w/ demos & intake"])
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertNotIn("{{", r.stdout)
        v = run(["verify", "--slug", "acme"], stdin=r.stdout)
        self.assertEqual(v.returncode, 0, v.stdout)


class VerifyTests(unittest.TestCase):
    def composed(self, slug="acme"):
        with tempfile.TemporaryDirectory() as d:
            p, t = pathlib.Path(d) / "p.md", pathlib.Path(d) / "t.md"
            p.write_text(PERSONA)
            t.write_text(TAIL)
            r = run(["compose", "--persona", str(p), "--tail", str(t),
                     "--slug", slug, "--slot", "AGENT_NAME=Multi", "--slot", "PURPOSE=x"])
            assert r.returncode == 0, r.stderr
            return r.stdout

    def test_round_trip_ok(self):
        self.assertEqual(run(["verify", "--slug", "acme"], stdin=self.composed()).returncode, 0)

    def test_shell_capture_round_trip(self):
        # provision-agent.sh does v="$(compose)"; printf '%s\n' "$v" — the $()
        # strip + printf re-add must be byte-identical for the hash to hold
        out = self.composed()
        self.assertTrue(out.endswith("\n") and not out.endswith("\n\n"))
        self.assertEqual(
            run(["verify", "--slug", "acme"], stdin=out.rstrip("\n") + "\n").returncode, 0)

    def test_tampered_body_is_4(self):
        r = run(["verify", "--slug", "acme"], stdin=self.composed() + "injected\n")
        self.assertEqual(r.returncode, 4)
        self.assertIn("hash mismatch", r.stdout)

    def test_wrong_slug_is_4(self):
        r = run(["verify", "--slug", "other"], stdin=self.composed(slug="acme"))
        self.assertEqual(r.returncode, 4)
        self.assertIn("cross-wired", r.stdout)

    def test_stock_prompt_is_3(self):
        r = run(["verify"], stdin="You are IronClaw Agent, a secure autonomous assistant.\n" + "x" * 400)
        self.assertEqual(r.returncode, 3)
        self.assertIn("STOCK", r.stdout)

    def test_unsentineled_custom_is_3(self):
        r = run(["verify"], stdin="A custom persona written by something other than compose.\n" + "detail " * 100)
        self.assertEqual(r.returncode, 3)
        self.assertIn("no sentinel", r.stdout)

    def test_blank_and_tiny_are_3(self):
        self.assertEqual(run(["verify"], stdin="  \n").returncode, 3)
        self.assertEqual(run(["verify"], stdin="short custom text").returncode, 3)


if __name__ == "__main__":
    unittest.main(verbosity=2)
