# Virtual high-refresh display for Lunaris capture (NVIDIA, X11)

## Why

The host's physical primary panel **DP-1 is 60Hz** (its xrandr mode list has no
mode above 60Hz at 1920x1080). NvFBC captures the framebuffer, which a 60Hz
panel only updates 60 times/second, so the stream can never exceed **60 unique
frames/sec** no matter what FPS the client requests. Forcing `xrandr --rate 240`
on DP-1 does nothing — the driver clamps it back to 60.

This config forces the **disconnected DP-2 connector** to appear as a connected
monitor that supports **1920x1080 @ 144Hz and 240Hz**, with no physical panel.
The GPU then composites that output at the high rate and NvFBC can capture real
>60fps from it.

> **Reality check:** you only get N *unique* frames/sec if something actually
> renders at N fps onto DP-2 (e.g. a game with vsync/uncapped fps running on that
> screen). A static desktop still only changes on redraw. To make the stream
> always *emit* the target FPS (padding duplicate frames), also run the agent
> with `LUNARIS_CONSTANT_FPS=1`.

## Files

| File | Purpose |
|------|---------|
| `20-lunaris-virtual-dp2.conf` | Xorg drop-in: forces DP-2 + 144/240Hz modelines |
| `apply.sh`  | Installs the drop-in into `/etc/X11/xorg.conf.d/` (sudo) |
| `revert.sh` | Removes it |

## Apply

```bash
scripts/virtual-display/apply.sh
# then restart X:
sudo systemctl restart display-manager     # or just log out and back in
```

After X restarts:

```bash
xrandr | grep -A6 'DP-2'          # DP-2 should now be "connected" with 144/240 modes
xrandr --output DP-2 --mode 1920x1080_240.00 --right-of DP-1
```

Then in the Lunaris client pick **display DP-2** and the FPS you want. Move the
window/game you want to stream onto the DP-2 area (it's an extended screen).

## ⚠️ RECOVERY (if X fails to start after the restart)

A bad Xorg config can leave you at a black screen. To recover:

1. Switch to a text console: **Ctrl+Alt+F3** (try F2–F6).
2. Log in, then remove the config:
   ```bash
   sudo rm /etc/X11/xorg.conf.d/20-lunaris-virtual-dp2.conf
   sudo systemctl restart display-manager
   ```
   (or run `scripts/virtual-display/revert.sh`).

Keep a phone/second device with these steps handy the first time you apply it.

## Notes / tuning

- Modelines were generated with `cvt 1920 1080 144` and `cvt 1920 1080 240`.
  Regenerate for other resolutions/rates and edit the `.conf`.
- Driver validated: NVIDIA 595.x open kernel module. Older drivers may need a
  real `CustomEDID` binary instead of `AllowNonEdidModes`.
- If DP-2 still shows as disconnected after restart, your driver may not accept
  `ConnectedMonitor` on that connector — use a **dummy HDMI/DP plug** (EDID
  emulator, ~$5) on any free port instead; it needs no config changes.
- The built-in "Virtual Display" toggle in the client is a *different*
  mechanism: it looks for an output literally named `VIRTUAL*` (Xorg
  VirtualHeads), which the NVIDIA driver does not expose — that is why it errors
  with "No disconnected VIRTUAL output found". Use the DP-2 approach here instead.
