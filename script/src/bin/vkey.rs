use clap::{Parser, ValueEnum};
use sp1_sdk::{blocking::MockProver, blocking::Prover, HashableKey, ProvingKey};
use zkapp_script::{BRIDGE_ELF, SETTLEMENT_ELF, WITHDRAW_ELF};

#[derive(Clone, Copy, Debug, ValueEnum)]
enum Program {
    Settlement,
    Bridge,
    Withdraw,
}

#[derive(Debug, Parser)]
struct Args {
    #[arg(long, value_enum, default_value_t = Program::Settlement)]
    program: Program,
}

fn main() {
    let args = Args::parse();
    let elf = match args.program {
        Program::Settlement => SETTLEMENT_ELF,
        Program::Bridge => BRIDGE_ELF,
        Program::Withdraw => WITHDRAW_ELF,
    };
    let prover = MockProver::new();
    let pk = prover.setup(elf).expect("failed to setup elf");
    println!("{}", pk.verifying_key().bytes32());
}
