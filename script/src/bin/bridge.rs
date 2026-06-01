//! Zeko SP1 — Bridge proof verifier
//!
//! Execute (no proof):
//! ```shell
//! RUST_LOG=info cargo run --release --bin bridge  -- --execute
//! cargo run --release --bin bridge -- --execute --input proofs/bridge-input-200.json
//! ```
//! Prove (local core proof):
//! ```shell
//! RUST_LOG=info cargo run --release --bin bridge  -- --prove
//! ```

use clap::Parser;
use sp1_sdk::{
    blocking::{ProveRequest, Prover, ProverClient},
    include_elf, Elf, HashableKey, ProvingKey, SP1Stdin,
};
use std::time::Instant;
use zeko_sp1_lib::{BridgeTransitionInput, BridgeTransitionPublicValues};

pub const BRIDGE_ELF: Elf = include_elf!("bridge-program");

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    #[arg(long)]
    execute: bool,
    #[arg(long)]
    prove: bool,
    #[arg(long, default_value = "proofs/bridge-input.json")]
    input: String,
}

fn main() {
    sp1_sdk::utils::setup_logger();
    dotenv::dotenv().ok();

    let args = Args::parse();
    if args.execute == args.prove {
        eprintln!("Error: specify either --execute or --prove");
        std::process::exit(1);
    }

    let input_json = std::fs::read_to_string(&args.input)
        .unwrap_or_else(|e| panic!("read bridge input {}: {e}", args.input));
    let input: BridgeTransitionInput =
        serde_json::from_str(&input_json).expect("deserialize bridge input");

    let mut stdin = SP1Stdin::new();
    stdin.write(&input);

    // Force the local prover for bridge proofs, even if network prover env vars are set.
    let client = ProverClient::builder().cpu().build();

    if args.execute {
        let (output, report) = client
            .execute(BRIDGE_ELF, stdin)
            .run()
            .expect("execution failed");

        let public_values: BridgeTransitionPublicValues =
            bincode::deserialize(output.as_slice()).expect("decode public values");

        println!("✓ Bridge program executed successfully");
        println!("  cycles   : {}", report.total_instruction_count());
        println!("  total gas: {:?}", report.gas());
        println!(
            "  ethereum_state_before: 0x{}",
            hex::encode(public_values.ethereum_state_before)
        );
        println!(
            "  ethereum_state_after : 0x{}",
            hex::encode(public_values.ethereum_state_after)
        );
        println!(
            "  zeko_action_before   : 0x{}",
            hex::encode(public_values.zeko_action_state_before)
        );
        println!(
            "  zeko_action_after    : 0x{}",
            hex::encode(public_values.zeko_action_state_after)
        );
        println!(
            "  withdraw_state_after : 0x{}",
            hex::encode(public_values.ethereum_withdraw_state_after)
        );
        println!(
            "  nonce_before         : {}",
            public_values.ethereum_nonce_before
        );
        println!(
            "  nonce_after          : {}",
            public_values.ethereum_nonce_after
        );
        println!("  deposit_count        : {}", public_values.deposit_count);
        println!("  withdraw_count       : {}", public_values.withdraw_count);
    } else {
        let pk = client.setup(BRIDGE_ELF).expect("failed to setup ELF");

        println!("Generating bridge proof...");
        let t = Instant::now();

        let proof = client.prove(&pk, stdin).run().expect("proof failed");

        println!("⏱  proving time: {:?}", t.elapsed());
        client
            .verify(&proof, pk.verifying_key(), None)
            .expect("verify failed");

        let public_values: BridgeTransitionPublicValues =
            bincode::deserialize(proof.public_values.as_slice()).expect("decode public values");

        println!("Program Verification Key: {}", pk.verifying_key().bytes32());
        println!(
            "  ethereum_state_after : 0x{}",
            hex::encode(public_values.ethereum_state_after)
        );
        println!(
            "  zeko_action_after    : 0x{}",
            hex::encode(public_values.zeko_action_state_after)
        );
        println!(
            "  withdraw_state_after : 0x{}",
            hex::encode(public_values.ethereum_withdraw_state_after)
        );

        std::fs::create_dir_all("proofs").expect("create proofs dir");
        proof
            .save("proofs/bridge-proof.bin")
            .expect("save bridge proof");
        println!("✓ Proof saved → proofs/bridge-proof.bin");
    }
}
