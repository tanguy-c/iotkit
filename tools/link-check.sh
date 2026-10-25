#!/bin/sh
# Fails when firmware links code the app does not use. Each rule names a symbol pattern, a build
# that must not contain it, and a control build that must (so a stale pattern cannot pass).
# Usage: tools/link-check.sh
set -eu
cd "$(dirname "$0")/.."
out=target/link-check
mkdir -p "$out"
[ -f keys/ota.pub ] || cargo xtask keygen
# The `tls` build needs a CA to build in: any will do here.
if [ ! -f "$out/ca.der" ]; then
    openssl ecparam -name prime256v1 -genkey -noout -out "$out/ca.key"
    openssl req -x509 -new -key "$out/ca.key" -subj /CN=link-check -outform der -out "$out/ca.der"
fi
export IOTKIT_TLS_CA="$PWD/$out/ca.der"

# name;cargo args (-p app first)
builds='devkit;-p devkit
showcase;-p showcase
lean;-p lean
devkit-tls;-p devkit --features tls
devkit-esphome;-p devkit --features esphome-api
devkit-matter;-p devkit --features matter-wifi
thread-light;-p thread-light
sleepy;-p sleepy
sleepy-esphome;-p sleepy --features esphome-api
thread-icd;-p thread-light --features iotkit/matter-icd
ble-beacon;-p ble-beacon'

# what;where (syms: `nm -C`, strings: string literals; inlined generics leave no symbol);pattern
# (extended regex);absent from;present in
rules='unused entity kinds;syms;<iotkit_core::model::appliances::(Cover|Valve|Lock|Fan|Climate) as iotkit_core::model::Entity>;devkit;showcase
unused components;strings;rgb_light write failed;devkit;showcase
unused status LEDs;syms;iotkit::components::indicator::;devkit;sleepy
no scheduled updates: no clock;syms;iotkit::clock::;sleepy;sleepy-esphome
no improv-ble: no Bluetooth stack;syms;r_ble_ll_|trouble_host;lean;devkit
no console: no commands;strings;Type .help. and press Enter;lean;devkit
no softap: no portal;syms;portal_task;lean;devkit
no tls: no TLS code;strings;TLS with;devkit;devkit-tls
no esphome-api: no API server;syms;api_task|esphome;devkit;devkit-esphome
no matter-icd: no sleepy Thread driver;syms;iotkit::matter::icd::;thread-light;thread-icd
no matter-ota: no OTA requestor;syms;iotkit::matter::ota::|ota_req::OtaRequestorHandler;devkit-matter;thread-light
no way to update: no OTA engine;syms;iotkit::ota::esp::;ble-beacon;devkit
no wifi: no IP stack;syms;embassy_net::|smoltcp::;ble-beacon;devkit'

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
