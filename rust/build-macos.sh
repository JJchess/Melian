#!/bin/bash
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root/rust"
[[ "$(uname -s)" == Darwin ]] || { echo 'macOS / Xcode Command Line Tools required' >&2; exit 1; }
arch="$(uname -m)"
case "$arch" in arm64) target=aarch64-apple-darwin; asset=arm64;; x86_64) target=x86_64-apple-darwin; asset=x64;; *) exit 1;; esac
export MACOSX_DEPLOYMENT_TARGET=13.0
cargo test --workspace --locked
node "$root/tests/extension-tests/run.mjs"
cargo build --release --locked --target "$target" -p ClaudeGuard -p claude-guard
stage="$(mktemp -d "$root/rust/dist-macos.XXXXXXXX")"
trap 'rm -rf -- "$stage"' EXIT
app="$stage/Melian.app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources/Melian.iconset" "$root/rust/dist"
cp "target/$target/release/ClaudeGuard" "target/$target/release/claude-guard" "$app/Contents/MacOS/"
cp "$root/LICENSE" "$root/docs/MACOS.md" "$app/Contents/Resources/"
version="$(sed -n 's/^version = "\([^"]*\)"/\1/p' Cargo.toml | head -1)"
cat > "$app/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>io.github.jjchess.melian</string>
<key>CFBundleName</key><string>Melian</string>
<key>CFBundleDisplayName</key><string>Melian</string>
<key>CFBundleExecutable</key><string>ClaudeGuard</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleShortVersionString</key><string>$version</string>
<key>CFBundleVersion</key><string>$version</string>
<key>CFBundleIconFile</key><string>Melian</string>
<key>LSMinimumSystemVersion</key><string>13.0</string>
<key>LSUIElement</key><true/>
<key>NSHighResolutionCapable</key><true/>
<key>NSAppleEventsUsageDescription</key><string>打开 Terminal 以运行受保护的 Claude Code 会话。</string>
</dict></plist>
EOF
for size in 16 32 128 256 512; do
  sips -z "$size" "$size" "$root/docs/brand-mark.png" --out "$app/Contents/Resources/Melian.iconset/icon_${size}x${size}.png" >/dev/null
  double=$((size*2))
  sips -z "$double" "$double" "$root/docs/brand-mark.png" --out "$app/Contents/Resources/Melian.iconset/icon_${size}x${size}@2x.png" >/dev/null
done
iconutil -c icns "$app/Contents/Resources/Melian.iconset" -o "$app/Contents/Resources/Melian.icns"
rm -rf -- "$app/Contents/Resources/Melian.iconset"
# Ad-hoc signature for arm64 execution. No claim of Apple notarization.
codesign --force --sign - "$app/Contents/MacOS/claude-guard"
codesign --force --deep --sign - "$app"
codesign --verify --deep --strict "$app"
node - "$app/Contents/MacOS/ClaudeGuard" "$root/rust/dist/macos-preview-$asset.png" <<'JS'
const { spawnSync } = require('node:child_process');
const { statSync } = require('node:fs');
const result = spawnSync(process.argv[2], ['--smoke-test', '--preview-output', process.argv[3]], { stdio: 'inherit', timeout: 20000 });
if (result.error) throw result.error;
if (result.status !== 0) throw new Error(`AppKit smoke exited ${result.status} / ${result.signal}`);
if (statSync(process.argv[3]).size < 100) throw new Error('AppKit did not produce a preview');
JS
COPYFILE_DISABLE=1 tar -czf "$root/rust/dist/Melian-macos-$asset.tar.gz" -C "$stage" Melian.app
echo "Built rust/dist/Melian-macos-$asset.tar.gz"
