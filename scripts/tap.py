#!/usr/bin/env python3
"""
System-wide tap injection on Sailfish OS via /dev/uinput (multitouch Type B style).

Designed to match the common Sailfish touchscreen profile seen in evtest:
  ABS_X: 0..720, ABS_Y: 0..1440
  ABS_MT_SLOT, ABS_MT_TRACKING_ID, ABS_MT_POSITION_X/Y, ABS_MT_TOUCH_MAJOR, ABS_MT_WIDTH_MAJOR
  BTN_TOUCH
  INPUT_PROP_DIRECT

Run as root (e.g. `devel-su -c "python3 tap.py 200 400"`).

Usage:
  python3 tap.py X Y
Env overrides:
  XMAX=720 YMAX=1440 SLOT_MAX=4
  TOUCH_MAJOR=19 WIDTH_MAJOR=19
  SETTLE=1.0   # seconds to wait after creating uinput device
  DOWN_MS=60   # press duration (ms)
"""

import os
import sys
import time
import ctypes
import fcntl

# ---------- Defaults (match your evtest dump) ----------
XMAX = int(os.environ.get("XMAX", "720"))
YMAX = int(os.environ.get("YMAX", "1440"))
SLOT_MAX = int(os.environ.get("SLOT_MAX", "4"))

TOUCH_MAJOR = int(os.environ.get("TOUCH_MAJOR", "19"))
WIDTH_MAJOR = int(os.environ.get("WIDTH_MAJOR", "19"))

SETTLE = float(os.environ.get("SETTLE", "1.0"))
DOWN_MS = int(os.environ.get("DOWN_MS", "60"))

# ---------- Input constants ----------
EV_SYN = 0x00
EV_KEY = 0x01
EV_ABS = 0x03

SYN_REPORT = 0

BTN_TOUCH = 0x14A
INPUT_PROP_DIRECT = 0x01

ABS_X = 0x00
ABS_Y = 0x01
ABS_MT_SLOT = 0x2F
ABS_MT_TOUCH_MAJOR = 0x30
ABS_MT_WIDTH_MAJOR = 0x32
ABS_MT_POSITION_X = 0x35
ABS_MT_POSITION_Y = 0x36
ABS_MT_TRACKING_ID = 0x39

# ---------- ioctl macros (portable) ----------
_IOC_NRBITS = 8
_IOC_TYPEBITS = 8
_IOC_SIZEBITS = 14
_IOC_DIRBITS = 2

_IOC_NRSHIFT = 0
_IOC_TYPESHIFT = _IOC_NRSHIFT + _IOC_NRBITS
_IOC_SIZESHIFT = _IOC_TYPESHIFT + _IOC_TYPEBITS
_IOC_DIRSHIFT = _IOC_SIZESHIFT + _IOC_SIZEBITS

_IOC_NONE = 0
_IOC_WRITE = 1

def _IOC(direction: int, t: int, nr: int, size: int) -> int:
    return (direction << _IOC_DIRSHIFT) | (t << _IOC_TYPESHIFT) | (nr << _IOC_NRSHIFT) | (size << _IOC_SIZESHIFT)

def _IO(t: int, nr: int) -> int:
    return _IOC(_IOC_NONE, t, nr, 0)

def _IOW(t: int, nr: int, size: int) -> int:
    return _IOC(_IOC_WRITE, t, nr, size)

U = ord('U')
INTSZ = ctypes.sizeof(ctypes.c_int)

UI_SET_EVBIT   = _IOW(U, 100, INTSZ)
UI_SET_KEYBIT  = _IOW(U, 101, INTSZ)
UI_SET_ABSBIT  = _IOW(U, 103, INTSZ)
UI_SET_PROPBIT = _IOW(U, 110, INTSZ)
UI_DEV_CREATE  = _IO(U, 1)
UI_DEV_DESTROY = _IO(U, 2)

# ---------- Structs that match your arch ----------
class TimeVal(ctypes.Structure):
    _fields_ = [("tv_sec", ctypes.c_long),
                ("tv_usec", ctypes.c_long)]

class InputEvent(ctypes.Structure):
    _fields_ = [("time", TimeVal),
                ("type", ctypes.c_ushort),
                ("code", ctypes.c_ushort),
                ("value", ctypes.c_int)]

class InputID(ctypes.Structure):
    _fields_ = [("bustype", ctypes.c_ushort),
                ("vendor", ctypes.c_ushort),
                ("product", ctypes.c_ushort),
                ("version", ctypes.c_ushort)]

ABS_CNT = 64

class UInputUserDev(ctypes.Structure):
    _fields_ = [
        ("name", ctypes.c_char * 80),
        ("id", InputID),
        ("ff_effects_max", ctypes.c_int),
        ("absmax", ctypes.c_int * ABS_CNT),
        ("absmin", ctypes.c_int * ABS_CNT),
        ("absfuzz", ctypes.c_int * ABS_CNT),
        ("absflat", ctypes.c_int * ABS_CNT),
    ]

def open_uinput() -> int:
    for p in ("/dev/uinput", "/dev/input/uinput"):
        try:
            return os.open(p, os.O_WRONLY | os.O_NONBLOCK)
        except OSError:
            pass
    raise SystemExit("ERROR: can't open /dev/uinput (try `devel-su` and `modprobe uinput`).")

def emit(fd: int, etype: int, code: int, value: int) -> None:
    os.write(fd, bytes(InputEvent(TimeVal(0, 0), etype, code, value)))

def syn(fd: int) -> None:
    emit(fd, EV_SYN, SYN_REPORT, 0)

def clamp(v: int, lo: int, hi: int) -> int:
    return max(lo, min(hi, v))

def main() -> int:
    if len(sys.argv) != 3:
        print("Usage: python3 tap.py X Y", file=sys.stderr)
        print("Example: devel-su -c \"python3 tap.py 200 400\"", file=sys.stderr)
        return 2

    x = int(sys.argv[1])
    y = int(sys.argv[2])
    x = clamp(x, 0, XMAX)
    y = clamp(y, 0, YMAX)

    fd = open_uinput()

    # Capabilities
    fcntl.ioctl(fd, UI_SET_EVBIT, EV_KEY)
    fcntl.ioctl(fd, UI_SET_EVBIT, EV_ABS)
    fcntl.ioctl(fd, UI_SET_EVBIT, EV_SYN)

    fcntl.ioctl(fd, UI_SET_KEYBIT, BTN_TOUCH)
    fcntl.ioctl(fd, UI_SET_PROPBIT, INPUT_PROP_DIRECT)

    for c in (
        ABS_X, ABS_Y,
        ABS_MT_SLOT, ABS_MT_TRACKING_ID, ABS_MT_POSITION_X, ABS_MT_POSITION_Y,
        ABS_MT_TOUCH_MAJOR, ABS_MT_WIDTH_MAJOR
    ):
        fcntl.ioctl(fd, UI_SET_ABSBIT, c)

    u = UInputUserDev()
    u.name = b"sfos-uinput-touch"
    # Mimic your real touch bus (0x18); vendor/product/version often 0 on these.
    u.id = InputID(0x18, 0x0, 0x0, 0x0)

    # Ranges (match evtest)
    u.absmin[ABS_X] = 0; u.absmax[ABS_X] = XMAX
    u.absmin[ABS_Y] = 0; u.absmax[ABS_Y] = YMAX
    u.absmin[ABS_MT_SLOT] = 0; u.absmax[ABS_MT_SLOT] = SLOT_MAX
    u.absmin[ABS_MT_TRACKING_ID] = 0; u.absmax[ABS_MT_TRACKING_ID] = 65535
    u.absmin[ABS_MT_POSITION_X] = 0; u.absmax[ABS_MT_POSITION_X] = XMAX
    u.absmin[ABS_MT_POSITION_Y] = 0; u.absmax[ABS_MT_POSITION_Y] = YMAX
    u.absmin[ABS_MT_TOUCH_MAJOR] = 0; u.absmax[ABS_MT_TOUCH_MAJOR] = 255
    u.absmin[ABS_MT_WIDTH_MAJOR] = 0; u.absmax[ABS_MT_WIDTH_MAJOR] = 255

    os.write(fd, bytes(u))
    fcntl.ioctl(fd, UI_DEV_CREATE)

    # Give lipstick/libinput time to notice the device
    time.sleep(SETTLE)

    tracking_id = int(time.time() * 1000) % 60000 + 1

    # DOWN sequence similar to evtest dump
    emit(fd, EV_ABS, ABS_MT_SLOT, 0)
    emit(fd, EV_ABS, ABS_MT_TRACKING_ID, tracking_id)
    emit(fd, EV_ABS, ABS_MT_POSITION_X, x)
    emit(fd, EV_ABS, ABS_MT_POSITION_Y, y)
    emit(fd, EV_ABS, ABS_MT_TOUCH_MAJOR, TOUCH_MAJOR)
    emit(fd, EV_ABS, ABS_MT_WIDTH_MAJOR, WIDTH_MAJOR)
    emit(fd, EV_KEY, BTN_TOUCH, 1)
    syn(fd)

    time.sleep(max(0.0, DOWN_MS / 1000.0))

    # UP
    emit(fd, EV_KEY, BTN_TOUCH, 0)
    emit(fd, EV_ABS, ABS_MT_TRACKING_ID, -1)
    syn(fd)

    time.sleep(0.2)
    fcntl.ioctl(fd, UI_DEV_DESTROY)
    os.close(fd)

    print(f"tap({x},{y}) in range 0..{XMAX},0..{YMAX}")
    return 0

if __name__ == "__main__":
    raise SystemExit(main())
