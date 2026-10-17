#!/bin/sh
# Publish the packages build-npm.mjs made in <dir> (default: npm) to npm, platform packages first,
# so @autter/lanes never points at a missing one. Safe to rerun: versions npm already has are
# skipped. Authenticates with NODE_AUTH_TOKEN, or with npm trusted publishing when that's set up.
#
#   sh packaging/publish-npm.sh <version> [dir]

set -eu
version="$1"
dir="${2:-npm}"

for pkg in "$dir"/lanes-* "$dir"/lanes; do
	name=$(node -p "require('./$pkg/package.json').name")
	if npm view "$name@$version" version >/dev/null 2>&1; then
		echo "$name@$version is already on npm."
		continue
	fi
	npm publish "./$pkg" --access public --provenance
done
