#!/usr/bin/env bash
# Makes Keynortia.app and the disk image to distribute it. Runs on macOS (it needs sips, iconutil,
# codesign and hdiutil), from the root of the repository.
#
#   packaging/macos/bundle.sh <binary> <version> <output folder>
#
# The binary can be for one architecture or universal (made with lipo).
set -euo pipefail

if [ "$#" -ne 3 ]; then
    echo "usage: $0 <binary> <version> <output folder>" >&2
    exit 2
fi
binary=$1
version=$2
out=$3

app="$out/Keynortia.app"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"

cp "$binary" "$app/Contents/MacOS/keynortia"
# the licences of the fonts built into the program, next to the program
mkdir -p "$app/Contents/Resources/licenses"
cp assets/fonts/OFL-*.txt "$app/Contents/Resources/licenses/"
chmod 755 "$app/Contents/MacOS/keynortia"
sed "s/@VERSION@/$version/g" packaging/macos/Info.plist > "$app/Contents/Info.plist"

# the icon: the sizes macOS wants, from the 512 px one
work=$(mktemp -d)
iconset="$work/keynortia.iconset"
mkdir "$iconset"
resize() { sips -z "$1" "$1" assets/icon-512.png --out "$iconset/$2" > /dev/null; }
resize 16 icon_16x16.png
resize 32 icon_16x16@2x.png
resize 32 icon_32x32.png
resize 64 icon_32x32@2x.png
resize 128 icon_128x128.png
resize 256 icon_128x128@2x.png
resize 256 icon_256x256.png
resize 512 icon_256x256@2x.png
resize 512 icon_512x512.png
iconutil -c icns "$iconset" -o "$app/Contents/Resources/keynortia.icns"

# An ad-hoc signature (no developer account): macOS will not run an unsigned program on Apple
# silicon, and `lipo` takes away the signature that the linker had put on each half.
codesign --force --deep --sign - "$app"
codesign --verify --deep --strict "$app"

# the disk image: the app, and a shortcut to Applications to drag it onto
dmg="$out/Keynortia-$version-macos.dmg"
stage="$work/dmg"
mkdir "$stage"
cp -R "$app" "$stage/"
ln -s /Applications "$stage/Applications"
rm -f "$dmg"
# hdiutil sometimes answers "Resource busy" on a busy machine: try again
for attempt in 1 2 3; do
    if hdiutil create -volname "Keynortia" -srcfolder "$stage" -ov -format UDZO "$dmg"; then
        break
    fi
    if [ "$attempt" -eq 3 ]; then
        echo "hdiutil failed" >&2
        exit 1
    fi
    sleep 5
done

rm -rf "$work"
echo "made $dmg"
