use std::env;
use std::path::PathBuf;

fn main() {
    let manifest =
        PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("Cargo sets CARGO_MANIFEST_DIR"));
    let assets = if env::var_os("CARGO_FEATURE_FIXTURE_RUNNER").is_some() {
        "dist-fixture-runner"
    } else {
        "dist"
    };
    // include_dir! does not track the embedded reader assets on stable Rust.
    println!(
        "cargo:rerun-if-changed={}",
        manifest.join("../../..").join(assets).display()
    );
}
