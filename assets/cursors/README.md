# Remote cursor images

Shown by the web and desktop clients when the host doesn't send its own
cursor image. They are the **Bibata Modern Classic** cursors by Abdulkaiz
Khatri (https://github.com/ful1e5/Bibata_Cursor, commit `ddb09273cde4`),
licensed under GPL-3.0 — the `svg/modern` sources with the "Classic"
colours from `render.json` applied and the canvas declared as 32×32.

| File | Bibata source | Hotspot (32 px) |
|------|---------------|-----------------|
| arrow.svg | left_ptr | 7, 2 |
| hand.svg | hand2 | 14, 2 |
| ibeam.svg | xterm | 16, 16 |
| cross.svg | cross | 16, 16 |
| move.svg | move | 16, 16 |
| resize-ns.svg | sb_v_double_arrow | 16, 16 |
| resize-ew.svg | sb_h_double_arrow | 16, 16 |
| resize-nesw.svg | fd_double_arrow | 16, 16 |
| resize-nwse.svg | bd_double_arrow | 16, 16 |
| unavailable.svg | crossed_circle | 16, 16 |

After editing, run `scripts/brand/generate-cursors.sh`: it copies the SVGs to
`web/public/cursors/` and renders the 64 px PNGs the Qt client embeds.
