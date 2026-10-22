#!/bin/zsh
set -euo pipefail

root=${0:A:h:h}
cd "$root"
cache=/private/tmp/notch-clang-cache
configuration=${KAI_BUILD_CONFIGURATION:-release}
if [[ "$configuration" != "release" && "$configuration" != "debug" ]]; then
    print -u2 -- "KAI_BUILD_CONFIGURATION must be release or debug"
    exit 1
fi
CLANG_MODULE_CACHE_PATH="$cache" SWIFTPM_MODULECACHE_OVERRIDE="$cache" swift build --disable-sandbox -c "$configuration"

destination="$root/.build/debug/Kai.app"
mkdir -p "$root/.build/debug"
staging=$(mktemp -d "$root/.build/debug/.kai-package.XXXXXX")
trap 'rm -rf "$staging"' EXIT
product="$staging/Kai.app"
mkdir -p "$product/Contents/MacOS"
cp "$root/.build/$configuration/NotchPrototype" "$product/Contents/MacOS/NotchPrototype"
cp "$root/Support/NotchPrototype-Info.plist" "$product/Contents/Info.plist"
mkdir -p "$product/Contents/Resources/kai_pets"
mkdir -p "$product/Contents/Resources/katex"
cp -R "$root/Sources/NotchPrototype/Resources/voice" "$product/Contents/Resources/"
cp -R "$root/Sources/NotchPrototype/Resources/integrations" "$product/Contents/Resources/"
cp "$root/kai_speech.py" "$root/kai.py" "$root/kai_bridge.py" "$root/kai_activity.py" "$root/kai_dot.py" "$root/kai_dot_mcp.py" "$root/kai_code.py" "$root/kai_voice.py" "$product/Contents/Resources/"
cp "$root/Sources/NotchPrototype/Resources/katex/renderer.html" "$root/Sources/NotchPrototype/Resources/katex/katex.min.js" "$root/Sources/NotchPrototype/Resources/katex/katex.min.css" "$root/Sources/NotchPrototype/Resources/katex/LICENSE" "$product/Contents/Resources/katex/"
cp -R "$root/Sources/NotchPrototype/Resources/katex/fonts" "$product/Contents/Resources/katex/"
cp "$root/kai_pets/animation_catalog.json" "$root/kai_pets/gradient_profiles.json" "$product/Contents/Resources/kai_pets/"
python3 - "$root" "$product/Contents/Resources" <<'PY'
import json
from pathlib import Path
import shutil
import sys

root, resources = map(Path, sys.argv[1:])
catalog = json.loads((root / "kai_pets/animation_catalog.json").read_text())
for pet in catalog["pets"]:
    source = root / "kai_pets" / pet["spritesheet"]
    destination = resources / "kai_pets" / pet["spritesheet"]
    destination.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(source, destination)
PY
# A certificate keeps the designated requirement stable across rebuilds.
# Ad-hoc signatures change with the executable and can invalidate privacy grants.
signing_identity=${KAI_SIGNING_IDENTITY:--}
if [[ "$signing_identity" == "-" ]] && security find-identity -v -p codesigning | rg -q '"system local code signing"'; then
    signing_identity="system local code signing"
fi
codesign --force --deep --sign "$signing_identity" "$product"
codesign --verify --deep --strict "$product"
# Never truncate a signed executable which an existing Kai process has mapped.
if [[ -e "$destination" ]]; then
    mv "$destination" "$staging/previous-Kai.app"
fi
if ! mv "$product" "$destination"; then
    [[ ! -e "$staging/previous-Kai.app" ]] || mv "$staging/previous-Kai.app" "$destination"
    exit 1
fi
# Refresh the signature at its installed path, then verify the delivered bundle.
# macOS may retain signing information for the previously installed executable.
codesign --force --deep --sign "$signing_identity" "$destination"
codesign --verify --deep --strict "$destination"
print -r -- "$destination"
