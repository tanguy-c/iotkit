#!/bin/sh
# Prints the GitHub release notes of a version: its CHANGELOG.md section, then how to install it.
# Used by .github/workflows/release.yml, and by hand to fix a past release:
#   tools/release-notes.sh 0.5.1 | gh release edit v0.5.1 --notes-file -
set -eu
cd "$(dirname "$0")/.."
version=${1:?usage: tools/release-notes.sh <version>}

# The section's body: from its heading to the next version heading or the link references.
notes=$(awk -v v="$version" '
    index($0, "## [" v "]") == 1 { on = 1; next }
    on && (/^## \[/ || /^\[[^]]+\]: /) { exit }
    on { print }
' CHANGELOG.md | sed -e '/./,$!d')

if [ -z "$notes" ]; then
    echo "CHANGELOG.md has no section for $version" >&2
    exit 1
fi

cat <<EOF
$notes

## Install

iotkit is not on crates.io yet: install the CLI from git, then follow the
[guide](https://tanguy-c.github.io/iotkit/getting-started.html).

\`\`\`bash
cargo install --locked --git https://github.com/tanguy-c/iotkit --tag v$version iotkit-cli
\`\`\`

To upgrade a project, follow [Upgrading](https://tanguy-c.github.io/iotkit/upgrading.html).
Full changes: [CHANGELOG.md](https://github.com/tanguy-c/iotkit/blob/v$version/CHANGELOG.md).
EOF
