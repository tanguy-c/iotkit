---
name: add-chip
description: Add support for another ESP32 chip (feature fan-out, build-time guards, CLI, template, wizard boards, CI). Use when porting iotkit to a new chip.
---

The C6 path must not change: everything below is additive. `git show bbc751c` (C3, C5, H2, S3)
is the worked example; follow `esp32c5` with `grep -rn esp32c5`.

1. **Features**: an `esp32xx` feature next to `esp32c6` in each chip-aware crate's `Cargo.toml`
   and in `apps/*/Cargo.toml`, forwarding to esp-hal, esp-rtos, esp-alloc and the iotkit crates.
   The workspace dependencies never pick a chip. No atomics: enable `portable-atomic` from that
   chip's features only, as `esp32c3` does.
2. **Guards**: add the chip to the "one chip" `compile_error!`s in
   `crates/iotkit-app/src/lib.rs`; refuse what cannot work (Matter, deep sleep) there and in
   `crates/iotkit-link-matter/src/lib.rs`.
3. **Chip code**: `cfg(feature = ...)` arms for the image chip id (`crates/iotkit-image`,
   `app::ESP32..`), `CHIP_ID` in `crates/iotkit-ota/src/esp.rs`, reset reasons in
   `crates/iotkit-diag/src/esp.rs`, pins in `apps/devkit` / `apps/thread-light`.
4. **CLI** (`xtask/src/main.rs`): a `Chip` entry (target, id); a new target also needs
   `.cargo/config.toml` (and `rust-toolchain.toml` for a stable target).
5. **Template**: the `chip` choices in `template/cargo-generate.toml` and what they drive
   (`Cargo.toml.liquid`, `.cargo/config.toml`, `rust-toolchain.toml`, `src/main.rs`);
   `template/check-sync.sh`.
6. **Wizard**: boards and pins in `xtask/src/new/boards.rs`; the chip loop in `render.rs` tests.
7. **CI**: a row in the `chips-riscv` matrix (or a job like `chip-s3`) and in the template job of
   `.github/workflows/ci.yml`.
8. **Docs**: the chip table in `README.md` (what works, what is build-tested only).
9. Prove C6 unchanged: `tools/size-check.sh` reports zero `.text`/`.data`/`.bss` delta on the
   three C6 builds; hardware-test the C6 if anything shared moved.
