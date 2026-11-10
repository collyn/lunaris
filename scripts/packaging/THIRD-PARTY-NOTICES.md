# Third-party components in Lunaris packages

Lunaris is free software, licensed under the GNU General Public License v3.0 or
later (`GPL-3.0-or-later`, see `LICENSE`). Its corresponding source code is the
Lunaris repository at the tag of the release: https://github.com/collyn/lunaris
(plus https://github.com/collyn/lunaris-media at the revision pinned in
`Cargo.lock`).

The prebuilt packages also contain the third-party components below. All are
GPL-3.0-compatible; nothing "nonfree" is included. The LGPL libraries are
dynamically linked shared objects that you may replace. Licence texts are
shipped next to this file (AppImage: `usr/share/licenses/`).

## Linux agent AppImage

| Component | Licence | Source |
|-----------|---------|--------|
| Qt 6.8 (Core, Gui, Qml, Quick, QuickControls2, Widgets, WaylandClient, DBus, plugins) | LGPL-3.0 | https://download.qt.io/official_releases/qt/6.8/ |
| FFmpeg 8.1 (libavcodec, libavutil, libswscale, …), built with `--enable-gpl` | GPL-2.0-or-later (as built) | https://github.com/FFmpeg/FFmpeg/tree/n8.1 |
| x264 (stable branch; commit in `x264/VERSION`), statically linked into libavcodec | GPL-2.0-or-later | https://code.videolan.org/videolan/x264 |
| SVT-AV1 2.3, statically linked into libavcodec | BSD-3-Clause-Clear + AOM Patent License 1.0 (royalty-free) | https://gitlab.com/AOMediaCodec/SVT-AV1 |
| libva 2.22 | MIT | https://github.com/intel/libva |
| oneVPL dispatcher (libvpl) 2.14 | MIT | https://github.com/intel/libvpl |
| nv-codec-headers 12.2 (headers only) | MIT | https://github.com/FFmpeg/nv-codec-headers |
| Other libraries copied from Ubuntu 22.04 (e.g. libopus, OpenSSL, ICU, xkbcommon, fontconfig) | BSD / Apache-2.0 / Unicode / MIT | https://packages.ubuntu.com/jammy/ |

The exact build recipe (versions and FFmpeg `configure` flags) is
`scripts/packaging/Containerfile.appimage` in the Lunaris repository.

## Windows packages

| Component | Licence | Source |
|-----------|---------|--------|
| Qt 6 (deployed with `windeployqt`) | LGPL-3.0 | https://download.qt.io/official_releases/qt/ |
| FFmpeg 8.1 "gpl-shared" build by BtbN (includes x264, x265, SVT-AV1, …) | GPL-3.0-or-later (as built) | https://github.com/BtbN/FFmpeg-Builds (sources: https://github.com/FFmpeg/FFmpeg/tree/n8.1) |
| opus, OpenSSL (from vcpkg) | BSD-3-Clause / Apache-2.0 | https://github.com/microsoft/vcpkg |

## Hardware encoders and drivers

Hardware encoders (VAAPI, NVENC, AMF, Quick Sync) and NvFBC run in the GPU
vendor's driver, which is not bundled; Lunaris loads those libraries at
runtime.
