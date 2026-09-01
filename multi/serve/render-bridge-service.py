#!/usr/bin/env python3
"""Render/install bridge.service with one authoritative systemd operator-state root."""
from __future__ import annotations

import argparse
import os
import pathlib
import tempfile


HERE = pathlib.Path(__file__).resolve().parent
TEMPLATE = HERE / "bridge.service"
DEFAULT_AGENCY_DIR = pathlib.Path("/home/multi/.agency")


def operator_dir(value=None):
    raw = str(value or os.environ.get("AGENCY_DIR") or DEFAULT_AGENCY_DIR)
    if any(ch.isspace() for ch in raw) or ":" in raw or "\0" in raw:
        raise ValueError("AGENCY_DIR for the systemd unit cannot contain whitespace, ':' or NUL")
    path = pathlib.Path(raw).expanduser()
    if not path.is_absolute():
        raise ValueError("AGENCY_DIR for the systemd unit must be an absolute path")
    return path


def render(value=None):
    root = operator_dir(value)
    source = TEMPLATE.read_text()
    default = str(DEFAULT_AGENCY_DIR)
    rendered = source.replace(default, str(root))
    # The four directives that must all move together. Named ONCE and counted FROM the list: the
    # literal `!= 4` beside a four-item tuple was two statements of one fact, and adding a fifth
    # directive would have satisfied the tuple while failing the count with a message naming
    # "four".
    required = (f"EnvironmentFile={root}/bridge.env",
                f"ExecStart=/usr/bin/env AGENCY_DIR={root} ",
                f"BindPaths={root}", f"ReadWritePaths={root}")
    if source.count(default) != len(required):
        raise RuntimeError(
            f"bridge.service carries {source.count(default)} operator-root occurrence(s); "
            f"this renderer expects {len(required)}, one per directive it checks below")
    if not all(item in rendered for item in required):
        raise RuntimeError("rendered bridge unit does not use one operator directory everywhere")
    return rendered


def install(destination, value=None):
    """Atomically install one rendered unit; the caller performs daemon-reload/restart."""
    destination = pathlib.Path(destination)
    destination.parent.mkdir(parents=True, exist_ok=True)
    fd, temporary = tempfile.mkstemp(prefix=destination.name + ".", dir=destination.parent)
    try:
        with os.fdopen(fd, "w") as out:
            out.write(render(value))
            out.flush()
            os.fsync(out.fileno())
        os.chmod(temporary, 0o644)
        os.replace(temporary, destination)
    except BaseException:
        try:
            os.unlink(temporary)
        except FileNotFoundError:
            pass
        raise
    return destination


def main(argv=None):
    parser = argparse.ArgumentParser()
    parser.add_argument("--agency-dir", help="absolute operator-state root (default: AGENCY_DIR or /home/multi/.agency)")
    parser.add_argument("--output", type=pathlib.Path,
                        help="atomically install here; omit to print the rendered unit")
    args = parser.parse_args(argv)
    if args.output:
        install(args.output, args.agency_dir)
    else:
        print(render(args.agency_dir), end="")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
