---
name: add-entity-kind
description: Add a new entity kind (like Valve or Lock) through the model, MQTT, the ESPHome API, Matter, the `cargo iotkit new` wizard and the showcase. Use when a device needs an entity type iotkit does not have.
---

Follow an existing kind end to end: `grep -rn Valve crates xtask apps` lists every place.

1. **Model** (`crates/iotkit-core/src/model`): a variant in `Kind` (`mod.rs`), the entity type in
   `entities.rs` or `appliances.rs` (`builders!`, `motion!`), its `Spec`, `State` and
   `Command` if new, the re-export in `mod.rs`; a test in `tests.rs`.
2. **MQTT** (`crates/iotkit/src/mqtt/ha.rs`): the discovery component and config, state
   payload, command parsing (`match` on `Kind`); a test in its `tests` module.
3. **ESPHome API** (`crates/iotkit/src/esphome/api.rs`): the entity's list, state and command
   messages (`match` on `Kind`, with `proto.rs` if new); a test in `api/tests.rs`.
4. **Matter** (`crates/iotkit/src/matter`), if a device type fits, else map the kind to `None`:
   - `mod.rs`: a `MatterKind`, the `Kind -> MatterKind` match, the table in the module docs;
   - `map.rs`: value conversions, host-tested there;
   - `endpoints.rs`: device type and clusters; `appliances.rs` or `clusters.rs`:
     the cluster handler; `handler.rs`: its dispatch.
5. **Wizard** (`xtask/src/new`): a `Kind` in `parts.rs` (stub, wiring, `matter` label),
   `platform()` in `codegen.rs` for a new entity type; a part in `xtask/answers/c6.toml` (CI
   generates and builds it).
6. **Showcase**: one simulated entity in `apps/showcase/src/main.rs`.
7. **Docs**: the entity tables in `docs/src/entities.md` and, for Matter, `docs/src/matter.md`.
8. Check: `cargo test-host`, xtask tests, clippy of `showcase` with and
   without `matter-wifi` and of `devkit` with `esphome-api` (see `.github/workflows/ci.yml`), then `tools/size-check.sh`: unused
   kinds must cost devkit nothing.
