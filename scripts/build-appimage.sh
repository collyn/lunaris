#!/bin/bash
set -e

# This script packages client-desktop or client-qml into an AppImage.
# Usage: ./scripts/build-appimage.sh [client-desktop|client-qml]

TARGET=${1:-client-desktop}

if [ "$TARGET" != "client-desktop" ] && [ "$TARGET" != "client-qml" ]; then
    echo "Usage: $0 [client-desktop|client-qml]"
    exit 1
fi

if [ ! -f "Cargo.toml" ]; then
    echo "Error: This script must be run from the repository root directory."
    exit 1
fi

echo "=================================================="
echo " Packaging $TARGET as AppImage...                 "
echo "=================================================="

# Check for patchelf
if ! command -v patchelf &> /dev/null; then
    echo "Error: patchelf is required but not installed."
    echo "Please install it using: sudo apt install patchelf"
    exit 1
fi

# Ensure target is built
echo "Building Rust binary..."
cargo build --release --bin client-desktop

# Prepare AppDir directory
APP_DIR="build-dir/${TARGET}.AppDir"
rm -rf "$APP_DIR"
mkdir -p "$APP_DIR"/{lib,plugins/platforms,plugins/multimedia,plugins/imageformats,plugins/xcbglintegrations,qml}

# Copy main binary
cp target/release/client-desktop "$APP_DIR/$TARGET"

# Locate Qt6 directories (supports standard system Qt6 on Debian/Ubuntu)
QT_LIB_DIR="/usr/lib/x86_64-linux-gnu"
QT_PLUGIN_DIR="/usr/lib/x86_64-linux-gnu/qt6/plugins"
QT_QML_DIR="/usr/lib/x86_64-linux-gnu/qt6/qml"

# Copy platform plugins
echo "Bundling Qt6 plugins..."
cp -L "$QT_PLUGIN_DIR/platforms/libqxcb.so" "$APP_DIR/plugins/platforms/" 2>/dev/null || true
cp -L "$QT_PLUGIN_DIR/platforms/libqwayland"*.so "$APP_DIR/plugins/platforms/" 2>/dev/null || true

# Copy other plugins
cp -rL "$QT_PLUGIN_DIR/multimedia/"* "$APP_DIR/plugins/multimedia/" 2>/dev/null || true
cp -rL "$QT_PLUGIN_DIR/imageformats/"* "$APP_DIR/plugins/imageformats/" 2>/dev/null || true
cp -rL "$QT_PLUGIN_DIR/xcbglintegrations/"* "$APP_DIR/plugins/xcbglintegrations/" 2>/dev/null || true

# Copy QML modules
echo "Bundling QML modules..."
for mod in QtQuick QtMultimedia QtQml Qt; do
  if [ -d "$QT_QML_DIR/$mod" ]; then
    cp -rL "$QT_QML_DIR/$mod" "$APP_DIR/qml/" 2>/dev/null || true
  fi
done

# Function to recursively resolve and bundle dependencies
resolve_dependencies() {
  echo "Resolving dependencies recursively..."
  
  # Download official AppImage excludelist if not exists
  EXCLUDELIST_FILE="build-dir/excludelist"
  if [ ! -f "$EXCLUDELIST_FILE" ]; then
      echo "Downloading AppImage excludelist..."
      curl -sfLo "$EXCLUDELIST_FILE" "https://raw.githubusercontent.com/AppImageCommunity/pkg2appimage/master/excludelist" || true
  fi

  # Minimal local exclude regex for lowest-level driver/system libraries that must NEVER be bundled
  EXCLUDE_REGEX="^(ld-linux|libc\.so|libm\.so|libdl\.so|libpthread\.so|librt\.so|libutil\.so|libstdc\+\+\.so|libgcc_s\.so|libGL\.so|libEGL\.so|libGLdispatch\.so|libGLX\.so|libOpenGL\.so|libdrm\.so|libgbm\.so|libasound\.so|libresolv\.so|libnss_|libudev\.so)"

  COPIED_ANY=true
  while [ "$COPIED_ANY" = true ]; do
    COPIED_ANY=false
    
    # Find all ELF binaries and shared libraries currently inside AppDir
    FILES_TO_CHECK=$(find "$APP_DIR" -type f | while read -r f; do
      if head -c 4 "$f" 2>/dev/null | grep -q '^.ELF'; then
        echo "$f"
      fi
    done)
    
    if [ -n "$FILES_TO_CHECK" ]; then
      # Run ldd and extract the absolute paths of dependencies
      DEPS=$(ldd $FILES_TO_CHECK 2>/dev/null | grep "=> /" | awk '{print $3}' | sort -u)
      
      for lib_path in $DEPS; do
        lib_name=$(basename "$lib_path")
        lib_base="${lib_name%%.so*}"
        
        # 1. Check against local exclude regex
        if echo "$lib_name" | grep -Eq "$EXCLUDE_REGEX"; then
          continue
        fi
        
        # 2. Check against downloaded excludelist (prefix match base name)
        if [ -f "$EXCLUDELIST_FILE" ]; then
          if grep -v '^#' "$EXCLUDELIST_FILE" | grep -v '^$' | grep -q "^${lib_base}\.so"; then
            continue
          fi
        fi
        
        # 3. Copy to AppDir/lib if not already present
        if [ ! -f "$APP_DIR/lib/$lib_name" ]; then
          echo "Bundling dependency: $lib_name"
          cp -aL "$lib_path" "$APP_DIR/lib/"
          COPIED_ANY=true
        fi
      done
    fi
  done
}

# Resolve and bundle dependencies recursively
resolve_dependencies

# Set RPATH on main binary
patchelf --force-rpath --set-rpath '$ORIGIN/lib' "$APP_DIR/$TARGET"

# Set RPATH on all bundled .so files
find "$APP_DIR/lib" "$APP_DIR/plugins" "$APP_DIR/qml" \
  -name "*.so*" -type f -exec patchelf --force-rpath --set-rpath '$ORIGIN:$ORIGIN/../lib:$ORIGIN/../../lib' {} \; 2>/dev/null || true

# Create qt.conf
cat <<EOF > "$APP_DIR/qt.conf"
[Paths]
Prefix = .
Libraries = lib
Plugins = plugins
QmlImports = qml
EOF

# Create AppRun
cat <<'EOF' > "$APP_DIR/AppRun"
#!/bin/bash
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
export LD_LIBRARY_PATH="$SCRIPT_DIR/lib:$LD_LIBRARY_PATH"
export QT_PLUGIN_PATH="$SCRIPT_DIR/plugins"
export QML2_IMPORT_PATH="$SCRIPT_DIR/qml"
exec "$SCRIPT_DIR/TARGET_BIN" "$@"
EOF
sed -i "s|TARGET_BIN|$TARGET|g" "$APP_DIR/AppRun"
chmod +x "$APP_DIR/AppRun"

# Create Desktop Entry
cat <<EOF > "$APP_DIR/${TARGET}.desktop"
[Desktop Entry]
Name=Lunaris Client
Exec=$TARGET
Icon=lunaris-client
Type=Application
Categories=Utility;RemoteAccess;
EOF

# Copy Icon
if [ "$TARGET" = "client-desktop" ]; then
    cp client-desktop/icons/128x128.png "$APP_DIR/lunaris-client.png"
else
    cp client-qml/qml/icon.png "$APP_DIR/lunaris-client.png"
fi

# Ensure build-dir exists for downloading appimagetool
mkdir -p build-dir

# Download appimagetool if not exists
APPIMAGE_TOOL="build-dir/appimagetool-x86_64.AppImage"
if [ ! -f "$APPIMAGE_TOOL" ] || [ ! -s "$APPIMAGE_TOOL" ] || grep -q "Not Found" "$APPIMAGE_TOOL"; then
    echo "Downloading appimagetool..."
    rm -f "$APPIMAGE_TOOL"
    curl -Lo "$APPIMAGE_TOOL" "https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-x86_64.AppImage"
    chmod +x "$APPIMAGE_TOOL"
fi

# Build AppImage
echo "Building AppImage file..."
# Use --appimage-extract-and-run in case FUSE is not available (like in Docker/CI)
export ARCH=x86_64
"$APPIMAGE_TOOL" --appimage-extract-and-run "$APP_DIR" "build-dir/${TARGET}-x86_64.AppImage"

echo "=================================================="
echo " Successfully built build-dir/${TARGET}-x86_64.AppImage"
echo "=================================================="
