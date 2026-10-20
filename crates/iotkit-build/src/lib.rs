//! `build.rs` helpers for iotkit firmware (a build dependency). In `build.rs`'s `main`:
//!
//! ```no_run
//! iotkit_build::link();
//! iotkit_build::require_key("keys/ota.pub");
//! ```

#![warn(missing_docs)]

use std::path::Path;

/// Links with esp-hal's `linkall.x`, and turns the usual undefined-symbol errors into hints.
pub fn link() {
    linker_be_nice();
    // make sure linkall.x is the last linker script (otherwise might cause problems with flip-link)
    println!("cargo:rustc-link-arg=-Tlinkall.x");
}

/// Fails the build with a hint when the OTA public key (`path`, relative to the package) is
/// missing: the firmware embeds it to check updates.
pub fn require_key(path: &str) {
    let dir = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default();
    let key = Path::new(&dir).join(path);
    println!("cargo:rerun-if-changed={}", key.display());
    if !key.is_file() {
        panic!(
            "\n\n💡 No OTA key at {}: create the signing key pair with `cargo iotkit keygen` \
             (`cargo xtask keygen` in the iotkit repository).\n",
            key.display()
        );
    }
}

/// From esp-generate's template: when the linker fails, it runs this build script again with
/// the error, which prints a hint for the common cases.
fn linker_be_nice() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() > 1 {
        let kind = &args[1];
        let what = &args[2];

        match kind.as_str() {
            "undefined-symbol" => match what.as_str() {
                what if what.starts_with("_defmt_") => {
                    eprintln!();
                    eprintln!(
                        "💡 `defmt` not found - make sure `defmt.x` is added as a linker script and you have included `use defmt_rtt as _;`"
                    );
                    eprintln!();
                }
                "_stack_start" => {
                    eprintln!();
                    eprintln!("💡 Is the linker script `linkall.x` missing?");
                    eprintln!();
                }
                what if what.starts_with("esp_rtos_") => {
                    eprintln!();
                    eprintln!(
                        "💡 `esp-radio` has no scheduler enabled. Make sure you have initialized `esp-rtos` or provided an external scheduler."
                    );
                    eprintln!();
                }
                "embedded_test_linker_file_not_added_to_rustflags" => {
                    eprintln!();
                    eprintln!(
                        "💡 `embedded-test` not found - make sure `embedded-test.x` is added as a linker script for tests"
                    );
                    eprintln!();
                }
                "free"
                | "malloc"
                | "calloc"
                | "get_free_internal_heap_size"
                | "malloc_internal"
                | "realloc_internal"
                | "calloc_internal"
                | "free_internal" => {
                    eprintln!();
                    eprintln!(
                        "💡 Did you forget the `esp-alloc` dependency or didn't enable the `compat` feature on it?"
                    );
                    eprintln!();
                }
                _ => (),
            },
            // we don't have anything helpful for "missing-lib" yet
            _ => {
                std::process::exit(1);
            }
        }

        std::process::exit(0);
    }

    // An lld option: the Xtensa chips (ESP32-S3) link with GCC instead.
    if std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() != Ok("xtensa") {
        println!("cargo:rustc-link-arg=--error-handling-script={}", std::env::current_exe().unwrap().display());
    }
}
