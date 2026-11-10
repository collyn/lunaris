# Third-party components in the Lunaris Agent AppImage

The Lunaris Agent AppImage bundles the libraries below. None of them is GPL or
"nonfree"; the LGPL libraries are dynamically linked shared objects that you may
replace (extract the AppImage with `--appimage-extract`, swap the `.so` files in
`usr/lib`, and run `AppRun`). The full licence texts are in this directory.

| Component | Licence | Source |
|-----------|---------|--------|
| Qt 6.8 (Core, Gui, Qml, Quick, QuickControls2, Widgets, WaylandClient, DBus, plugins) | LGPL-3.0 | https://download.qt.io/official_releases/qt/6.8/ |
| FFmpeg 8.0 (libavcodec, libavutil, libswscale, …), built without `--enable-gpl` / `--enable-nonfree` | LGPL-2.1-or-later | https://github.com/FFmpeg/FFmpeg/tree/n8.0 |
| libva 2.22 | MIT | https://github.com/intel/libva |
| oneVPL dispatcher (libvpl) 2.14 | MIT | https://github.com/intel/libvpl |
| SVT-AV1 2.3 (statically linked into libavcodec) | BSD-3-Clause-Clear + AOM Patent License 1.0 (royalty-free) | https://gitlab.com/AOMediaCodec/SVT-AV1 |
| nv-codec-headers 12.2 (headers only) | MIT | https://github.com/FFmpeg/nv-codec-headers |
| Other system libraries copied from Ubuntu 22.04 (e.g. libopus, OpenSSL, ICU, xkbcommon, fontconfig) | BSD / Apache-2.0 / Unicode / MIT | https://packages.ubuntu.com/jammy/ |

FFmpeg configure line (for rebuilding the same LGPL libraries): see
`scripts/packaging/Containerfile.appimage` in the Lunaris repository.

Hardware encoders (VAAPI, NVENC, Quick Sync) run in the GPU vendor's driver,
which is not bundled. The only bundled software video encoder is SVT-AV1 (AV1,
royalty-free); no H.264/H.265 software encoder is included.
