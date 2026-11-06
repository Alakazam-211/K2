#!/bin/sh
# By hand, on a Linux desktop session (GNOME, KDE, Hyprland, Sway...) with
# xdg-desktop-portal, its desktop backend and PipeWire running: share a
# screen or a window through the portal and check frames arrive.
#
# Usage, from the repository root, in a terminal inside the session:
#   sh crates/kessel-video/scripts/try-linux-screen.sh
# The portal's picker appears; pick a display or a window within 2 minutes.
# Cancelling fails the test with not_found.
set -eu
if [ -z "${WAYLAND_DISPLAY:-}${DISPLAY:-}" ]; then
    echo "run this inside a desktop session (no WAYLAND_DISPLAY or DISPLAY)" >&2
    exit 2
fi
if [ -z "${DBUS_SESSION_BUS_ADDRESS:-}" ] && [ ! -S "${XDG_RUNTIME_DIR:-/nonexistent}/bus" ]; then
    echo "no session bus: the portal can't be reached" >&2
    exit 2
fi
exec cargo test -p kessel-video --lib real_portal_screen_delivers_frames -- --ignored --nocapture
