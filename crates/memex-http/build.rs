//! Rebuilds this crate when the console changes, since Cargo can't see the folder `rust-embed` reads. Watching the parent while it's missing avoids a rebuild on every run.
#![allow(clippy::disallowed_methods, clippy::disallowed_macros)]
fn main() {
    let dist = std::path::Path::new("../../web/apps/console/dist");
    println!(
        "cargo:rerun-if-changed={}",
        if dist.exists() { dist } else { dist.parent().unwrap_or(dist) }.display()
    );
    println!("cargo:rerun-if-changed=build.rs");
}
