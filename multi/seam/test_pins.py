#!/usr/bin/env python3
"""The one pin reader, and the one place its second adapter is checked against it.
Run: python3 test_pins.py   (from multi/seam — the suites import siblings by bare name)

WHAT THIS GUARDS. `MODEL_PIN` and `IRONCLAW_PIN` used to be parsed by five separate
implementations across two languages. Four are now one module; the fifth
(`deploy/lib/fleet.sh::fleet_model_pin`) has to stay, because provisioning is shell and cannot
import Python. That is a real seam with two adapters — and the failure it invites is silent:
the shell reader and the Python reader disagreeing about what the pin says, with provisioning
smoke-testing one model while the seam serves another.

So the last test here runs the shell adapter for real and asserts it agrees. A divergence fails
a gate instead of a client turn.
"""
import os
import pathlib
import subprocess
import tempfile

try:
    from . import pins
except ImportError:
    import pins

ROOT = pathlib.Path(__file__).resolve().parents[2]


def test_the_parse_rule_drops_the_trailing_comment():
    """Both pin files carry a trailing comment explaining the choice. The comment is not the
    value, and a reader that returned it would send a whole sentence as a model name."""
    with tempfile.TemporaryDirectory() as d:
        (pathlib.Path(d) / "MODEL_PIN").write_text(
            "Some/Model-X  # TEE-hosted, chosen 2026-01-01\n#\n# more prose\n")
        assert pins.pin_value("MODEL_PIN", d) == ("Some/Model-X", None)
    print("  PASS the parse rule keeps the value and drops the comment")


def test_an_unreadable_or_empty_pin_never_yields_a_value():
    """FAIL LOUD is the whole point: a fallback literal is the one value that can silently
    outrank the pin. Neither shape may invent one."""
    with tempfile.TemporaryDirectory() as d:
        value, why = pins.pin_value("MODEL_PIN", d)
        assert value is None and "unreadable" in why, "a missing pin produced a value"
        (pathlib.Path(d) / "MODEL_PIN").write_text("# only a comment\n")
        value, why = pins.pin_value("MODEL_PIN", d)
        assert value is None and "names nothing" in why, "a comment-only pin produced a value"
        try:
            pins.require_pin("MODEL_PIN", d)
        except pins.PinError as e:
            assert "do not hardcode" in str(e)
        else:
            raise AssertionError("require_pin returned on a comment-only pin")
    print("  PASS an unreadable or empty pin raises, and never falls back to a literal")


def test_the_two_shapes_agree_and_only_differ_on_failure():
    """`pin_value` reports, `require_pin` raises. That difference is deliberate — the console
    must stay alive to say which check failed — but they must never disagree on a GOOD pin."""
    value, why = pins.pin_value("MODEL_PIN", ROOT)
    assert why is None and value, "the repo's own MODEL_PIN does not read"
    assert pins.require_pin("MODEL_PIN", ROOT) == value
    assert pins.ironclaw_pin(ROOT) == pins.pin_value("IRONCLAW_PIN", ROOT)[0]
    print("  PASS the reporting and raising shapes agree on a good pin")


def test_the_model_env_override_wins_but_only_for_model():
    """`MODEL` is the documented one-off override. It must not leak into other pin files."""
    prior = os.environ.get("MODEL")
    os.environ["MODEL"] = "Override/Model"
    try:
        assert pins.model_pin(ROOT) == "Override/Model"
        assert pins.ironclaw_pin(ROOT) != "Override/Model", \
            "the MODEL override leaked into the runtime pin"
    finally:
        os.environ.pop("MODEL", None)
        if prior is not None:
            os.environ["MODEL"] = prior
    print("  PASS the MODEL override applies to the model pin and nothing else")


def _shell(fn, root=None):
    """Run one `fleet.sh` reader and return the completed process.

    `FLEET_REPO_ROOT` is assigned AFTER the source, not before: fleet.sh derives it from its own
    location and overwrites whatever the caller set, so a fixture root only takes effect on this
    side of the `.` — the reason this helper exists rather than each test writing the line.

    `MODEL` is stripped so the file, not the environment, is what gets compared.
    """
    script = (f'set -eu; . {ROOT}/deploy/lib/fleet.sh; '
              f'FLEET_REPO_ROOT={root or ROOT}; {fn}')
    env = {k: v for k, v in os.environ.items() if k != "MODEL"}
    return subprocess.run(["bash", "-c", script], capture_output=True, text=True, env=env)


def test_the_shell_adapter_agrees_with_this_one():
    """THE SECOND ADAPTER, for BOTH pins. `fleet.sh` cannot import this module, so it
    re-implements the parse in shell. Run it and compare — this is the only thing standing
    between the two readers and a silent divergence.

    The runtime pin is here because it was NOT: `fleet.sh` had a model reader and no IronClaw
    one, so verify-pin.sh, migrate-image.sh and run-proof.sh each parsed IRONCLAW_PIN with
    `cut -d' ' -f1` — a different rule, under no gate at all."""
    for fn, want in (("fleet_model_pin", pins.model_pin(ROOT)),
                     ("fleet_ironclaw_pin", pins.ironclaw_pin(ROOT))):
        r = _shell(fn)
        assert r.returncode == 0, f"{fn} failed: {r.stderr.strip()}"
        assert r.stdout.strip() == want, (
            f"{fn} reads {r.stdout.strip()!r} where this module reads {want!r} — the fleet "
            "would provision, tag and certify against a different pin than the seam serves")
    print("  PASS the shell adapter and this module read the same model and runtime pin")


def test_the_shell_adapter_drops_the_comment_the_same_way():
    """The rule is `#`-delimited, not space-delimited, and it takes ONE line. Both distinctions
    are load-bearing and neither is exercised by the repo's own pin files today: IRONCLAW_PIN
    happens to have a space before its `#` and happens to be one line. `cut -d' ' -f1` agreed
    with the real rule only by that accident — on `<rev># tag` it keeps the `#`, and on a pin
    that grew a second comment line (MODEL_PIN already has twenty) it returns BOTH lines, which
    no `=` comparison can ever match. verify-pin.sh compares exactly that way."""
    with tempfile.TemporaryDirectory() as d:
        for name in ("MODEL_PIN", "IRONCLAW_PIN"):
            (pathlib.Path(d) / name).write_text("deadbeef# tag, no space before the hash\n"
                                                "# a second line of prose\n")
        for fn in ("fleet_model_pin", "fleet_ironclaw_pin"):
            r = _shell(fn, root=d)
            assert r.returncode == 0, f"{fn} failed: {r.stderr.strip()}"
            assert r.stdout.strip() == "deadbeef", (
                f"{fn} read {r.stdout.strip()!r} — the shell parse is not the `#` rule")
        assert pins.pin_value("MODEL_PIN", d)[0] == "deadbeef"
    print("  PASS the shell adapter drops the comment on both shapes the space-split got wrong")


def test_the_shell_adapter_refuses_a_pin_it_cannot_read():
    """FAIL LOUD, the shell half. A reader that prints nothing and returns 0 hands its caller an
    empty image tag or an empty comparison value — the fail-open shape pins.py's header argues
    against. Both pin files are tracked, so unreadable means a broken checkout."""
    with tempfile.TemporaryDirectory() as d:
        for fn in ("fleet_model_pin", "fleet_ironclaw_pin"):
            r = _shell(fn, root=d)                       # nothing in this root at all
            assert r.returncode != 0, f"{fn} returned 0 on a missing pin file"
            assert "broken checkout" in r.stderr, r.stderr
        (pathlib.Path(d) / "IRONCLAW_PIN").write_text("# only a comment\n")
        r = _shell("fleet_ironclaw_pin", root=d)
        assert r.returncode != 0, "fleet_ironclaw_pin returned 0 on a comment-only pin"
        assert "names nothing" in r.stderr, r.stderr
    print("  PASS the shell adapter fails loudly rather than yielding an empty pin")


if __name__ == "__main__":
    for _name, _fn in list(globals().items()):
        if _name.startswith("test_") and callable(_fn):
            _fn()
    print("ALL PIN TESTS PASS")
