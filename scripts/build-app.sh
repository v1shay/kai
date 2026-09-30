#!/bin/zsh
set -euo pipefail

root=${0:A:h:h}
cd "$root"
cache=/private/tmp/notch-clang-cache
CLANG_MODULE_CACHE_PATH="$cache" SWIFTPM_MODULECACHE_OVERRIDE="$cache" swift build --disable-sandbox

product="$root/.build/debug/Kai.app"
mkdir -p "$product/Contents/MacOS"
cp "$root/.build/debug/NotchPrototype" "$product/Contents/MacOS/NotchPrototype"
cp "$root/Support/NotchPrototype-Info.plist" "$product/Contents/Info.plist"
mkdir -p "$product/Contents/Resources/kai_pets"
mkdir -p "$product/Contents/Resources/katex"
cp "$root/kai.py" "$root/kai_bridge.py" "$root/kai_activity.py" "$product/Contents/Resources/"
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
codesign --force --deep --sign - "$product"
print -r -- "$product"
