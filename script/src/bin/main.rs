//! Zeko SP1 zkApp/Pickles proof verifier.

use clap::Parser;
use pickles_verifier::types::VerifiableProof;
use pickles_verifier::wire::{
    parse_app_statement_fields, parse_wrap_proof, parse_wrap_vk, OcamlProof,
};
use sp1_core_executor::Program;
use sp1_core_executor_runner::MinimalExecutorRunner;
use sp1_sdk::{
    blocking::{ProveRequest, Prover, ProverClient},
    include_elf, Elf, ProvingKey, SP1Stdin,
};
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;
use zeko_sp1_lib::ZkappPublicValues;

pub const ZKAPP_ELF: Elf = include_elf!("settlement-program");

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    #[arg(long)]
    execute: bool,
    /// Calculate SP1 prover gas units during local execution. This is slower
    /// than the cycle-only executor, but the resulting value can be used with
    /// the read-only live network quote command.
    #[arg(long, requires = "execute")]
    calculate_gas: bool,
    #[arg(long)]
    prove: bool,
    #[arg(long, default_value = "proofs/mainnet-blockchain-snark")]
    fixture_dir: String,
    /// Test-only: mutate a recursive challenge so the guest must reject the
    /// resulting accumulator commitment.
    #[arg(long, requires = "execute")]
    corrupt_accumulator_challenge: bool,
}

fn main() {
    sp1_sdk::utils::setup_logger();
    dotenv::dotenv().ok();

    let args = Args::parse();
    if args.execute == args.prove {
        eprintln!("Error: specify either --execute or --prove");
        std::process::exit(1);
    }

    let mut verifiable = load_verifiable(Path::new(&args.fixture_dir));
    if args.corrupt_accumulator_challenge {
        verifiable.raw_bulletproof_challenges[0] += pickles_verifier::types::StepField::from(1u64);
    }
    let mut stdin = SP1Stdin::new();
    stdin.write(&verifiable);
    stdin.write(&Option::<zeko_sp1_lib::SettlementWitnessV1>::None);

    if args.execute {
        let (output, cycles, gas) = if args.calculate_gas {
            // Force the local prover even when network prover environment
            // variables are present. This simulates PGU without creating a
            // paid Succinct Network request.
            let client = ProverClient::builder().cpu().build();
            let (output, report) = client
                .execute(ZKAPP_ELF, stdin)
                .calculate_gas(true)
                .run()
                .expect("execution failed");
            (
                output.as_slice().to_vec(),
                report.total_instruction_count(),
                report.gas(),
            )
        } else {
            let (output, cycles) = execute_minimal(ZKAPP_ELF, stdin).expect("execution failed");
            (output, cycles, None)
        };

        println!("Program executed successfully");
        println!("  cycles   : {cycles}");
        match gas {
            Some(gas) => println!("  total gas: {gas}"),
            None => println!("  total gas: not calculated"),
        }

        let public_values: ZkappPublicValues =
            bincode::deserialize(&output).expect("decode public values");

        print_public_values(&public_values);

        assert!(public_values.proof_valid, "Pickles proof invalid");
        println!("Pickles proof verified successfully");
    } else {
        let client = ProverClient::from_env();
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

fn execute_minimal(elf: Elf, stdin: SP1Stdin) -> anyhow::Result<(Vec<u8>, u64)> {
    let program = Arc::new(
        Program::from(&*elf).map_err(|e| anyhow::anyhow!("failed to disassemble program: {e}"))?,
    );
    let mut executor = MinimalExecutorRunner::simple(program);

    for input in stdin.buffer {
        executor.with_input(&input);
    }

    while executor
        .try_execute_chunk()
        .map_err(|e| anyhow::anyhow!("execute chunk failed: {e}"))?
        .is_some()
    {}

    let exit_code = executor.exit_code();
    if exit_code != 0 {
        anyhow::bail!("program exited with status {exit_code}");
    }

    let cycles = executor.global_clk();
    Ok((executor.into_public_values_stream(), cycles))
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
    // TODO: Derive the application statement from the transaction inside the
    // guest instead of accepting host-prepared statement fields.
    let app_stmt = parse_app_statement_fields(&app_stmt_json).expect("parse app_statement.json");

    ocaml
        .into_verifiable(wrap_proof, &wrap_vk, &app_stmt)
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
