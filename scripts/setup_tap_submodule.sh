#!/usr/bin/env bash
set -euo pipefail

# Usage: scripts/setup_tap_submodule.sh <tap-repo-url>
# Example: scripts/setup_tap_submodule.sh git@github.com:iskng/homebrew-tap.git

if [ $# -lt 1 ]; then
  echo "Usage: $0 <tap-repo-url>" >&2
  exit 1
fi

TAP_URL="$1"
DEST="packaging/homebrew-tap"

if [ -e "$DEST" ] && [ ! -d "$DEST/.git" ]; then
  echo "Destination $DEST exists and is not a git repo; remove or choose another path." >&2
  exit 1
fi

if [ ! -d "$DEST" ]; then
  git submodule add "$TAP_URL" "$DEST"
fi

mkdir -p "$DEST/Formula"

if [ ! -f "$DEST/Formula/repod.rb" ]; then
  cp packaging/homebrew/repod.rb "$DEST/Formula/repod.rb"
  git -C "$DEST" add Formula/repod.rb
  git -C "$DEST" commit -m "Add repod formula"
  echo "Initialized tap with formula. Now push the submodule repo:"
  echo "  git -C $DEST push origin HEAD"
fi

echo "Submodule ready at $DEST. Commit submodule pointer in main repo:"
echo "  git add .gitmodules $DEST && git commit -m 'Add tap submodule' && git push"
