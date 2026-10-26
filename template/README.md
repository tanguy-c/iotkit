# iotkit device template

`cargo iotkit new` (the questions, see the [guide](https://tanguy-c.github.io/iotkit/getting-started.html)) renders these files too; `cargo generate
--path <iotkit>/template --name my-device` is the way without questions. It asks for the chip, and
optionally the absolute path of an iotkit checkout: the generated project uses `iotkit` and
`iotkit-build` from there, or from the GitHub repository when it is left empty.

`src/main.rs` is a small iotkit app (the LED as a light that shows the device's status). Signed builds and OTA use the
`cargo iotkit` CLI (`cargo install --path <iotkit>/xtask`).

`partitions*.csv`, `rustfmt.toml`, `.clippy.toml`, `.vscode/extensions.json`
and `.cargo/secrets.toml.example` are copies of the repository's; CI (`check-sync.sh`) fails
when they drift. CI also generates a project, then runs keygen, a signed build and verify.
