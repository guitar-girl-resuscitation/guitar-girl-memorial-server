use std::{env, fs, path::PathBuf};

use sha2::{Digest, Sha256};

fn main() {
    let path = PathBuf::from("../../policy/memorial-policy.json");
    println!("cargo:rerun-if-changed={}", path.display());
    let bytes = fs::read(path).expect("read memorial policy manifest");
    let digest = Sha256::digest(bytes);
    let encoded = digest
        .iter()
        .map(|byte| format!("{byte:02X}"))
        .collect::<String>();
    println!("cargo:rustc-env=GGFM_POLICY_SHA256={encoded}");
    let output =
        PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR")).join("policy_fingerprint.rs");
    fs::write(
        output,
        format!(
            "// @generated from policy/memorial-policy.json\n\
             pub const POLICY_SHA256: &str = \"{encoded}\";\n"
        ),
    )
    .expect("write policy fingerprint");
}
