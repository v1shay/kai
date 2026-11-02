#!/bin/zsh
set -euo pipefail
root=${0:A:h:h}
cd "$root"
configuration=${KAI_UNIFIED_CONFIGURATION:-release}
if [[ "$configuration" != release && "$configuration" != debug ]]; then
  print -u2 'KAI_UNIFIED_CONFIGURATION must be release or debug'; exit 1
fi
export CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-$root/.build/unified-cargo}
export CLANG_MODULE_CACHE_PATH=${CLANG_MODULE_CACHE_PATH:-/private/tmp/kai-unified-clang}
swift_cache=${KAI_SWIFT_MODULE_CACHE:-/private/tmp/kai-unified-swift-v2}
mkdir -p "$root/.build/unified" "$swift_cache"
args=(build --offline --manifest-path "$root/integrations/Code/Cargo.toml" --bin orbit)
[[ "$configuration" != release ]] || args+=(--release)
cargo "${args[@]}"
swift_optimization=(-O -whole-module-optimization)
[[ "$configuration" == release ]] || swift_optimization=(-Onone)
python3 "$root/scripts/build-native-cached.py" "$root/.build/unified/libKaiNotch.dylib" -parse-as-library -emit-library "${swift_optimization[@]}" -D KAI_UNIFIED -module-name KaiNotch -module-cache-path "$swift_cache" "$root"/Sources/NotchPrototype/*.swift
python3 - "$root/.build/unified/libKaiNotch.dylib" <<'PYABI'
import ctypes,sys
library=ctypes.CDLL(sys.argv[1])
for name in ['start','publish','toggle','collapse','ready','workspace_visible','levels','diagnostics','voice_host','wallpaper','quick_surface']:
 getattr(library,'kai_unified_'+name)
print('Verified all eleven native ABI exports')
PYABI
python3 "$root/scripts/build-native-cached.py" "$root/.build/unified/libCodeVoice.dylib" -parse-as-library -emit-library "${swift_optimization[@]}" -module-cache-path "$swift_cache" "$root/integrations/Code/native_helpers/CodeVoiceMedia.swift" -framework AppKit -framework WebKit -framework AVFoundation
staging=$(mktemp -d "$root/.build/unified/.package.XXXXXX")
trap 'rm -rf "$staging"' EXIT
product="$staging/Kai Unified.app"
resources="$product/Contents/Resources"
mkdir -p "$product/Contents/MacOS" "$resources/Licenses"
cp "$CARGO_TARGET_DIR/$configuration/orbit" "$product/Contents/MacOS/kai-unified"
cp "$root/.build/unified/"*.dylib "$resources/"
cp -R "$root/Sources/NotchPrototype/Resources/"* "$resources/"
mkdir -p "$resources/kai_pets" "$resources/providers"
for provider in codex claude gemini opencode cursor grok antigravity ninja tailscale model-moon model-earth model-sun model-star; do cp "$root/integrations/Code/assets/$provider.svg" "$resources/providers/$provider.svg"; done
for frame in 0 1 2 3; do cp "$root/integrations/Code/assets/ninja-walk-$frame.svg" "$resources/providers/"; done
for style in fire lightning earth ice energy water; do cp "$root/integrations/Code/assets/ninja-$style.svg" "$root/integrations/Code/assets/ninja-$style.gif" "$resources/providers/"; done
for mark in "$root"/integrations/Code/assets/{lab,engine,machine}-*.svg; do cp "$mark" "$resources/providers/"; done
mkdir -p "$resources/skills"
cp -R "$root/integrations/Code/skills/"* "$resources/skills/"
cp "$root/kai_speech.py" "$resources/"
cp "$root/integrations/Code/native_helpers/kai_inference_stream.py" "$resources/"
cp "$root/integrations/Code/native_helpers/kai_ninja.py" "$resources/"
cp "$root/integrations/Code/native_helpers/kai_ninja_host.py" "$resources/"
cp "$root/integrations/Code/native_helpers/kai_ninja_handoff.py" "$resources/"
cp "$root/integrations/Code/native_helpers/kai_workspace_mcp.py" "$resources/"
cp "$root/integrations/Code/native_helpers/kai_remote.py" "$resources/"
cp "$root/integrations/Code/native_helpers/kai_agy_hook.py" "$resources/"
cp "$root/integrations/Code/native_helpers/kai_mcp_app.html" "$resources/"
cp -R "$root/integrations/Code/LICENSES/"* "$resources/Licenses/"
cp "$root/integrations/Code/native_helpers/KAI-SOURCE.md" "$resources/Licenses/Kai-Voice-Attribution.txt"
cp "$root/integrations/Code/assets/LICENSES/Lobe-Icons-MIT.txt" "$resources/Licenses/"
cp "$root/integrations/Code/assets/NINJA-LOGOS.md" "$resources/Licenses/"
cp "$root/integrations/Code/assets/NOTICE.md" "$resources/Licenses/Code-Assets.txt"
cp "$root/integrations/Code/UPSTREAM.json" "$resources/Code-Upstream.json"
python3 - "$root" "$product" <<'PY'
import json,plistlib,shutil,sys
from pathlib import Path
root,product=map(Path,sys.argv[1:]);resources=product/'Contents/Resources'
catalog=json.loads((root/'kai_pets/animation_catalog.json').read_text())
for name in ['animation_catalog.json','gradient_profiles.json']:
 shutil.copy2(root/'kai_pets'/name,resources/'kai_pets'/name)
for pet in catalog['pets']:
 source=root/'kai_pets'/pet['spritesheet'];target=resources/'kai_pets'/pet['spritesheet']
 target.parent.mkdir(parents=True,exist_ok=True);shutil.copy2(source,target)
info=plistlib.loads((root/'integrations/Code/scripts/Info.plist').read_bytes())
info.update(CFBundleName='Kai Unified',CFBundleDisplayName='Kai Unified',CFBundleIdentifier='com.kai.unified.experimental',CFBundleExecutable='kai-unified',CFBundleVersion='89',CFBundleShortVersionString='0.12.52')
icon=root/'integrations/Code/assets/Code.icns'
if icon.exists():shutil.copy2(icon,resources/'Code-Bonsai.icns')
info['CFBundleURLTypes']=[{'CFBundleURLName':'Kai workspace','CFBundleURLSchemes':['kai']}]
info['CFBundleDocumentTypes']=[{'CFBundleTypeName':'Folders and source files','CFBundleTypeRole':'Viewer','LSHandlerRank':'Alternate','LSItemContentTypes':['public.folder','public.data']}]
info['NSServices']=[{'NSMenuItem':{'default':'Open in Kai Unified'},'NSMessage':'openInKai','NSPortName':'Kai Unified','NSSendTypes':['public.file-url'],'NSRequiredContext':{}}]
info['NSLocalNetworkUsageDescription']='Kai connects to your selected inference machines and persistent Ninja hosts over Tailscale. Assigned remote tasks run on your selected host; reference files may be sent with Taildrop.'
info['NSDownloadsFolderUsageDescription']='Kai imports local pet packs and the workspace handoffs you explicitly send through Taildrop.'
info['NSAppleEventsUsageDescription']='Kai uses app automation for tasks you authorize and optional media controls.'
(product/'Contents/Info.plist').write_bytes(plistlib.dumps(info))
PY
python3 "$root/scripts/build-app-intents.py" "$resources/libKaiNotch.dylib" "$resources"
identity=${KAI_SIGNING_IDENTITY:--}
for library in "$resources/"*.dylib; do codesign --force --sign "$identity" "$library"; done
codesign --force --sign "$identity" "$product"
codesign --verify --deep --strict "$product"
destination="$root/.build/unified/Kai Unified.app"
[[ ! -e "$destination" ]] || mv "$destination" "$staging/previous.app"
if ! mv "$product" "$destination"; then
  [[ ! -e "$staging/previous.app" ]] || mv "$staging/previous.app" "$destination"
  exit 1
fi
print -r -- "$destination"
