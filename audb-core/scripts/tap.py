#!/usr/bin/env python3
"""
Tap injection on Sailfish/Aurora OS.

Two modes:
  1. Default (uinput): Creates virtual device, reliable but slower (~150ms setup)
  2. Fast (--event): Direct evdev injection, fast but requires correct device

Usage:
  python3 tap.py X Y                    # uinput mode (safe)
  python3 tap.py X Y --event /dev/input/event4   # direct evdev (fast)
  python3 tap.py X Y --event auto       # auto-detect touchscreen
  python3 tap.py X Y --duration 1000    # long press (1000ms)
  python3 tap.py X Y --no-rotate        # disable rotation handling

Run as root: devel-su -c "python3 tap.py 200 400"

Env overrides:
  XMAX=720 YMAX=1440 SLOT_MAX=4
  TOUCH_MAJOR=19 WIDTH_MAJOR=19
  SETTLE=0.15   # seconds to wait after creating uinput device
  DOWN_MS=30    # press duration (ms), overridden by --duration
"""

import os
import sys
import time
import ctypes
import fcntl
import glob
import subprocess

# ---------- Defaults ----------
XMAX = int(os.environ.get("XMAX", "720"))
YMAX = int(os.environ.get("YMAX", "1440"))
SLOT_MAX = int(os.environ.get("SLOT_MAX", "4"))

TOUCH_MAJOR = int(os.environ.get("TOUCH_MAJOR", "19"))
WIDTH_MAJOR = int(os.environ.get("WIDTH_MAJOR", "19"))

SETTLE = float(os.environ.get("SETTLE", "0.15"))
DOWN_MS = int(os.environ.get("DOWN_MS", "30"))

# ---------- Orientation constants (Qt::ScreenOrientation) ----------
ORIENTATION_PORTRAIT = 1
ORIENTATION_LANDSCAPE = 2
ORIENTATION_INVERTED_PORTRAIT = 4
ORIENTATION_INVERTED_LANDSCAPE = 8

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

# ---------- ioctl macros ----------
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

def _IOC(direction, t, nr, size):
    return (direction << _IOC_DIRSHIFT) | (t << _IOC_TYPESHIFT) | (nr << _IOC_NRSHIFT) | (size << _IOC_SIZESHIFT)

def _IO(t, nr):
    return _IOC(_IOC_NONE, t, nr, 0)

def _IOW(t, nr, size):
    return _IOC(_IOC_WRITE, t, nr, size)

U = ord('U')
INTSZ = ctypes.sizeof(ctypes.c_int)

UI_SET_EVBIT   = _IOW(U, 100, INTSZ)
UI_SET_KEYBIT  = _IOW(U, 101, INTSZ)
UI_SET_ABSBIT  = _IOW(U, 103, INTSZ)
UI_SET_PROPBIT = _IOW(U, 110, INTSZ)
UI_DEV_CREATE  = _IO(U, 1)
UI_DEV_DESTROY = _IO(U, 2)

# ---------- Structs ----------
class TimeVal(ctypes.Structure):
    _fields_ = [("tv_sec", ctypes.c_long), ("tv_usec", ctypes.c_long)]

class InputEvent(ctypes.Structure):
    _fields_ = [("time", TimeVal), ("type", ctypes.c_ushort), ("code", ctypes.c_ushort), ("value", ctypes.c_int)]

class InputID(ctypes.Structure):
    _fields_ = [("bustype", ctypes.c_ushort), ("vendor", ctypes.c_ushort), ("product", ctypes.c_ushort), ("version", ctypes.c_ushort)]

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

def emit(fd, etype, code, value):
    os.write(fd, bytes(InputEvent(TimeVal(0, 0), etype, code, value)))

def syn(fd):
    emit(fd, EV_SYN, SYN_REPORT, 0)

def clamp(v, lo, hi):
    return max(lo, min(hi, v))

def find_touchscreen():
    """Auto-detect touchscreen device."""
    for dev_path in sorted(glob.glob("/dev/input/event*")):
        try:
            num = dev_path.split("event")[-1]
            name_path = f"/sys/class/input/event{num}/device/name"
            if os.path.exists(name_path):
                with open(name_path) as f:
                    name = f.read().strip().lower()
                    if any(x in name for x in ["touch", "tpd", "ts", "silead", "goodix", "fts", "atmel", "synaptics", "elan", "chsc", "himax"]):
                        return dev_path
        except:
            pass
    return "/dev/input/event3"  # fallback

def get_screen_orientation():
    """Get current screen orientation from dconf."""
    try:
        result = subprocess.run(
            ["dconf", "read", "/desktop/lipstick-jolla-home/dialog_orientation"],
            capture_output=True, text=True, timeout=2
        )
        if result.returncode == 0 and result.stdout.strip():
            return int(result.stdout.strip())
    except:
        pass
    return ORIENTATION_PORTRAIT  # default to portrait

def transform_coordinates(x, y, orientation):
    """Transform screen coordinates based on orientation.
    
    Input: screen coordinates (what user sees)
    Output: raw touchscreen coordinates (hardware)
    
    Screen is XMAX x YMAX in portrait mode (hardware native).
    """
    if orientation == ORIENTATION_PORTRAIT:
        # No transformation needed
        return x, y
    elif orientation == ORIENTATION_LANDSCAPE:
        # Screen rotated 90° CCW: user's X becomes hardware Y, user's Y becomes (XMAX - hardware X)
        # User sees: width=YMAX, height=XMAX
        # Transform: hw_x = XMAX - y, hw_y = x
        return XMAX - y, x
    elif orientation == ORIENTATION_INVERTED_PORTRAIT:
        # Screen rotated 180°
        return XMAX - x, YMAX - y
    elif orientation == ORIENTATION_INVERTED_LANDSCAPE:
        # Screen rotated 90° CW (270° CCW)
        # User sees: width=YMAX, height=XMAX
        # Transform: hw_x = y, hw_y = YMAX - x
        return y, YMAX - x
    else:
        return x, y

# ---------- UINPUT MODE (default, safe) ----------
def tap_uinput(x, y):
    x = clamp(x, 0, XMAX)
    y = clamp(y, 0, YMAX)

    # Open uinput
    fd = None
    for p in ("/dev/uinput", "/dev/input/uinput"):
        try:
            fd = os.open(p, os.O_WRONLY | os.O_NONBLOCK)
            break
        except OSError:
            pass
    if fd is None:
        raise SystemExit("ERROR: can't open /dev/uinput")

    # Setup capabilities
    fcntl.ioctl(fd, UI_SET_EVBIT, EV_KEY)
    fcntl.ioctl(fd, UI_SET_EVBIT, EV_ABS)
    fcntl.ioctl(fd, UI_SET_EVBIT, EV_SYN)
    fcntl.ioctl(fd, UI_SET_KEYBIT, BTN_TOUCH)
    fcntl.ioctl(fd, UI_SET_PROPBIT, INPUT_PROP_DIRECT)

    for c in (ABS_X, ABS_Y, ABS_MT_SLOT, ABS_MT_TRACKING_ID, ABS_MT_POSITION_X, ABS_MT_POSITION_Y, ABS_MT_TOUCH_MAJOR, ABS_MT_WIDTH_MAJOR):
        fcntl.ioctl(fd, UI_SET_ABSBIT, c)

    u = UInputUserDev()
    u.name = b"sfos-uinput-touch"
    u.id = InputID(0x18, 0x0, 0x0, 0x0)
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
    time.sleep(SETTLE)

    tracking_id = int(time.time() * 1000) % 60000 + 1

    # DOWN
    emit(fd, EV_ABS, ABS_MT_SLOT, 0)
    emit(fd, EV_ABS, ABS_MT_TRACKING_ID, tracking_id)
    emit(fd, EV_ABS, ABS_MT_POSITION_X, x)
    emit(fd, EV_ABS, ABS_MT_POSITION_Y, y)
    emit(fd, EV_ABS, ABS_MT_TOUCH_MAJOR, TOUCH_MAJOR)
    emit(fd, EV_ABS, ABS_MT_WIDTH_MAJOR, WIDTH_MAJOR)
    emit(fd, EV_KEY, BTN_TOUCH, 1)
    syn(fd)

    time.sleep(DOWN_MS / 1000.0)

    # UP
    emit(fd, EV_KEY, BTN_TOUCH, 0)
    emit(fd, EV_ABS, ABS_MT_TRACKING_ID, -1)
    syn(fd)

    time.sleep(0.02)
    fcntl.ioctl(fd, UI_DEV_DESTROY)
    os.close(fd)

    print(f"tap({x},{y}) in range 0..{XMAX},0..{YMAX}")

# ---------- EVDEV MODE (fast, requires correct device) ----------
def tap_evdev(x, y, device):
    if device == "auto":
        device = find_touchscreen()
    
    fd = os.open(device, os.O_WRONLY)
    tracking_id = int(time.time() * 1000) % 60000 + 1

    # DOWN
    emit(fd, EV_ABS, ABS_MT_SLOT, 0)
    emit(fd, EV_ABS, ABS_MT_TRACKING_ID, tracking_id)
    emit(fd, EV_ABS, ABS_MT_POSITION_X, x)
    emit(fd, EV_ABS, ABS_MT_POSITION_Y, y)
    emit(fd, EV_ABS, ABS_MT_TOUCH_MAJOR, TOUCH_MAJOR)
    emit(fd, EV_ABS, ABS_MT_WIDTH_MAJOR, WIDTH_MAJOR)
    emit(fd, EV_KEY, BTN_TOUCH, 1)
    syn(fd)

    time.sleep(DOWN_MS / 1000.0)

    # UP
    emit(fd, EV_KEY, BTN_TOUCH, 0)
    emit(fd, EV_ABS, ABS_MT_TRACKING_ID, -1)
    syn(fd)

    os.close(fd)
    print(f"tap({x},{y}) via {device}")

def main():
    global DOWN_MS
    # Parse args
    args = sys.argv[1:]
    event_device = None
    no_rotate = False
    
    if "--event" in args:
        idx = args.index("--event")
        if idx + 1 < len(args):
            event_device = args[idx + 1]
            args = args[:idx] + args[idx+2:]
        else:
            print("ERROR: --event requires device path or 'auto'", file=sys.stderr)
            return 2

    if "--duration" in args:
        idx = args.index("--duration")
        if idx + 1 < len(args):
            DOWN_MS = int(args[idx + 1])
            args = args[:idx] + args[idx+2:]
        else:
            print("ERROR: --duration requires milliseconds value", file=sys.stderr)
            return 2

    if "--no-rotate" in args:
        args.remove("--no-rotate")
        no_rotate = True

    if len(args) != 2:
        print("Usage: python3 tap.py X Y [--event DEV] [--duration MS] [--no-rotate]", file=sys.stderr)
        return 2

    x = int(args[0])
    y = int(args[1])

    # Transform coordinates based on screen orientation
    if not no_rotate:
        orientation = get_screen_orientation()
        orig_x, orig_y = x, y
        x, y = transform_coordinates(x, y, orientation)
        if orientation != ORIENTATION_PORTRAIT:
            orient_name = {
                ORIENTATION_LANDSCAPE: "landscape",
                ORIENTATION_INVERTED_PORTRAIT: "inverted-portrait",
                ORIENTATION_INVERTED_LANDSCAPE: "inverted-landscape"
            }.get(orientation, f"unknown({orientation})")
            print(f"orientation: {orient_name}, ({orig_x},{orig_y}) -> ({x},{y})", file=sys.stderr)

    if event_device:
        tap_evdev(x, y, event_device)
    else:
        tap_uinput(x, y)
    
    return 0

if __name__ == "__main__":
    raise SystemExit(main())
