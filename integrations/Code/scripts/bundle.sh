#!/bin/zsh
set -euo pipefail
cd "${0:A:h:h}"
cargo build --release --bin orbit
mkdir -p dist/Code.app/Contents/{MacOS,Resources}
cp target/release/orbit dist/Code.app/Contents/MacOS/code.next
mv -f dist/Code.app/Contents/MacOS/code.next dist/Code.app/Contents/MacOS/code
cp scripts/Info.plist dist/Code.app/Contents/Info.plist
if [[ ! -f assets/Code.icns || assets/wallpapers/bonsai-sakura.png -nt assets/Code.icns || scripts/make-icon.swift -nt assets/Code.icns ]]; then
  mkdir -p .build
  CLANG_MODULE_CACHE_PATH="$PWD/.build/clang" swift scripts/make-icon.swift .build/Code.iconset assets/wallpapers/bonsai-sakura.png
  iconutil -c icns .build/Code.iconset -o assets/Code.icns
fi
mkdir -p dist/Code.app/Contents/Resources/Licenses
cp LICENSES/Zstandard-BSD.txt LICENSES/zstd-rs-MIT.txt LICENSES/Alacritty-Terminal-Apache-2.0.txt LICENSES/Monocode-Theme-MIT.txt dist/Code.app/Contents/Resources/Licenses/
cp assets/Code.icns dist/Code.app/Contents/Resources/Code-Bonsai.icns
rm -f dist/Code.app/Contents/Resources/Code.icns
# Lazy Apple WebRTC media shim. Rust owns authentication, signaling and UI.
mkdir -p .build/voice-swift-cache
if [[ ! -f .build/libCodeVoice.dylib || native_helpers/CodeVoiceMedia.swift -nt .build/libCodeVoice.dylib ]]; then
  swiftc -parse-as-library -emit-library -O -module-cache-path "$PWD/.build/voice-swift-cache" native_helpers/CodeVoiceMedia.swift -framework AppKit -framework WebKit -framework AVFoundation -o .build/libCodeVoice.dylib
fi
cp .build/libCodeVoice.dylib dist/Code.app/Contents/Resources/libCodeVoice.dylib
cp native_helpers/KAI-SOURCE.md dist/Code.app/Contents/Resources/Licenses/Kai-Voice-Attribution.txt
codesign --force --sign - dist/Code.app/Contents/Resources/libCodeVoice.dylib
codesign --force --sign - dist/Code.app
print "Built $PWD/dist/Code.app"
