use sp1_sdk::{blocking::MockProver, blocking::Prover, include_elf, Elf, HashableKey, ProvingKey};

/// The ELF (executable and linkable format) file for the Succinct RISC-V zkVM.
const SETTLEMENT_ELF: Elf = include_elf!("settlement-program");

fn main() {
    let prover = MockProver::new();
    let pk = prover.setup(SETTLEMENT_ELF).expect("failed to setup elf");
    println!("{}", pk.verifying_key().bytes32());
}
