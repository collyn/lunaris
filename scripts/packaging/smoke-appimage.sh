#!/usr/bin/env bash
# Start-up check for a built AppImage, the Linux twin of windows/smoke-qml.ps1.
#
# Runs the AppImage for a few seconds with none of the build machine's Qt visible
# (a nearly empty environment: no LD_LIBRARY_PATH, QML2_IMPORT_PATH, QT_PLUGIN_PATH,
# QTDIR ...) and fails when it
#   - exits early (a missing library, a crash, a missing platform plugin), or
#   - prints one of the messages Qt emits when a QML module or plugin is missing
#     (a missing module leaves the process running without a window).
#
# Without a display it starts a virtual X server (xvfb-run) and uses the real "xcb"
# platform plugin: "offscreen" is not bundled, and asking Qt for a platform plugin
# that is not in the bundle can only fail, which would make the check meaningless.
# With DISPLAY / WAYLAND_DISPLAY set (a desktop) it uses that session; force a
# platform with QT_QPA_PLATFORM=xcb|wayland.
#
# usage: smoke-appimage.sh <file.AppImage> [seconds=8] [app args...]
#   env: SMOKE_FORBIDDEN  extra egrep pattern that must not appear in the output
set -uo pipefail

APPIMAGE="$(readlink -f "${1:?usage: smoke-appimage.sh <file.AppImage> [seconds] [app args...]}")"
SECONDS_TO_RUN="${2:-8}"
shift 2 2>/dev/null || shift $#
APP_ARGS=("$@")

FORBIDDEN='is not installed|failed to load component|is not a type|Could not find the Qt platform plugin|no Qt platform plugin|Could not load|Cannot load library|error while loading shared libraries|Failed to create RHI|Failed to load client buffer integration|U_MISSING_RESOURCE_ERROR|Unable to load ICU'
[[ -n "${SMOKE_FORBIDDEN:-}" ]] && FORBIDDEN="$FORBIDDEN|$SMOKE_FORBIDDEN"

HOME_DIR="$(mktemp -d)"          # throw-away HOME: the agent writes its config there
LOG="$HOME_DIR/output.log"
trap 'rm -rf "$HOME_DIR"' EXIT

# Host session variables are passed through, everything else is dropped.
ENV_ARGS=(HOME="$HOME_DIR" PATH=/usr/bin:/bin LANG=C.UTF-8
          QT_FORCE_STDERR_LOGGING=1 LUNARIS_ALLOW_SECOND_INSTANCE=1
          APPIMAGE_EXTRACT_AND_RUN=1 TMPDIR="$HOME_DIR" XDG_CACHE_HOME="$HOME_DIR/.cache" MESA_SHADER_CACHE_DISABLE=true)
for v in DISPLAY XAUTHORITY WAYLAND_DISPLAY XDG_RUNTIME_DIR XDG_SESSION_TYPE DBUS_SESSION_BUS_ADDRESS QT_QPA_PLATFORM; do
    [[ -n "${!v:-}" ]] && ENV_ARGS+=("$v=${!v}")
done

LAUNCH=()
if [[ -z "${DISPLAY:-}${WAYLAND_DISPLAY:-}" ]]; then
    command -v xvfb-run >/dev/null || { echo "smoke: no display and no xvfb-run" >&2; exit 2; }
    LAUNCH=(xvfb-run -a -s "-screen 0 1280x800x24")
    ENV_ARGS+=(QT_QPA_PLATFORM="${QT_QPA_PLATFORM:-xcb}")
fi

echo "smoke: $APPIMAGE ${APP_ARGS[*]} for ${SECONDS_TO_RUN}s (platform: ${QT_QPA_PLATFORM:-auto})"
if [[ ${#LAUNCH[@]} -gt 0 ]]; then
    # xvfb-run exports DISPLAY / XAUTHORITY to its command; env -i must keep them.
    setsid "${LAUNCH[@]}" sh -c 'exec env -i ${DISPLAY:+DISPLAY=$DISPLAY} ${XAUTHORITY:+XAUTHORITY=$XAUTHORITY} "$@"' sh \
        "${ENV_ARGS[@]}" "$APPIMAGE" "${APP_ARGS[@]}" >"$LOG" 2>&1 &
else
    setsid env -i "${ENV_ARGS[@]}" "$APPIMAGE" "${APP_ARGS[@]}" >"$LOG" 2>&1 &
fi
PID=$!

# Poll instead of sleeping the whole time, so an early exit is reported at once.
EXITED=0
for ((i = 0; i < SECONDS_TO_RUN * 2; i++)); do
    sleep 0.5
    if ! kill -0 "$PID" 2>/dev/null; then EXITED=1; break; fi
done
if [[ $EXITED -eq 0 ]]; then
    # setsid made the child a process-group leader: stop the whole group (xvfb-run,
    # Xvfb, the AppImage runtime, the app).
    kill -TERM -- "-$PID" 2>/dev/null
    sleep 1; kill -KILL -- "-$PID" 2>/dev/null
fi
wait "$PID" 2>/dev/null; RC=$?

if [[ $EXITED -eq 1 ]]; then
    cat "$LOG"; echo "smoke: FAIL: exited early with code $RC" >&2; exit 1
fi
if grep -aEq "$FORBIDDEN" "$LOG"; then
    cat "$LOG"; echo "smoke: FAIL: it did not load its QML, a plugin or a library (see the output above)" >&2; exit 1
fi
echo "smoke: OK: ran ${SECONDS_TO_RUN}s and loaded its QML ($(wc -l <"$LOG") log lines)"
