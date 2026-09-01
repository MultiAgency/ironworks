#!/usr/bin/env python3
"""A CONNECT-only forward proxy that allows exactly the destinations you name.

This is the egress gateway for the IronClaw runtime. The runtime itself sits on a Docker
network with `internal: true`, which removes its default route entirely — measured, not
assumed: from such a network, a public hostname, a literal public IP and the cloud-metadata
address all fail at the transport, and they fail whether or not any tool is enabled. This
process is then the only way out, and it forwards to nothing except the hosts in its allowlist.

WHY NOT SQUID / TINYPROXY. Because the allowlist is the whole security property, and it should
be forty lines someone can read in a code review rather than a config dialect layered over a
general-purpose cache. This has no cache, no ACL language, no rewriting, and no HTTP proxying
at all — only CONNECT, only to an exact `host:port` in the allowlist, only TCP relay after
that. TLS is end-to-end to the real origin: this never terminates it, never sees a byte of
plaintext, and needs no certificate.

WHAT THE ALLOWLIST THEREFORE DOES AND DOES NOT CONSTRAIN. It is matched against the CONNECT
REQUEST LINE — the target a client asks for — and nothing after the `200`. Once the relay starts
this cannot see, and does not look at, the ClientHello: a permitted tunnel may carry any SNI the
client chooses, and reaching a different origin that way depends only on whether the allowed
host's address also fronts one. The DNS lookup is likewise THIS process's, against the host
resolver, unpinned.

That is deliberate, not an omission to close later. Reading SNI means parsing TLS here, and
enforcing it means terminating TLS here — which would move the one component that currently sits
OUTSIDE the trust boundary inside it, holding plaintext and a CA, to police a one-entry
allowlist. `docs/EGRESS_CONTAINMENT.md` § Residual risk states the scope and names the two
triggers for revisiting it: a second allowlist entry, or a provider host on shared edge
infrastructure. Do not "fix" this without changing that document first.

WHY CONNECT AND NOT A DNS ALIAS. A network alias pointing `cloud-api.near.ai` at a relay makes
the runtime resolve the provider host to a PRIVATE address, and the runtime's own provider
policy classifies addresses after resolution. Naming the proxy explicitly keeps the provider
host resolving to what it really is and puts the decision in one readable place.

THE ALLOWLIST IS EXACT HOST:PORT. No wildcards, no suffix matching. `evil-cloud-api.near.ai`
and `cloud-api.near.ai.attacker.example` are both refused by a suffix rule someone will
eventually write; they are refused here because they are not the string in the list.

Env:
  EGRESS_ALLOW   comma-separated host:port list (required, non-empty, every entry ported)
  EGRESS_PORT    listen port (default 3128)
  EGRESS_HOST    listen address (default 0.0.0.0 — reachable only on the internal network)

Every decision is logged: the destination and the verdict, never a request body (there is none
to see) and never a credential (CONNECT carries no Authorization to the origin).
"""
import hashlib
import os
import pathlib
import select
import socket
import socketserver
import sys


def split_hostport(entry):
    """(host, port) for `host:port` or `[v6]:port`; (None, None) if it is not that shape.

    IPv6 NEEDS ITS OWN BRANCH, and not having one was accept-then-fail rather than a refusal.
    `rpartition(":")` splits `[::1]:443` into `[::1` and `443`, which passed the startup
    validation below — so the "allow-nothing proxy wearing a working one's face" guard let the
    entry through — and then `socket.create_connection(("[::1]", 443))` raised gaierror, because
    Python does not strip the brackets. Every request to that destination 502'd forever, with a
    startup banner claiming it was allowed.

    A bare IPv6 with no brackets is rejected outright: `::1:443` is ambiguous — that trailing
    group could be a port or another hextet — and guessing is how an allowlist entry comes to
    mean something other than what its author read.
    """
    entry = entry.strip()
    if entry.startswith("["):
        host, sep, port = entry.partition("]:")
        return (host[1:], port) if sep and host[1:] else (None, None)
    host, sep, port = entry.rpartition(":")
    if not sep or not host or ":" in host:
        return None, None
    return host, port


def _ported(entry):
    """An allowlist entry must carry a port, because the lookup it feeds is an exact match."""
    host, port = split_hostport(entry)
    return bool(host) and port.isdigit() and 0 < int(port) < 65536


ALLOW = {a.strip().lower() for a in os.environ.get("EGRESS_ALLOW", "").split(",") if a.strip()}
if not ALLOW:
    raise SystemExit("!! EGRESS_ALLOW is empty — refusing to start an allow-nothing proxy that "
                     "would look like a working one. Name the destinations explicitly, e.g. "
                     "EGRESS_ALLOW=cloud-api.near.ai:443")
PORTLESS = sorted(a for a in ALLOW if not _ported(a))
if PORTLESS:
    raise SystemExit("!! EGRESS_ALLOW entr(ies) without a port: " + ", ".join(PORTLESS) +
                     " — a CONNECT target is normalised to host:port before the lookup, so a "
                     "portless entry matches nothing. That is the same allow-nothing proxy the "
                     "empty list refuses, only harder to see: it starts, it logs the destination "
                     "as allowed, and it denies every request. Name the port, e.g. "
                     "EGRESS_ALLOW=cloud-api.near.ai:443")
PORT = int(os.environ.get("EGRESS_PORT", "3128"))
HOST = os.environ.get("EGRESS_HOST", "0.0.0.0")
BUF = 65536
CONNECT_TIMEOUT = 15


def log(msg):
    print(msg, flush=True)


class Handler(socketserver.BaseRequestHandler):
    def handle(self):
        client = self.request
        client.settimeout(CONNECT_TIMEOUT)
        try:
            head = b""
            while b"\r\n\r\n" not in head and len(head) < 8192:
                chunk = client.recv(BUF)
                if not chunk:
                    return
                head += chunk
        except OSError:
            return

        # WHATEVER ARRIVED AFTER THE HEADER IS THE TUNNEL'S FIRST BYTES, NOT LEFTOVERS. `recv`
        # returns whatever the kernel has, so a client that puts its TLS ClientHello in the same
        # segment as the CONNECT — legal, and what a pipelining client does — had those bytes
        # read into `head` and dropped on the floor here. The relay then started from an empty
        # buffer, the server never saw a handshake, and the connection hung to the 300s select
        # timeout looking like an upstream problem.
        head, _, pipelined = head.partition(b"\r\n\r\n")

        line = head.split(b"\r\n", 1)[0].decode("latin-1", "replace")
        parts = line.split()
        # ONLY CONNECT. A plain-HTTP proxy request would make this a general-purpose relay,
        # which is the thing an egress boundary exists to not be.
        if len(parts) < 2 or parts[0].upper() != "CONNECT":
            log(f"deny  method={parts[0] if parts else '?'!r} — only CONNECT is served")
            self._refuse(client, "405 Method Not Allowed")
            return

        target = parts[1].strip().lower()
        if ":" not in target:
            target += ":443"
        if target not in ALLOW:
            log(f"DENY  {target} — not in the allowlist ({len(ALLOW)} entr(ies))")
            self._refuse(client, "403 Forbidden")
            return

        # Brackets are stripped for the CONNECTION and kept for the LOOKUP: the allowlist is
        # compared as text, so both sides must spell an address the same way, while
        # `create_connection` wants the bare form.
        host, port = split_hostport(target)
        try:
            upstream = socket.create_connection((host, int(port)), timeout=CONNECT_TIMEOUT)
        except OSError as e:
            log(f"fail  {target} — upstream unreachable ({type(e).__name__})")
            self._refuse(client, "502 Bad Gateway")
            return

        log(f"allow {target}")
        try:
            client.sendall(b"HTTP/1.1 200 Connection Established\r\n\r\n")
            if pipelined:
                # Before the relay, and before anything else can be read from the client: these
                # are the tunnel's first bytes and they must arrive in order.
                upstream.sendall(pipelined)
            self._relay(client, upstream)
        finally:
            try:
                upstream.close()
            except OSError:
                pass

    @staticmethod
    def _refuse(sock, status):
        try:
            sock.sendall(f"HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                         .encode())
        except OSError:
            pass

    @staticmethod
    def _relay(a, b):
        a.settimeout(None)
        b.settimeout(None)
        socks = [a, b]
        while True:
            try:
                readable, _, errored = select.select(socks, [], socks, 300)
            except (OSError, ValueError):
                return
            if errored or not readable:
                return
            for s in readable:
                other = b if s is a else a
                try:
                    data = s.recv(BUF)
                except OSError:
                    return
                if not data:
                    return
                try:
                    other.sendall(data)
                except OSError:
                    return


class Server(socketserver.ThreadingTCPServer):
    allow_reuse_address = True
    daemon_threads = True


def loaded_implementation():
    """sha256 of the bytes THIS PROCESS is running, announced at startup.

    THE FINGERPRINT COULD NOT SEE THE RUNNING PROXY. `docker-compose.egress.yml` runs the gateway
    as a generic pinned base image — `python:3.12-slim@sha256:…` — with this file BIND-MOUNTED in.
    So the gateway's image id never moves when the implementation changes, and the container keeps
    serving the bytes it started with. Edit this file, do not recreate the container, re-run the
    probe: it passes, and the stamp records the NEW file's hash while the OLD implementation is
    what actually enforced the boundary. The stamp then certifies a proxy that is not loaded.

    Emitting it here closes that: the reader compares this against the file it hashed, so a
    gateway running different bytes cannot be certified. It is a hash of source already published
    in this repository — no secret, and no process identity. Deliberately NOT the pid, container
    id or start time: a restart with byte-identical code changes none of the guarantee and must
    not force re-certification.
    """
    try:
        return hashlib.sha256(pathlib.Path(__file__).read_bytes()).hexdigest()
    except OSError:
        return ""


if __name__ == "__main__":
    log(f"impl sha256={loaded_implementation()}")
    log(f"egress gateway on {HOST}:{PORT}; CONNECT allowed to: {', '.join(sorted(ALLOW))}")
    try:
        with Server((HOST, PORT), Handler) as srv:
            srv.serve_forever()
    except KeyboardInterrupt:
        sys.exit(0)
