#!/bin/sh
# Generates a Debian .changes file next to a .deb built by cargo-deb.
# cargo-deb does not produce a .changes file (it is not a source build), so we
# derive one from the finished .deb control fields plus the latest changelog
# entry. The result can be signed with debsign and uploaded with dput.
#
# Usage: pkg/make-changes.sh <path-to.deb> [path-to-changelog]
#   path-to-changelog defaults to pkg/changelog.

set -eu

deb="${1:?usage: make-changes.sh <path-to.deb> [changelog]}"
changelog="${2:-pkg/changelog}"

[ -f "$deb" ] || {
    echo "deb not found: $deb" >&2
    exit 1
}
[ -f "$changelog" ] || {
    echo "changelog not found: $changelog" >&2
    exit 1
}

# Control fields from the finished package.
control="$(dpkg-deb -f "$deb")"
field() { printf '%s\n' "$control" | sed -n "s/^$1: //p"; }

package="$(field Package)"
version="$(field Version)"
arch="$(field Architecture)"
section="$(field Section)"
priority="$(field Priority)"
maintainer="$(field Maintainer)"
description_short="$(field Description)"

[ -n "$package" ] && [ -n "$version" ] && [ -n "$arch" ] || {
    echo "failed to read control fields from $deb" >&2
    exit 1
}

# Latest changelog entry: header line, body, and the trailer date.
# Header:  dwd-backend (0.1.0-1) unstable; urgency=medium
header="$(sed -n '1p' "$changelog")"
distribution="$(printf '%s' "$header" | sed -n 's/.*) \([^;]*\);.*/\1/p')"
urgency="$(printf '%s' "$header" | sed -n 's/.*urgency=\([^ ]*\).*/\1/p')"
[ -n "$distribution" ] || distribution="unstable"
[ -n "$urgency" ] || urgency="medium"

# Trailer line " -- Maintainer <email>  Date" gives the RFC-2822 date.
date="$(sed -n 's/^ -- .*>  //p' "$changelog" | sed -n '1p')"
[ -n "$date" ] || date="$(date -R)"

# Changes block: the whole latest entry, with a leading '.' for blank lines
# (dpkg control paragraph continuation).
changes_block="$(awk 'NR==1{print; next} /^ -- /{print; exit} {print}' "$changelog" |
    sed 's/^$/./' | sed 's/^/ /')"

deb_file="$(basename "$deb")"
size="$(wc -c <"$deb" | tr -d ' ')"
md5="$(md5sum "$deb" | cut -d' ' -f1)"
sha1="$(sha1sum "$deb" | cut -d' ' -f1)"
sha256="$(sha256sum "$deb" | cut -d' ' -f1)"

out="$(dirname "$deb")/${package}_${version}_${arch}.changes"

{
    echo "Format: 1.8"
    echo "Date: $date"
    echo "Source: $package"
    echo "Binary: $package"
    echo "Architecture: $arch"
    echo "Version: $version"
    echo "Distribution: $distribution"
    echo "Urgency: $urgency"
    echo "Maintainer: $maintainer"
    echo "Changed-By: $maintainer"
    echo "Description:"
    echo " $package - $description_short"
    echo "Changes:"
    printf '%s\n' "$changes_block"
    echo "Checksums-Sha1:"
    echo " $sha1 $size $deb_file"
    echo "Checksums-Sha256:"
    echo " $sha256 $size $deb_file"
    echo "Files:"
    echo " $md5 $size $section $priority $deb_file"
} >"$out"

echo "$out"
