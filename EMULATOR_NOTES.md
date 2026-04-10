# Aurora Emulator Notes

## Environment

- Aurora SDK: `/home/kotdath/AuroraOS`
- Emulator name: `AuroraOS-5.2.0.180`
- QEMU process currently started by Aurora SDK with:
  - `-device virtio-tablet-pci`
  - `-display sdl,gl=on,show-cursor=off`
- SSH to emulator works via:
  - host `127.0.0.1`
  - port `2223`
  - key `/home/kotdath/AuroraOS/vmshare/ssh/private_keys/sdk`

## Input Findings

- Inside the guest, the active pointer device is `/dev/input/event1`.
- Device name: `QEMU Virtio Tablet`.
- Real axis ranges from `EVIOCGABS`:
  - `ABS_X`: `0..32767`
  - `ABS_Y`: `0..32767`
  - `ABS_MT_POSITION_X/Y`: not supported (`0..0`)
- `defaultuser` is in the `input` group and can open `/dev/input/event1` without `devel-su`.

## Tap

- Direct writes to `/dev/input/event1` work for taps.
- This means emulator `tap` does not require `AudbBridge`.
- Practical mapping from screen pixels to device absolute coordinates:

```text
abs_x = round(x_px * 32767 / 359)
abs_y = round(y_px * 32767 / 799)
```

- Example tap that successfully toggled `Показать область выреза`:
  - absolute coordinates: `x=13653`, `y=15565`
  - approximate screen position: `149 px`, `380 px`

## Swipe

- Direct drag/swipe over `/dev/input/event1` does not behave like a true touch gesture.
- Tested variants:
  - plain absolute drag
  - drag with `BTN_LEFT`
  - drag with `BTN_LEFT + BTN_TOUCH + BTN_TOOL_FINGER`
  - host-side drag over the SDL window with `xdotool`
- Result:
  - clicks/toggles work
  - gesture semantics do not
  - no reliable homescreen swipe / app close / touch-style list scroll was observed

## Why Swipe Fails

- Current Aurora emulator uses `virtio-tablet-pci`, not a multitouch device.
- Aurora guest starts `lipstick` with a QEMU touch plugin:

```text
LIPSTICK_LIBINPUT_OPTIONS=-plugin VBoxTouch:qemu:evdev=/dev/input/event1
```

- The same emulator also explicitly tells libinput to ignore `QEMU Virtio Tablet`:

```text
ATTRS{name}=="QEMU Virtio Tablet", ENV{LIBINPUT_IGNORE_DEVICE}="1"
```

- This setup is sufficient for taps, but appears insufficient for gesture-grade touch input.

## Screenshot

- Guest-side screenshot through lipstick D-Bus as `defaultuser` failed with:
  - `GDBus.Error:org.freedesktop.DBus.Error.AccessDenied: PID ... is not in privileged group`
- For emulator experiments, screenshot verification was done from the host by capturing the SDL window:

```bash
import -window $(xdotool search --name 'AuroraOS-5.2.0.180' | tail -n1) /tmp/screen.png
```

- This host-side screenshot method works and was used to verify tap results.

## Next Experiment

- QEMU build bundled with Aurora SDK supports `virtio-multitouch-pci`.
- Next useful step is to run the same emulator image manually with:
  - `virtio-multitouch-pci` instead of `virtio-tablet-pci`
- If that works, emulator `swipe` support in `audb` should target the multitouch-backed path, not the current tablet-backed one.

## Manual `virtio-multitouch` Experiment

- A second QEMU instance was launched manually from the same Aurora image using:
  - an overlay qcow2 on top of the original emulator disk
  - `-device virtio-multitouch-pci`
  - a separate SSH forward on host port `32223`
  - `LIBGL_ALWAYS_SOFTWARE=1` on the host side
- Without `LIBGL_ALWAYS_SOFTWARE=1`, manual QEMU launch failed on this machine.

## Multitouch Guest Findings

- Inside the guest, `/dev/input/event1` became `QEMU Virtio MultiTouch`.
- `libinput list-devices` now reports capabilities:
  - `pointer touch`
- Real axis ranges from `EVIOCGABS`:
  - `ABS_X`: not used (`0..0`)
  - `ABS_Y`: not used (`0..0`)
  - `ABS_MT_SLOT`: `0..10`
  - `ABS_MT_POSITION_X`: `0..32767`
  - `ABS_MT_POSITION_Y`: `0..32767`
  - `ABS_MT_TRACKING_ID`: `0..10`

## Multitouch Results

- Direct `ABS_MT_*` tap injection into `/dev/input/event1` works.
- Direct `ABS_MT_*` swipe injection now has real touch semantics.
- Observed results:
  - tapping the dock icon opened `Settings`
  - swipe from the bottom of `Settings` moved the app into the homescreen card state
  - swipe inside the `Settings` list scrolled the list normally

## Conclusion

- The main swipe problem is not Aurora emulator support in general.
- The problem is the current SDK launcher using `virtio-tablet-pci`.
- When the same guest runs with `virtio-multitouch-pci`, touch gestures work.
- For `audb`, emulator gesture support should target a multitouch-backed path.

## Hybrid Mode: Manual Mouse + Automation Touch

- A multitouch-only VM fixes gestures, but removes normal manual mouse interaction.
- To avoid that tradeoff, a hybrid QEMU instance was launched with both:
  - `-device virtio-tablet-pci`
  - `-device virtio-multitouch-pci`
- In this mode the guest sees:
  - `/dev/input/event1` = `QEMU Virtio Tablet`
  - `/dev/input/event2` = `QEMU Virtio MultiTouch`
  - `/dev/input/event3` = `QEMU Virtio Keyboard`
- `libinput list-devices` reports:
  - `QEMU Virtio Tablet` -> `Capabilities: pointer`
  - `QEMU Virtio MultiTouch` -> `Capabilities: pointer touch`

### Why hybrid mode matters

- Manual control can continue to use the tablet/pointer device.
- Automated tap/swipe can target the multitouch device.
- This avoids stealing the host mouse cursor and avoids the gesture limitations of the stock tablet-only setup.

## QMP Findings

- The stock Aurora SDK emulator does not expose a usable host-side `QMP` socket.
- For manual QEMU launches, `QMP` was enabled explicitly with:
  - `-qmp unix:/tmp/<name>.qmp,server=on,wait=off`
- `input-send-event` is available in this QEMU build.
- `QMP` itself does not require multitouch, but real touch gestures do:
  - `btn/abs` over tablet semantics are still pointer-like
  - `mtt` events over `virtio-multitouch-pci` produce real touch events in the guest

## Working QMP Touch Path

- For the hybrid VM, `QMP` `mtt` events were verified to reach the guest.
- Guest-side validation with `libinput debug-events --verbose --device /dev/input/event2` showed:
  - `TOUCH_MOTION`
  - `TOUCH_UP`
- The successful gesture path was:
  - `mtt(begin, slot, tracking_id)`
  - `btn(touch, down=true)`
  - `mtt(data, axis=x, value=...)`
  - `mtt(data, axis=y, value=...)`
  - repeated `mtt(update, ...) + btn(touch,true) + mtt(data,x) + mtt(data,y)` frames
  - `mtt(end, slot, tracking_id=-1)`

### Coordinate system for QMP multitouch

- QMP multitouch coordinates must use the guest absolute touch range, not raw screen pixels.
- For Aurora emulator `360x800`, the working conversion is:

```text
abs_x = round(x_px * 32767 / 359)
abs_y = round(y_px * 32767 / 799)
```

## QMP Tap and Swipe Results

- `QMP` multitouch on the hybrid VM produced reproducible UI changes without touching the host cursor.
- Verified outcomes:
  - scroll inside `Settings` page
  - repeated upward swipe changed the visible content of the page
  - upward swipe from app card/home context opened the application grid
- Host-side screenshots of the hybrid SDL window confirmed these UI transitions.

### Practical direction

- The best emulator automation direction found so far is:
  - hybrid QEMU launch: `virtio-tablet-pci + virtio-multitouch-pci`
  - manual interaction through tablet
  - automated touch gestures through `QMP -> input-send-event -> mtt`
- This is currently the cleanest path that preserves manual usability while enabling real gesture automation.

## Stock-like `tablet-only + QMP` Experiment

- A separate tablet-only VM was launched manually with:
  - `-device virtio-tablet-pci`
  - no multitouch device
  - `QMP` enabled explicitly
- Inside the guest it exposed only:
  - `/dev/input/event1` = `QEMU Virtio Tablet`
  - `libinput` capability: `pointer`

### `QMP` over tablet-only device

- `QMP` `abs + btn(left)` events were accepted by QEMU.
- Observed behavior:
  - a simple `tap` did not behave like reliable touch input
  - the pointer/cursor became visible and moved
  - tablet-style drag from the homescreen/app-card context still caused a visible UI transition
  - in one verified case, upward drag opened the app grid

### Meaning

- `QMP` does not magically turn a tablet device into a touch device.
- On tablet-only VMs, `QMP` can still reproduce some pointer-driven system drags.
- However, this is weaker and less trustworthy than real multitouch:
  - no proper `TOUCH_*` semantics
  - behavior depends on context
  - not a clean replacement for `virtio-multitouch-pci`

### Practical conclusion

- `tablet-only + QMP` is interesting as a fallback experiment.
- `hybrid + QMP multitouch` remains the preferred approach for `audb`.

## Qt Creator `Pointing device mode`

- The `Touchpad` / `Mouse` toggle in Qt Creator does exist in SDK code:
  - `Sfdk::Emulator::setMouseInputMode(...)`
  - `Sfdk::EmulatorPrivate::updateConfigsForMouseInputMode(...)`
- It does **not** change the QEMU command line device type.
- The live SDK emulator still starts with:
  - `-device virtio-tablet-pci`

### What it actually changes

- `libSfdk` rewrites guest-side config files under emulator `vmshare`:
  - `60-emul-wayland-ui.conf`
  - `99-qemu-touch.rules`
- Relevant functions/symbols found in `libSfdk.so`:
  - `Sfdk::Config::CompositorConfig::switchMouseMode(...)`
  - `Sfdk::Config::ConfigQemuTouchRules::switchMouseMode(...)`

### Current observed state after changing mode in Qt Creator

- `60-emul-wayland-ui.conf` now has the touch plugin line commented out:

```text
#LIPSTICK_LIBINPUT_OPTIONS=-plugin VBoxTouch:qemu:evdev=/dev/input/event1
QT_QPA_EVDEV_MOUSE_PARAMETERS=/dev/nomouse
```

- `99-qemu-touch.rules` now has the libinput-ignore rule commented out:

```text
#ATTRS{name}=="QEMU Virtio Tablet", ENV{LIBINPUT_IGNORE_DEVICE}="1"
```

- Guest-side result on the running SDK emulator:
  - device is still `QEMU Virtio Tablet`
  - `libinput` sees only `pointer`
  - no `ABS_MT_*` support appears

### Practical meaning

- The Qt Creator mode switch changes how the existing tablet device is routed and handled.
- It does **not** upgrade the emulator to a real multitouch-backed input device.
- Because of that, it does not solve the real swipe problem for `audb`.

## Where `virtio-tablet-pci` Comes From

- The active SDK launcher does not read the pointing-device type from `emulator-setup.ini`.
- The QEMU command line is assembled inside `libSfdk.so`, in:
  - `Sfdk::QemuVirtualMachinePrivate::start(...)`
  - `Sfdk::(anonymous namespace)::QemuSystemArgumentsComposer::setDevice(...)`

### Evidence from `libSfdk.so`

- `QemuVirtualMachinePrivate::start(...)` builds the QEMU argument list and calls:
  - `QemuSystemArgumentsComposer::setDevice(QList<QString>&, bool) const`
- The same library contains embedded UTF-16 strings:

```text
virtio-%1%2,xres=%3,yres=%4
virtio-%1%2,xres=%3,yres=%4,max_outputs=2,xres2=%5,yres2=%6,id=video
virtio-tablet-pci
virtio-tablet-pci,display=video,head=0
virtio-tablet-pci,display=video,head=1
virtio-keyboard-pci
```

- No `virtio-multitouch-pci` string was found in `libSfdk.so`.

### Consequence

- In the stock Aurora SDK path, `virtio-tablet-pci` is hardcoded in the launcher logic itself.
- Changing emulator mode in Qt Creator cannot produce a multitouch QEMU device unless `libSfdk.so` is changed or replaced.

## Exact Commands

This section fixes the exact commands that are currently known to work on this machine.
The canonical profile is now the no-GL hybrid profile because it has been verified
to support:

- `QMP` tap
- `QMP` swipe
- `QMP` screendump

### Files

- Canonical hybrid VM launcher:
  - `/home/kotdath/omp/personal/rust/audb/scripts/run-aurora-hybrid-screendump.sh`
- Legacy GL hybrid VM launcher:
  - `/home/kotdath/omp/personal/rust/audb/scripts/run-aurora-hybrid.sh`
- QMP touch helper:
  - `/home/kotdath/omp/personal/rust/audb/scripts/qmp_touch.py`
- QMP screendump helper:
  - `/home/kotdath/omp/personal/rust/audb/scripts/qmp_screendump.py`
- End-to-end probe flow:
  - `/home/kotdath/omp/personal/rust/audb/scripts/qmp_probe_flow.sh`

### 1. Create the overlay disk explicitly

The launcher scripts create overlays automatically if they do not exist, but the
exact standalone commands are fixed here so the emulator can be recreated without
reading shell code.

Base image used by both manual profiles:

```bash
/home/kotdath/AuroraOS/share/qemu/bin/qemu-img create \
  -f qcow2 -F qcow2 \
  -b /home/kotdath/AuroraOS/emulator/AuroraOS-5.2.0.180/image.qcow2 \
  /home/kotdath/AuroraOS/emulator/overlay_test.qcow2
```

Canonical no-GL screendump profile overlay:

```bash
/home/kotdath/AuroraOS/share/qemu/bin/qemu-img create \
  -f qcow2 -F qcow2 \
  -b /home/kotdath/AuroraOS/emulator/AuroraOS-5.2.0.180/image.qcow2 \
  /home/kotdath/AuroraOS/emulator/overlay_screendump.qcow2
```

If the file already exists and a clean overlay is needed, remove it first:

```bash
rm -f /home/kotdath/AuroraOS/emulator/overlay_screendump.qcow2
```

Then recreate it with the command above.

### 2. Start the canonical working hybrid emulator

This launches the manual no-GL QEMU instance with:

- `virtio-tablet-pci`
- `virtio-multitouch-pci`
- `QMP` on `/tmp/aurora-screendump.qmp`
- SSH forward on host port `33223`
- overlay disk `/home/kotdath/AuroraOS/emulator/overlay_screendump.qcow2`
- `virtio-vga`
- `-display sdl,show-cursor=off`

Command:

```bash
/home/kotdath/omp/personal/rust/audb/scripts/run-aurora-hybrid-screendump.sh
```

To print the fully expanded QEMU command line without starting the VM:

```bash
/home/kotdath/omp/personal/rust/audb/scripts/run-aurora-hybrid-screendump.sh --print-only
```

### 3. Verify that the QMP socket exists

```bash
ls -l /tmp/aurora-screendump.qmp
```

### 4. Connect to the guest over SSH

```bash
ssh -i /home/kotdath/AuroraOS/vmshare/ssh/private_keys/sdk -p 33223 defaultuser@127.0.0.1
```

### 5. Verify that hybrid input devices are present in the guest

Run inside the guest:

```bash
libinput list-devices
```

Expected shape:

- `QEMU Virtio Tablet` with capability `pointer`
- `QEMU Virtio MultiTouch` with capability `pointer touch`

Typical device mapping observed in the working hybrid VM:

- `/dev/input/event1` = `QEMU Virtio Tablet`
- `/dev/input/event2` = `QEMU Virtio MultiTouch`
- `/dev/input/event3` = `QEMU Virtio Keyboard`

### 6. Host-side QMP tap

Tap near screen center:

```bash
python3 /home/kotdath/omp/personal/rust/audb/scripts/qmp_touch.py \
  --socket /tmp/aurora-screendump.qmp tap --at 180,400
```

Tap lower on the screen:

```bash
python3 /home/kotdath/omp/personal/rust/audb/scripts/qmp_touch.py \
  --socket /tmp/aurora-screendump.qmp tap --at 180,700
```

Longer press:

```bash
python3 /home/kotdath/omp/personal/rust/audb/scripts/qmp_touch.py \
  --socket /tmp/aurora-screendump.qmp tap --at 180,400 --hold-ms 90
```

### 7. Host-side QMP swipe

Ready-made upward swipe from bottom to top:

```bash
python3 /home/kotdath/omp/personal/rust/audb/scripts/qmp_touch.py \
  --socket /tmp/aurora-screendump.qmp swipe-up
```

Explicit upward swipe with coordinates:

```bash
python3 /home/kotdath/omp/personal/rust/audb/scripts/qmp_touch.py \
  --socket /tmp/aurora-screendump.qmp \
  swipe --from 180,790 --to 180,60 --steps 36 --duration-ms 700 --hold-ms 160
```

These commands are intended for the `360x800` Aurora emulator viewport.

### 8. Host-side QMP screendump

```bash
python3 /home/kotdath/omp/personal/rust/audb/scripts/qmp_screendump.py \
  --socket /tmp/aurora-screendump.qmp \
  --output /tmp/aurora-screendump-test.png
```

### 9. QMP touch sequence that worked

The working host-side gesture path is:

- `mtt(begin, slot, tracking_id, axis=x, value=start_x)`
- `btn(touch, down=true)`
- `mtt(data, axis=x, value=...)`
- `mtt(data, axis=y, value=...)`
- repeated `mtt(update, ...) + btn(touch,true) + mtt(data,x) + mtt(data,y)` frames
- `mtt(end, slot, tracking_id=-1, axis=x, value=end_x)`

Current implementation of this flow lives in:

- `/home/kotdath/omp/personal/rust/audb/scripts/qmp_touch.py`

### 10. Coordinate conversion used by the QMP helper

For viewport `360x800`, the helper converts screen pixels to QMP absolute coordinates with:

```text
abs_x = round(x_px * 32767 / 359)
abs_y = round(y_px * 32767 / 799)
```

### 11. Guest-side verification of incoming multitouch events

Run inside the guest:

```bash
libinput debug-events --verbose --device /dev/input/event2
```

Then, from the host, run a tap or swipe command from this section.

Expected event shapes in the guest:

- `TOUCH_MOTION`
- `TOUCH_UP`

### 12. Confirmed end-to-end probe flow

The following sequence has been confirmed on the canonical no-GL profile:

1. Send upward swipe:

```bash
python3 /home/kotdath/omp/personal/rust/audb/scripts/qmp_touch.py \
  --socket /tmp/aurora-screendump.qmp swipe-up
```

2. Take screenshot:

```bash
python3 /home/kotdath/omp/personal/rust/audb/scripts/qmp_screendump.py \
  --socket /tmp/aurora-screendump.qmp \
  --output /tmp/aurora-after-swipe.png
```

3. In the captured frame, the bottom dock was visible. The second icon from the left
   (gear/settings-like icon) was then tapped at:

```text
134,746
```

4. Send tap:

```bash
python3 /home/kotdath/omp/personal/rust/audb/scripts/qmp_touch.py \
  --socket /tmp/aurora-screendump.qmp tap --at 134,746 --hold-ms 90
```

5. Take screenshot again:

```bash
python3 /home/kotdath/omp/personal/rust/audb/scripts/qmp_screendump.py \
  --socket /tmp/aurora-screendump.qmp \
  --output /tmp/aurora-after-tap.png
```

Observed result:

- first screenshot showed the homescreen/dock after the swipe
- second screenshot showed a different UI scene, confirming that the tap changed state

The same confirmed sequence is automated in:

- `/home/kotdath/omp/personal/rust/audb/scripts/qmp_probe_flow.sh`

### 12. What is not the working path

These are not the preferred path for reliable gesture automation:

- stock SDK emulator started by `sfdk`
- Qt Creator `Pointing device mode` toggle
- `tablet-only + QMP`
- guest-side direct drag over `QEMU Virtio Tablet`

### 13. Current practical recommendation

For `audb` emulator automation on this machine, the canonical setup is:

- launch `/home/kotdath/omp/personal/rust/audb/scripts/run-aurora-hybrid-screendump.sh`
- send touch with `/home/kotdath/omp/personal/rust/audb/scripts/qmp_touch.py --socket /tmp/aurora-screendump.qmp`
- take screenshots with `/home/kotdath/omp/personal/rust/audb/scripts/qmp_screendump.py --socket /tmp/aurora-screendump.qmp`
- use the GL launcher only as a legacy comparison profile, not as the canonical one

## QMP Screendump Status

### Current result for the original GL hybrid config

The original working touch config:

- `/home/kotdath/omp/personal/rust/audb/scripts/run-aurora-hybrid.sh`
- uses `virtio-vga-gl`
- uses `-display sdl,gl=on,show-cursor=off`

On this config, `QMP screendump` is available but fails with:

```text
GenericError: no surface
```

### Working screendump experiment config

A second launch profile was added specifically for `QMP screendump` experiments:

- `/home/kotdath/omp/personal/rust/audb/scripts/run-aurora-hybrid-screendump.sh`

Its effective differences are:

- `VGA_DEVICE=virtio-vga`
- `DISPLAY_OPTS=sdl,show-cursor=off`
- no `gl=on`
- separate overlay:
  - `/home/kotdath/AuroraOS/emulator/overlay_screendump.qcow2`
- separate SSH port:
  - `33223`
- separate QMP socket:
  - `/tmp/aurora-screendump.qmp`

### Exact commands for the working screendump config

Start the VM:

```bash
/home/kotdath/omp/personal/rust/audb/scripts/run-aurora-hybrid-screendump.sh
```

Take a screenshot through QMP:

```bash
python3 /home/kotdath/omp/personal/rust/audb/scripts/qmp_screendump.py \
  --socket /tmp/aurora-screendump.qmp \
  --output /tmp/aurora-screendump-test.png
```

### Verified result

The command above was tested on this machine and succeeded:

- `query-display-options` returned:
  - `{"type":"sdl","show-cursor":false}`
- output file was created:
  - `/tmp/aurora-screendump-test.png`
- observed size:
  - `262820` bytes

### Practical meaning

- `QMP screendump` fails on the GL hybrid config with `no surface`
- `QMP tap`, `QMP swipe`, and `QMP screendump` all work on the no-GL hybrid config
- the important difference is that `sdl + gl=on + virtio-vga-gl` gave `no surface`, while `sdl + virtio-vga` worked
