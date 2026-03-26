#!/bin/bash
set -euo pipefail

APP_PATH="${1:-src-tauri/target/release/bundle/macos/claude-devtools-tauri.app}"
DMG_PATH="${2:-src-tauri/target/release/bundle/macos/claude-devtools-tauri.dmg}"

if [ ! -d "$APP_PATH" ] && [ ! -f "$DMG_PATH" ]; then
  echo "Error: Build not found. Run 'pnpm build:macos' first."
  exit 1
fi

echo "# Performance Benchmark Results"
echo ""
echo "## Build Size"
echo ""

if [ -f "$DMG_PATH" ]; then
  dmg_size=$(du -sh "$DMG_PATH" | cut -f1)
  echo "| Metric | Size |"
  echo "|--------|------|"
  echo "| DMG installer | $dmg_size |"
fi

if [ -d "$APP_PATH" ]; then
  app_size=$(du -sh "$APP_PATH" | cut -f1)
  echo "| .app bundle | $app_size |"
  binary_size=$(du -sh "$APP_PATH/Contents/MacOS/claude-devtools-tauri" 2>/dev/null | cut -f1 || echo "N/A")
  echo "| Binary | $binary_size |"
fi

echo ""
echo "## Memory Usage"
echo ""

# Get PID if running
PID=$(pgrep -f "claude-devtools-tauri" | head -1 || true)
if [ -n "$PID" ]; then
  rss_kb=$(ps -o rss= -p "$PID" | tr -d ' ')
  rss_mb=$((rss_kb / 1024))
  echo "| Metric | Value |"
  echo "|--------|-------|"
  echo "| RSS (resident) | ${rss_mb}MB |"
else
  echo "App not running. Start with 'pnpm tauri dev' then re-run."
fi

echo ""
echo "## Electron Comparison"
echo ""
echo "| Metric | Electron | Tauri | Improvement |"
echo "|--------|----------|-------|-------------|"
echo "| Installer | ~120MB | ${dmg_size:-N/A} | - |"
echo "| Memory (idle) | 200-300MB | ${rss_mb:-N/A}MB | - |"
echo "| Cold start | 2-3s | measure manually | - |"
