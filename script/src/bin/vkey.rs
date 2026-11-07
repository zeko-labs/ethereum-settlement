use clap::{Parser, ValueEnum};
use sp1_sdk::{blocking::MockProver, blocking::Prover, HashableKey, ProvingKey};
use std::path::PathBuf;
use zkapp_script::{Artifacts, ARTIFACTS_DIR_ENV, DEFAULT_ARTIFACTS_DIR};

#[derive(Clone, Copy, Debug, ValueEnum)]
enum Program {
    Settlement,
    Bridge,
}

#[derive(Debug, Parser)]
struct Args {
    #[arg(long, value_enum, default_value_t = Program::Settlement)]
    program: Program,
    /// Directory holding settlement-program, bridge-program and
    /// settlement-vk.serde.json.
    #[arg(long, env = ARTIFACTS_DIR_ENV, default_value = DEFAULT_ARTIFACTS_DIR)]
    artifacts_dir: PathBuf,
}

fn main() {
    let args = Args::parse();
    let artifacts = Artifacts::load(&args.artifacts_dir)
        .unwrap_or_else(|error| panic!("load settlement artifacts: {error:#}"));
    let elf = match args.program {
        Program::Settlement => artifacts.settlement_elf.clone(),
        Program::Bridge => artifacts.bridge_elf.clone(),
    };
    let prover = MockProver::new();
    let pk = prover.setup(elf).expect("failed to setup elf");
    println!("{}", pk.verifying_key().bytes32());
}
