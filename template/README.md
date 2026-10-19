# iotkit device template

`cargo generate --git <repo> template --name my-device`, or from a checkout:
`cargo generate --path v2/template --name my-device`.

`build.rs`, `partitions.csv` and the toolchain files are copies of the repository's
`apps/devkit` and root files; CI fails when they drift. `src/main.rs` is a minimal app
(LED + heartbeat) until the iotkit crates can be used from a git or crates.io dependency;
`apps/devkit` shows them in use.
The generated project does not have `cargo xtask` yet (signed builds, OTA): that
comes with `iotkit-cli`.
