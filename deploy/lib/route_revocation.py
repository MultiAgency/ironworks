#!/usr/bin/env python3
"""Fail-closed authority for the bridge's in-memory route during deprovisioning."""
import argparse
import datetime
import json
import os
import pathlib
import subprocess
import sys

ABSENT, PRESENT, UNKNOWN = "ABSENT", "PRESENT", "UNKNOWN"


def _command(args):
    try:
        p = subprocess.run(args, capture_output=True, text=True, timeout=10)
        return p.returncode, p.stdout, p.stderr
    except (OSError, subprocess.TimeoutExpired) as e:
        return 1, "", type(e).__name__


def _kill_probe(pid):
    try:
        os.kill(pid, 0)
        return True, ""
    except ProcessLookupError:
        return False, "process does not exist"
    except PermissionError:
        return None, "permission denied while probing the process (EPERM)"
    except OSError as e:
        return None, f"process probe failed: {type(e).__name__}: {e}"


def _service_state(unit):
    rc, out, err = _command([
        "systemctl", "show", unit, "--property=LoadState", "--property=ActiveState",
        "--property=SubState", "--property=MainPID",
    ])
    if rc != 0:
        return None, f"authoritative service state is unavailable ({err.strip() or 'systemctl failed'})"
    fields = {}
    for line in out.splitlines():
        key, sep, value = line.partition("=")
        if sep:
            fields[key] = value.strip()
    required = {"LoadState", "ActiveState", "SubState", "MainPID"}
    if not required <= fields.keys():
        return None, "authoritative service state returned incomplete metadata"
    return fields, ""


def _bridge_pid(path, seam_dir):
    if not pathlib.Path(path).is_file():
        return None, "bridge state database is absent, so its process identity is unmeasured"
    sys.path.insert(0, str(seam_dir))
    try:
        import bridge_state as bs
        st = bs.BridgeState(path, migrate=False)
        try:
            raw = (st.progress_snapshot() or {}).get("pid")
        finally:
            st.close()
    except Exception as e:
        return None, f"bridge PID metadata is unreadable ({type(e).__name__}: {e})"
    if raw in (None, ""):
        return None, "bridge PID metadata is missing"
    try:
        pid = int(raw)
    except (TypeError, ValueError):
        return None, f"bridge PID metadata is malformed ({raw!r})"
    if pid <= 0:
        return None, f"bridge PID metadata is malformed ({raw!r})"
    return pid, ""


def _process_started_at(pid):
    rc, out, err = _command(["ps", "-o", "lstart=", "-p", str(pid)])
    value = out.strip()
    if rc != 0 or not value:
        return None, f"process start time is unavailable ({err.strip() or 'ps returned no value'})"
    try:
        parsed = datetime.datetime.strptime(" ".join(value.split()), "%a %b %d %H:%M:%S %Y")
        # POSIX ps renders lstart in the host's local timezone without an offset.  astimezone()
        # attaches that host timezone; assuming UTC would misorder the process and registry
        # events on every non-UTC operator machine.
        return parsed.astimezone().timestamp(), ""
    except ValueError:
        return None, f"process start time is malformed ({value!r})"


def _serving_pid(service):
    """(pid, terminal document). Exactly one is non-None."""
    load, active, raw_main = service["LoadState"], service["ActiveState"], service["MainPID"]
    try:
        main_pid = int(raw_main)
    except ValueError:
        return None, {"state": UNKNOWN, "reason": f"service MainPID is malformed ({raw_main!r})"}
    if load != "loaded":
        return None, {"state": UNKNOWN,
                      "reason": f"service unit is not authoritatively loaded (LoadState={load})"}
    if active in {"inactive", "failed"} and main_pid == 0:
        return None, {"state": ABSENT,
                      "reason": f"systemd authoritatively reports the bridge {active} "
                                "with MainPID=0"}
    if active != "active" or main_pid <= 0:
        return None, {"state": UNKNOWN,
                      "reason": f"service state is transitional or contradictory "
                                f"(ActiveState={active}, SubState={service['SubState']}, "
                                f"MainPID={main_pid})"}
    return main_pid, None


def _compare_epochs(main_pid, started, registry_removed_at):
    try:
        removed = float(registry_removed_at)
    except (TypeError, ValueError):
        return {"state": UNKNOWN, "reason": "registry removal time is missing or malformed"}
    if started > removed:
        return {"state": ABSENT, "pid": main_pid, "started_at": started,
                "reason": "the currently active bridge started after registry removal"}
    return {"state": PRESENT, "pid": main_pid, "started_at": started,
            "reason": "the currently active bridge started before registry removal"}


def evaluate(db_path, registry_removed_at, seam_dir, unit="multi-bridge.service"):
    """Return a state document. Only ABSENT permits lifecycle success."""
    service, why = _service_state(unit)
    if service is None:
        return {"state": UNKNOWN, "reason": why}
    main_pid, terminal = _serving_pid(service)
    if terminal is not None:
        return terminal

    recorded_pid, why = _bridge_pid(db_path, seam_dir)
    if recorded_pid is None:
        return {"state": UNKNOWN, "reason": why}
    if recorded_pid != main_pid:
        return {"state": UNKNOWN,
                "reason": f"service MainPID {main_pid} contradicts bridge-state PID {recorded_pid}"}
    alive, why = _kill_probe(main_pid)
    if alive is not True:
        return {"state": UNKNOWN,
                "reason": f"active service process {main_pid} could not be positively inspected: {why}"}
    started, why = _process_started_at(main_pid)
    if started is None:
        return {"state": UNKNOWN, "reason": why}
    return _compare_epochs(main_pid, started, registry_removed_at)


def main(argv=None):
    p = argparse.ArgumentParser()
    p.add_argument("--db", required=True)
    p.add_argument("--registry-removed-at", required=True)
    p.add_argument("--seam-dir", required=True)
    p.add_argument("--unit", default=os.environ.get("BRIDGE_SERVICE_UNIT", "multi-bridge.service"))
    a = p.parse_args(argv)
    print(json.dumps(evaluate(a.db, a.registry_removed_at, a.seam_dir, a.unit), sort_keys=True))


if __name__ == "__main__":
    main()
