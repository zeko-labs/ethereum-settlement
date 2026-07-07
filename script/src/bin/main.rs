//! Zeko SP1 zkApp/Pickles proof verifier.

use clap::Parser;
use pickles_verifier::types::VerifiableProof;
use pickles_verifier::wire::{parse_app_statement, parse_wrap_proof, parse_wrap_vk, OcamlProof};
use sp1_sdk::{
    blocking::{ProveRequest, Prover, ProverClient},
    include_elf, Elf, HashableKey, ProvingKey, SP1Stdin,
};
use std::path::Path;
use std::time::Instant;
use zeko_sp1_lib::ZkappPublicValues;

pub const ZKAPP_ELF: Elf = include_elf!("settlement-program");

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    #[arg(long)]
    execute: bool,
    #[arg(long)]
    prove: bool,
    #[arg(long, default_value = "proofs/mainnet-blockchain-snark")]
    fixture_dir: String,
}

fn main() {
    sp1_sdk::utils::setup_logger();
    dotenv::dotenv().ok();

    let args = Args::parse();
    if args.execute == args.prove {
        eprintln!("Error: specify either --execute or --prove");
        std::process::exit(1);
    }

    let verifiable = load_verifiable(Path::new(&args.fixture_dir));
    let mut stdin = SP1Stdin::new();
    stdin.write(&verifiable);

    let client = ProverClient::from_env();

    if args.execute {
        let (output, report) = client
            .execute(ZKAPP_ELF, stdin)
            .run()
            .expect("execution failed");

        println!("Program executed successfully");
        println!("  cycles   : {}", report.total_instruction_count());
        println!("  total gas: {:?}", report.gas());

        let mut entries: Vec<(&String, &u64)> = report.cycle_tracker.iter().collect();
        entries.sort_by(|a, b| a.0.cmp(b.0));
        for (name, cycles) in entries {
            println!("  [{name}] cycles: {cycles}");
        }

        let public_values: ZkappPublicValues =
            bincode::deserialize(output.as_slice()).expect("decode public values");

        print_public_values(&public_values);

        let pk = client.setup(ZKAPP_ELF).expect("failed to setup ELF");
        println!("Program Verification Key: {}", pk.verifying_key().bytes32());
        assert!(public_values.proof_valid, "Pickles proof invalid");
        println!("Pickles proof verified successfully");
    } else {
        let pk = client.setup(ZKAPP_ELF).expect("failed to setup ELF");

        println!("Generating proof...");
        let t = Instant::now();

        let proof = client.prove(&pk, stdin).run().expect("proof failed");

        println!("proving time: {:?}", t.elapsed());
        client
            .verify(&proof, pk.verifying_key(), None)
            .expect("verify failed");

        let public_values: ZkappPublicValues =
            bincode::deserialize(proof.public_values.as_slice()).expect("decode public values");

        print_public_values(&public_values);

        assert!(public_values.proof_valid, "Pickles proof invalid");
        std::fs::create_dir_all("proofs").expect("create proofs dir");
        proof.save("proofs/proof.bin").expect("save proof");
        println!("Proof saved to proofs/proof.bin");
    }
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

fn print_public_values(public_values: &ZkappPublicValues) {
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
}
