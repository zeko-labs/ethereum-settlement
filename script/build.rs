use mina_curves::pasta::{Pallas, Vesta};
use pickles_verifier::serialize::encode_verifier_blob;
use pickles_verifier::wire::parse_wrap_vk;
use poly_commitment::precomputed_srs::get_srs;
use sha2::{Digest, Sha256};
use sp1_build::build_program_with_args;
use std::{env, fs, path::PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../Cargo.lock");
    println!("cargo:rerun-if-changed=../lib");
    println!("cargo:rerun-if-changed=../program/settlement");
    println!("cargo:rerun-if-changed=../program/bridge");
    println!("cargo:rerun-if-changed=../program/withdraw");
    println!("cargo:rerun-if-env-changed=SETTLEMENT_VK_JSON");
    if let Some(path) = env::var_os("SETTLEMENT_VK_JSON") {
        println!("cargo:rerun-if-changed={}", path.to_string_lossy());
    }

    build_native_settlement_verifier();

    build_program_with_args("../program/settlement", Default::default());
    build_program_with_args("../program/bridge", Default::default());
    build_program_with_args("../program/withdraw", Default::default());
}

fn build_native_settlement_verifier() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let vk_path = env::var_os("SETTLEMENT_VK_JSON")
        .map(PathBuf::from)
        .unwrap_or_else(|| manifest_dir.join("../proofs/mainnet-blockchain-snark/vk.serde.json"));
    println!("cargo:rerun-if-changed={}", vk_path.display());
    let vk_json = fs::read_to_string(&vk_path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", vk_path.display()));
    let wrap_vk = parse_wrap_vk(&vk_json)
        .unwrap_or_else(|error| panic!("failed to parse {}: {error}", vk_path.display()));
    let verifier = encode_verifier_blob(&get_srs::<Vesta>(), &get_srs::<Pallas>(), 1, &wrap_vk);
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    fs::write(out_dir.join("native_settlement_verifier.bin"), verifier)
        .expect("write native settlement verifier");
    fs::write(out_dir.join("native_settlement_vk.json"), &vk_json)
        .expect("write native settlement VK JSON");
    fs::write(
        out_dir.join("native_settlement_vk_hash.bin"),
        Sha256::digest(vk_json.as_bytes()),
    )
    .expect("write native settlement VK hash");
}
