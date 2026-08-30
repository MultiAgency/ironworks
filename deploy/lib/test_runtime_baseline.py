"""Static contract for the exact IronClaw 1.4.0 runtime baseline.

These checks do not claim a live topology proof. They prevent the committed deployment paths from
quietly enabling the new opt-in surfaces before the mandatory live re-certification can observe
them.
"""
import json
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
PIN = "4cb47cfaf3fbacd3a2f575a8da654a6542d5fdfe"
MODEL = "qwen/qwen3.5-397b-a17b"
RUNTIME_PATHS = (
    "multi/instance/docker-compose.yml",
    "deploy/secretary/instance/docker-compose.yml",
    "deploy/egress/proof/docker-compose.proof.yml",
    "deploy/provision-agent.sh",
)
OPT_IN_ENV = (
    "IRONCLAW_REBORN_SSH_PUBLIC_KEY",
    "IRONCLAW_REBORN_SANDBOX_PROXY_IMAGE",
    "IRONCLAW_SANDBOX_EXTRA_ALLOWED_DOMAINS",
)


def value(path):
    return (ROOT / path).read_text().split("#", 1)[0].strip()


def active_lines(path):
    return [line.split("#", 1)[0] for line in (ROOT / path).read_text().splitlines()]


def test_exact_runtime_and_model_pins():
    assert value("IRONCLAW_PIN") == PIN
    assert value("MODEL_PIN") == MODEL


def test_new_opt_in_runtime_surfaces_are_not_configured():
    for path in RUNTIME_PATHS:
        active = "\n".join(active_lines(path))
        for name in OPT_IN_ENV:
            assert name not in active, f"{path} enables {name}"
        assert "curation_interval_turns" not in active, f"{path} enables memory curation"


def test_no_runtime_path_publishes_ssh():
    for path in RUNTIME_PATHS:
        active = "\n".join(active_lines(path))
        assert ":2222" not in active and "2222:" not in active, f"{path} publishes SSH"


def test_service_capability_freeze_is_unchanged():
    expected = {"account_records": "read-only", "writes": "none", "egress": "none",
                "outreach": "none"}

    for path in (ROOT / "multi/services").glob("*.json"):
        assert json.loads(path.read_text())["capabilities"] == expected


if __name__ == "__main__":
    for _name, _fn in list(globals().items()):
        if _name.startswith("test_") and callable(_fn):
            _fn()
    print("ALL RUNTIME BASELINE TESTS PASS")
