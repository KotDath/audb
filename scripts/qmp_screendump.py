#!/usr/bin/env python3
"""
Minimal QMP screendump helper for the Aurora hybrid emulator.

Examples:
  python3 qmp_screendump.py
  python3 qmp_screendump.py --output /tmp/frame.png --format png
  python3 qmp_screendump.py --output /tmp/frame.ppm --format ppm
"""

from __future__ import annotations

import argparse
import json
import os
import socket
import sys
from pathlib import Path


DEFAULT_SOCKET = "/tmp/aurora-hybrid.qmp"
DEFAULT_OUTPUT = "/tmp/aurora-qmp-screendump.png"


class QmpClient:
    def __init__(self, socket_path: str):
        self.socket_path = socket_path
        self.sock: socket.socket | None = None
        self.file = None

    def __enter__(self) -> "QmpClient":
        self.connect()
        return self

    def __exit__(self, exc_type, exc, tb) -> None:
        self.close()

    def connect(self) -> None:
        path = Path(self.socket_path)
        if not path.exists():
            raise FileNotFoundError(f"QMP socket not found: {self.socket_path}")

        self.sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.sock.connect(self.socket_path)
        self.file = self.sock.makefile("rwb", buffering=0)

        greeting = self._read_message()
        if "QMP" not in greeting:
            raise RuntimeError(f"unexpected QMP greeting: {greeting}")

        self.execute("qmp_capabilities")

    def close(self) -> None:
        if self.file is not None:
            self.file.close()
            self.file = None
        if self.sock is not None:
            self.sock.close()
            self.sock = None

    def _read_message(self) -> dict:
        assert self.file is not None
        while True:
            raw = self.file.readline()
            if not raw:
                raise RuntimeError("QMP connection closed")
            message = json.loads(raw.decode("utf-8"))
            if "event" in message:
                continue
            return message

    def execute(self, command: str, arguments: dict | None = None) -> dict:
        assert self.file is not None
        payload = {"execute": command}
        if arguments:
            payload["arguments"] = arguments

        self.file.write(json.dumps(payload).encode("utf-8") + b"\n")
        reply = self._read_message()
        if "error" in reply:
            raise RuntimeError(f"QMP {command} failed: {reply['error']}")
        return reply


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--socket", default=DEFAULT_SOCKET, help="QMP unix socket path")
    parser.add_argument("--output", default=DEFAULT_OUTPUT, help="output image path")
    parser.add_argument("--format", choices=("png", "ppm"), default="png", help="screendump format")
    parser.add_argument("--device", help="optional QEMU display device id")
    parser.add_argument("--head", type=int, help="optional display head")
    parser.add_argument("--keep", action="store_true", help="keep existing output file instead of replacing it")
    return parser


def main() -> int:
    parser = build_parser()
    args = parser.parse_args()

    output = Path(args.output)
    if output.exists() and not args.keep:
        output.unlink()

    screendump_args: dict[str, object] = {
        "filename": str(output),
        "format": args.format,
    }
    if args.device:
        screendump_args["device"] = args.device
    if args.head is not None:
        screendump_args["head"] = args.head

    try:
        with QmpClient(args.socket) as client:
            display = client.execute("query-display-options")
            version = client.execute("query-version")
            print("display:", json.dumps(display["return"], ensure_ascii=False))
            print("version:", json.dumps(version["return"], ensure_ascii=False))
            print("screendump:", json.dumps(screendump_args, ensure_ascii=False))
            client.execute("screendump", screendump_args)
    except Exception as exc:
        print(f"ERROR: {exc}", file=sys.stderr)
        print(f"output_exists={output.exists()}", file=sys.stderr)
        if output.exists():
            print(f"output_size={output.stat().st_size}", file=sys.stderr)
        return 1

    print(f"output_exists={output.exists()}")
    if output.exists():
        print(f"output_size={output.stat().st_size}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
