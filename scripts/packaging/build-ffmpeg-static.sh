#!/usr/bin/env bash
# Builds a trimmed, static FFmpeg 8.1 (GPL: includes x264) into <prefix>, with
# exactly what the Lunaris agent and client use:
#   encoders  VAAPI / NVENC / QSV (h264, hevc, av1), x264, SVT-AV1
#   decoders  h264, hevc, av1 (+ dav1d) with VAAPI and NVDEC acceleration
#   swscale and the few filters/bitstream filters the pipelines touch.
# Linking it statically keeps distro packages independent of each distro's
# FFmpeg version (and of Fedora's ffmpeg-free, which lacks H.264/HEVC).
# libva and libdrm stay dynamic (system libraries, tied to the GPU drivers).
#
# Needs: a C/C++ toolchain, nasm, meson, ninja, cmake, git, pkg-config,
#        libva and libdrm development files.
# Usage: build-ffmpeg-static.sh <prefix>
set -euo pipefail

PREFIX="$(mkdir -p "$1" && cd "$1" && pwd)"
JOBS="$(nproc)"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
cd "$WORK"

NVCODEC_VERSION=n12.2.72.0
X264_BRANCH=stable
SVTAV1_VERSION=v2.3.0
DAV1D_VERSION=1.5.1
VPL_VERSION=v2.14.0
FFMPEG_VERSION=n8.1

export PKG_CONFIG_PATH="$PREFIX/lib/pkgconfig${PKG_CONFIG_PATH:+:$PKG_CONFIG_PATH}"
LIC="$PREFIX/share/licenses"
mkdir -p "$LIC"

clone() { git clone -q --depth 1 -b "$2" "$1" "$3"; }

echo "== nv-codec-headers $NVCODEC_VERSION"
clone https://github.com/FFmpeg/nv-codec-headers.git "$NVCODEC_VERSION" nvcodec
make -C nvcodec PREFIX="$PREFIX" install >/dev/null
install -Dm644 nvcodec/README "$LIC/nv-codec-headers/README"

echo "== x264 ($X264_BRANCH)"
clone https://code.videolan.org/videolan/x264.git "$X264_BRANCH" x264
(cd x264 && ./configure --prefix="$PREFIX" --enable-static --enable-pic --disable-cli >/dev/null \
    && make -j"$JOBS" >/dev/null && make install >/dev/null)
install -Dm644 x264/COPYING "$LIC/x264/COPYING"
git -C x264 rev-parse HEAD > "$LIC/x264/VERSION"

# SVT-AV1 enables LTO in release builds; its objects would then be GCC LTO
# bytecode that lld (Rust's default linker) cannot read, so turn LTO off.
echo "== SVT-AV1 $SVTAV1_VERSION"
clone https://gitlab.com/AOMediaCodec/SVT-AV1.git "$SVTAV1_VERSION" svtav1
cmake -S svtav1 -B svtav1/build -G Ninja -DCMAKE_BUILD_TYPE=Release \
    -DCMAKE_INSTALL_PREFIX="$PREFIX" -DCMAKE_INSTALL_LIBDIR=lib -DBUILD_SHARED_LIBS=OFF \
    -DBUILD_APPS=OFF -DBUILD_DEC=OFF -DCMAKE_POSITION_INDEPENDENT_CODE=ON \
    -DSVT_AV1_LTO=OFF -DCMAKE_INTERPROCEDURAL_OPTIMIZATION=OFF -DCMAKE_C_FLAGS=-fno-lto >/dev/null
ninja -C svtav1/build install >/dev/null
install -Dm644 svtav1/LICENSE.md "$LIC/svt-av1/LICENSE.md"
install -Dm644 svtav1/PATENTS.md "$LIC/svt-av1/PATENTS.md"

echo "== dav1d $DAV1D_VERSION"
clone https://code.videolan.org/videolan/dav1d.git "$DAV1D_VERSION" dav1d
meson setup dav1d/build dav1d --prefix="$PREFIX" --libdir=lib --buildtype=release \
    --default-library=static -Denable_tools=false -Denable_tests=false >/dev/null
ninja -C dav1d/build install >/dev/null
install -Dm644 dav1d/COPYING "$LIC/dav1d/COPYING"

echo "== oneVPL dispatcher $VPL_VERSION"
clone https://github.com/intel/libvpl.git "$VPL_VERSION" vpl
cmake -S vpl -B vpl/build -G Ninja -DCMAKE_BUILD_TYPE=Release -DCMAKE_INSTALL_PREFIX="$PREFIX" \
    -DCMAKE_INSTALL_LIBDIR=lib -DBUILD_SHARED_LIBS=OFF -DCMAKE_POSITION_INDEPENDENT_CODE=ON \
    -DBUILD_TESTS=OFF -DBUILD_EXAMPLES=OFF -DINSTALL_EXAMPLES=OFF >/dev/null
ninja -C vpl/build install >/dev/null
install -Dm644 vpl/LICENSE "$LIC/libvpl/LICENSE"

echo "== FFmpeg $FFMPEG_VERSION"
clone https://github.com/FFmpeg/FFmpeg.git "$FFMPEG_VERSION" ffmpeg
(cd ffmpeg && ./configure --prefix="$PREFIX" --libdir="$PREFIX/lib" \
    --enable-static --disable-shared --enable-pic --pkg-config-flags=--static \
    --disable-programs --disable-doc --disable-debug --disable-autodetect --disable-everything \
    --enable-gpl --enable-pthreads \
    --enable-vaapi --enable-libdrm --enable-ffnvcodec --enable-cuda --enable-nvenc --enable-nvdec \
    --enable-libvpl --enable-libx264 --enable-libsvtav1 --enable-libdav1d \
    --enable-encoder=h264_vaapi,hevc_vaapi,av1_vaapi,h264_nvenc,hevc_nvenc,av1_nvenc,h264_qsv,hevc_qsv,av1_qsv,libx264,libsvtav1 \
    --enable-decoder=h264,hevc,av1,libdav1d \
    --enable-hwaccel=h264_vaapi,hevc_vaapi,av1_vaapi,h264_nvdec,hevc_nvdec,av1_nvdec \
    --enable-parser=h264,hevc,av1 \
    --enable-bsf=extract_extradata,dump_extradata,h264_metadata,hevc_metadata,av1_metadata,h264_mp4toannexb,hevc_mp4toannexb \
    --enable-filter=scale,format,hwupload,hwdownload,hwmap,scale_vaapi \
    --extra-libs="-lpthread -lm" >/dev/null \
    && make -j"$JOBS" >/dev/null && make install >/dev/null)
install -Dm644 ffmpeg/COPYING.GPLv2 "$LIC/ffmpeg/COPYING.GPLv2"
install -Dm644 ffmpeg/LICENSE.md "$LIC/ffmpeg/LICENSE.md"

echo "Static FFmpeg installed in $PREFIX"
pkg-config --modversion libavcodec
