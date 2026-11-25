#!/bin/zsh
set -euo pipefail
root="${0:A:h:h}"
if (( $# != 1 )); then
  print -u2 "Usage: $0 /path/to/code"
  exit 2
fi
code_root="$1"
if [[ ! -f "$code_root/Cargo.toml" || ! -f "$code_root/src/main.rs" ]]; then
  print -u2 "Expected the Code Rust project folder."
  exit 1
fi
patch="$root/integrations/code-host/code-connection.patch"
git -C "$code_root" apply --check "$patch"
git -C "$code_root" apply "$patch"
print "Applied the Kai connection to $code_root. Build Code with its scripts/bundle.sh."
