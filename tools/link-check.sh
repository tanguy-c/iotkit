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
showcase;-p showcase
lean;-p lean'

# what;where (syms: `nm -C`, strings: string literals; inlined generics leave no symbol);pattern
# (extended regex);absent from;present in
rules='unused entity kinds;syms;<iotkit_model::appliances::(Cover|Valve|Lock|Fan|Climate) as iotkit_model::Entity>;devkit;showcase
unused components;strings;rgb_light write failed;devkit;showcase
no improv-ble: no Bluetooth stack;syms;r_ble_ll_|trouble_host;lean;devkit
no console: no commands;strings;Type .help. and press Enter;lean;devkit
no softap: no portal;syms;portal_task;lean;devkit'

echo "$builds" | while IFS=';' read -r name args; do
    set -- $args
    cargo build --quiet --release $args
    elf=target/riscv32imac-unknown-none-elf/release/$2
    nm -C "$elf" >"$out/$name.syms"
    strings -a "$elf" >"$out/$name.strings"
done

failed=0
echo "$rules" | {
    while IFS=';' read -r what kind pattern absent present; do
        a=$(grep -cE "$pattern" "$out/$absent.$kind" || true)
        p=$(grep -cE "$pattern" "$out/$present.$kind" || true)
        if [ "$a" != 0 ]; then
            echo "FAIL $what: $a matches in $absent"; failed=1
        elif [ "$p" = 0 ]; then
            echo "FAIL $what: pattern matches nothing in $present (stale?)"; failed=1
        else
            echo "ok   $what: none in $absent ($p in $present)"
        fi
    done
    exit $failed
}
