# iotkit device template

`cargo generate --path <iotkit>/template --name my-device`. It asks for the chip, and the
absolute path of the iotkit checkout: the generated project uses `iotkit-app` and
`iotkit-build` from there (a git dependency once the repository is published).

`src/main.rs` is a small iotkit app (the LED as a light). Signed builds and OTA use the
`cargo iotkit` CLI (`cargo install --path <iotkit>/xtask`).

`partitions*.csv`, the toolchain and format files, `.clippy.toml`, `.vscode/extensions.json`
and `.cargo/secrets.toml.example` are copies of the repository's; CI (`check-sync.sh`) fails
when they drift. CI also generates a project, then runs keygen, a signed build and verify.
