#!/bin/sh
# package.sh TARGET BINARY ENGINES DIST: writes the release archive for TARGET into DIST, from the
# built BINARY and ENGINES (what fetch-engines.sh left: engines/, fonts/, licenses/). Run it from the
# repository, after `cargo about` has written THIRD-PARTY-LICENSES.txt.
set -eu
target=$1 binary=$2 engines=$3 dist=$4
version=v$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml)
name=diagram-render-$version-$target
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
mkdir -p "$dist" "$work/$name"
cp "$binary" "$work/$name/"
cp -R "$engines/engines" "$engines/fonts" "$engines/licenses" "$work/$name/"
cp LICENSE NOTICE README.md "$work/$name/"
cp THIRD-PARTY-LICENSES.txt "$work/$name/licenses/"
chmod +x "$work/$name"/diagram-render* "$work/$name"/engines/*
# The layout every archive promises.
cd "$work/$name"
test "$(ls engines | wc -l)" -eq 3
test "$(ls fonts/*.ttf | wc -l)" -eq 8
test "$(ls fonts/excalidraw/*.woff2 | wc -l)" -eq 20
test "$(ls licenses/fonts | wc -l)" -eq 7
for f in merman/THIRD_PARTY_NOTICES.md merman/THIRD_PARTY_LICENSES merman/LICENSE-MIT d2/LICENSE.txt \
  d2/THIRD_PARTY_NOTICES.txt graphviz/COPYING graphviz/expat-COPYING; do
  test -e "licenses/$f"
done
dist=$(cd "$OLDPWD" && cd "$dist" && pwd)
if [ "$target" = windows-x64 ]; then
  zip -q -9 -r "$dist/$name.zip" .
  archive=$dist/$name.zip
else
  tar -cf - . | gzip -9 > "$dist/$name.tar.gz"
  archive=$dist/$name.tar.gz
fi
size=$(wc -c < "$archive")
unpacked=$(du -sk . | cut -f1)
echo "$name: $size bytes, $unpacked KB unpacked"
test "$size" -le $((60 << 20))
test "$unpacked" -le $((130 << 10))
