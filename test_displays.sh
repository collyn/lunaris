#!/bin/bash
# Test script to list available displays/monitors on the system

echo "=== xrandr outputs ==="
xrandr --query 2>/dev/null | grep -E "connected|disconnected" || echo "xrandr not available"

echo ""
echo "=== xrandr active modes ==="
xrandr --query 2>/dev/null | grep -E "^\s+[0-9]+x[0-9]+" | head -20 || echo "no modes found"

echo ""
echo "=== VIRTUAL outputs ==="
xrandr --query 2>/dev/null | grep -i "virtual" || echo "no VIRTUAL outputs found"

echo ""
echo "=== NvFBC status (requires nvidia-smi) ==="
nvidia-smi --query-gpu=name,display_mode --format=csv,noheader 2>/dev/null || echo "nvidia-smi not available"

echo ""
echo "=== /sys/class/drm connectors ==="
for conn in /sys/class/drm/card*-*/status; do
    if [ -f "$conn" ]; then
        name=$(basename $(dirname "$conn"))
        status=$(cat "$conn" 2>/dev/null)
        echo "  $name: $status"
    fi
done 2>/dev/null || echo "no drm connectors found"

echo ""
echo "=== Display environment ==="
echo "DISPLAY=$DISPLAY"
echo "WAYLAND_DISPLAY=$WAYLAND_DISPLAY"
echo "XDG_SESSION_TYPE=$XDG_SESSION_TYPE"
