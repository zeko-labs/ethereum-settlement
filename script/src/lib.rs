use anyhow::{Context, Result};
use pickles_verifier::types::VerifiableProof;
use pickles_verifier::wire::{parse_app_statement, parse_wrap_proof, parse_wrap_vk, OcamlProof};
use sp1_sdk::{include_elf, Elf, SP1Stdin};
use std::path::Path;

pub const SETTLEMENT_ELF: Elf = include_elf!("settlement-program");
pub const BRIDGE_ELF: Elf = include_elf!("bridge-program");
pub const WITHDRAW_ELF: Elf = include_elf!("withdraw-program");

pub fn settlement_stdin(fixture_dir: &str) -> Result<SP1Stdin> {
    let verifiable = load_verifiable(Path::new(fixture_dir))?;
    let mut stdin = SP1Stdin::new();
    stdin.write(&verifiable);
    Ok(stdin)
}

fn load_verifiable(fixture_dir: &Path) -> Result<VerifiableProof> {
    let read = |name: &str| -> Result<String> {
        let path = fixture_dir.join(name);
        std::fs::read_to_string(&path).with_context(|| format!("failed to read {}", path.display()))
    };

    let vk_json = read("vk.serde.json")?;
    let proof_json = read("proof.serde.json")?;
    let skeleton_json = read("public_input_skeleton.json")?;
    let app_stmt_json = read("app_statement.json")?;

    let wrap_vk = parse_wrap_vk(&vk_json).context("parse vk.serde.json")?;
    let wrap_proof = parse_wrap_proof(&proof_json).context("parse proof.serde.json")?;
    let ocaml = OcamlProof::parse(&skeleton_json).map_err(anyhow::Error::msg)?;
    let app_stmt = parse_app_statement(&app_stmt_json).map_err(anyhow::Error::msg)?;

    ocaml
        .into_verifiable(wrap_proof, &wrap_vk, &[app_stmt])
        .map_err(anyhow::Error::msg)
}
