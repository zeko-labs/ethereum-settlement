//! Zeko SP1 EVM-compatible Pickles proof verifier.

use clap::{Parser, ValueEnum};
use pickles_verifier::types::VerifiableProof;
use pickles_verifier::wire::{parse_app_statement, parse_wrap_proof, parse_wrap_vk, OcamlProof};
use serde::{Deserialize, Serialize};
use sp1_sdk::{
    include_elf, network::NetworkMode, utils, Elf, HashableKey, ProveRequest, Prover, ProverClient,
    ProvingKey, SP1ProofWithPublicValues, SP1Stdin, SP1VerifyingKey,
};
use std::path::{Path, PathBuf};
use std::time::Instant;
use zeko_sp1_lib::ZkappPublicValues;

pub const ZKAPP_ELF: Elf = include_elf!("settlement-program");

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct EVMArgs {
    #[arg(long, default_value = "proofs/mainnet-blockchain-snark")]
    fixture_dir: String,

    #[arg(long, value_enum, default_value = "groth16")]
    system: ProofSystem,
}

#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, ValueEnum, Debug)]
enum ProofSystem {
    Plonk,
    Groth16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SP1ProofFixture {
    system: String,
    fixture_dir: String,
    proof_valid: bool,
    vkey: String,
    public_values: String,
    proof: String,
}

#[tokio::main]
async fn main() {
    utils::setup_logger();
    dotenv::dotenv().ok();

    let args = EVMArgs::parse();

    let verifiable = load_verifiable(Path::new(&args.fixture_dir));
    let mut stdin = SP1Stdin::new();
    stdin.write(&verifiable);

    let client = ProverClient::builder()
        .network_for(NetworkMode::Mainnet)
        .build()
        .await;

    let t_setup = Instant::now();
    let pk = client.setup(ZKAPP_ELF).await.expect("failed to setup ELF");
    println!("Setup time: {:?}", t_setup.elapsed());
    println!("Proof System: {:?}", args.system);

    println!("Generating EVM-compatible proof on Succinct Prover Network...");
    let t_prove = Instant::now();

    let proof = match args.system {
        ProofSystem::Plonk => client.prove(&pk, stdin).plonk().await,
        ProofSystem::Groth16 => client.prove(&pk, stdin).groth16().await,
    }
    .expect("failed to generate proof");

    println!("Proving time: {:?}", t_prove.elapsed());

    client
        .verify(&proof, pk.verifying_key(), None)
        .expect("verify failed");

    create_proof_fixture(&proof, pk.verifying_key(), &args.fixture_dir, args.system);
}

fn load_verifiable(fixture_dir: &Path) -> VerifiableProof {
    let read = |name: &str| {
        let path = fixture_dir.join(name);
        std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()))
    };

    let vk_json = read("vk.serde.json");
    let proof_json = read("proof.serde.json");
    let skeleton_json = read("public_input_skeleton.json");
    let app_stmt_json = read("app_statement.json");

    let wrap_vk = parse_wrap_vk(&vk_json).expect("parse vk.serde.json");
    let wrap_proof = parse_wrap_proof(&proof_json).expect("parse proof.serde.json");
    let ocaml = OcamlProof::parse(&skeleton_json).expect("parse public_input_skeleton.json");
    let app_stmt = parse_app_statement(&app_stmt_json).expect("parse app_statement.json");

    ocaml
        .into_verifiable(wrap_proof, &wrap_vk, &[app_stmt])
        .expect("OcamlProof::into_verifiable")
}

fn create_proof_fixture(
    proof: &SP1ProofWithPublicValues,
    vk: &SP1VerifyingKey,
    fixture_dir: &str,
    system: ProofSystem,
) {
    let public_values: ZkappPublicValues =
        bincode::deserialize(proof.public_values.as_slice()).expect("decode public values");

    println!("  proof_valid: {}", public_values.proof_valid);
    println!("  vk_hash: 0x{}", hex::encode(public_values.vk_hash));

    for (i, s) in public_values.state_before.iter().enumerate() {
        println!("  state_before[{}]: 0x{}", i, hex::encode(s));
    }

    for (i, s) in public_values.state_after.iter().enumerate() {
        println!("  state_after[{}]: 0x{}", i, hex::encode(s));
    }

    println!(
        "  action_state_before: 0x{}",
        hex::encode(public_values.action_state_before)
    );

    assert!(public_values.proof_valid, "Pickles proof invalid");
    println!("Pickles proof verified successfully");

    let fixture = SP1ProofFixture {
        system: format!("{:?}", system).to_lowercase(),
        fixture_dir: fixture_dir.to_owned(),
        proof_valid: public_values.proof_valid,
        vkey: vk.bytes32().to_string(),
        public_values: format!("0x{}", hex::encode(proof.public_values.as_slice())),
        proof: format!("0x{}", hex::encode(proof.bytes())),
    };

    println!("Verification Key: {}", fixture.vkey);
    println!("Public Values: {}", fixture.public_values);
    println!("Proof Bytes: {}", fixture.proof);

    let fixture_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../contracts/src/fixtures");
    std::fs::create_dir_all(&fixture_path).expect("failed to create fixture path");
    std::fs::write(
        fixture_path.join(format!("{:?}-fixture.json", system).to_lowercase()),
        serde_json::to_string_pretty(&fixture).unwrap(),
    )
    .expect("failed to write fixture");

    std::fs::create_dir_all("proofs").expect("create proofs dir");
    proof.save("proofs/evm-proof.bin").expect("save proof");
    println!("Proof saved to proofs/evm-proof.bin");
}
