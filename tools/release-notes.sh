#!/bin/sh
# Prints the GitHub release notes of a version: its CHANGELOG.md section, then how to install it.
# Used by .github/workflows/release.yml, and by hand to fix a past release:
#   tools/release-notes.sh 0.5.1 | gh release edit v0.5.1 --notes-file -
set -eu
cd "$(dirname "$0")/.."
version=${1:?usage: tools/release-notes.sh <version>}

# GitHub shows a release's line breaks: join each paragraph or list item into one line.
unwrap() {
    awk '
        /^$/ || /^#/ || /^ *- / { if (line != "") print line; line = $0; if (/^$/ || /^#/) { print; line = "" } next }
        { sub(/^ +/, ""); line = (line == "") ? $0 : line " " $0 }
        END { if (line != "") print line }
    '
}

# Relative links resolve against the release page: point them to the guide or to the tag's files.
absolute_links() {
    sed -E \
        -e 's#\]\(docs/src/([a-z-]+)\.md#](https://tanguy-c.github.io/iotkit/\1.html#g' \
        -e "s#\\]\\(([a-zA-Z][^:)]*)\\)#](https://github.com/tanguy-c/iotkit/tree/v$version/\\1)#g"
}

# The section's body: from its heading to the next version heading or the link references.
notes=$(awk -v v="$version" '
    index($0, "## [" v "]") == 1 { on = 1; next }
    on && (/^## \[/ || /^\[[^]]+\]: /) { exit }
    on { print }
' CHANGELOG.md | sed -e '/./,$!d' | unwrap | absolute_links)

if [ -z "$notes" ]; then
    echo "CHANGELOG.md has no section for $version" >&2
    exit 1
fi

cat <<EOF
$notes

## Install

iotkit is not on crates.io yet: install the CLI from git, then follow the [guide](https://tanguy-c.github.io/iotkit/getting-started.html).

\`\`\`bash
cargo install --locked --git https://github.com/tanguy-c/iotkit --tag v$version iotkit-cli
\`\`\`

To upgrade a project, follow [Upgrading](https://tanguy-c.github.io/iotkit/upgrading.html). Full changes: [CHANGELOG.md](https://github.com/tanguy-c/iotkit/blob/v$version/CHANGELOG.md).
EOF
