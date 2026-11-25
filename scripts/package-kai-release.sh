#!/bin/zsh
set -euo pipefail

if (( $# != 2 )); then
  print -u2 'Usage: package-kai-release.sh SIGNED_APP OUTPUT_DIRECTORY'
  exit 2
fi
app=${1:A}
out=${2:A}
if [[ ${app:t} != 'Kai Unified.app' ]]; then
  print -u2 'The update archive must contain Kai Unified.app'
  exit 2
fi
info="$app/Contents/Info.plist"
helper="$app/Contents/MacOS/kai-server"
build=$(/usr/libexec/PlistBuddy -c 'Print :CFBundleVersion' "$info")
bundle=$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$info")
if [[ "$build" != <190-> || "$bundle" != com.kai.unified.experimental || "$("$helper" self-test)" != "$build" ]]; then
  print -u2 'The app and server must be matching Kai build 190 or newer'
  exit 1
fi
/usr/bin/codesign --verify --deep --strict "$app"
requirement=$(/usr/bin/codesign -d -r- "$app" 2>&1)
if [[ "$requirement" != *'certificate'* && "$requirement" != *'anchor apple'* ]]; then
  print -u2 'Use a certificate-backed signing identity'
  exit 1
fi
if [[ -n "$(/usr/bin/find "$app" -type l -print -quit)" ]]; then
  print -u2 'The Kai updater requires an archive without symlinks'
  exit 1
fi
architecture=$(/usr/bin/lipo -archs "$helper")
case "$architecture" in
  arm64) architecture=arm64 ;;
  x86_64) architecture=amd64 ;;
  'x86_64 arm64'|'arm64 x86_64') architecture=universal ;;
  *) print -u2 'Unsupported Kai Server architecture'; exit 1 ;;
esac
mkdir -p "$out"
scratch=$(mktemp -d "$out/.release.XXXXXX")
trap 'rm -r "$scratch"' EXIT
asset="Kai-Unified-${build}-macos-${architecture}.zip"
/usr/bin/ditto -c -k --norsrc --keepParent "$app" "$scratch/$asset"
/usr/bin/unzip -tq "$scratch/$asset"
/bin/mv -f "$scratch/$asset" "$out/$asset"
print -r -- "$out/$asset"
