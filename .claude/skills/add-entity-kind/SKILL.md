---
name: add-entity-kind
description: Add a new entity kind (like Valve or Lock) through the model, the MQTT and Matter links, the `cargo iotkit new` wizard and the showcase. Use when a device needs an entity type iotkit does not have.
---

Follow an existing kind end to end: `grep -rn Valve crates xtask apps` lists every place.

1. **Model** (`crates/iotkit-core/src/model`): a variant in `Kind` (`mod.rs`), the entity type in
   `src/entities.rs` or `src/appliances.rs` (`builders!`, `motion!`), its `Spec`, `State` and
   `Command` if new, the re-export in `lib.rs`; a test in `src/tests.rs`.
2. **MQTT** (`crates/iotkit/src/mqtt/ha.rs`): the discovery component and config, state
   payload, command parsing (`match` on `Kind`); a test in its `tests` module.
3. **Matter** (`crates/iotkit/src/matter`), if a device type fits, else map the kind to `None`:
   - `src/lib.rs`: a `MatterKind`, the `Kind -> MatterKind` match, the table in the crate docs;
   - `src/map.rs`: value conversions, host-tested there;
   - `src/endpoints.rs`: device type and clusters; `src/appliances.rs` or `src/clusters.rs`:
     the cluster handler; `src/handler.rs`: its dispatch.
4. **Wizard** (`xtask/src/new`): a `Kind` in `parts.rs` (stub, wiring, `matter` label),
   `platform()` in `codegen.rs` for a new entity type; a part in `xtask/answers/c6.toml` (CI
   generates and builds it).
5. **Showcase**: one simulated entity in `apps/showcase/src/main.rs`.
6. **Docs**: the entity tables in `docs/src/entities.md` and, for Matter, `docs/src/home-assistant.md`.
7. Check: `cargo test-host`, xtask tests, clippy of `showcase` with and
   without `matter-wifi` (see `.github/workflows/ci.yml`), then `tools/size-check.sh`: unused
   kinds must cost devkit nothing.
