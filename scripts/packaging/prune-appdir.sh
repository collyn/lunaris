#!/usr/bin/env bash
# Removes what the Lunaris AppImages never use from a deployed AppDir, so the
# download stays small. Sourced by make-appimage.sh.
#
# What the apps need (from their QML, agent-app/qml and client-qml/qml):
#   QtQuick, QtQuick.Controls (Basic and Fusion), QtQuick.Layouts, QtQuick.Shapes
#   (client), QtQuick.Window/Templates (dependencies), Qt.labs.platform (tray icon,
#   file dialogs), QtMultimedia (client: VideoOutput / QVideoSink only).
#
# Everything removed here was verified to be unused by starting the apps and by
# the failing-build check in smoke-appimage.sh; add to the lists only with the same
# care. Sizes are the uncompressed ones of the Qt 6.8.3 online binaries.

# prune_appdir <AppDir> <agent|client-desktop|client-qml>
prune_appdir() {
    local appdir="$1" variant="$2"
    local u="$appdir/usr" lib="$appdir/usr/lib" qml="$appdir/usr/qml"
    local before after
    before=$(du -sk "$appdir" | cut -f1)

    # 1. Qt's own UI translations (~5 MB): the apps ship their strings in QML and
    #    do not load qt_*.qm.
    rm -rf "$u/translations"

    # 2. Qt Quick Controls styles nobody selects (~10 MB). Basic and Fusion stay:
    #    the client sets Fusion on Linux, and the agent, being a QApplication, gets
    #    Fusion as its default ("module QtQuick.Controls.Fusion is not installed"
    #    when it was pruned). Their libraries go with them.
    local styles=(Imagine Material Universal FluentWinUI3)
    local s
    for s in "${styles[@]}"; do
        rm -rf "$qml/QtQuick/Controls/$s"
        rm -f "$lib/libQt6QuickControls2$s"*
    done
    rm -rf "$qml/QtQuick/Controls/designer"

    # 3. QML modules that are not imported by any Lunaris QML file, with their
    #    libraries (~6 MB): file dialogs (the apps use Qt.labs.platform), particles,
    #    LocalStorage (SQL), shader effects, SVG-as-QML, the native-style fallback,
    #    and the Qt Creator tooling data.
    local m
    for m in Dialogs Particles LocalStorage Effects VectorImage NativeStyle tooling; do
        rm -rf "$qml/QtQuick/$m"
    done
    rm -f "$lib"/libQt6QuickDialogs2* "$lib"/libQt6QuickParticles* "$lib"/libQt6QuickEffects* \
          "$lib"/libQt6QuickVectorImage* "$lib"/libQt6QmlLocalStorage* "$lib"/libQt6Sql* \
          "$lib"/libQt6QuickControls2NativeStyle*
    # Qt.labs modules other than platform (settings, folderlistmodel, sharedimage ...).
    local d
    for d in "$qml"/Qt/labs/*/; do
        [[ "$(basename "$d")" == platform ]] || rm -rf "$d"
    done

    # 4. QML type-description files are only read by Qt Creator / qmllint.
    find "$qml" -name '*.qmltypes' -delete
    find "$qml" -name 'designer' -type d -prune -exec rm -rf {} +

    # 5. Client only: Qt Multimedia's FFmpeg plugin needs libav*; use the minimal build.
    [[ -d "$u/plugins/multimedia" ]] && slim_qt_ffmpeg "$appdir"

    # 6. ICU: swap libicudata (30 MB) for the filtered build of the same ICU release.
    slim_icu_data "$appdir"

    after=$(du -sk "$appdir" | cut -f1)
    echo "prune: AppDir $((before / 1024)) MB -> $((after / 1024)) MB"

    check_no_dangling_qt "$appdir"
}

# Fails when a remaining binary needs a Qt/ICU library that is not in the AppDir
# (a prune that cut too deep would otherwise only show at run time).
check_no_dangling_qt() {
    local appdir="$1" f need bad=0
    while IFS= read -r f; do
        for need in $(readelf -d "$f" 2>/dev/null | sed -n 's/.*Shared library: \[\(libQt6[^]]*\|libicu[^]]*\)\].*/\1/p'); do
            if [[ ! -e "$appdir/usr/lib/$need" ]]; then
                echo "prune: $f needs $need, which is not in the AppDir" >&2
                bad=1
            fi
        done
    done < <(find "$appdir/usr" -type f \( -name '*.so' -o -name '*.so.*' -o -perm -u+x \) -exec sh -c \
                'for x; do head -c4 "$x" | grep -q "^.ELF" && echo "$x"; done' sh {} +)
    [[ $bad -eq 0 ]] || return 1
}

# Qt's official binaries link ICU and bundle libicudata.so.<major> (all locales,
# ~30 MB). The build image carries a build of the same ICU release restricted to
# what Qt Core uses (collation, break iteration, charset conversion, root/en/vi
# locale data): see icu-data-filter.json and the ICU stage of Containerfile.appimage.
# Only the data file is replaced, the code libraries stay Qt's own.
slim_icu_data() {
    local appdir="$1" slim=/opt/icu-data bundled
    bundled=$(find "$appdir/usr/lib" -maxdepth 1 -name 'libicudata.so.*' | head -1)
    [[ -n "$bundled" ]] || { echo "prune: no libicudata in the AppDir (Qt without ICU?), nothing to slim"; return 0; }
    if [[ ! -f "$slim/$(basename "$bundled")" ]]; then
        echo "prune: the bundled $(basename "$bundled") has no filtered counterpart in $slim;" \
             "update ICU_VERSION in Containerfile.appimage to Qt's ICU release" >&2
        return 1
    fi
    echo "prune: libicudata $(du -k "$bundled" | cut -f1) kB -> $(du -k "$slim/$(basename "$bundled")" | cut -f1) kB"
    install -m755 "$slim/$(basename "$bundled")" "$bundled"
    install -Dm644 "$slim/LICENSE" "$appdir/usr/share/licenses/icu/LICENSE"
}

# The client paints CPU video through QVideoSink, which on Linux needs Qt Multimedia's
# FFmpeg plugin, which links Qt's own FFmpeg 7.1 (libavcodec.so.61 ... 19 MB with every
# codec). Replace those libraries by the codec-less build of the same release made in
# the image (see Containerfile.appimage). Same sonames; the plugin needs only the glue.
slim_qt_ffmpeg() {
    local appdir="$1" mini=/opt/qt-ffmpeg-min lib="$1/usr/lib" b name before=0 after=0 undefined
    while IFS= read -r b; do
        name=$(basename "$b")
        [[ -f "$mini/lib/$name" ]] || { echo "prune: no minimal build of $name in $mini (Qt's FFmpeg changed?)" >&2; return 1; }
        before=$((before + $(du -k "$b" | cut -f1)))
        install -m755 "$mini/lib/$name" "$b"
        patchelf --set-rpath '$ORIGIN' "$b"
        after=$((after + $(du -k "$b" | cut -f1)))
    done < <(find "$lib" -maxdepth 1 \( -name 'libav*.so.*' -o -name 'libsw*.so.*' \))
    echo "prune: Qt's FFmpeg libraries ${before} kB -> ${after} kB"
    install -Dm644 "$mini/share/licenses/ffmpeg-qt/COPYING.LGPLv2.1" "$appdir/usr/share/licenses/ffmpeg-qt/COPYING.LGPLv2.1"
    # Every symbol the plugin imports must still resolve.
    undefined=$(LD_LIBRARY_PATH="$lib" ldd -r "$appdir/usr/plugins/multimedia/libffmpegmediaplugin.so" 2>&1 | grep -a 'undefined symbol' || true)
    if [[ -n "$undefined" ]]; then echo "prune: the minimal FFmpeg lacks symbols the plugin needs:" >&2; echo "$undefined" >&2; return 1; fi
}
