#!/bin/sh
# Fails when the template's copies drift from the repository's files.
set -eu
cd "$(dirname "$0")/.."
status=0
for pair in \
    .cargo/secrets.toml.example:template/.cargo/secrets.toml.example \
    partitions.csv:template/partitions.csv \
    partitions-4mb.csv:template/partitions-4mb.csv \
    rust-toolchain.toml:template/rust-toolchain.toml \
    rustfmt.toml:template/rustfmt.toml \
    .clippy.toml:template/.clippy.toml \
    .vscode/extensions.json:template/.vscode/extensions.json; do
    src=${pair%%:*} dst=${pair#*:}
    if ! cmp -s "$src" "$dst"; then
        echo "template out of sync: $dst differs from $src (copy it over)"
        status=1
    fi
done
exit $status
