#!/usr/bin/env bash
# Cross-compiles a trimmed FFmpeg 8.1 for Windows x64 (shared DLLs + MSVC import
# .lib files) on Linux with mingw-w64 — what the Windows agent and client ship
# instead of a full "gpl-shared" FFmpeg build:
#   encoders  NVENC, AMF (h264/hevc/av1), x264, SVT-AV1
#   decoders  h264, hevc, av1 (+ dav1d) with D3D11VA / DXVA2 / NVDEC
#   swscale and the bitstream filters the pipelines use.
# x264, SVT-AV1 and dav1d are linked into the DLLs, and libgcc/winpthread are
# linked statically, so the DLLs only depend on system DLLs. (Intel Quick Sync
# is handled by the agent's native Windows encoder, not through FFmpeg.)
#
# Needs (Debian/Ubuntu): build-essential (host gcc for FFmpeg's build tools), mingw-w64,
#                        nasm, meson, ninja-build, cmake, git, pkg-config
# Usage: build-ffmpeg-mingw.sh <output dir>   → <out>/{bin,lib,include,share/licenses}
set -euo pipefail

OUT="$(mkdir -p "$1" && cd "$1" && pwd)"
HOST=x86_64-w64-mingw32
JOBS="$(nproc)"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
cd "$WORK"

NVCODEC_VERSION=n12.2.72.0
AMF_VERSION=v1.4.36
X264_BRANCH=stable
SVTAV1_VERSION=v2.3.0
DAV1D_VERSION=1.5.1
FFMPEG_VERSION=n8.1

DEPS="$WORK/deps"            # static dependencies, not shipped
export PKG_CONFIG_PATH="$DEPS/lib/pkgconfig"
export PKG_CONFIG_LIBDIR="$DEPS/lib/pkgconfig"
LIC="$OUT/share/licenses"
mkdir -p "$LIC" "$DEPS"

clone() { git clone -q --depth 1 -b "$2" "$1" "$3"; }

cat > cross.cmake <<EOF
set(CMAKE_SYSTEM_NAME Windows)
set(CMAKE_SYSTEM_PROCESSOR x86_64)
set(CMAKE_C_COMPILER $HOST-gcc)
set(CMAKE_CXX_COMPILER $HOST-g++)
set(CMAKE_RC_COMPILER $HOST-windres)
set(CMAKE_FIND_ROOT_PATH /usr/$HOST $DEPS)
set(CMAKE_FIND_ROOT_PATH_MODE_PROGRAM NEVER)
set(CMAKE_FIND_ROOT_PATH_MODE_LIBRARY ONLY)
set(CMAKE_FIND_ROOT_PATH_MODE_INCLUDE ONLY)
EOF

echo "== nv-codec-headers $NVCODEC_VERSION"
clone https://github.com/FFmpeg/nv-codec-headers.git "$NVCODEC_VERSION" nvcodec
make -C nvcodec PREFIX="$DEPS" install >/dev/null
install -Dm644 nvcodec/README "$LIC/nv-codec-headers/README"

echo "== AMF headers $AMF_VERSION"
git clone -q --depth 1 -b "$AMF_VERSION" --filter=blob:none --sparse \
    https://github.com/GPUOpen-LibrariesAndSDKs/AMF.git amf
git -C amf sparse-checkout set amf/public/include
mkdir -p "$DEPS/include/AMF"
cp -r amf/amf/public/include/* "$DEPS/include/AMF/"
install -Dm644 amf/LICENSE.txt "$LIC/amf/LICENSE.txt"

echo "== x264 ($X264_BRANCH)"
clone https://code.videolan.org/videolan/x264.git "$X264_BRANCH" x264
(cd x264 && ./configure --host="$HOST" --cross-prefix="$HOST-" --prefix="$DEPS" \
    --enable-static --disable-cli --enable-pic >/dev/null \
    && make -j"$JOBS" >/dev/null && make install >/dev/null)
install -Dm644 x264/COPYING "$LIC/x264/COPYING"
git -C x264 rev-parse HEAD > "$LIC/x264/VERSION"

echo "== SVT-AV1 $SVTAV1_VERSION"
clone https://gitlab.com/AOMediaCodec/SVT-AV1.git "$SVTAV1_VERSION" svtav1
cmake -S svtav1 -B svtav1/build -G Ninja -DCMAKE_TOOLCHAIN_FILE="$WORK/cross.cmake" \
    -DCMAKE_BUILD_TYPE=Release -DCMAKE_INSTALL_PREFIX="$DEPS" -DCMAKE_INSTALL_LIBDIR=lib \
    -DBUILD_SHARED_LIBS=OFF -DBUILD_APPS=OFF -DBUILD_DEC=OFF -DBUILD_TESTING=OFF \
    -DSVT_AV1_LTO=OFF -DCMAKE_INTERPROCEDURAL_OPTIMIZATION=OFF -DCMAKE_POLICY_VERSION_MINIMUM=3.5 >/dev/null
ninja -C svtav1/build install >/dev/null
install -Dm644 svtav1/LICENSE.md "$LIC/svt-av1/LICENSE.md"
install -Dm644 svtav1/PATENTS.md "$LIC/svt-av1/PATENTS.md"

echo "== dav1d $DAV1D_VERSION"
clone https://code.videolan.org/videolan/dav1d.git "$DAV1D_VERSION" dav1d
meson setup dav1d/build dav1d --cross-file dav1d/package/crossfiles/x86_64-w64-mingw32.meson \
    --prefix="$DEPS" --libdir=lib --buildtype=release --default-library=static \
    -Denable_tools=false -Denable_tests=false >/dev/null
ninja -C dav1d/build install >/dev/null
install -Dm644 dav1d/COPYING "$LIC/dav1d/COPYING"

echo "== FFmpeg $FFMPEG_VERSION"
clone https://github.com/FFmpeg/FFmpeg.git "$FFMPEG_VERSION" ffmpeg
(cd ffmpeg && exec ./configure --prefix="$OUT" --target-os=mingw32 --arch=x86_64 --cross-prefix="$HOST-" \
    --pkg-config=pkg-config --pkg-config-flags=--static \
    --enable-shared --disable-static \
    --disable-programs --disable-doc --disable-debug --disable-autodetect --disable-everything \
    --enable-gpl --enable-w32threads \
    --enable-d3d11va --enable-dxva2 --enable-ffnvcodec --enable-cuda --enable-nvenc --enable-nvdec \
    --enable-amf --enable-libx264 --enable-libsvtav1 --enable-libdav1d \
    --enable-encoder=h264_nvenc,hevc_nvenc,av1_nvenc,h264_amf,hevc_amf,av1_amf,libx264,libsvtav1 \
    --enable-decoder=h264,hevc,av1,libdav1d \
    --enable-hwaccel=h264_d3d11va,h264_d3d11va2,hevc_d3d11va,hevc_d3d11va2,av1_d3d11va,av1_d3d11va2,h264_dxva2,hevc_dxva2,av1_dxva2,h264_nvdec,hevc_nvdec,av1_nvdec \
    --enable-parser=h264,hevc,av1 \
    --enable-bsf=extract_extradata,dump_extradata,h264_metadata,hevc_metadata,av1_metadata,h264_mp4toannexb,hevc_mp4toannexb \
    --enable-filter=scale,format,hwupload,hwdownload,hwmap \
    --extra-cflags="-I$DEPS/include" --extra-ldflags="-L$DEPS/lib -static-libgcc -static" >/dev/null \
) || { echo "FFmpeg configure failed:"; tail -n 40 ffmpeg/ffbuild/config.log; exit 1; }
make -C ffmpeg -j"$JOBS" >/dev/null
make -C ffmpeg install >/dev/null
install -Dm644 ffmpeg/COPYING.GPLv2 "$LIC/ffmpeg/COPYING.GPLv2"
install -Dm644 ffmpeg/LICENSE.md "$LIC/ffmpeg/LICENSE.md"

# Import libraries for MSVC: FFmpeg writes avcodec.lib next to the DLLs (via
# dlltool) when building with mingw; keep them in lib/ like BtbN's builds.
find "$OUT/bin" -name '*.lib' -exec mv {} "$OUT/lib/" \; 2>/dev/null || true
rm -rf "$OUT/lib/pkgconfig" "$OUT/share/ffmpeg"
"$HOST-strip" --strip-unneeded "$OUT"/bin/*.dll

echo "== result"
ls -la "$OUT/bin" "$OUT/lib"
for dll in "$OUT"/bin/*.dll; do
    deps="$("$HOST-objdump" -p "$dll" | sed -n 's/^\s*DLL Name: //p' | tr '\n' ' ')"
    echo "$(basename "$dll"): $deps"
done
