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
    "deploy/migrate-image.sh",
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


def compose_service(path, name):
    """Return one two-space-indented Compose service without needing a YAML dependency."""
    lines = (ROOT / path).read_text().splitlines()
    marker = f"  {name}:"
    start = lines.index(marker)
    end = next((i for i in range(start + 1, len(lines))
                if lines[i].startswith("  ") and not lines[i].startswith("    ")
                and lines[i].strip()), len(lines))
    return "\n".join(line.split("#", 1)[0] for line in lines[start:end])


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


def test_compose_runtimes_prepare_workspace_without_dac_override():
    stacks = (
        ("multi/instance/docker-compose.yml", "${IRONCLAW_IMAGE:-ironclaw:main}",
         "ic-data:/data"),
        ("deploy/secretary/instance/docker-compose.yml",
         "${IRONCLAW_IMAGE:-ironclaw:main}", "secretary-data:/data"),
        ("deploy/egress/proof/docker-compose.proof.yml",
         "${PROOF_IMAGE:-ironclaw:main}", "ic-data:/data"),
    )
    for path, image, volume in stacks:
        initializer = compose_service(path, "workspace-init")
        runtime = compose_service(path, "ic" if "egress/proof" in path else "ironclaw")

        for required in (image, 'user: "1000:1000"',
                         '["/bin/mkdir", "-p", "/data/ironclaw-reborn/workspace"]',
                         "network_mode: none", "read_only: true", volume,
                         "no-new-privileges:true", "cap_drop: [ALL]"):
            assert required in initializer, f"{path} workspace initializer lost {required}"
        assert "cap_add:" not in initializer, f"{path} initializer gained capabilities"
        assert "workspace-init: { condition: service_completed_successfully }" in runtime
        assert "cap_drop: [ALL]" in runtime
        assert "cap_add: [CHOWN, SETUID, SETGID]" in runtime
        assert "DAC_OVERRIDE" not in runtime


def test_fleet_paths_use_the_same_workspace_and_runtime_security_contract():
    fleet = (ROOT / "deploy/lib/fleet.sh").read_text()
    for required in ("fleet_prepare_workspace()", "--network none", "--read-only",
                     "--user 1000:1000", "--cap-drop ALL",
                     "--security-opt no-new-privileges:true", "--entrypoint /bin/mkdir",
                     'FLEET_WORKSPACE_ROOT="/data/ironclaw-reborn/workspace"'):
        assert required in fleet, f"fleet workspace initializer lost {required}"

    for path in ("deploy/provision-agent.sh", "deploy/migrate-image.sh"):
        active = "\n".join(active_lines(path))
        assert "fleet_prepare_workspace" in active, f"{path} skips workspace initialization"
        assert "--security-opt no-new-privileges:true" in active
        assert "--cap-drop ALL --cap-add CHOWN --cap-add SETUID --cap-add SETGID" in active
        assert "DAC_OVERRIDE" not in active, f"{path} grants DAC_OVERRIDE"


if __name__ == "__main__":
    for _name, _fn in list(globals().items()):
        if _name.startswith("test_") and callable(_fn):
            _fn()
    print("ALL RUNTIME BASELINE TESTS PASS")
