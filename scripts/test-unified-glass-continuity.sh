#!/bin/zsh
set -euo pipefail
root=${0:A:h:h}
staging=$(mktemp -d /private/tmp/kai-glass-continuity.XXXXXX)
trap 'rm -rf "$staging"' EXIT
swiftc -parse-as-library -module-cache-path /private/tmp/kai-glass-continuity-cache "$root/Sources/NotchPrototype/WorkspaceGlassBridge.swift" "$root/Sources/NotchPrototype/WorkspaceMorph.swift" "$root/Tests/UnifiedNative/GlassContinuityHarness.swift" -o "$staging/audit"
"$staging/audit"
