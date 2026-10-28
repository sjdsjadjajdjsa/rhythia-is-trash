#!/bin/sh
# Builds rhythia-is-trash-x86_64.AppImage in the repository root from a release build.
# Maps, settings and scores of the AppImage live in $XDG_DATA_HOME/rhythia-is-trash
# (~/.local/share/rhythia-is-trash by default), since the image itself is read-only.
# Needs curl and file besides the Rust build tools.
set -eu
cd "$(dirname "$0")/../.."

version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
out=rhythia-is-trash-x86_64.AppImage
work=target/appimage
appdir=$work/AppDir
tool=${APPIMAGETOOL:-$work/appimagetool-x86_64.AppImage}

cargo build --release

rm -rf "$appdir"
mkdir -p "$appdir/usr/bin" "$appdir/usr/share/applications" \
    "$appdir/usr/share/icons/hicolor/256x256/apps" "$appdir/usr/share/doc/rhythia-is-trash"
install -m 755 target/release/rhythm "$appdir/usr/bin/rhythm"
install -m 644 packaging/linux/rhythia-is-trash.desktop "$appdir/usr/share/applications/"
install -m 644 packaging/linux/rhythia-is-trash.png "$appdir/usr/share/icons/hicolor/256x256/apps/"
install -m 644 README.md LICENSE "$appdir/usr/share/doc/rhythia-is-trash/"
install -m 644 assets/fonts/NOTICE.txt "$appdir/usr/share/doc/rhythia-is-trash/FONTS-NOTICE.txt"
ln -s usr/share/applications/rhythia-is-trash.desktop "$appdir/rhythia-is-trash.desktop"
ln -s usr/share/icons/hicolor/256x256/apps/rhythia-is-trash.png "$appdir/rhythia-is-trash.png"
ln -s rhythia-is-trash.png "$appdir/.DirIcon"
ln -s usr/bin/rhythm "$appdir/AppRun"

if [ ! -x "$tool" ]; then
    curl -fsSL -o "$tool" https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-x86_64.AppImage
    chmod +x "$tool"
fi

rm -f "$out" "$out.zsync"
# AppImageUpdate and launchers read this to fetch the newest GitHub release.
update="gh-releases-zsync|sjdsjadjajdjsa|rhythia-is-trash|latest|$out.zsync"
ARCH=x86_64 VERSION="$version" APPIMAGE_EXTRACT_AND_RUN=1 "$tool" --no-appstream -u "$update" "$appdir" "$out"
