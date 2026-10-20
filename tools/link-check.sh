#!/bin/sh
# Fails when firmware links code the app does not use. Each rule names a symbol pattern, a build
# that must not contain it, and a control build that must (so a stale pattern cannot pass).
# Usage: tools/link-check.sh
set -eu
cd "$(dirname "$0")/.."
out=target/link-check
mkdir -p "$out"
[ -f keys/ota.pub ] || cargo xtask keygen

# name;cargo args (-p app first)
builds='devkit;-p devkit
showcase;-p showcase'

# what;pattern (`nm -C`, extended regex);absent from;present in
rules='unused entity kinds;<iotkit_model::appliances::(Cover|Valve|Lock|Fan|Climate) as iotkit_model::Entity>;devkit;showcase'

echo "$builds" | while IFS=';' read -r name args; do
    set -- $args
    cargo build --quiet --release $args
    nm -C "target/riscv32imac-unknown-none-elf/release/$2" >"$out/$name.syms"
done

failed=0
echo "$rules" | {
    while IFS=';' read -r what pattern absent present; do
        a=$(grep -cE "$pattern" "$out/$absent.syms" || true)
        p=$(grep -cE "$pattern" "$out/$present.syms" || true)
        if [ "$a" != 0 ]; then
            echo "FAIL $what: $a symbols in $absent"; failed=1
        elif [ "$p" = 0 ]; then
            echo "FAIL $what: pattern matches nothing in $present (stale?)"; failed=1
        else
            echo "ok   $what: none in $absent ($p in $present)"
        fi
    done
    exit $failed
}
