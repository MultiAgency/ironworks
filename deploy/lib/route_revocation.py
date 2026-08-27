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

# THE UNIT THIS REPOSITORY SHIPS, which is `multi/serve/bridge.service` — installed under that
# name (`multi/serve/README.md`: `systemctl enable bridge.service`). This defaulted to
# `multi-bridge.service`, a name that exists nowhere in the tree: its siblings are
# `multi-backup.service` and `multi-watchdog.service`, so the prefix looked right and was not.
# `systemctl show` exits 0 for a unit it does not know, reporting `LoadState=not-found`, so the
# mismatch surfaced as "service unit is not authoritatively loaded" — UNKNOWN on every run,
# deprovision unable to converge on the host it was written for, and an operator sent to inspect
# a systemd that was perfectly healthy. `test_route_revocation` ties this to the shipped file, so
# renaming one without the other fails in CI instead of on a host.
DEFAULT_UNIT = "bridge.service"


def _command(args):
    # LC_ALL=C: `ps -o lstart=` renders month and day names in the caller's locale, and
    # `_process_started_at` parses them with English format codes. Under, say, LC_TIME=de_DE the
    # parse fails, the result is UNKNOWN, and a deprovision can never converge — fail-safe, but
    # unusable. Pinning the locale for every subprocess here keeps machine-read output machine-
    # readable; nothing in this module wants localised text.
    try:
        p = subprocess.run(args, capture_output=True, text=True, timeout=10,
                           env={**os.environ, "LC_ALL": "C"})
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


def _bridge_record(path, seam_dir):
    """((pid, started_at_iso), "") for the process the bridge recorded, or (None, why).

    `started_at` travels with the pid because a pid ALONE cannot say whose it is. The bridge
    compare-and-clears the pid on a clean stop, so a lingering one means an unclean exit — and on
    a busy host that number is eventually handed to something unrelated. Carrying the start time
    the bridge recorded for itself is what separates "the process that wrote this record" from
    "a process that inherited its number".
    """
    if not pathlib.Path(path).is_file():
        return None, "bridge state database is absent, so its process identity is unmeasured"
    sys.path.insert(0, str(seam_dir))
    try:
        import bridge_state as bs
        st = bs.BridgeState(path, migrate=False)
        try:
            snapshot = st.progress_snapshot() or {}
            raw, started = snapshot.get("pid"), snapshot.get("started_at")
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
    return (pid, started), ""


def _bridge_pid(path, seam_dir):
    record, why = _bridge_record(path, seam_dir)
    return (record[0] if record else None), why


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


def _serving_pid(service, unit):
    """(pid, terminal document). Exactly one is non-None."""
    load, active, raw_main = service["LoadState"], service["ActiveState"], service["MainPID"]
    try:
        main_pid = int(raw_main)
    except ValueError:
        return None, {"state": UNKNOWN, "reason": f"service MainPID is malformed ({raw_main!r})"}
    if load != "loaded":
        # NAME THE OVERRIDE. `render-bridge-service.py --output` installs this unit wherever the
        # operator says, so a host that named it something else is a supported configuration,
        # not a fault — and `LoadState=not-found` alone reads as a broken unit rather than a
        # unit this command was never told about. A refusal in this tree names its own fix.
        return None, {"state": UNKNOWN,
                      "reason": f"systemd does not have {unit!r} loaded (LoadState={load}). If "
                                "this host installed the bridge under another unit name, set "
                                "BRIDGE_SERVICE_UNIT to it; otherwise the unit is not installed."}
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


# A bridge records its own start time only after loading the registry, verifying every member
# and resolving account scopes — network work that can take a while — so the process is always
# older than the record. This is the slack in the other direction: a process that began well
# AFTER the record was written did not write it.
_RECORD_SLACK_SECONDS = 60


def _is_the_recorded_process(pid, started_iso):
    """(verdict, why) — is the process at `pid` the one that wrote this record?

    `True` it is, `False` the number was reused, `None` cannot tell. `None` is not "no": an
    unmeasurable process must not be read as a stopped one.
    """
    if not started_iso:
        return None, "the bridge recorded no start time to identify its process by"
    try:
        recorded = datetime.datetime.fromisoformat(started_iso).timestamp()
    except (TypeError, ValueError):
        return None, f"the recorded bridge start time is malformed ({started_iso!r})"
    actual, why = _process_started_at(pid)
    if actual is None:
        return None, why
    if actual > recorded + _RECORD_SLACK_SECONDS:
        return False, (f"PID {pid} began at least "
                       f"{int(actual - recorded)}s after the bridge recorded its own start, so "
                       "the number was reused by an unrelated process")
    return True, ""


def _contradicted_by_a_live_recorded_pid(db_path, seam_dir):
    """"" if nothing contradicts systemd's "stopped"; else why the claim cannot be trusted.

    THE ONE BRANCH THAT GRANTS SUCCESS MAY NOT REST ON A SINGLE WITNESS. `_serving_pid` returns
    ABSENT on systemd's word alone, and a bridge started outside that unit — by hand for
    debugging, or under a different unit name — is invisible to it while still holding every
    route in memory. The store's recorded pid is the only evidence independent of systemd, and
    it was being skipped in exactly the branch that reports the group unroutable.

    But a live pid is not by itself a live BRIDGE. The pid is cleared on a clean stop, so one
    that lingers means an unclean exit, and that number is eventually reused. Claiming "a bridge
    outside this unit still holds the route" on a reused pid is a false statement that sends an
    operator hunting a process that does not exist, and blocks a deprovision that had in fact
    converged. So identity is established before the contradiction is raised.

    An absent store or an absent pid is not a contradiction: nothing has claimed to be running.
    A pid that cannot be identified IS one — unmeasured must not read as stopped.
    """
    record, _why = _bridge_record(db_path, seam_dir)
    if record is None:
        return ""
    recorded_pid, started_iso = record
    alive, why = _kill_probe(recorded_pid)
    if alive is False:
        return ""
    if alive is None:
        return f"bridge-state PID {recorded_pid} could not be probed: {why}"
    same, why = _is_the_recorded_process(recorded_pid, started_iso)
    if same is False:
        return ""                       # a stale number wearing someone else's process
    if same is None:
        return (f"bridge-state PID {recorded_pid} is running but could not be identified as the "
                f"recorded bridge: {why}")
    return (f"systemd reports the unit stopped, but the bridge that recorded PID {recorded_pid} "
            "is still running — a bridge outside this unit still holds the route")


def evaluate(db_path, registry_removed_at, seam_dir, unit=DEFAULT_UNIT):
    """Return a state document. Only ABSENT permits lifecycle success."""
    service, why = _service_state(unit)
    if service is None:
        return {"state": UNKNOWN, "reason": why}
    main_pid, terminal = _serving_pid(service, unit)
    if terminal is not None:
        if terminal["state"] == ABSENT:
            contradiction = _contradicted_by_a_live_recorded_pid(db_path, seam_dir)
            if contradiction:
                return {"state": UNKNOWN, "reason": contradiction}
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
    p.add_argument("--unit", default=os.environ.get("BRIDGE_SERVICE_UNIT", DEFAULT_UNIT))
    a = p.parse_args(argv)
    print(json.dumps(evaluate(a.db, a.registry_removed_at, a.seam_dir, a.unit), sort_keys=True))


if __name__ == "__main__":
    main()
