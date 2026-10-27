#!/bin/zsh
set -euo pipefail
root=${0:A:h:h}
harness=StateHarness
[[ "${1:-}" != voice ]] || harness=VoiceShortcutHarness
[[ "${1:-}" != history ]] || harness=SavedHistoryHarness
[[ "${1:-}" != composer ]] || harness=ComposerHarness
staging=$(mktemp -d /private/tmp/kai-native-tests.XXXXXX)
trap 'rm -rf "$staging"' EXIT
product="$staging/Kai Native Tests.app"
mkdir -p "$product/Contents/MacOS" "$product/Contents/Resources/providers"
ln -s "$root/kai_pets" "$product/Contents/Resources/kai_pets"
for provider in codex claude gemini opencode cursor grok antigravity model-moon model-earth model-sun model-star; do cp "$root/integrations/Code/assets/$provider.svg" "$product/Contents/Resources/providers/"; done
cat > "$product/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?><!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd"><plist version="1.0"><dict><key>CFBundleIdentifier</key><string>com.kai.unified.native-tests</string><key>CFBundleExecutable</key><string>native-tests</string><key>CFBundleName</key><string>Kai Native Tests</string><key>LSUIElement</key><true/></dict></plist>
PLIST
swiftc -parse-as-library -D KAI_UNIFIED -module-cache-path /private/tmp/kai-fixes-swift-cache "$root"/Sources/NotchPrototype/*.swift "$root/Tests/UnifiedNative/$harness.swift" -o "$product/Contents/MacOS/native-tests"
"$product/Contents/MacOS/native-tests" --unified-smoke-test
