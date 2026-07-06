//! Zeko SP1 EVM-compatible zkApp proof verifier.

use clap::{Parser, ValueEnum};
use mina_p2p_messages::v2::MinaBaseVerificationKeyWireStableV1;
use serde::{Deserialize, Serialize};
use sp1_sdk::{
    include_elf, network::NetworkMode, utils, Elf, HashableKey, ProveRequest, Prover, ProverClient,
    ProvingKey, SP1ProofWithPublicValues, SP1Stdin, SP1VerifyingKey,
};
use std::path::PathBuf;
use std::time::Instant;
use zeko_sp1_lib::ZkappPublicValues;

#[path = "../parser.rs"]
mod parser;
use parser::parse_graphql_zkapp_file;

use ledger::{
    scan_state::transaction_logic::{
        verifiable,
        zkapp_command::{verifiable::create, ZkAppCommand},
        TransactionStatus, WithStatus,
    },
    verifier::common::{check, CheckResult},
    VerificationKey, VerificationKeyWire,
};

/// The ELF for the zkApp SP1 program.
pub const ZKAPP_ELF: Elf = include_elf!("settlement-program");

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct EVMArgs {
    #[arg(long, default_value = "proofs/graphql.txt")]
    graphql: String,

    #[arg(long, default_value = "proofs/vk.txt")]
    vk: String,

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
    graphql_path: String,
    vk_path: String,
    proof_valid: bool,
    zkapp_command_bytes_len: usize,
    zkapp_stmt_bytes_len: usize,
    vkey: String,
    public_values: String,
    proof: String,
}

#[tokio::main]
async fn main() {
    utils::setup_logger();
    dotenv::dotenv().ok();

    let args = EVMArgs::parse();

    // ------------------------------------------------------------------
    // 1. Parse
    // ------------------------------------------------------------------
    let vk_b64 =
        std::fs::read_to_string(&args.vk).unwrap_or_else(|e| panic!("read vk {}: {e}", args.vk));
    let parsed = parse_graphql_zkapp_file(&args.graphql)
        .unwrap_or_else(|e| panic!("parse graphql {}: {e}", args.graphql));

    let vk_wire =
        MinaBaseVerificationKeyWireStableV1::from_base64(vk_b64.trim()).expect("decode vk base64");
    let vk: VerificationKey = (&vk_wire).try_into().expect("vk wire -> runtime");
    let cmd: ZkAppCommand = (&parsed.zkapp_command)
        .try_into()
        .expect("wire -> ZkAppCommand");

    eprintln!("parsed");

    let zkapp_cmd_bytes =
        bincode::serialize(&parsed.zkapp_command).expect("serialize zkapp_command wire");
    eprintln!("zkapp_command: {} bytes", zkapp_cmd_bytes.len());

    // ------------------------------------------------------------------
    // 2. Derive ZkappStatement on the host
    // ------------------------------------------------------------------
    let cmd_verifiable = create(&cmd, false, |_, _| Ok(VerificationKeyWire::new(vk.clone())))
        .expect("verifiable::create");

    let (_, zkapp_stmt, _) = match check(WithStatus {
        data: verifiable::UserCommand::ZkAppCommand(Box::new(cmd_verifiable)),
        status: TransactionStatus::Applied,
    }) {
        CheckResult::ValidAssuming((_valid, mut xs)) => xs.pop().expect("empty"),
        other => panic!("expected ValidAssuming, got: {other:?}"),
    };

    eprintln!("zkapp_stmt derived");

    // ------------------------------------------------------------------
    // 3. Serialize guest inputs
    // ------------------------------------------------------------------
    let zkapp_stmt_bytes = bincode::serialize(&zkapp_stmt).expect("serialize zkapp_stmt");
    eprintln!("zkapp_stmt: {} bytes", zkapp_stmt_bytes.len());

    let mut stdin = SP1Stdin::new();
    stdin.write(&vk_wire);
    stdin.write(&parsed.proof);
    stdin.write_slice(&zkapp_stmt_bytes);
    stdin.write_slice(&zkapp_cmd_bytes);

    // ------------------------------------------------------------------
    // 4. Setup and prove on the Succinct Prover Network
    // ------------------------------------------------------------------
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

    create_proof_fixture(
        &proof,
        pk.verifying_key(),
        &args.graphql,
        &args.vk,
        zkapp_cmd_bytes.len(),
        zkapp_stmt_bytes.len(),
        args.system,
    );
}

fn create_proof_fixture(
    proof: &SP1ProofWithPublicValues,
    vk: &SP1VerifyingKey,
    graphql_path: &str,
    vk_path: &str,
    zkapp_command_bytes_len: usize,
    zkapp_stmt_bytes_len: usize,
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
        graphql_path: graphql_path.to_owned(),
        vk_path: vk_path.to_owned(),
        proof_valid: public_values.proof_valid,
        zkapp_command_bytes_len,
        zkapp_stmt_bytes_len,
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
