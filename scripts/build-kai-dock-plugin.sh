#!/bin/zsh
set -euo pipefail
root=${0:A:h:h}
plugin="$1/Contents/PlugIns/KaiPet.docktileplugin/Contents"
mkdir -p "$plugin/MacOS"
xcrun clang -fobjc-arc -bundle -framework AppKit -mmacosx-version-min=13.0 \
  "$root/scripts/KaiPetDockTile.m" -o "$plugin/MacOS/KaiPetDockTile"
python3 - "$plugin/Info.plist" <<'PY'
import plistlib,sys
with open(sys.argv[1], 'wb') as output:
    plistlib.dump(dict(CFBundleIdentifier='com.kai.unified.dock-pet', CFBundlePackageType='BNDL',
                       CFBundleExecutable='KaiPetDockTile', CFBundleVersion='1',
                       NSPrincipalClass='KaiPetDockTile'), output)
PY
