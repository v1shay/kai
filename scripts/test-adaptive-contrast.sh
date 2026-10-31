#!/bin/zsh
# Isolated native and GPUI checks. Never requests capture permission or runs an agent.
set -euo pipefail
root=${0:A:h:h}
export CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-$root/.build/unified-test-cargo}
export CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS:-2}
cargo test --offline --manifest-path "$root/integrations/Code/Cargo.toml" --bin orbit adaptive -- --test-threads=2
cargo test --offline --manifest-path "$root/integrations/Code/Cargo.toml" --bin orbit native_reentry_tests -- --test-threads=1
staging=$(mktemp -d /private/tmp/kai-adaptive-tests.XXXXXX)
trap 'rm -rf "$staging"' EXIT
sources=("$root/Sources/NotchPrototype/AdaptiveColorMath.swift" "$root/Sources/NotchPrototype/AdaptiveNativeInk.swift")
cache=/private/tmp/kai-adaptive-swift-cache
xcrun swiftc -parse-as-library -module-cache-path "$cache" "${sources[@]}" "$root/Tests/UnifiedNative/AdaptiveInkHarness.swift" -o "$staging/ink-tests"
"$staging/ink-tests"
product="$staging/Kai Adaptive Validation.app"
mkdir -p "$product/Contents/MacOS"
cat > "$product/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?><!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd"><plist version="1.0"><dict><key>CFBundleIdentifier</key><string>com.kai.unified.adaptive-validation</string><key>CFBundleExecutable</key><string>adaptive-validation</string><key>CFBundleName</key><string>Kai Adaptive Validation</string><key>LSUIElement</key><true/></dict></plist>
PLIST
xcrun swiftc -parse-as-library -module-cache-path "$cache" "${sources[@]}" "$root/Sources/NotchPrototype/AdaptiveBackdrop.swift" "$root/Tests/UnifiedNative/AdaptiveBackdropHarness.swift" -o "$product/Contents/MacOS/adaptive-validation"
codesign --force --sign - "$product"
"$product/Contents/MacOS/adaptive-validation"
