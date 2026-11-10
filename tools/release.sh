#!/bin/sh
# Opens the pull request of a release: tools/release.sh 0.6.0
#
# It sets the version (the firmware workspace and the CLI), turns CHANGELOG.md's Unreleased section
# into the version's, and opens "Release <version>" on a branch of its own. Test the branch on a C6
# board, then merge: .github/workflows/release.yml tags the merge and publishes the GitHub release.
set -eu
cd "$(dirname "$0")/.."
version=${1:?usage: tools/release.sh <version>}
repo=https://github.com/tanguy-c/iotkit

case $version in
    *[!0-9.]* | .* | *. | *..*) echo "not a version: $version" >&2; exit 1 ;;
esac
# From an up-to-date main. CHANGELOG.md and docs/ may hold the release's edits: the summary, the
# upgrade steps, the hardware test table.
git fetch --quiet origin main
if [ "$(git rev-parse HEAD)" != "$(git rev-parse origin/main)" ]; then
    echo "check out main, up to date with origin/main" >&2
    exit 1
fi
if git status --porcelain --untracked-files=no | grep -qvE ' (CHANGELOG\.md|docs/.*)$'; then
    echo "commit or stash your changes first (CHANGELOG.md and docs/ may stay)" >&2
    exit 1
fi

current=$(sed -n 's/^version *= *"\(.*\)"/\1/p' Cargo.toml | head -n 1)
newest=$(printf '%s\n%s\n' "$current" "$version" | sort -V | tail -n 1)
if [ "$version" = "$current" ] || [ "$newest" != "$version" ]; then
    echo "$version is not newer than $current" >&2
    exit 1
fi

# The release's summary: the paragraph between "## [Unreleased]" and its first "###".
summary=$(awk '
    /^## \[Unreleased\]/ { on = 1; next }
    on && /^##/ { exit }
    on { print }
' CHANGELOG.md | sed -e '/./,$!d' -e '/^$/,$d')
if [ -z "$summary" ]; then
    echo "CHANGELOG.md: write the release's summary under ## [Unreleased], before its sections" >&2
    exit 1
fi

for manifest in Cargo.toml xtask/Cargo.toml; do
    sed -i "0,/^version *= *\"$current\"/s//version      = \"$version\"/" "$manifest"
done
cargo update --quiet --workspace
cargo update --quiet --workspace --manifest-path xtask/Cargo.toml

today=$(date +%Y-%m-%d)
sed -i \
    -e "s/^## \[Unreleased\]$/## [Unreleased]\n\n## [$version] - $today/" \
    -e "s|^\[Unreleased\]: .*|[Unreleased]: $repo/compare/v$version...HEAD\n[$version]: $repo/compare/v$current...v$version|" \
    CHANGELOG.md

git switch --quiet -c "release-$version"
git add CHANGELOG.md docs Cargo.toml Cargo.lock xtask/Cargo.toml xtask/Cargo.lock
git commit --quiet -m "Release $version" -m "$summary"
git push --quiet -u origin "release-$version"
gh pr create --title "Release $version" --body "$summary

Before merging, test this branch on a C6 board. Merging tags v$version and publishes the release."
