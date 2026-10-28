---
name: add-component
description: Add a ready-made part to `iotkit::components` (an async fn that drives one entity from embedded-hal hardware) and offer it in the `cargo iotkit new` wizard. Use when a common part (sensor, actuator, input) should work without app code.
---

Follow an existing part: `relay` (output), `button` (input with timing), `poll` (sensor).

1. **The part** (`crates/iotkit/src/components/`): `pub async fn name(entity: &Kind, hw: impl Trait, ..) -> !`
   over embedded-hal(-async) traits, not a HAL type. Never a registry, a list of parts or a
   `#[used]`: the app's own task awaits it, so an unused part is not linked. Pure logic (timing,
   maths) in plain functions or a `filter`.
2. **Dependencies**: an external driver crate is an optional dependency behind its own feature,
   forwarded by `iotkit` (`smartled` is the pattern). It must be `no_std`, embedded-hal 1.0,
   and allowed by `deny.toml`.
3. **Tests** (`src/tests.rs`): mock pins and the mock clock (`MockDriver`), one timed test at a
   time (`CLOCK`). `cargo test-host` runs them.
4. **Wizard**: a `[code]` table in the `components/<id>.toml` it fits, or a new file (format:
   `components/README.md`; a sensor chip's bus and address checked against two sources): `uses`,
   `takes`, `features`, then `setup`, `param`, `arg` and `body` as Liquid templates. `cargo test
   --manifest-path xtask/Cargo.toml --target host-tuple` renders it on every chip;
   `xtask/answers/c6.toml` must cover it, and `cargo iotkit add <id>` then adds it to a project.
   A reading also gets `read` (once per wake-up on battery; `xtask/answers/c3.toml` covers it). An
   I²C chip: its driver in `components::i2c` (stateless, a mock-bus test in `tests.rs`) and an
   `[i2c]` table instead of `[code]`, its address checked against two sources.
5. **Docs**: the table in `docs/src/components.md`, and the generated AGENTS.md line in
   `codegen::agents_md`.
6. **Checks**: a rule in `tools/link-check.sh` when the part has a distinctive string or symbol
   (generics inline: use a string), then the `size-check` skill. A generated c6 project must pass
   clippy and `cargo iotkit build`.
