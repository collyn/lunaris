# Virtual high-refresh display for Lunaris capture (NVIDIA, X11)

## Why

The host's physical primary panel **DP-1 is 60Hz** (its xrandr mode list has no
mode above 60Hz at 1920x1080). NvFBC captures the framebuffer, which a 60Hz
panel only updates 60 times/second, so the stream can never exceed **60 unique
frames/sec** no matter what FPS the client requests. Forcing `xrandr --rate 240`
on DP-1 does nothing — the driver clamps it back to 60 (the agent now logs this
honestly instead of claiming success).

This setup forces the **disconnected DP-2 connector** to appear as a connected
monitor with a **real EDID advertising 1920x1080 @ 144/120/60 Hz**, with no
physical panel. The GPU composites that output at the high rate and NvFBC can
capture genuine >60fps from it.

> **Reality check:** you only get N *unique* frames/sec if something actually
> renders at N fps onto DP-2 (a game with vsync/uncapped fps on that screen). A
> static desktop still only changes on redraw. To make the stream always *emit*
> the target FPS (padding duplicate frames), run the agent with
> `LUNARIS_CONSTANT_FPS=1` (this is now the default).

## Automatic setup (recommended)

You normally don't need to run these scripts by hand. The first time you enable
the virtual screen from the app UI, the **agent auto-installs** this Xorg config
+ EDID via `pkexec` (a graphical auth prompt) — it detects the currently
connected outputs and builds the `ConnectedMonitor` list for you. After a single
**log out / log back in** (or X restart), the virtual output stays forced across
reboots and enable/disable is pure UI. The scripts below are the manual
fallback (e.g. headless hosts with no polkit agent).

## ⚠️ The `ConnectedMonitor` footgun

On NVIDIA, `Option "ConnectedMonitor"` **replaces** the entire connected-monitor
list — it does not add to it. An earlier version that set only
`ConnectedMonitor "DP-2"` **disabled the real monitors** (DP-1/HDMI-0 went dark).
The config here therefore lists **all** outputs you want active:
`ConnectedMonitor "DP-1, HDMI-0, DP-2"`. **Edit that line to match your own
connected outputs** (`xrandr --query`) before applying.

## Files

| File | Purpose |
|------|---------|
| `gen_edid.py` | Generates the EDID (`dp2-1080p-highrefresh.bin`), self-verified |
| `dp2-1080p-highrefresh.bin` | 128-byte EDID advertising 1080p @ 144/120/60 |
| `20-lunaris-virtual-dp2.conf` | Xorg drop-in: CustomEDID + ConnectedMonitor |
| `apply.sh`  | Installs EDID + drop-in (sudo), with confirmation |
| `revert.sh` | Removes both |
| `activate-dp2.sh` | Runtime: sets DP-2 mode/rate, keeps DP-1 primary |

## Apply

```bash
# (optional) regenerate the EDID, e.g. for a different resolution:
python3 scripts/virtual-display/gen_edid.py scripts/virtual-display/dp2-1080p-highrefresh.bin

scripts/virtual-display/apply.sh          # installs EDID + config (asks to confirm)
sudo systemctl restart display-manager    # or log out and back in
```

After X restarts:

```bash
xrandr | grep -A8 'DP-2'                   # DP-2 connected with 143.88/119.93/59.96
xrandr --query | grep -E ' connected'      # DP-1 and HDMI-0 must still be there
scripts/virtual-display/activate-dp2.sh 144 # set 144Hz, keep DP-1 primary
```

Then in the Lunaris client pick **display DP-2** and the FPS you want, and run
the content you want to stream on the DP-2 screen area (it's an extended screen).

## ⚠️ RECOVERY (if X fails to start, or a monitor goes dark)

A bad Xorg config can leave you at a black screen. To recover:

1. Switch to a text console: **Ctrl+Alt+F3** (try F2–F6).
2. Log in, then remove the config and restart X:
   ```bash
   sudo scripts/virtual-display/revert.sh
   sudo systemctl restart display-manager
   ```

Keep a phone/second device with these steps handy the first time you apply it.

## If it still doesn't work

Pure-software forcing on NVIDIA is finicky and driver-version dependent. If DP-2
stays disconnected, or won't accept the 144Hz mode, the **most reliable** fix is
a **dummy HDMI/DisplayPort plug** (EDID emulator, ~$5) on any free port — it
presents a real high-refresh EDID with zero config and cannot disable your real
monitors. Pick one rated for 1080p@120/144.

## Notes

- Driver validated: NVIDIA 595.x open kernel module, Xorg, X11.
- The EDID advertises 1920x1080 @ **240 / 144 / 120 / 60 Hz** (240 is the preferred
  mode). 240 uses reduced-blanking timing (`cvt -r`, ~606 MHz) because a standard
  1080p240 pixel clock (~809 MHz) overflows the EDID detailed-timing field.
  Edit `gen_edid.py` for other resolutions/rates.
- The built-in "Virtual Display" toggle in the client is a *different* mechanism:
  it looks for an output literally named `VIRTUAL*` (Xorg VirtualHeads), which the
  NVIDIA driver does not expose — that is why it errors with "No disconnected
  VIRTUAL output found". Use this DP-2 CustomEDID approach instead.

## Windows

This directory (Xorg config + EDID) is **Linux-only**. On Windows the same
"Virtual Screen" UI toggle works through an **IddCx virtual display driver**
(handled by the agent via `lunaris-media`), which is hot-pluggable — a virtual
monitor is created/destroyed at runtime with **no reboot**. The only one-time
prerequisite is installing an IddCx driver on the host:

- [usbmmidd / IddSampleDriver](https://github.com/ge9/IddSampleDriver)
- [Virtual Display Driver (VDD)](https://github.com/itsmikethetech/Virtual-Display-Driver) — supports up to 240Hz

After the driver is installed, Enable/Disable + refresh selection in the app just
work; no scripts and no `xorg.conf`.

