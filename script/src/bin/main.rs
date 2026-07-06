//! Zeko SP1 zkApp proof verifier.

use clap::Parser;
use sp1_sdk::{
    blocking::{ProveRequest, Prover, ProverClient},
    include_elf, Elf, HashableKey, ProvingKey, SP1Stdin,
};
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
use mina_p2p_messages::v2::MinaBaseVerificationKeyWireStableV1;

pub const ZKAPP_ELF: Elf = include_elf!("settlement-program");

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    #[arg(long)]
    execute: bool,
    #[arg(long)]
    prove: bool,
    #[arg(long, default_value = "proofs/graphql.txt")]
    graphql: String,
    #[arg(long, default_value = "proofs/vk.txt")]
    vk: String,
}

fn main() {
    sp1_sdk::utils::setup_logger();
    dotenv::dotenv().ok();

    let args = Args::parse();
    if args.execute == args.prove {
        eprintln!("Error: specify either --execute or --prove");
        std::process::exit(1);
    }

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

    let client = ProverClient::from_env();

    if args.execute {
        let (output, report) = client
            .execute(ZKAPP_ELF, stdin)
            .run()
            .expect("execution failed");

        println!("Program executed successfully");
        println!("  cycles   : {}", report.total_instruction_count());
        println!("  total gas: {:?}", report.gas());
        for (name, cycles) in &report.cycle_tracker {
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
