#!/usr/bin/env bash
# Install-and-start check for the built .deb packages, the twin of
# ../smoke-appimage.sh. Run it as root in a bare container of every release the
# packages claim to support (ubuntu:24.04, ubuntu:26.04, debian:13): the build
# distro alone cannot tell that a dependency has another name elsewhere
# (libqt6gui6t64 on Ubuntu 24.04 is libqt6gui6 on 26.04 and Debian 13).
#
# Fails when
#   - apt cannot install the packages with their Depends alone (no Recommends),
#   - a binary misses a shared library or a symbol (ldd -r), or
#   - a program exits early (the agent runs headless with a new config; the
#     approval helper with an agent to answer for) or prints one of the messages Qt emits when a QML
#     module or a plugin is missing (the distro's Qt is newer than the one the
#     package was built with), or
#   - the server, with lunaris-agent installed too, does not start its gateway
#     (server/src/gateway.rs: the agent as its child, connected to).
#
# usage: smoke-deb.sh <dir with lunaris-*.deb> [seconds=8]
set -uo pipefail

DIR="$(readlink -f "${1:?usage: smoke-deb.sh <dir with lunaris-*.deb> [seconds]}")"
SECONDS_TO_RUN="${2:-8}"

FORBIDDEN='is not installed|failed to load component|is not a type|Could not find the Qt platform plugin|no Qt platform plugin|Could not load|Cannot load library|error while loading shared libraries|symbol lookup error|undefined symbol|Failed to create RHI'

shopt -s nullglob
DEBS=("$DIR"/lunaris-*.deb)
[[ ${#DEBS[@]} -gt 0 ]] || { echo "smoke: no lunaris-*.deb in $DIR" >&2; exit 2; }

. /etc/os-release
echo "smoke: $PRETTY_NAME: ${DEBS[*]##*/}"

export DEBIAN_FRONTEND=noninteractive
for i in 1 2 3 4 5; do apt-get -o Acquire::Retries=5 update -qq && break; sleep 20; done
# apt reads a local package as the _apt user: give it a copy it can reach.
STAGE="$(mktemp -d)"; chmod 755 "$STAGE"; cp "${DEBS[@]}" "$STAGE/"
if ! apt-get install -y --no-install-recommends "$STAGE"/*.deb >"$STAGE/apt.log" 2>&1; then
    tail -40 "$STAGE/apt.log"
    echo "smoke: FAILED: the packages do not install on $PRETTY_NAME" >&2
    exit 1
fi
dpkg-query -W -f '${db:Status-Abbrev}${Package} ${Version}\n' 'libqt6core6*' 'libqt6gui6*' 'libqt6widgets6*' 'lunaris-*' 2>/dev/null \
    | sed -n 's/^ii //p'

FAILED=0
for deb in "${DEBS[@]}"; do
    name="$(dpkg-deb -f "$deb" Package)"
    bin="/usr/bin/$name"

    missing="$(ldd -r "$bin" 2>&1 | grep -E 'not found|undefined symbol' || true)"
    if [[ -n "$missing" ]]; then
        echo "$missing"
        echo "smoke: FAILED: $name misses libraries or symbols" >&2
        FAILED=1; continue
    fi

    home="$(mktemp -d)"; log="$home/output.log"
    mkdir -m 700 "$home/run"
    ENV=(env -i HOME="$home" PATH=/usr/bin:/bin LANG=C.UTF-8 XDG_RUNTIME_DIR="$home/run"
        QT_QPA_PLATFORM=offscreen QT_FORCE_STDERR_LOGGING=1 LUNARIS_ALLOW_SECOND_INSTANCE=1
        LUNARIS_DATA_DIR="$home/data" LUNARIS_ADMIN_PASSWORD=smoke-test-password)
    RUN=()
    if [[ "$name" == lunaris-approval-helper ]]; then
        # The helper starts Qt only once an agent of its own user lets it
        # answer (an agent running as root offers no channel): both run as
        # an ordinary user, the agent (no config: a new one, nothing
        # shared) a little longer.
        id lunaris-smoke >/dev/null 2>&1 || useradd -M -d "$home" lunaris-smoke
        chown -R lunaris-smoke "$home"
        RUN=(runuser -u lunaris-smoke --)
        "${RUN[@]}" "${ENV[@]}" timeout --kill-after=5 "$((SECONDS_TO_RUN + 4))" /usr/bin/lunaris-agent --cli \
            >"$home/agent.log" 2>&1 &
        for _ in 1 2 3 4 5 6 7 8 9 10; do [[ -S "$home/run/lunaris/approval.sock" ]] && break; sleep 0.5; done
    fi
    "${RUN[@]}" "${ENV[@]}" timeout --kill-after=5 "$SECONDS_TO_RUN" "$bin" >"$log" 2>&1
    rc=$?
    wait
    sed 's/\x1b\[[0-9;]*m//g' "$log" | tail -20
    # 124: still running when the time was up, which is what a started app does.
    if [[ $rc -ne 124 ]]; then
        echo "smoke: FAILED: $name exited by itself with status $rc" >&2
        FAILED=1
    elif grep -E "$FORBIDDEN" "$log"; then
        echo "smoke: FAILED: $name printed a missing-module message" >&2
        FAILED=1
    elif [[ "$name" == lunaris-server && -x /usr/bin/lunaris-agent ]] \
        && ! sed 's/\x1b\[[0-9;]*m//g' "$log" | grep -q 'Agent registered (added by address): .* (gateway)'; then
        echo "smoke: FAILED: lunaris-server did not start and reach its gateway (lunaris-agent --bridge-only)" >&2
        FAILED=1
    else
        echo "smoke: OK: $name ran for ${SECONDS_TO_RUN}s on $PRETTY_NAME"
    fi
    rm -rf "$home"
done
rm -rf "$STAGE"
exit $FAILED
