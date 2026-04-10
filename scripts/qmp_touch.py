#!/usr/bin/env python3
"""
Minimal QMP multitouch helper for the Aurora hybrid emulator.

Examples:
  python3 qmp_touch.py swipe-up
  python3 qmp_touch.py swipe --from 180,740 --to 180,120
  python3 qmp_touch.py tap --at 180,400

Defaults target the hybrid VM started by run-aurora-hybrid.sh:
  socket: /tmp/aurora-hybrid.qmp
  viewport: 360x800
  QMP multitouch absolute range: 0..32767
"""

from __future__ import annotations

import argparse
import json
import socket
import sys
import time
from pathlib import Path


DEFAULT_SOCKET = "/tmp/aurora-hybrid.qmp"
DEFAULT_WIDTH = 360
DEFAULT_HEIGHT = 800
DEFAULT_ABS_MAX = 32767


def parse_point(value: str) -> tuple[int, int]:
    try:
        x_str, y_str = value.split(",", 1)
        return int(x_str), int(y_str)
    except ValueError as exc:
        raise argparse.ArgumentTypeError(
            f"invalid point '{value}', expected X,Y"
        ) from exc


def clamp(value: int, lo: int, hi: int) -> int:
    return max(lo, min(hi, value))


def px_to_abs(x_px: int, y_px: int, width: int, height: int, abs_max: int) -> tuple[int, int]:
    x_px = clamp(x_px, 0, width - 1)
    y_px = clamp(y_px, 0, height - 1)
    x_abs = round(x_px * abs_max / (width - 1))
    y_abs = round(y_px * abs_max / (height - 1))
    return x_abs, y_abs


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
            # Ignore asynchronous events while waiting for command replies.
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


def qmp_btn_touch(down: bool) -> dict:
    return {"type": "btn", "data": {"button": "touch", "down": down}}


def qmp_mtt(event_type: str, slot: int, tracking_id: int, axis: str, value: int) -> dict:
    data = {
        "type": event_type,
        "slot": slot,
        "tracking-id": tracking_id,
        "axis": axis,
        "value": value,
    }
    return {"type": "mtt", "data": data}


def send_tap(
    client: QmpClient,
    x_px: int,
    y_px: int,
    width: int,
    height: int,
    abs_max: int,
    hold_ms: int,
    slot: int,
    tracking_id: int,
) -> None:
    x_abs, y_abs = px_to_abs(x_px, y_px, width, height, abs_max)
    print(f"tap px=({x_px},{y_px}) abs=({x_abs},{y_abs})")

    client.execute(
        "input-send-event",
        {
            "events": [
                qmp_mtt("begin", slot, tracking_id, "x", x_abs),
                qmp_btn_touch(True),
                qmp_mtt("data", slot, tracking_id, "x", x_abs),
                qmp_mtt("data", slot, tracking_id, "y", y_abs),
            ]
        },
    )
    time.sleep(hold_ms / 1000.0)
    client.execute(
        "input-send-event",
        {
            "events": [
                qmp_mtt("end", slot, -1, "x", x_abs),
            ]
        },
    )


def send_swipe(
    client: QmpClient,
    start_px: tuple[int, int],
    end_px: tuple[int, int],
    width: int,
    height: int,
    abs_max: int,
    steps: int,
    duration_ms: int,
    hold_ms: int,
    slot: int,
    tracking_id: int,
) -> None:
    x0_abs, y0_abs = px_to_abs(start_px[0], start_px[1], width, height, abs_max)
    x1_abs, y1_abs = px_to_abs(end_px[0], end_px[1], width, height, abs_max)

    print(
        "swipe "
        f"px=({start_px[0]},{start_px[1]}) -> ({end_px[0]},{end_px[1]}) "
        f"abs=({x0_abs},{y0_abs}) -> ({x1_abs},{y1_abs}) "
        f"steps={steps} duration_ms={duration_ms} hold_ms={hold_ms}"
    )

    client.execute(
        "input-send-event",
        {
            "events": [
                qmp_mtt("begin", slot, tracking_id, "x", x0_abs),
                qmp_btn_touch(True),
                qmp_mtt("data", slot, tracking_id, "x", x0_abs),
                qmp_mtt("data", slot, tracking_id, "y", y0_abs),
            ]
        },
    )
    if hold_ms > 0:
        time.sleep(hold_ms / 1000.0)

    if steps < 1:
        steps = 1
    delay_s = duration_ms / 1000.0 / steps

    for i in range(1, steps + 1):
        t = i / steps
        x_abs = round(x0_abs + (x1_abs - x0_abs) * t)
        y_abs = round(y0_abs + (y1_abs - y0_abs) * t)
        client.execute(
            "input-send-event",
            {
                "events": [
                    qmp_mtt("update", slot, tracking_id, "x", x_abs),
                    qmp_btn_touch(True),
                    qmp_mtt("data", slot, tracking_id, "x", x_abs),
                    qmp_mtt("data", slot, tracking_id, "y", y_abs),
                ]
            },
        )
        time.sleep(delay_s)

    client.execute(
        "input-send-event",
        {
            "events": [
                qmp_mtt("end", slot, -1, "x", x1_abs),
            ]
        },
    )


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--socket", default=DEFAULT_SOCKET, help="QMP unix socket path")
    parser.add_argument("--width", type=int, default=DEFAULT_WIDTH, help="guest viewport width in px")
    parser.add_argument("--height", type=int, default=DEFAULT_HEIGHT, help="guest viewport height in px")
    parser.add_argument("--abs-max", type=int, default=DEFAULT_ABS_MAX, help="QMP absolute axis maximum")
    parser.add_argument("--slot", type=int, default=0, help="multitouch slot")
    parser.add_argument("--tracking-id", type=int, default=1001, help="tracking id for touch sequence")

    subparsers = parser.add_subparsers(dest="command", required=True)

    tap_parser = subparsers.add_parser("tap", help="single tap via QMP multitouch")
    tap_parser.add_argument("--at", type=parse_point, required=True, help="tap point as X,Y in px")
    tap_parser.add_argument("--hold-ms", type=int, default=40, help="press duration in ms")

    swipe_parser = subparsers.add_parser("swipe", help="generic swipe via QMP multitouch")
    swipe_parser.add_argument("--from", dest="start", type=parse_point, required=True, help="start point X,Y in px")
    swipe_parser.add_argument("--to", dest="end", type=parse_point, required=True, help="end point X,Y in px")
    swipe_parser.add_argument("--steps", type=int, default=28, help="move frames")
    swipe_parser.add_argument("--duration-ms", type=int, default=420, help="swipe duration in ms")
    swipe_parser.add_argument("--hold-ms", type=int, default=90, help="initial press-hold before motion in ms")

    swipe_up_parser = subparsers.add_parser("swipe-up", help="ready-made bottom-to-top swipe")
    swipe_up_parser.add_argument("--x", type=int, default=180, help="horizontal swipe anchor in px")
    swipe_up_parser.add_argument("--start-y", type=int, default=780, help="starting y in px")
    swipe_up_parser.add_argument("--end-y", type=int, default=90, help="ending y in px")
    swipe_up_parser.add_argument("--steps", type=int, default=30, help="move frames")
    swipe_up_parser.add_argument("--duration-ms", type=int, default=460, help="swipe duration in ms")
    swipe_up_parser.add_argument("--hold-ms", type=int, default=110, help="initial press-hold before motion in ms")

    return parser


def main() -> int:
    parser = build_parser()
    args = parser.parse_args()

    try:
        with QmpClient(args.socket) as client:
            if args.command == "tap":
                send_tap(
                    client,
                    args.at[0],
                    args.at[1],
                    args.width,
                    args.height,
                    args.abs_max,
                    args.hold_ms,
                    args.slot,
                    args.tracking_id,
                )
            elif args.command == "swipe":
                send_swipe(
                    client,
                    args.start,
                    args.end,
                    args.width,
                    args.height,
                    args.abs_max,
                    args.steps,
                    args.duration_ms,
                    args.hold_ms,
                    args.slot,
                    args.tracking_id,
                )
            elif args.command == "swipe-up":
                send_swipe(
                    client,
                    (args.x, args.start_y),
                    (args.x, args.end_y),
                    args.width,
                    args.height,
                    args.abs_max,
                    args.steps,
                    args.duration_ms,
                    args.hold_ms,
                    args.slot,
                    args.tracking_id,
                )
            else:
                parser.error(f"unknown command: {args.command}")
    except Exception as exc:  # pragma: no cover - CLI path
        print(f"ERROR: {exc}", file=sys.stderr)
        return 1

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
