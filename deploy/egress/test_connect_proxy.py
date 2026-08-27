#!/usr/bin/env python3
"""The egress gateway's allowlist, driven over real sockets. Offline, stdlib only, no docker.

Run: python3 deploy/egress/test_connect_proxy.py

connect-proxy.py is the network guarantee SECURITY.md makes, and until this file existed its
only coverage was the BYPASS block of deploy/egress/proof/proof_checks.py — which needs docker,
a pinned IronClaw image and a live provider key, so it runs by hand and never in CI. That is the
wrong amount of friction for the forty lines the whole boundary rests on.

NOTHING HERE IS STUBBED. The allowlist is the entire security property, so a test that patched
the membership check would be testing its own patch. This starts the real script in its own
process (it reads EGRESS_ALLOW at import and refuses bad ones with SystemExit, so a process is
also the only honest way to reach the startup guards), points the allowlist at a local sink, and
speaks HTTP by hand. The status line the gateway returns IS the verdict.

The allowlist is deliberately two entries: the sink, which can actually be reached, and
`cloud-api.near.ai:443`, which cannot. The second one is there so the lookalike cases assert
what the module docstring argues — that `evil-cloud-api.near.ai` and
`cloud-api.near.ai.attacker.example` are refused against a list that really does contain
`cloud-api.near.ai` — rather than the vacuous fact that an unrelated host is not in an unrelated
list. Neither lookalike ever reaches a connect(), so no test here touches the network.
"""
import os
import pathlib
import socket
import socketserver
import subprocess
import sys
import threading
import time
import unittest

PROXY = pathlib.Path(__file__).resolve().parent / "connect-proxy.py"
PROVIDER = "cloud-api.near.ai:443"


def free_port():
    """A port nothing is listening on. Used both to place the gateway and, unbound, as a
    destination that is refused for its PORT rather than its host."""
    s = socket.socket()
    s.bind(("127.0.0.1", 0))
    port = s.getsockname()[1]
    s.close()
    return port


class SinkHandler(socketserver.BaseRequestHandler):
    def handle(self):
        self.server.reached += 1
        try:
            # ANNOUNCE FIRST, THEN LISTEN. The order is load-bearing for the existing relay test,
            # which connects and expects bytes back without ever sending any — reading first
            # would block it until the timeout.
            self.request.sendall(b"SINK\n")
        except OSError:
            pass
        # What the client sent through the tunnel, so a test can assert that bytes pipelined with
        # the CONNECT actually ARRIVED rather than inferring it from a status code.
        try:
            self.request.settimeout(2)
            self.server.received += self.request.recv(4096)
        except OSError:
            pass


def start_sink():
    """The one destination that legitimately exists: it announces itself, and it counts. The
    count is what makes "never relayed" an assertion instead of an inference from a status code."""
    srv = socketserver.ThreadingTCPServer(("127.0.0.1", 0), SinkHandler)
    srv.daemon_threads = True
    srv.reached = 0
    srv.received = b""
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    return srv


class _V6Server(socketserver.ThreadingTCPServer):
    address_family = socket.AF_INET6


def start_v6_sink():
    """The same sink on the IPv6 loopback, or None where the host has no ::1.

    Returned rather than skipped-at-import so the caller can say WHY it skipped: "this host has
    no IPv6 loopback" and "the gateway cannot reach IPv6" are different results and only one of
    them is about the code under test.
    """
    try:
        srv = _V6Server(("::1", 0), SinkHandler)
    except OSError:
        return None
    srv.daemon_threads = True
    srv.reached = 0
    srv.received = b""
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    return srv


class Gateway:
    """The real connect-proxy.py, in its own process, listening on loopback."""

    def __init__(self, allow):
        self.port = free_port()
        self.log = []
        self.proc = subprocess.Popen(
            [sys.executable, str(PROXY)],
            env={**os.environ, "EGRESS_ALLOW": allow, "EGRESS_PORT": str(self.port),
                 "EGRESS_HOST": "127.0.0.1"},
            stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
        self.drain = threading.Thread(target=self._drain, daemon=True)
        self.drain.start()
        self._await_listen()

    def _drain(self):
        for line in self.proc.stdout:
            self.log.append(line.rstrip("\n"))

    def _await_listen(self, deadline=15.0):
        end = time.monotonic() + deadline
        while time.monotonic() < end:
            if self.proc.poll() is not None:
                raise AssertionError("the gateway exited before it listened:\n"
                                     + "\n".join(self.log))
            try:
                socket.create_connection(("127.0.0.1", self.port), timeout=0.5).close()
                return
            except OSError:
                time.sleep(0.05)
        raise AssertionError("the gateway never accepted a connection")

    def stop(self):
        self.proc.terminate()
        try:
            self.proc.wait(timeout=10)
        except subprocess.TimeoutExpired:
            self.proc.kill()
            self.proc.wait(timeout=10)
        self.drain.join(timeout=10)
        self.proc.stdout.close()

    def speak(self, raw, timeout=10):
        """Send bytes, return the still-open socket and whatever the gateway said first."""
        sock = socket.create_connection(("127.0.0.1", self.port), timeout=timeout)
        sock.sendall(raw)
        return sock, sock.recv(4096).decode("latin-1", "replace")

    def connect(self, target):
        return self.speak(f"CONNECT {target} HTTP/1.1\r\nHost: {target}\r\n\r\n".encode())

    def wait_for_log(self, needle, timeout=10.0):
        """The decision log is the gateway's other output, and for the normalisation case it is
        the only place the normalised target is visible."""
        end = time.monotonic() + timeout
        while time.monotonic() < end:
            hit = [line for line in self.log if needle in line]
            if hit:
                return hit[-1]
            time.sleep(0.02)
        raise AssertionError(f"no gateway log line contains {needle!r}:\n" + "\n".join(self.log))


def read_until(sock, seen, needle, timeout=10):
    end = time.monotonic() + timeout
    while needle not in seen and time.monotonic() < end:
        try:
            chunk = sock.recv(4096)
        except OSError:
            break
        if not chunk:
            break
        seen += chunk.decode("latin-1", "replace")
    return seen


class Allowlist(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.sink = start_sink()
        cls.sink_port = cls.sink.server_address[1]
        cls.sink_addr = f"127.0.0.1:{cls.sink_port}"
        cls.gw = Gateway(f"{cls.sink_addr},{PROVIDER}")

    @classmethod
    def tearDownClass(cls):
        cls.gw.stop()
        cls.sink.shutdown()
        cls.sink.server_close()

    def setUp(self):
        self.reached_before = self.sink.reached

    def assertNothingRelayed(self):
        self.assertEqual(self.sink.reached, self.reached_before,
                         "the gateway opened an upstream connection for a refused request")

    def status(self, target):
        sock, head = self.gw.connect(target)
        self.addCleanup(sock.close)
        return head.split("\r\n")[0], head, sock

    def test_the_exact_host_port_in_the_list_is_allowed_and_really_relayed(self):
        """200 alone would only say the lookup passed. The sink's own bytes coming back through
        the tunnel say the relay runs, which is the other half of what the gateway is for."""
        line, head, sock = self.status(self.sink_addr)
        self.assertIn("200", line, head)
        self.assertIn("SINK", read_until(sock, head, "SINK"))
        self.assertEqual(self.sink.reached, self.reached_before + 1)
        self.assertIn(f"allow {self.sink_addr}", self.gw.wait_for_log("allow "))

    def test_a_prefix_lookalike_of_an_allowed_host_is_refused(self):
        """The case the module docstring names: a suffix rule would pass this."""
        line, head, _ = self.status("evil-cloud-api.near.ai:443")
        self.assertIn("403", line, head)
        self.gw.wait_for_log("DENY  evil-cloud-api.near.ai:443")
        self.assertNothingRelayed()

    def test_a_suffix_lookalike_of_an_allowed_host_is_refused(self):
        """The other case it names: the allowed host as a LABEL of an attacker's domain."""
        line, head, _ = self.status("cloud-api.near.ai.attacker.example:443")
        self.assertIn("403", line, head)
        self.gw.wait_for_log("DENY  cloud-api.near.ai.attacker.example:443")
        self.assertNothingRelayed()

    def test_an_unlisted_target_is_refused(self):
        line, head, _ = self.status("198.51.100.7:443")
        self.assertIn("403", line, head)
        self.assertNothingRelayed()

    def test_the_port_is_part_of_the_identity_not_decoration(self):
        """Same host as the sink, a port nobody listens on: refused at the lookup, so the
        allowlist entry is a destination and not a hostname with a port stapled to it."""
        line, head, _ = self.status(f"127.0.0.1:{free_port()}")
        self.assertIn("403", line, head)
        self.assertNothingRelayed()

    def test_a_non_CONNECT_method_is_405_and_nothing_is_relayed(self):
        """An absolute-form GET is the request that would turn an egress boundary into a
        general-purpose relay. It must be answered, never forwarded — including to a
        destination that IS on the allowlist, which is the only case where forwarding it
        would otherwise look defensible."""
        sock, head = self.gw.speak(
            f"GET http://{self.sink_addr}/ HTTP/1.1\r\nHost: {self.sink_addr}\r\n\r\n".encode())
        self.addCleanup(sock.close)
        self.assertIn("405", head.split("\r\n")[0], head)
        self.assertNotIn("SINK", head)
        self.gw.wait_for_log("only CONNECT is served")
        self.assertNothingRelayed()

    def test_a_portless_target_gets_443_appended_BEFORE_the_allowlist_check(self):
        """`CONNECT host` with no port is legal, and the gateway normalises it to :443. The
        status code alone cannot show that — a bare host is absent from the list either way —
        so the assertion is on the decision log, which names the target it actually looked up.
        The host used here IS in the list at another port, so a lookup on the un-normalised
        string would have to be answered before the port was appended to fail differently."""
        line, head, _ = self.status("127.0.0.1")
        self.assertIn("403", line, head)
        self.gw.wait_for_log("DENY  127.0.0.1:443")
        self.assertNothingRelayed()


class StartupGuards(unittest.TestCase):
    """An allowlist that cannot match anything must not produce a running proxy. Both shapes of
    that mistake fail here rather than at the first denied request."""

    def refuses(self, allow):
        """A guard that is missing does not fail an assertion, it serves forever — so the
        wait is bounded and a process still alive at the end is the failure, reported as one."""
        p = subprocess.Popen(
            [sys.executable, str(PROXY)],
            env={**os.environ, "EGRESS_ALLOW": allow, "EGRESS_PORT": str(free_port()),
                 "EGRESS_HOST": "127.0.0.1"},
            stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        try:
            out, err = p.communicate(timeout=15)
        except subprocess.TimeoutExpired:
            p.kill()
            p.communicate()
            self.fail(f"the gateway started and kept serving on EGRESS_ALLOW={allow!r}")
        self.assertNotEqual(p.returncode, 0, f"it exited 0 on EGRESS_ALLOW={allow!r}")
        return out + err

    def test_an_empty_allowlist_refuses_to_start(self):
        self.assertIn("EGRESS_ALLOW is empty", self.refuses(""))

    def test_a_portless_entry_refuses_to_start(self):
        """Without this the process starts, logs `CONNECT allowed to: cloud-api.near.ai`, and
        then denies everything — an allow-nothing proxy wearing a working one's face, which is
        the exact failure the empty-list guard already refuses to ship."""
        out = self.refuses("cloud-api.near.ai")
        self.assertIn("without a port", out)
        self.assertIn("cloud-api.near.ai", out)

    def test_one_portless_entry_poisons_an_otherwise_good_list(self):
        """The dead entry is what the operator believes is live, so half-starting is worse than
        not starting: every request to it would be denied by a proxy reporting itself healthy."""
        out = self.refuses(f"{PROVIDER},cloud-api.near.ai")
        self.assertIn("without a port", out)
        self.assertNotIn(PROVIDER + " —", out)

    def test_a_bare_port_with_no_host_refuses_to_start(self):
        self.assertIn("without a port", self.refuses(":443"))

    def test_a_bracketed_IPv6_destination_with_a_port_still_starts(self):
        """The guard must not cost the tree a legitimate spelling: `[::1]:443` is a host and a
        port, and the proof's BYPASS block already speaks literal IPv6 targets."""
        gw = Gateway(f"[2606:4700:4700::1111]:443,{PROVIDER}")
        self.addCleanup(gw.stop)
        self.assertIsNone(gw.proc.poll())

    def test_an_unbracketed_IPv6_entry_refuses_to_start(self):
        """`::1:443` cannot be split without guessing whether the last group is a port or a
        hextet, and an allowlist entry that means something other than what its author read is
        the failure this whole guard exists to prevent. Refused, not interpreted."""
        self.assertIn("without a port", self.refuses("::1:443"))


class PipelinedBytes(unittest.TestCase):
    """Bytes that arrive in the SAME segment as the CONNECT belong to the tunnel.

    `recv` returns whatever the kernel has, so a client that writes its TLS ClientHello together
    with the CONNECT request — legal, and what a pipelining client does — had those bytes read
    into the header buffer and dropped. The relay then began from empty, upstream never saw a
    handshake, and the connection hung to the 300s select timeout looking like an upstream fault.
    """

    def setUp(self):
        self.sink = start_sink()
        self.addCleanup(self.sink.shutdown)
        self.addr = "127.0.0.1:%d" % self.sink.server_address[1]
        self.gw = Gateway(self.addr)
        self.addCleanup(self.gw.stop)

    def test_bytes_sent_with_the_CONNECT_reach_upstream(self):
        payload = b"PIPELINED-CLIENT-HELLO"
        raw = (f"CONNECT {self.addr} HTTP/1.1\r\nHost: {self.addr}\r\n\r\n".encode() + payload)
        sock, head = self.gw.speak(raw)
        self.addCleanup(sock.close)
        self.assertIn("200", head.split("\r\n")[0], head)
        end = time.monotonic() + 10
        while payload not in self.sink.received and time.monotonic() < end:
            time.sleep(0.02)
        self.assertIn(payload, self.sink.received,
                      "the bytes pipelined with the CONNECT never reached upstream — the tunnel "
                      "opened and silently swallowed the client's first write")

    def test_the_ordinary_unpipelined_case_still_works(self):
        """THE POSITIVE CONTROL. Forwarding a buffer that is normally empty must not disturb the
        path every real client takes."""
        sock, head = self.gw.connect(self.addr)
        self.addCleanup(sock.close)
        self.assertIn("200", head.split("\r\n")[0], head)
        sock.sendall(b"AFTER-THE-200")
        end = time.monotonic() + 10
        while b"AFTER-THE-200" not in self.sink.received and time.monotonic() < end:
            time.sleep(0.02)
        self.assertIn(b"AFTER-THE-200", self.sink.received)


class IPv6Tunnel(unittest.TestCase):
    """A bracketed IPv6 entry must CONNECT, not merely start.

    It used to do exactly half of that: `[::1]:443` passed the startup guard, so the gateway
    reported the destination as allowed — and then `socket.create_connection(("[::1]", 443))`
    raised gaierror, because Python does not strip the brackets. Every request 502'd, forever,
    against a banner saying the host was permitted. The old test asserted only that the process
    stayed up, which is the half that already worked.
    """

    def setUp(self):
        self.sink = start_v6_sink()
        if self.sink is None:
            self.skipTest("this host has no IPv6 loopback; the gateway's own handling is "
                          "covered by the startup guards above")
        self.addCleanup(self.sink.shutdown)
        self.addr = "[::1]:%d" % self.sink.server_address[1]
        self.gw = Gateway(self.addr)
        self.addCleanup(self.gw.stop)

    def test_a_bracketed_IPv6_destination_actually_relays(self):
        sock, head = self.gw.connect(self.addr)
        self.addCleanup(sock.close)
        self.assertIn("200", head.split("\r\n")[0],
                      "the allowlisted IPv6 destination was not reachable: " + head)
        self.assertIn("SINK", read_until(sock, head, "SINK"))
        self.assertEqual(self.sink.reached, 1)


if __name__ == "__main__":
    unittest.main(verbosity=2)
