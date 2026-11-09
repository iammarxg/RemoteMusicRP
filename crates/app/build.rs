// SPDX-License-Identifier: AGPL-3.0-only
// Expose the target triple so the updater can pick the right release asset.
fn main() {
    let target = std::env::var("TARGET").unwrap_or_else(|_| "unknown".into());
    println!("cargo:rustc-env=SA_TARGET={target}");
    println!("cargo:rerun-if-env-changed=REMOTEMUSICRP_UPDATE_PUBKEY");
    println!("cargo:rerun-if-env-changed=REMOTEMUSICRP_INSTALL_KIND");
}
