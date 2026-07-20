use std::{env, fs, path::PathBuf};

use mina_curves::pasta::{Pallas, Vesta};
use pickles_verifier::serialize::encode_verifier_blob;
use pickles_verifier::wire::parse_wrap_vk;
use poly_commitment::precomputed_srs::get_srs;
use sha2::{Digest, Sha256};

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let default_vk = manifest_dir.join("../../proofs/mainnet-blockchain-snark/vk.serde.json");
    let vk_path = env::var_os("SETTLEMENT_VK_JSON")
        .map(PathBuf::from)
        .unwrap_or(default_vk);

    println!("cargo::rerun-if-changed={}", vk_path.display());
    println!("cargo::rerun-if-env-changed=SETTLEMENT_VK_JSON");

    let vk_json = fs::read_to_string(&vk_path)
        .unwrap_or_else(|e| panic!("failed to read {}: {e}", vk_path.display()));
    let wrap_vk =
        parse_wrap_vk(&vk_json).unwrap_or_else(|e| panic!("failed to parse wrap VK: {e}"));

    let vesta_srs = get_srs::<Vesta>();
    let wrap_srs = get_srs::<Pallas>();
    let blob = encode_verifier_blob(
        &vesta_srs, &wrap_srs, /* step_num_chunks */ 1, &wrap_vk,
    );

    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    fs::write(out_dir.join("verifier.bin"), &blob).expect("failed to write verifier.bin");

    let vk_hash = Sha256::digest(vk_json.as_bytes());
    fs::write(out_dir.join("vk_hash.bin"), vk_hash).expect("failed to write vk_hash.bin");
}
