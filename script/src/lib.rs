use anyhow::{Context, Result};
use pickles_verifier::types::VerifiableProof;
use pickles_verifier::wire::{
    parse_app_statement_fields, parse_wrap_proof, parse_wrap_vk, OcamlProof,
};
use serde::{Deserialize, Serialize};
use sp1_core_executor::Program;
use sp1_core_executor_runner::MinimalExecutorRunner;
use sp1_sdk::{include_elf, Elf, SP1Stdin};
use std::path::Path;
use std::sync::Arc;
use zeko_sp1_lib::{SettlementBindingV1, SettlementContextV1, SettlementWitnessV1};

pub const SETTLEMENT_ELF: Elf = include_elf!("settlement-program");
pub const BRIDGE_ELF: Elf = include_elf!("bridge-program");
pub const WITHDRAW_ELF: Elf = include_elf!("withdraw-program");

/// Execute without proving using the low-memory runner. SP1 SDK 6.1's execute
/// wrapper can fail while extracting the public-value digest for this very
/// large program even after successful execution.
pub fn execute_minimal(elf: Elf, stdin: SP1Stdin) -> Result<(Vec<u8>, u64)> {
    let program = Arc::new(
        Program::from(&*elf).map_err(|error| anyhow::anyhow!("disassemble program: {error}"))?,
    );
    let mut executor = MinimalExecutorRunner::simple(program);
    for input in stdin.buffer {
        executor.with_input(&input);
    }
    while executor
        .try_execute_chunk()
        .map_err(|error| anyhow::anyhow!("execute chunk: {error}"))?
        .is_some()
    {}
    let exit_code = executor.exit_code();
    if exit_code != 0 {
        anyhow::bail!("program exited with status {exit_code}");
    }
    let cycles = executor.global_clk();
    Ok((executor.into_public_values_stream(), cycles))
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SettlementProofBundle {
    pub vk_json: String,
    pub proof_json: String,
    pub public_input_skeleton_json: String,
    pub app_statement_json: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binding: Option<SettlementBindingV1>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<SettlementContextV1>,
}

pub fn settlement_stdin(fixture_dir: &str) -> Result<SP1Stdin> {
    let verifiable = load_verifiable(Path::new(fixture_dir))?;
    Ok(settlement_stdin_for_verifiable(&verifiable, None))
}

pub fn settlement_stdin_from_bundle(bundle: &SettlementProofBundle) -> Result<SP1Stdin> {
    let verifiable = load_verifiable_bundle(bundle)?;
    let witness = match (&bundle.binding, &bundle.context) {
        (Some(binding), Some(context)) => Some(SettlementWitnessV1 {
            binding: binding.clone(),
            context: context.clone(),
        }),
        (None, None) => None,
        _ => anyhow::bail!("settlement binding and context must either both be present or absent"),
    };
    Ok(settlement_stdin_for_verifiable(&verifiable, witness))
}

fn settlement_stdin_for_verifiable(
    verifiable: &VerifiableProof,
    witness: Option<SettlementWitnessV1>,
) -> SP1Stdin {
    let mut stdin = SP1Stdin::new();
    stdin.write(verifiable);
    stdin.write(&witness);
    stdin
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

    load_verifiable_bundle(&SettlementProofBundle {
        vk_json,
        proof_json,
        public_input_skeleton_json: skeleton_json,
        app_statement_json: app_stmt_json,
        binding: None,
        context: None,
    })
}

pub fn load_verifiable_bundle(bundle: &SettlementProofBundle) -> Result<VerifiableProof> {
    let wrap_vk = parse_wrap_vk(&bundle.vk_json).context("parse vk JSON")?;
    let wrap_proof = parse_wrap_proof(&bundle.proof_json).context("parse proof JSON")?;
    let ocaml = OcamlProof::parse(&bundle.public_input_skeleton_json)
        .map_err(anyhow::Error::msg)
        .context("parse public input skeleton JSON")?;
    let app_statement = parse_app_statement_fields(&bundle.app_statement_json)
        .map_err(anyhow::Error::msg)
        .context("parse application statement JSON")?;

    ocaml
        .into_verifiable(wrap_proof, &wrap_vk, &app_statement)
        .map_err(anyhow::Error::msg)
}
