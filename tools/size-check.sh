#!/bin/sh
# Compares the C6 firmware sizes of the working tree against a base revision (default:
# origin/main, else HEAD~). Fails when code or RAM grows; ALLOW_GROWTH=1 only reports.
# Usage: tools/size-check.sh [base-rev]
set -eu
cd "$(dirname "$0")/.."
root=$PWD
base=$(git rev-parse --verify "${1:-$(git rev-parse -q --verify origin/main || echo HEAD~)}^{commit}")
out=$root/target/size-check
wt=$out/base
mkdir -p "$out"

# Base worktree under target/ (not /tmp), with this tree's signing key and secrets.
git worktree remove -f "$wt" 2>/dev/null || rm -rf "$wt"
git worktree prune
git worktree add -q --detach "$wt" "$base"
[ -f keys/ota.pub ] || cargo xtask keygen
mkdir -p "$wt/keys" && cp keys/ota.pub keys/ota.key "$wt/keys/"
[ ! -f .cargo/secrets.toml ] || cp .cargo/secrets.toml "$wt/.cargo/"

# A target dir per tree: cargo names workspace crates by their relative path, so a shared one
# reuses the other tree's `iotkit` when its files look older. The base's stays for later runs.
build() { # tree target-dir output cargo-args (-p app first)
    tree=$1 target=$2 elf=$3; shift 3
    (cd "$tree" && CARGO_TARGET_DIR="$target" cargo build --quiet --release "$@")
    cp "$target/riscv32imac-unknown-none-elf/release/$2" "$elf"
}
# Symbol sizes by name (same-named locals summed; no `$d`/`.L` assembler labels).
syms() { nm -S -t d --size-sort -C "$1" | awk '$4 !~ /^[$.]/ { n = $4; for (i = 5; i <= NF; i++) n = n " " $i; s[n] += $2 } END { for (n in s) print s[n] "\t" n }'; }

grew=0
for cfg in "devkit -p devkit" "devkit-matter -p devkit --features matter-wifi" "thread-light -p thread-light"; do
    set -- $cfg; name=$1; shift
    b=$out/$name.base h=$out/$name.head
    build "$wt" "$out/target" "$b" "$@"
    build "$root" "$root/target" "$h" "$@"
    echo "== $name: $(git rev-parse --short "$base") -> working tree (section, size, delta)"
    size -A "$b" >"$b.size"; size -A "$h" >"$h.size"
    awk 'NR == FNR { s[$1] = $2; next } /^\./ && !/^\.(debug|comment|riscv)/ { d = $2 - s[$1]
            if (d || $1 ~ /^\.(text|data|bss|rodata)$/) printf "  %-26s %9d %+8d\n", $1, $2, d
            if (d > 0 && $1 ~ /^\.(text|bss)$|^\.(rwtext|data)/) g = 1 } END { exit g }' "$b.size" "$h.size" || grew=1
    syms "$b" >"$b.syms"; syms "$h" >"$h.syms"
    awk -F '\t' 'NR == FNR { s[$2] = $1; next } { d = $1 - s[$2]; delete s[$2]; if (d) print (d < 0 ? -d : d) "\t" d "\t" $2 }
            END { for (n in s) print s[n] "\t" (-s[n]) "\t" n }' "$b.syms" "$h.syms" |
        sort -t "$(printf '\t')" -k1,1nr | head -10 | awk -F '\t' '$1 { printf "  %+8d  %s\n", $2, $3 }'
done

if [ "$grew" = 1 ]; then
    echo "C6 code or RAM grew (see above); ALLOW_GROWTH=1 (CI: the size-ok PR label) accepts it."
    [ "${ALLOW_GROWTH:-0}" = 1 ] || exit 1
fi
