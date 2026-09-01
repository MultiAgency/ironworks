# Egress containment

This document owns the current network topology, precise guarantee, destination policy,
operations, and residual risk. It does not record rollout status or host inventories. The live
authority for each host is `./deploy/ironworks egress status`.

## Topology

```text
host :3020 -> ingress relay ---------> routed edge network
                 |
runtime + database on internal network
                 |
                 +-> CONNECT-only gateway -> allowed model provider
```

The runtime and database sit on a Docker network marked `internal: true`, so the runtime has no
direct routed path to the public internet. An ingress relay preserves the host-facing runtime
port. A gateway attached to both networks is the runtime's only outbound path.

## Guarantee

Only when `./deploy/ironworks egress status` reports `VERIFIED` may an operator claim:

- the runtime has no direct public route;
- outbound connections must cross the CONNECT gateway;
- the gateway admits only exact allowlisted `host:port` CONNECT targets, and relays TLS it never
  terminates — see § Residual risk for what that constrains and what it does not; and
- the boundary holds independently of which model-visible tools are enabled.

The guarantee applies to the contained runtime, not every process or container on the host. A
gateway failure is fail-closed for inference: provider access stops while the runtime remains
without a direct public route.

## Destination policy

The current runtime path uses `https://cloud-api.near.ai`; the default gateway allowlist is the
single exact destination `cloud-api.near.ai:443`. Matching is not based on prefixes, suffixes,
redirects, or wildcard domains. The gateway accepts CONNECT only and relays TLS without reading
provider authorization headers.

Changing the provider host, port, base URL, model path, or URL-fetch behavior requires policy
review and renewed verification. Do not expand the allowlist merely to make a failing probe pass.

**What the implementation identity is, and is not.** The gateway announces a hash of the
connect-proxy source it loaded, and `egress status` compares that against the file in this tree.
That is **drift detection under an honest process, not an attestation**: a compromised gateway
would report whichever hash keeps it VERIFIED. It is not the control that stops one — a gateway
executing attacker code has already defeated the boundary, and what stands against that is the
measured forbidden-destination legs and `internal: true`. What the identity does catch is the
realistic failure: the implementation is bind-mounted into a generic base image, so an edited
file with no container recreate leaves the proxy serving its original bytes while the image id
never moves. Nothing outside the process can observe that — `docker cp` and `docker exec` resolve
the mount to the current host file, not to what is in memory — so the process's own report is the
only available answer, and it is treated as a report.

The stamp's `checks_passed` counts **evidence only**. A forbidden destination is counted when an
uncontained container completes a handshake there, because only then does the same attempt failing
inside the boundary distinguish containment from its absence. Destinations that are merely
routable — a timeout, or an active refusal — are still asserted but not counted, and a run where
no forbidden destination answers an uncontained container is reported BLOCKED rather than stamped:
it has measured the gateway and nothing else.

Renewed verification is **enforced, not merely expected**. The verification stamp records the
allowlist as the running gateway enforces it, and `egress status` compares that recording against
the gateway's live `EGRESS_ALLOW` on every evaluation: a destination added or removed drops the
state from VERIFIED back to RUNNING until `probe-egress.sh` has been re-run. Widening the
allowlist does not change the runtime's image, so the image binding alone did not catch it. If
the gateway's environment cannot be read, the state is likewise not VERIFIED — an allowlist
nothing could check is not one that was proved.

## Verify, apply, and roll back

```sh
./deploy/ironworks egress status
./deploy/egress/egress-control.sh verify
./deploy/egress/egress-control.sh activate --confirm
./deploy/egress/egress-control.sh rollback --i-accept-unrestricted-egress
```

Activation recreates the runtime container. Gracefully stop the bridge first, activate the
boundary, require runtime health and containment verification, run the applicable isolation
proofs, and only then restart traffic. The operator runbook in
[`deploy/README.md`](../deploy/README.md) owns maintenance and recovery sequencing.

Rollback deliberately removes the network boundary and returns the runtime to unrestricted
egress. Its confirmation flag is an explicit acceptance of that degraded state. Verify and
record the resulting status before serving traffic.

For a disposable proof of the topology and service path, run:

```sh
./deploy/egress/proof/run-proof.sh --service-path
```

## Residual risk

The allowed model provider remains an intentional data sink: a prompt-injected turn could encode
tenant context in traffic to that provider. Containment prevents arbitrary destinations; it does
not make inference private from the selected provider, validate provider retention claims, or
replace organization scoping, token custody, redaction, and per-bearer tool confinement.

**The allowlist is enforced on the CONNECT request line, not on the traffic.** The gateway matches
the target a client *asks* for, answers `200`, and from then on relays bytes — it never terminates
TLS and never reads the ClientHello. So the guarantee constrains which host the runtime may open a
tunnel to; it does not constrain what the runtime then speaks inside that tunnel. Two consequences
follow, and neither is closed by the allowlist:

- a client that opens a permitted tunnel may send a ClientHello naming a **different** server. Whether
  that reaches anything depends on whether the allowed host's address also fronts other origins, as
  shared CDN or edge infrastructure commonly does;
- the **gateway** performs the DNS lookup, against the host's resolver, with no pinning. Poisoning or
  rebinding that name redirects the tunnel. (The contained runtime has no resolver path out at all —
  measured, `deploy/egress/proof/proof_checks.py` §14 — so this is the gateway's resolution, not
  the client's.)

This is the design working as intended rather than a defect to fix. Inspecting SNI would mean
parsing TLS in the one component deliberately built never to look inside it, and terminating TLS
would put the component that currently sits outside the trust boundary inside it, holding
plaintext and a certificate authority. Both are worse trades for a single-entry allowlist. The
exposure is also narrow and sits behind two other controls: reaching it requires code execution
inside the contained runtime, where per-bearer confinement has already disabled the egress tools,
and traffic to the provider itself is an accepted sink above.

**Revisit when either trigger fires:** the allowlist gains a second entry, or the provider host is
found to resolve to shared, multi-tenant edge infrastructure. Until then this is stated, not
mitigated.
