fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("android") {
        // Cargo names this crate from its package name, while the patcher ships
        // the stable public ABI as libggfm_server.so.  Android's loader follows
        // DT_SONAME/DT_NEEDED, so make that runtime contract explicit instead
        // of leaking a host build path into the bootstrap library.
        println!("cargo:rustc-link-arg=-Wl,-soname,libggfm_server.so");
    }
}
