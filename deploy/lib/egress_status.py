"""Is the egress boundary actually enforcing, right now?

DERIVED FROM THE RUNNING CONTAINER, NEVER FROM FILES. A compose overlay on disk proves an
intention; `docker inspect` on the live runtime proves a fact. Those are different, and the
difference is exactly where a security control quietly stops existing — someone edits the
overlay, forgets the recreate, and every report goes green against a container that still has a
default route. So nothing here reads the overlay to decide state.

FOUR STATES, and they are not a severity scale — they answer different questions:

  RUNNING      the runtime is on internal-only networks and a gateway is up. Structurally
               contained, but nobody has proved traffic actually behaves.
  VERIFIED     RUNNING, plus deploy/egress/probe-egress.sh passed against THIS image recently.
               The only state that may be described as containment.
  FAILED       the runtime is reachable but NOT contained — it holds a routed network, or the
               gateway is down while it still has a way out. An active tenant here is serving
               without the boundary.
  BLOCKED      cannot be evaluated (no docker, no such container). Never a pass.

There was a fifth, CONFIGURED, and it is gone rather than unreached. It described the OVERLAY,
and this module's first rule is that state comes from the container and never from a file — so
`evaluate` could not return it by construction, and never did. It survived only in this list and
in the console's legend, which together promised the operator a state the boundary cannot be in.
Whether the overlay parses is still reported, as the `overlay_configured` FACT beside the state
(see the function of that name) — which is what it always was.

A verification stamp is bound to the image id, so a pin bump invalidates it automatically: the
tool taxonomy and the HTTP client can both change across revs, and an inherited VERIFIED would
be the most dangerous kind of stale.
"""
import datetime
import hashlib
import json
import os
import pathlib
import re
import subprocess
import time

from agency_paths import agency_dir
from compose_env import placeholder_env
from private_state import write_private

RUNNING, VERIFIED, FAILED, BLOCKED = "RUNNING", "VERIFIED", "FAILED", "BLOCKED"

# How long a passing probe counts for. Long enough that an operator is not re-running it
# hourly; short enough that "verified" never means "verified last quarter".
STAMP_MAX_AGE_SECONDS = int(os.environ.get("EGRESS_STAMP_MAX_AGE", str(30 * 24 * 3600)))


def stamp_path():
    return pathlib.Path(os.environ.get("EGRESS_STAMP")
                        or agency_dir("egress-verified.json"))


def _docker(*args):
    try:
        p = subprocess.run(["docker", *args], capture_output=True, text=True, timeout=20)
        return p.returncode, p.stdout.strip(), p.stderr.strip()
    except (OSError, subprocess.TimeoutExpired) as e:
        return 1, "", type(e).__name__


def read_stamp():
    try:
        return json.loads(stamp_path().read_text())
    except (OSError, ValueError):
        return {}


def _record(p, doc):
    """Publish an operator state file atomically and privately, returning the document written.

    This function used to argue for fchmod-then-write in its docstring and do the opposite in
    its body, alone among the three operator state writers. It now calls the shared one, where
    the argument is made once and a test watches the temp file mid-write rather than the
    published mode — the assertion that a write-then-chmod also satisfies."""
    write_private(p, doc)
    return doc


def write_stamp(container, image_id, allow, checks_passed, gateway_image=None):
    """Recorded only by a PASSING probe run. Holds no secret — a container name, image ids, the
    allowlist, a count, and a hash of the proof definition that earned it.

    `proof_fingerprint` is computed HERE rather than passed in, so a caller cannot stamp without
    one. A stamp is evidence about a procedure; recording which procedure is what makes it
    re-checkable later.
    """
    gateway = os.environ.get("EGRESS_GATEWAY", "multi-egress-1")
    topology, why = current_topology(container, gateway)
    if topology is None or not topology["shared"]:
        raise RuntimeError("cannot stamp an unverifiable egress topology: " +
                           (why or "runtime and gateway share no network"))
    return _record(stamp_path(),
                   {"container": container, "image_id": image_id, "allow": allow,
                    "checks_passed": int(checks_passed),
                    "gateway_image": gateway_image or "",
                    "topology": topology,
                    "proof_contract": PROOF_CONTRACT,
                    "proof_fingerprint": proof_fingerprint(gateway_image),
                    "at": int(time.time()),
                    "at_iso": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())})


def inspect(container):
    """Networks, image id, running flag and current start epoch for a container.

    `running` is the third value because EXISTING IS NOT RUNNING, and this module's callers had
    been reading one as the other. `docker inspect` on an EXITED container still exits 0 with
    `NetworkSettings.Networks` fully populated, so a stopped gateway looked up and a stopped
    runtime looked contained — the two FAILED cases the docstring at the top of this file
    defines, both reported OK."""
    rc, out, _ = _docker("inspect", container)
    if rc != 0 or not out:
        return None, None, False, None
    try:
        doc = json.loads(out)[0]
    except (ValueError, IndexError, KeyError):
        return None, None, False, None
    nets = list((doc.get("NetworkSettings", {}).get("Networks") or {}).keys())
    state = doc.get("State") or {}
    return nets, doc.get("Image"), bool(state.get("Running")), state.get("StartedAt")


def network_metadata(name):
    rc, out, _ = _docker("network", "inspect", name)
    if rc != 0 or not out:
        return None
    try:
        doc = json.loads(out)[0]
        network_id = doc.get("Id")
        internal = doc.get("Internal")
        if not network_id or not isinstance(internal, bool):
            return None
        return {"name": name, "id": network_id, "internal": internal}
    except (ValueError, IndexError, KeyError):
        return None


def network_is_internal(name):
    metadata = network_metadata(name)
    return None if metadata is None else metadata["internal"]


def current_topology(container, gateway):
    """Normalized complete network relationship whose equivalence a stamp certifies."""
    runtime_nets, _image, runtime_up, _started = inspect(container)
    gateway_nets, _gw_image, gateway_up, _gw_started = inspect(gateway)
    if runtime_nets is None or gateway_nets is None or not runtime_up or not gateway_up:
        return None, "runtime or gateway topology is unavailable or not running"

    def describe(role, names):
        result = []
        for name in sorted(names):
            metadata = network_metadata(name)
            if metadata is None:
                return None, f"{role} network metadata is unreadable for {name!r}"
            result.append(metadata)
        return result, ""

    runtime, why = describe("runtime", runtime_nets)
    if runtime is None:
        return None, why
    gateway_doc, why = describe("gateway", gateway_nets)
    if gateway_doc is None:
        return None, why
    return {"runtime": runtime, "gateway": gateway_doc,
            "shared": sorted(set(runtime_nets) & set(gateway_nets))}, ""


# WHAT A PASS MEANS. Bump when the MEANING changes — a leg added or removed, a verdict
# reclassified, the counting rule altered — so that stamps earned under the old meaning stop
# certifying the new one. It is part of the fingerprint below, not a separate check, because an
# operator should never have to reason about which of the two moved.
PROOF_CONTRACT = 1

# The files that DEFINE the proof: what the gateway enforces, which destinations are asserted,
# how a leg is classified, and how the run decides PASS. A stamp is evidence about a procedure,
# so it is only evidence while the procedure is the same one.
_PROOF_INPUTS = (
    "deploy/egress/connect-proxy.py",           # what the boundary actually enforces
    "deploy/egress/forbidden-destinations.json",  # what must not be reachable
    "deploy/egress/probe_attempts.py",          # discriminating / corroborating / unmeasurable
    "deploy/egress/probe_contained.py",         # the contained leg, and what it counts
    "deploy/egress/probe-egress.sh",            # what is asserted, and how the run decides PASS
)

_REPO = pathlib.Path(__file__).resolve().parents[2]


def _sha256_of(path):
    try:
        return hashlib.sha256(pathlib.Path(path).read_bytes()).hexdigest()
    except OSError:
        return None


def proof_fingerprint(gateway_image=None, root=None):
    """One hash over the material proof definition. `None` if an input cannot be read.

    THE STAMP WAS BOUND TO THE RUNTIME IMAGE AND NOTHING ELSE, so it survived every change to the
    proof itself. Measured: a stamp recording `checks_passed: 0`, written by a probe
    implementation that no longer exists and against a destination manifest that has since gained
    and lost entries, still evaluated VERIFIED. "An inherited VERIFIED is the most dangerous kind
    of stale" was already this module's rule; it just had no way to see this kind of staleness.

    The enforced allowlist is deliberately NOT folded in here. `evaluate` compares it separately
    because a hash can only say "something changed", and for the one input an operator edits on
    purpose it is worth being able to say WHICH destination was added.

    Returning None rather than a hash of partial input matters: an unreadable proof definition is
    unevaluated, and the caller must refuse VERIFIED rather than compare against a guess.
    """
    root = pathlib.Path(root or _REPO)
    h = hashlib.sha256()
    h.update(f"contract={PROOF_CONTRACT}\n".encode())
    for rel in _PROOF_INPUTS:
        digest = _sha256_of(root / rel)
        if digest is None:
            return None
        h.update(f"{rel}={digest}\n".encode())
    h.update(f"gateway_image={gateway_image or ''}\n".encode())
    return h.hexdigest()


_COMPOSE_PREFIX = re.compile(r"^[^|]*\| ")

DECISION_VERBS = ("allow", "deny", "fail")


def decision_lines(logs):
    """The gateway's CONNECT decisions, from raw `docker compose logs gw` output.

    `docker compose logs` prefixes every line with the service and a pipe — `gw-1  | allow
    cloud-api.near.ai:443` — so the first token of a RAW line is the container name, never the
    verb. A reader that split the raw line found no decisions at all and, because it then asked
    `any(...)` of an empty list, reported "no decisions" as False rather than as unmeasurable.

    Lives here rather than in the proof script so it can be tested without docker: it decides
    whether the leak checks are reading the log of the gateway that served the run, and a check
    that cannot pass is indistinguishable from one that is merely failing.
    """
    return [line for line in decision_lines_source(logs)
            if line.split(" ", 1)[0].strip().lower() in DECISION_VERBS]


_IMPL_LINE = re.compile(r"impl sha256=([0-9a-f]{64})")


def _timestamp(value):
    try:
        text = str(value).strip()
        if text.endswith("Z"):
            text = text[:-1] + "+00:00"
        return datetime.datetime.fromisoformat(text).timestamp()
    except (TypeError, ValueError):
        return None


def gateway_implementation(gateway):
    """The sha256 the RUNNING gateway announced for its own source, or None if unreadable.

    The gateway runs a generic pinned base image with `connect-proxy.py` bind-mounted, so its
    image id does not move when the implementation does, and a container keeps serving the bytes
    it started with. Hashing the repository file therefore says what the proxy SHOULD be, never
    what it IS. This is the other half: the process reports what it loaded, and `evaluate`
    refuses to certify a boundary whose proxy is running something else.

    None and a mismatch are different answers and the caller treats them differently, but neither
    is VERIFIED.
    """
    # BOTH STREAMS: the proxy prints to stdout, but `docker logs` splits them and a container
    # started before this banner existed has neither. Searching one only would report "no
    # identity" for a gateway that announced one.
    started_raw = inspect(gateway)[3]
    started = _timestamp(started_raw)
    if started is None:
        return None
    rc, out, err = _docker("logs", "--timestamps", gateway)
    if rc != 0:
        return None
    found = []
    for order, line in enumerate(decision_lines_source(out) + decision_lines_source(err)):
        m = _IMPL_LINE.search(line)
        logged = _timestamp(line.split(None, 1)[0] if line.split() else "")
        if m and logged is not None and logged >= started:
            found.append((logged, order, m.group(1)))
    # A restart during the log read invalidates the epoch we filtered against. Do not bind an
    # identity until one complete read describes one current process generation.
    if inspect(gateway)[3] != started_raw:
        return None
    return max(found)[2] if found else None


def decision_lines_source(logs):
    """Every log line with the compose prefix stripped — the shape both readers need."""
    return [_COMPOSE_PREFIX.sub("", raw) for raw in (logs or "").splitlines()]


def normalize_allow(raw):
    """An `EGRESS_ALLOW` string as the set the proxy actually enforces.

    Exactly `connect-proxy.py`'s own parse — comma split, strip, lowercase, discard empties — so
    that a reordering or a stray space is not reported as a changed boundary, and a genuinely
    added destination is. Comparing raw strings would do the opposite of both.
    """
    return frozenset(a.strip().lower() for a in (raw or "").split(",") if a.strip())


def gateway_allowlist(gateway):
    """The gateway's LIVE `EGRESS_ALLOW`, or None if it could not be read.

    None and empty-string are different answers and the caller must not merge them: a gateway
    that cannot be inspected is unmeasured, while a gateway with no allowlist cannot have
    started (`connect-proxy.py` refuses an empty list). Read from the container's environment
    for this module's founding reason — the overlay on disk proves an intention, `docker
    inspect` proves what the proxy is enforcing.
    """
    rc, out, _ = _docker("inspect", gateway, "-f",
                         "{{range .Config.Env}}{{println .}}{{end}}")
    if rc != 0:
        return None
    for line in out.splitlines():
        if line.startswith("EGRESS_ALLOW="):
            return line[len("EGRESS_ALLOW="):].strip()
    return None


def degraded_mark_path():
    """Where a deliberate rollback is recorded. The WRITER (egress-control.sh) and the READER
    (`ironworks doctor`, via `evaluate`) must derive this identically — when they did not, a
    rollback was written to one path, looked for at another, and the boundary went on reporting
    VERIFIED. One function, asked by both."""
    return pathlib.Path(os.environ.get("EGRESS_DEGRADED_MARK")
                        or agency_dir("egress-degraded.json"))


def degraded_mark():
    """A deliberate rollback recorded as a state, not just a log line. A degraded security
    posture nobody is tracking is the same as one nobody knows about."""
    try:
        return json.loads(degraded_mark_path().read_text())
    except (OSError, ValueError):
        return {}


def write_degraded_mark(container, note):
    """Record that the boundary was deliberately removed. Written by egress-control.sh."""
    return _record(degraded_mark_path(),
                   {"state": "EGRESS_ROLLED_BACK", "container": container, "note": note,
                    "at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())})


def _stamp_still_certifies(stamp, gateway, container):
    """"" if this stamp still certifies the boundary now running; else why it does not.

    Two questions, in order, and the order is the argument. FIRST: is the PROOF still the one
    that ran? A stamp is evidence about a procedure — change the gateway implementation, the
    forbidden destinations, or how a leg is counted, and the old result attests to a procedure
    that no longer exists. Measured: a stamp recording `checks_passed: 0`, written before the
    probe gained an uncontained control and before the destination manifest was corrected, read
    VERIFIED against the corrected tree. SECOND, and only then: does the recorded allowlist still
    match the gateway's? Asking that first would compare a list whose meaning had already moved.

    Kept out of `evaluate` because it is the one judgement here with more than two inputs, and
    inline it grew an `if why: pass`.
    """
    topology, topology_why = current_topology(container, gateway)
    certified = stamp.get("topology")
    if topology is None:
        return "the current containment topology is unverifiable: " + topology_why
    if not certified:
        return ("the verification stamp predates topology certification; re-run "
                "deploy/egress/probe-egress.sh")
    if topology != certified:
        return ("the material runtime/gateway network topology has CHANGED since it was proved "
                f"(certified={certified!r}, current={topology!r}); re-run the egress proof")
    if not topology["shared"]:
        return "the runtime and gateway share no network, so the gateway cannot contain egress"

    gw_image = inspect(gateway)[1]
    want, have = proof_fingerprint(gw_image), stamp.get("proof_fingerprint")
    if want is None:
        return ("the proof definition under deploy/egress/ could not be read, so this stamp "
                "cannot be checked against the procedure that earned it")
    if not have:
        return ("the verification stamp predates proof fingerprinting — it records no proof "
                "definition, so nothing can confirm it was earned under the checks running now. "
                "Re-run deploy/egress/probe-egress.sh")
    if have != want:
        return ("the EGRESS PROOF ITSELF has changed since this stamp was written — the gateway "
                "implementation, the forbidden destinations, the probe protocol, or the gateway "
                "image. The stamp certifies a procedure that is no longer the one in this tree. "
                "Re-run deploy/egress/probe-egress.sh")

    # ...AND THE PROXY MUST BE RUNNING THE BYTES THAT WERE HASHED. The gateway is a generic
    # pinned base image with `connect-proxy.py` bind-mounted, so its image id does not move when
    # the implementation does and the container keeps serving what it started with. Edit the file,
    # skip the recreate, re-run the probe: everything above passes and the stamp records the NEW
    # hash while the OLD proxy is what enforces the boundary. Compared live, like the allowlist,
    # and for the same reason — this is a fact about the running system, not a definition.
    running = gateway_implementation(gateway)
    on_disk = _sha256_of(_REPO / "deploy/egress/connect-proxy.py")
    if running is None:
        return ("the gateway did not report the implementation it loaded, so it cannot be shown "
                "to be running the proxy this stamp certifies. A container started before the "
                "gateway announced its own identity will not report one: recreate the egress "
                "service, then re-run deploy/egress/probe-egress.sh")
    if on_disk is None or running != on_disk:
        # REPORTS, not IS. This value is the gateway's own account of the bytes it loaded, and
        # the wording has to say so — a compromised proxy would simply report the matching hash.
        # That is not the control that stops one: a gateway executing attacker code has already
        # defeated the boundary, and what stands against that is the measured legs and
        # `internal: true`. What this DOES catch is the realistic case — an edited file and a
        # container never recreated — which nothing outside the process can observe, because the
        # bind mount makes `docker cp`/`exec` resolve to the current host file rather than to
        # the bytes in memory.
        return ("the gateway REPORTS a different connect-proxy.py than this tree holds "
                f"(reports {running[:12]}…, on disk {(on_disk or 'unreadable')[:12]}…). The "
                "container is most likely still serving the bytes it started with — recreate the "
                "egress service so it loads this implementation, then re-run "
                "deploy/egress/probe-egress.sh")

    # THE ALLOWLIST IS PART OF WHAT WAS PROVED, so a change to it invalidates the proof exactly as
    # a rebuild does. This field was written and never read: `probe-egress.sh` goes to some length
    # to record the list the GATEWAY enforces rather than the operator's environment, and nothing
    # compared it afterwards. Adding a destination then recreating only the `egress` service
    # leaves the runtime's image id untouched, so every check above passed and `doctor` reported
    # VERIFIED against a stamp naming a narrower boundary than the one now running.
    live = gateway_allowlist(gateway)
    if live is None:
        return (f"the gateway's live allowlist could not be read from {gateway!r}, so the stamp "
                "cannot be confirmed to describe the boundary now running. Structurally "
                "contained, but not re-provable without it.")
    now, then = normalize_allow(live), normalize_allow(stamp.get("allow"))
    if now != then:
        added, removed = sorted(now - then), sorted(then - now)
        return ("the gateway's allowlist has CHANGED since it was proved" +
                (f" (now also permits: {', '.join(added)})" if added else "") +
                (f" (no longer permits: {', '.join(removed)})" if removed else "") +
                " — containment must be re-proved against the boundary actually running. "
                "Run deploy/egress/probe-egress.sh")
    return ""


def evaluate(container, gateway=None):
    """The state of the boundary around `container`, with the reasons that decided it."""
    gateway = gateway or os.environ.get("EGRESS_GATEWAY", "multi-egress-1")
    rc, _, _ = _docker("version")
    if rc != 0:
        return {"state": BLOCKED, "why": ["docker is not available on this host"],
                "container": container}

    nets, image_id, running, _started = inspect(container)
    if nets is None:
        return {"state": BLOCKED, "why": [f"no such container: {container}"],
                "container": container}
    if not running:
        return {"state": BLOCKED,
                "why": [f"{container} exists but is not running — its recorded networks are the "
                        "ones it had when it stopped, so nothing about the live boundary was "
                        "measured. Start it and re-run."],
                "container": container}

    why, routed = [], []
    for n in nets:
        internal = network_is_internal(n)
        if internal is not True:
            routed.append(n + ("" if internal is False else " (unreadable)"))
    _gw_nets, _gw_image, gw_up, _gw_started = inspect(gateway)

    if routed:
        why.append(f"the runtime is attached to routed network(s): {', '.join(routed)} — it has "
                   "a default route, so there is no network boundary regardless of what the "
                   "overlay says")
        state = FAILED
    elif not gw_up:
        why.append(f"the runtime is internal-only but the egress gateway ({gateway}) is not "
                   "running — the boundary is intact and the product cannot reach its model "
                   "provider. This is fail-CLOSED, not fail-open.")
        state = FAILED
    else:
        state = RUNNING

    stamp = read_stamp()
    if state == RUNNING:
        age = time.time() - (stamp.get("at") or 0)
        if not stamp:
            why.append("no verification stamp — structurally contained, but no probe has proved "
                       "traffic behaves. Run deploy/egress/probe-egress.sh")
        elif stamp.get("container") != container:
            why.append(f"the verification stamp is for container {stamp.get('container')!r}")
        elif image_id and stamp.get("image_id") != image_id:
            why.append("the verification stamp is for a DIFFERENT image — the runtime has been "
                       "rebuilt or the pin bumped, and containment must be re-proved")
        elif age > STAMP_MAX_AGE_SECONDS:
            why.append(f"the verification stamp is {age / 86400:.0f} days old "
                       f"(max {STAMP_MAX_AGE_SECONDS / 86400:.0f})")
        else:
            reason = _stamp_still_certifies(stamp, gateway, container)
            if reason:
                why.append(reason)
            else:
                state = VERIFIED

    mark = degraded_mark()
    if mark:
        why.append(f"the boundary was deliberately ROLLED BACK at {mark.get('at')} — this host "
                   "is knowingly running without network containment")
        state = FAILED

    return {"state": state, "why": why, "container": container, "gateway": gateway,
            "deliberately_rolled_back": bool(mark),
            "networks": nets, "routed_networks": routed, "gateway_running": gw_up,
            "image_id": image_id, "stamp": stamp}


def overlay_configured(repo_root):
    """Does the overlay exist and parse? This is about INTENT, and it is deliberately not a
    state — a file cannot activate a security control. Reported beside the state, never as one."""
    overlay = pathlib.Path(repo_root) / "deploy" / "egress" / "docker-compose.egress.yml"
    base = pathlib.Path(repo_root) / "multi" / "instance" / "docker-compose.yml"
    if not overlay.is_file():
        return False, f"{overlay} is missing"
    # DERIVED from the two files being validated, not listed here. The hand-written set this
    # replaces named five variables where `run-quality.py`'s named nine, so a new `${VAR:?}` in
    # the overlay would have made this report the boundary UNCONFIGURED while the quality gate
    # went on passing.
    env = placeholder_env(base, overlay, base=os.environ)
    try:
        p = subprocess.run(["docker", "compose", "-f", str(base), "-f", str(overlay),
                            "config", "-q"], capture_output=True, text=True, timeout=60, env=env)
    except (OSError, subprocess.TimeoutExpired) as e:
        return False, f"could not validate the overlay: {type(e).__name__}"
    return (p.returncode == 0), (p.stderr.strip()[:200] if p.returncode else "")
