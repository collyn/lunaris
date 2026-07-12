#!/usr/bin/env python3
"""Generate a valid 128-byte EDID advertising 1920x1080 @ 144/120/60 Hz.

Used as a CustomEDID for a forced virtual display (e.g. NVIDIA DP-2) so the
capture pipeline has a real high-refresh source. No external tools needed except
`cvt` (for the modelines); the script self-verifies header + checksum and prints
the decoded detailed timings so you can eyeball them.

Usage: python3 gen_edid.py [output.bin]
"""
import subprocess
import sys


def cvt_modeline(w, h, r, reduced=False):
    # EDID detailed-timing pixel clock is 16-bit in 10 kHz units (max 655.35 MHz).
    # Standard CVT for 1080p@240 is ~809 MHz which overflows it, so high rates
    # must use reduced blanking (`cvt -r`, ~606 MHz) to fit.
    args = ["cvt", "-r", str(w), str(h), str(r)] if reduced else ["cvt", str(w), str(h), str(r)]
    out = subprocess.check_output(args, text=True)
    for line in out.splitlines():
        line = line.strip()
        if line.startswith("Modeline"):
            toks = line.split()
            # Modeline "name" pclk hdisp hss hse htot vdisp vss vse vtot -hsync +vsync
            pclk = float(toks[2])
            nums = list(map(int, toks[3:11]))
            hdisp, hss, hse, htot, vdisp, vss, vse, vtot = nums
            hsync_pos = "+hsync" in line.lower()
            vsync_pos = "+vsync" in line.lower()
            return dict(pclk=pclk, hdisp=hdisp, hss=hss, hse=hse, htot=htot,
                        vdisp=vdisp, vss=vss, vse=vse, vtot=vtot,
                        hpos=hsync_pos, vpos=vsync_pos)
    raise RuntimeError("cvt returned no Modeline")


def dtd(m, hmm=527, vmm=296):
    """Build an 18-byte Detailed Timing Descriptor from a modeline dict."""
    pclk10 = round(m["pclk"] * 100)  # in 10 kHz units
    hact, htot = m["hdisp"], m["htot"]
    vact, vtot = m["vdisp"], m["vtot"]
    hblank = htot - hact
    vblank = vtot - vact
    hfp = m["hss"] - hact
    hsw = m["hse"] - m["hss"]
    vfp = m["vss"] - vact
    vsw = m["vse"] - m["vss"]

    b = [0] * 18
    b[0] = pclk10 & 0xFF
    b[1] = (pclk10 >> 8) & 0xFF
    b[2] = hact & 0xFF
    b[3] = hblank & 0xFF
    b[4] = ((hact >> 8) << 4) | ((hblank >> 8) & 0x0F)
    b[5] = vact & 0xFF
    b[6] = vblank & 0xFF
    b[7] = ((vact >> 8) << 4) | ((vblank >> 8) & 0x0F)
    b[8] = hfp & 0xFF
    b[9] = hsw & 0xFF
    b[10] = ((vfp & 0x0F) << 4) | (vsw & 0x0F)
    b[11] = (((hfp >> 8) & 0x3) << 6) | (((hsw >> 8) & 0x3) << 4) \
        | (((vfp >> 4) & 0x3) << 2) | ((vsw >> 4) & 0x3)
    b[12] = hmm & 0xFF
    b[13] = vmm & 0xFF
    b[14] = ((hmm >> 8) << 4) | ((vmm >> 8) & 0x0F)
    b[15] = 0  # h border
    b[16] = 0  # v border
    # flags: digital separate sync (bits4:3=11), vsync pol bit2, hsync pol bit1
    b[17] = 0x18 | (0x04 if m["vpos"] else 0) | (0x02 if m["hpos"] else 0)
    return b


def text_descriptor(tag, text):
    """18-byte display descriptor: 00 00 00 <tag> 00 <13 bytes text/pad>."""
    body = text.encode("ascii")[:13]
    body = body + b"\x0a" + b"\x20" * (13 - len(body) - 1) if len(body) < 13 else body
    return [0x00, 0x00, 0x00, tag, 0x00] + list(body[:13])


def main():
    out_path = sys.argv[1] if len(sys.argv) > 1 else "dp2-1080p-highrefresh.bin"

    m240 = cvt_modeline(1920, 1080, 240, reduced=True)  # reduced blanking to fit DTD pclk
    m144 = cvt_modeline(1920, 1080, 144)
    m120 = cvt_modeline(1920, 1080, 120)
    m60 = cvt_modeline(1920, 1080, 60)

    e = [0] * 128
    # Header
    e[0:8] = [0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00]
    # Manufacturer "LUN" (5-bit letters, A=1)
    mfg = (ord('L') - 64) << 10 | (ord('U') - 64) << 5 | (ord('N') - 64)
    e[8] = (mfg >> 8) & 0xFF
    e[9] = mfg & 0xFF
    e[10], e[11] = 0x44, 0x01          # product code 0x0144
    e[12:16] = [0x01, 0x00, 0x00, 0x00]  # serial
    e[16] = 1                            # week
    e[17] = 2026 - 1990                  # year
    e[18], e[19] = 1, 4                  # EDID 1.4
    e[20] = 0xA5                         # digital, 8bpc, DisplayPort
    e[21], e[22] = 53, 30                # screen size cm
    e[23] = 0x78                         # gamma 2.2
    e[24] = 0x02                         # preferred timing is native; non-continuous
    # Chromaticity (standard sRGB)
    coords = dict(rx=0.640, ry=0.330, gx=0.300, gy=0.600,
                  bx=0.150, by=0.060, wx=0.3127, wy=0.3290)
    q = {k: min(1023, round(v * 1024)) for k, v in coords.items()}
    e[25] = ((q['rx'] & 3) << 6) | ((q['ry'] & 3) << 4) | ((q['gx'] & 3) << 2) | (q['gy'] & 3)
    e[26] = ((q['bx'] & 3) << 6) | ((q['by'] & 3) << 4) | ((q['wx'] & 3) << 2) | (q['wy'] & 3)
    e[27] = q['rx'] >> 2
    e[28] = q['ry'] >> 2
    e[29] = q['gx'] >> 2
    e[30] = q['gy'] >> 2
    e[31] = q['bx'] >> 2
    e[32] = q['by'] >> 2
    e[33] = q['wx'] >> 2
    e[34] = q['wy'] >> 2
    # Established timings: 640x480@60, 800x600@60, 1024x768@60
    e[35], e[36], e[37] = 0x21, 0x08, 0x00
    # Standard timings: first = 1920x1080@60, rest unused
    e[38], e[39] = 0xD1, 0x80
    for i in range(40, 54, 2):
        e[i], e[i + 1] = 0x01, 0x01
    # Four 18-byte descriptors — all detailed timings so every rate the UI
    # offers (240/144/120/60) is a real EDID mode. The monitor-name descriptor
    # is dropped to free the 4th slot (name is cosmetic, not required).
    e[54:72] = dtd(m240)                                   # preferred: 240 Hz
    e[72:90] = dtd(m144)                                   # 144 Hz
    e[90:108] = dtd(m120)                                  # 120 Hz
    e[108:126] = dtd(m60)                                  # 60 Hz
    e[126] = 0                                             # no extensions
    e[127] = (256 - (sum(e[:127]) % 256)) % 256            # checksum

    assert len(e) == 128
    with open(out_path, "wb") as f:
        f.write(bytes(e))

    # ---- self verification ----
    assert e[0:8] == [0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00], "bad header"
    assert sum(e) % 256 == 0, "bad checksum"

    def decode_dtd(b):
        pclk = ((b[1] << 8) | b[0]) * 10 / 1000.0
        hact = ((b[4] >> 4) << 8) | b[2]
        vact = ((b[7] >> 4) << 8) | b[5]
        htot = hact + ((((b[4] & 0xF) << 8) | b[3]))
        vtot = vact + ((((b[7] & 0xF) << 8) | b[6]))
        refresh = pclk * 1e6 / (htot * vtot)
        return hact, vact, refresh

    print(f"Wrote {out_path} ({len(e)} bytes) — header OK, checksum OK (0x{e[127]:02X})")
    for slot in (slice(54, 72), slice(72, 90), slice(90, 108), slice(108, 126)):
        h, v, r = decode_dtd(e[slot])
        print(f"  DTD: {h}x{v} @ {r:.2f} Hz")


if __name__ == "__main__":
    main()
