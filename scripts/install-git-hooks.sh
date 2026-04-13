#!/usr/bin/env bash
# Install diffcore's git hooks into .git/hooks.
#
# Symlinks each file in scripts/git-hooks/ into .git/hooks/ so that future
# edits in the tracked location take effect immediately. Run once after
# cloning the repo:
#
#     ./scripts/install-git-hooks.sh
set -euo pipefail

repo_root=$(git rev-parse --show-toplevel)
src_dir="$repo_root/scripts/git-hooks"
dst_dir="$repo_root/.git/hooks"

if [ ! -d "$src_dir" ]; then
  echo "install-git-hooks: $src_dir does not exist" >&2
  exit 1
fi

mkdir -p "$dst_dir"

for src in "$src_dir"/*; do
  name=$(basename "$src")
  dst="$dst_dir/$name"
  ln -sf "$src" "$dst"
  chmod +x "$src"
  echo "installed $name -> $src"
done
