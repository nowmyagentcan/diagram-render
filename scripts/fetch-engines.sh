#!/bin/sh
# fetch-engines.sh TARGET DEST: puts the engines pinned in engines.lock for TARGET (windows-x64,
# macos-arm64, macos-x64, linux-x64, linux-arm64) in DEST/engines, the fonts PNGs are drawn with in
# DEST/fonts, and their licences in DEST/licenses.
# Runs on Linux (windows-x64 is cross-built with mingw-w64) or on macOS, for the machine's own CPU.
# merman and d2 are the upstream builds, except Linux merman: its release needs glibc 2.38, so the
# unmodified crate is built as a static musl binary. dot is built from the unmodified Graphviz and
# expat source tarballs as one static executable with no plugins to load.
set -eu
target=$1
dest=$(mkdir -p "$2" && cd "$2" && pwd)
lock=$(cd "$(dirname "$0")" && pwd)/engines.lock
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
jobs=$(getconf _NPROCESSORS_ONLN 2>/dev/null || sysctl -n hw.ncpu)
exe= host= static=-all-static
case $target in
  windows-x64) exe=.exe host=x86_64-w64-mingw32 ;;
  macos-*) static= ;;
  linux-x64 | linux-arm64) ;;
  *) echo "fetch-engines: unknown target $target" >&2; exit 2 ;;
esac

# fetch NAME DIR: downloads NAME's file for this target, checks its SHA-256 and unpacks it into DIR.
fetch() {
  set -- $(awk -v n="$1" -v t="$target" '$1 == n && ($3 == t || $3 == "all" || ($3 == "linux" && t ~ /^linux/))' "$lock") "$2"
  [ $# -eq 6 ] || { echo "fetch-engines: engines.lock has no row for $target" >&2; exit 1; }
  f=$work/${5##*/}
  curl -fsSL --retry 8 --retry-all-errors --retry-delay 3 -o "$f" "$5"
  sum=$(sha256sum "$f" 2>/dev/null || shasum -a 256 "$f")
  [ "${sum%% *}" = "$4" ] || { echo "fetch-engines: ${5##*/} doesn't match its SHA-256" >&2; exit 1; }
  mkdir -p "$6"
  case $f in
    *.zip) unzip -q "$f" -d "$6" ;;
    *) tar -xf "$f" -C "$6" --strip-components=1 ;;
  esac
}

mkdir -p "$dest/engines" "$dest/fonts" "$dest/licenses/merman" "$dest/licenses/d2" "$dest/licenses/graphviz"   "$dest/licenses/fonts"

fetch liberation "$work/liberation"
for f in Sans-Regular Sans-Bold Sans-Italic Sans-BoldItalic Serif-Regular Serif-Bold Mono-Regular Mono-Bold; do
  cp "$work/liberation/Liberation$f.ttf" "$dest/fonts/"
done
cp "$work/liberation/LICENSE" "$dest/licenses/fonts/Liberation-LICENSE.txt"

case $target in
  linux-*)
    fetch merman-notices "$work/merman"
    fetch merman-crate "$work/crate"
    triple=$(uname -m)-unknown-linux-musl
    rustup target add "$triple" >/dev/null 2>&1
    (cd "$work/crate" && CARGO_TARGET_DIR="$work/cargo" cargo build --quiet --release --locked --target "$triple")
    cp "$work/cargo/$triple/release/merman-cli" "$dest/engines/"
    strip "$dest/engines/merman-cli" ;;
  *)
    fetch merman "$work/merman"
    cp "$work/merman/merman-cli$exe" "$dest/engines/" ;;
esac
cp -R "$work"/merman/LICENSE-* "$work/merman/THIRD_PARTY_NOTICES.md" "$work/merman/THIRD_PARTY_LICENSES" "$dest/licenses/merman/"

fetch d2 "$work/d2"
cp "$work/d2/bin/d2$exe" "$dest/engines/"
cp "$work/d2/LICENSE.txt" "$work/d2/THIRD_PARTY_NOTICES.txt" "$dest/licenses/d2/"

fetch expat "$work/expat"
fetch graphviz "$work/graphviz"
cross=${host:+--host=$host}
(cd "$work/expat" && ./configure -q $cross --prefix="$work/deps" --disable-shared --enable-static \
  --without-docbook --without-examples --without-tests && make -s -j"$jobs" && make -s install) >/dev/null
cd "$work/graphviz"
# Every optional library is off, so the same text-width tables lay out text on every OS (zlib by its
# configure cache variable: there is no switch). Only lib/, plugin/ and dot are built, never bindings.
./configure -q $cross --enable-static --disable-shared --disable-ltdl --without-x --with-expat \
  --without-gts --without-gdk --without-gdk-pixbuf --without-gtk --without-pangocairo --without-freetype2 \
  --without-fontconfig --without-gd --without-webp --without-rsvg --without-poppler --without-ghostscript \
  --without-lasi --without-glut --without-qt --without-devil --without-ann --without-quartz --without-gdiplus \
  ac_cv_header_zlib_h=no \
  CPPFLAGS="-I$work/deps/include" LDFLAGS="-L$work/deps/lib" CFLAGS="-O2 -w" CXXFLAGS="-O2 -w" >/dev/null
for d in lib plugin; do make -s -j"$jobs" -C $d >/dev/null; done
make -s -C cmd/dot "dot_static$exe" LDFLAGS="-L$work/deps/lib $static" >/dev/null
cp "cmd/dot/dot_static$exe" "$dest/engines/dot$exe"
cp COPYING cpl1.0.txt "$dest/licenses/graphviz/"
cp "$work/expat/COPYING" "$dest/licenses/graphviz/expat-COPYING"

${host:+$host-}strip "$dest/engines/dot$exe"
case $target in macos-*) codesign --force --sign - "$dest/engines/dot" ;; esac
ls -l "$dest/engines" "$dest/fonts"
