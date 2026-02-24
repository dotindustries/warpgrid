fn main() {
    // Only link libpq when targeting wasm32 — on native targets this crate
    // compiles as a stub (all methods return errors or are cfg'd out).
    let target_arch = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    if target_arch != "wasm32" {
        return;
    }

    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let project_root = std::path::Path::new(&manifest_dir)
        .parent()
        .unwrap()
        .parent()
        .unwrap();

    let libpq_dir = project_root.join("build").join("libpq-wasm").join("lib");
    let sysroot_lib = project_root
        .join("build")
        .join("sysroot-patched")
        .join("lib")
        .join("wasm32-wasip2");

    println!("cargo:rustc-link-search=native={}", libpq_dir.display());
    println!("cargo:rustc-link-search=native={}", sysroot_lib.display());
    println!("cargo:rustc-link-lib=static=pq");

    // Re-run if the library changes
    println!("cargo:rerun-if-changed={}", libpq_dir.join("libpq.a").display());
}
