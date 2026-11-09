#!/usr/bin/env bash
# Build a source RPM: source tarball + vendored crates tarball + spec.
#
# Usage: packaging/make-srpm.sh [PACKAGE] [OUTDIR]
#   PACKAGE  fullscreen-ws (default) or cosmic-ext-applet-fullscreen-ws
#   OUTDIR   where the .src.rpm goes (default: ./dist)
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
name=${1:-fullscreen-ws}
outdir=$(realpath -m "${2:-$root/dist}")

case "$name" in
    fullscreen-ws) crate_dir=. ;;
    cosmic-ext-applet-fullscreen-ws) crate_dir=applet ;;
    *) echo "unknown package: $name" >&2; exit 1 ;;
esac

spec="$root/packaging/$name.spec"
version=$(sed -n 's/^version = "\(.*\)"/\1/p' "$root/$crate_dir/Cargo.toml" | head -1)
spec_version=$(sed -n 's/^Version:[[:space:]]*//p' "$spec")
if [ "$version" != "$spec_version" ]; then
    echo "version mismatch: $crate_dir/Cargo.toml has $version, $name.spec has $spec_version" >&2
    exit 1
fi

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
src="$work/$name-$version"
mkdir -p "$outdir" "$work/SOURCES" "$src"

# Source tarball: tracked files if this is a git checkout, else the tree minus build output.
if git -C "$root" rev-parse --git-dir >/dev/null 2>&1; then
    git -C "$root" ls-files -z | tar -C "$root" --null -T - -cf - | tar -C "$src" -xf -
else
    tar -C "$root" --exclude=target --exclude=./dist --exclude=.git -cf - . | tar -C "$src" -xf -
fi
tar -C "$work" -czf "$work/SOURCES/$name-$version.tar.gz" "$name-$version"

# Vendored dependencies (needs network; the RPM build itself runs offline).
# cargo vendor prints the matching .cargo/config.toml; ship it inside the tarball.
(cd "$src/$crate_dir" && cargo vendor --locked vendor > "$work/cargo-config.toml")
cp "$work/cargo-config.toml" "$src/$crate_dir/vendor/cargo-config.toml"
tar -C "$src/$crate_dir" -czf "$work/SOURCES/$name-$version-vendor.tar.gz" vendor

rpmbuild -bs \
    --define "_sourcedir $work/SOURCES" \
    --define "_srcrpmdir $outdir" \
    "$spec"
