# Review of v2 before new features (2026-10-19)

Scope: every crate, both apps (thread-light only skimmed), xtask, template, CI. Pick findings by id; each batch becomes one commit.

Overall: the protocol code (manifest, MQTT codec, DNS, DHCP, mDNS, Improv, HTTP, portal form) is bounds-checked and has no `unwrap` outside tests. The two `unsafe` blocks (RTC crash record) are justified. Library code doesn't panic on network input. The problems are in **ordering and state across tasks**, plus the **DX gap** listed at the end.

## Already fixed (Step 0)

| Id | What | Fix |
|---|---|---|
| F1 | CI `cargo clippy --workspace` fails: devkit (Wi-Fi) and thread-light (802.15.4) unify into one esp-radio build, which panics "802.15.4 and Wi-Fi won't work together" | `ci.yml` checks `-p devkit`; thread-light keeps its own line |
| F2 | `cargo deny check` failed on licenses (MPL-2.0 in rs-matter's build-time codegen, 0BSD, tinyrlibc with no license field), the rs-matter-embassy git source, and 2 unmaintained transitive crates | `deny.toml`: allow-list, `clarify` for tinyrlibc, `allow-git`, `ignore` with a reason for each |

Every other CI job passes locally: fmt, host tests (90), all clippy lines, signed build + verify, template sync + generate + build.

## Bugs

| Id | Where | Failure | Fix |
|---|---|---|---|
| B1 | [iotkit-ota/src/esp.rs:285-297](crates/iotkit-ota/src/esp.rs#L285-L297) | otadata is switched to the new slot **before** `Pending` is saved. A power cut between the two boots the new image with no trial counter, so a broken image is never rolled back (only the bootloader could, and ours has no rollback) | Save `Pending` first, then switch. A cut in between boots the old slot, and `on_boot` already handles that as `Abandoned` |
| B2 | [iotkit-provision/src/esp.rs:608](crates/iotkit-provision/src/esp.rs#L608), console `factory-reset` | Factory reset erases `config` only. Matter pairings live in `nvs` and survive. The setup code (in `config`) is regenerated, so the device stays paired to its old controllers under a new code | One `iotkit_config::esp::factory_reset()` that erases `config` **and** `nvs`, used by the button and the console |
| B3 | [iotkit-model/src/entities.rs:279-284](crates/iotkit-model/src/entities.rs#L279-L284), [link-matter clusters.rs](crates/iotkit-link-matter/src/clusters.rs) | `Light::command` merges a partial command onto the last **reported** state. Two commands before the app calls `set` (Matter sends OnOff, Level and Color as separate invokes; HA can send brightness then color) means the second one undoes the first. Same for Matter `toggle` on a `Switch` | Keep the pending target in the entity: merge onto pending-or-current, clear it when `wait_command` hands it over. Add a host test with two commands in a row |
| B4 | [iotkit-link-mqtt/src/esp.rs:86-95](crates/iotkit-link-mqtt/src/esp.rs#L86-L95) | `session` returns `Ok` only on shutdown, so `backoff.succeeded()` never runs. After 5 broker drops in the device's life, every reconnect waits 30 s | Reset the backoff once CONNACK is accepted (e.g. when `CONNECTED` goes true) |
| B6 | [iotkit-net/src/esp.rs](crates/iotkit-net/src/esp.rs) (found on hardware) | With `matter-wifi`, the static IPv6 link-local address makes `stack.is_config_up()` true at boot, with no Wi-Fi. So `wait_config_up()` returns at once: MQTT connects without a network, the NETWORK health check passes (an update could be confirmed with broken Wi-Fi), `join()` reports success before DHCP, and Improv says "provisioned" | `Net::is_online()` / `wait_online()` (link up and an IPv4 address), used everywhere |
| B5 | [iotkit-link-mqtt/src/esp.rs:158](crates/iotkit-link-mqtt/src/esp.rs#L158) | The ping timer is recreated every loop turn. With entity changes more often than every 30 s (normal for sensors), PINGREQ is never sent and the `last_rx` dead-link check never runs. Only the TCP timeout notices a half-open link | A fixed deadline: `Timer::at(next_ping)`, moved forward only when a ping is sent |

## Risks

| Id | Where | Concern | Suggestion |
|---|---|---|---|
| R1 | [iotkit-config/src/esp.rs:33-38](crates/iotkit-config/src/esp.rs#L33-L38) | Every flash operation runs inside `critical_section` (interrupts off): a sector erase takes about 50 ms; `factory_reset` (64 KiB) and the Matter `nvs` reset (16 KiB) take hundreds of ms. Radio interrupts and BLE timing stall | Erase sector by sector with a yield in between; check whether esp-storage already masks what it must, so a plain `RefCell` + async mutex is enough |
| R2 | [iotkit-config/src/lib.rs:140-156](crates/iotkit-config/src/lib.rs#L140-L156), [link-matter wifi.rs:145-150](crates/iotkit-link-matter/src/wifi.rs#L145-L150) | Any `Corrupted` result wipes the whole store: Wi-Fi, broker, and **OTA `Pending`**, so an update on trial stops being rolled back. Matter erases `nvs` on any startup error | Erase only data that was never a store (erased flash or a magic marker missing); otherwise log, record, and keep the data |
| R3 | [apps/devkit/src/main.rs:108-139](apps/devkit/src/main.rs#L108-L139) | A crash before `ota::boot` (e.g. `config::open` panics) is never counted, so a bad update boot-loops forever | Count trial boots in the RTC record too (it survives resets), or also build the bootloader with rollback as a second safety net |
| R4 | [iotkit-ota/src/esp.rs:220-227](crates/iotkit-ota/src/esp.rs#L220-L227) | `BUSY` is not cleared if the `install` future is dropped mid-way: OTA stays "busy" until restart | Clear it with a drop guard |
| R5 | OTA endpoint, setup window | Anyone on the LAN can trigger `/ota` (it only installs newer **signed** images, but it costs a download and a restart). During a setup window, anyone in range can set the Wi-Fi network (open access point, Improv without authorization) | Write it down in the README. An option to make Improv require a BOOT press (`AuthorizationRequired`) |
| R6 | [iotkit-net/src/esp.rs:160-164](crates/iotkit-net/src/esp.rs#L160-L164) | Concurrent `join()` calls (portal + BLE + serial) share one `JOINED` signal and can read each other's answers | Serialize `join` with an async mutex |
| R7 | [iotkit-net/src/lib.rs:36-41](crates/iotkit-net/src/lib.rs#L36-L41), [esp.rs:296](crates/iotkit-net/src/esp.rs#L296) | A 1–7 character password is accepted, then `unwrap_or_default()` silently makes it empty, and the join fails with no clear reason | Validate 8..=63 characters (or 64 hex digits) in `WifiCredentials::new`, and say why in the portal, Improv and the console |
| R8 | [iotkit-link-mqtt/src/ha.rs:31-35](crates/iotkit-link-mqtt/src/ha.rs#L31-L35), [iotkit-model/src/lib.rs:264](crates/iotkit-model/src/lib.rs#L264) | Topics are `String<128>` and write errors are ignored: a long entity id silently truncates its topic. Ids aren't checked against `[a-z0-9_]` | A `const` check in `Meta::new` (fails the build) |
| R9 | [iotkit-link-mqtt/src/ha.rs:139](crates/iotkit-link-mqtt/src/ha.rs#L139) | A sensor going back to unknown (NaN) publishes nothing, and Home Assistant keeps showing the last retained value | Publish an "unknown" state (decide which payload HA expects) |
| R10 | [iotkit-net/src/http.rs:175](crates/iotkit-net/src/http.rs#L175) | `Url::parse` rejects spaces but not CR/LF; the manifest's `url` isn't signed, so a request header could be injected into the device's own GET | Reject control characters |

## Cleanups

| Id | What |
|---|---|
| C1 | The `install` future lives in both `serve_task` and the console (about 20 KB RAM each: the 16 KiB TCP window in `get_from`, the 4 KiB sector, the 1 KiB manifest). One OTA task fed by a channel |
| C2 | `Store` uses `NoCache`: every load or save scans the whole 64 KiB partition. Use a sequential-storage cache |
| C3 | `build.rs` is copied 3 times (devkit, thread-light, template) |
| C4 | `BootStats::crashes_in_a_row` is computed but nothing uses it; a "safe mode" after N crashes fits in the builder |
| C5 | `cargo build --workspace` can't work (same cause as F1); say so in the root `Cargo.toml` comment |
| C6 | thread-light wasn't reviewed closely (untested end to end anyway) |

## Developer experience

The path a new developer takes, and where it hurts:

| Id | Friction |
|---|---|
| D1 | **The template gives a LED blinker with no iotkit.** A generated project has none of Wi-Fi setup, OTA, diagnostics, MQTT or Matter |
| D2 | **App wiring is about 180 lines** ([devkit main.rs:104-226](apps/devkit/src/main.rs#L104-L226)): 13 spawns, the boot order kept in comments only (crash record first, `record_boot` before `ota::boot`), 4 health checks with "skip when not configured" logic spread around, 5 provisioning tasks to remember, Matter version packed by hand |
| D3 | **The console is app code but generic** ([apps/devkit/src/console.rs](apps/devkit/src/console.rs)): info, net, provision, ota, mqtt, matter and factory-reset belong to the kit, and every app would copy it |
| D4 | **Persisting entity state is hand-written** (`SavedLight` + a 3 s debounce in `led_task`) |
| D5 | **The BOOT short press** (`Gesture::Press`) is "left to the app", but there's no way to receive it |
| D6 | **README is stale:** "design target, adapt names" disclaimer, a placeholder `cargo generate --git https://github.com/you/...` URL, a 4 MB partition example (the default is 8 MB), `ota publish` documented but not implemented, C3/Xtensa prerequisites for a C6-only kit, `esp-generate` offered as an alternative |
| D7 | **Errors without guidance:** a missing `keys/ota.pub` gives an `include_str!` error; `ota push --host x.local` needs mDNS in the host's resolver (nss-mdns); a monitor holding the port hangs espflash. A `cargo xtask doctor` could check keys, secrets, port, toolchain and mDNS |
| D8 | VSCode **OTA push** depends on **Build signed**, then builds again (no `--no-build`) |

## Proposed batches

1. **Correctness:** B1–B5, R4, R6, R7. Small, local changes, with host tests where the logic is host-side (B3, B4/B5 timing logic, R7).
2. **Robustness:** R1, R2, R3, R8–R10, C1, C2. Needs hardware checks (power cut, erase timing).
3. **DX (Step 3 of the plan):** the `iotkit-app` builder (D2, D4, D5, C4), the console moved into it (D3), the template as an iotkit app (D1), README (D6), `build.rs` dedupe (C3), D8; `xtask doctor` (D7) if wanted.
4. **Docs only:** R5, C5.
