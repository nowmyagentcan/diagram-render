#!/bin/sh
# Installs diagram-render @VERSION@ with its engines and fonts into ~/.local/share/diagram-render,
# linked from ~/.local/bin, after checking the archive's SHA-256.
# curl -fsSL https://github.com/nowmyagentcan/diagram-render/releases/latest/download/install.sh | sh
set -eu
version=@VERSION@
base=${DIAGRAM_RENDER_BASE:-https://github.com/nowmyagentcan/diagram-render/releases/download/$version}
case "$(uname -s)-$(uname -m)" in
  Linux-x86_64) target=linux-x64 ;;
  Linux-aarch64 | Linux-arm64) target=linux-arm64 ;;
  Darwin-x86_64) target=macos-x64 ;;
  Darwin-arm64) target=macos-arm64 ;;
  *) echo "diagram-render: no build for $(uname -s) $(uname -m)." >&2; exit 1 ;;
esac
archive=diagram-render-$version-$target.tar.gz
dir=$HOME/.local/share/diagram-render
new=$dir.new.$$
old=$dir.old.$$
tmp=$(mktemp -d)
trap 'rm -rf "$tmp" "$new"' EXIT
curl -fsSL -o "$tmp/$archive" "$base/$archive"
curl -fsSL -o "$tmp/SHA256SUMS" "$base/SHA256SUMS"
expected=$(awk -v f="$archive" '$2 == f { print $1 }' "$tmp/SHA256SUMS")
if command -v sha256sum >/dev/null; then actual=$(sha256sum "$tmp/$archive"); else actual=$(shasum -a 256 "$tmp/$archive"); fi
if [ -z "$expected" ] || [ "${actual%% *}" != "$expected" ]; then
  echo "diagram-render: the SHA-256 of $archive doesn't match SHA256SUMS; nothing was installed." >&2
  exit 1
fi
mkdir -p "$new"
tar -xzf "$tmp/$archive" -C "$new"
# An empty file "runs" as an empty script, so the answer is checked too.
case $("$new/diagram-render" --version 2>/dev/null || :) in
  "diagram-render "*) ;;
  *) echo "diagram-render: $archive has no diagram-render that runs here; nothing was installed." >&2; exit 1 ;;
esac
# The folder is swapped whole: a running engine keeps its old file, and a failed move puts the old
# folder back.
[ ! -e "$dir" ] || mv "$dir" "$old"
if ! mv "$new" "$dir"; then
  [ ! -e "$old" ] || mv "$old" "$dir"
  echo "diagram-render: can't write $dir; the old version stays." >&2
  exit 1
fi
rm -rf "$old"
bin=$HOME/.local/bin
mkdir -p "$bin"
ln -sf "$dir/diagram-render" "$bin/diagram-render"
"$bin/diagram-render" --version
case ":$PATH:" in
  *":$bin:"*) ;;
  *) echo "Add $bin to your PATH: export PATH=\"\$HOME/.local/bin:\$PATH\"" ;;
esac
