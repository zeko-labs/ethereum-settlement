use anyhow::{Context, Result};
use serde_json::Value;
use sp1_sdk::{
    blocking::{Prover as BlockingProver, ProverClient as BlockingProverClient},
    network::{NetworkMode, B256},
    HashableKey, ProveRequest, Prover, ProverClient, ProvingKey, SP1ProofWithPublicValues,
    SP1Stdin,
};
use zeko_sp1_lib::{
    BridgeTransitionInput, BridgeTransitionPublicValues, WithdrawTransitionInput,
    WithdrawTransitionPublicValues, ZkappPublicValues,
};
use zkapp_script::{settlement_stdin, BRIDGE_ELF, SETTLEMENT_ELF, WITHDRAW_ELF};

pub struct ProofOutput {
    pub proof: SP1ProofWithPublicValues,
    pub public_values: Vec<u8>,
}

pub enum Preflight {
    Settlement {
        values: ZkappPublicValues,
        public_values: Vec<u8>,
    },
    Bridge {
        values: BridgeTransitionPublicValues,
        public_values: Vec<u8>,
    },
    Withdraw {
        values: WithdrawTransitionPublicValues,
        public_values: Vec<u8>,
    },
}

impl Preflight {
    pub fn public_values(&self) -> &[u8] {
        match self {
            Preflight::Settlement { public_values, .. }
            | Preflight::Bridge { public_values, .. }
            | Preflight::Withdraw { public_values, .. } => public_values,
        }
    }
}

pub async fn preflight(kind: &str, input: &Value, settlement_vk: &str) -> Result<Preflight> {
    let kind = kind.to_owned();
    let input = input.clone();
    let settlement_vk = settlement_vk.to_owned();
    tokio::task::spawn_blocking(move || {
        let (elf, stdin) = stdin_for(&kind, &input, &settlement_vk)?;
        let client = BlockingProverClient::builder().cpu().build();
        let (output, _) = client
            .execute(elf, stdin)
            .run()
            .context("execute SP1 preflight")?;
        let public_values = output.as_slice().to_vec();
        match kind.as_str() {
            "settlement" => Ok(Preflight::Settlement {
                values: bincode::deserialize(output.as_slice())?,
                public_values,
            }),
            "bridge" => Ok(Preflight::Bridge {
                values: bincode::deserialize(output.as_slice())?,
                public_values,
            }),
            "withdraw" => Ok(Preflight::Withdraw {
                values: bincode::deserialize(output.as_slice())?,
                public_values,
            }),
            _ => anyhow::bail!("unsupported proof kind: {kind}"),
        }
    })
    .await?
}

pub async fn request_proof(
    kind: &str,
    input: &Value,
    settlement_vk: &str,
    system: &str,
) -> Result<String> {
    let (elf, stdin) = stdin_for(kind, input, settlement_vk)?;
    let client = ProverClient::builder()
        .network_for(NetworkMode::Mainnet)
        .build()
        .await;
    let pk = client.setup(elf).await.context("setup SP1 program")?;
    let request_id = match system {
        "groth16" => client.prove(&pk, stdin).groth16().request().await,
        "plonk" => client.prove(&pk, stdin).plonk().request().await,
        _ => anyhow::bail!("unsupported proof system: {system}"),
    }
    .context("request SP1 Network proof")?;
    Ok(request_id.to_string())
}

pub async fn wait_proof(kind: &str, request_id: &str) -> Result<ProofOutput> {
    let elf = elf_for(kind)?;
    let request_id: B256 = request_id.parse().context("invalid SP1 proof request id")?;
    let client = ProverClient::builder()
        .network_for(NetworkMode::Mainnet)
        .build()
        .await;
    let proof = client
        .wait_proof(request_id, None, None)
        .await
        .context("wait for SP1 Network proof")?;
    let pk = client.setup(elf).await.context("setup SP1 program")?;
    client
        .verify(&proof, pk.verifying_key(), None)
        .context("verify generated SP1 proof")?;
    let public_values = proof.public_values.as_slice().to_vec();
    Ok(ProofOutput {
        proof,
        public_values,
    })
}

pub async fn program_vkey(kind: &str) -> Result<String> {
    let kind = kind.to_owned();
    tokio::task::spawn_blocking(move || {
        let elf = elf_for(&kind)?;
        let client = BlockingProverClient::builder().mock().build();
        let pk = client.setup(elf).context("setup SP1 program")?;
        Ok(pk.verifying_key().bytes32().to_string())
    })
    .await?
}

fn elf_for(kind: &str) -> Result<sp1_sdk::Elf> {
    match kind {
        "settlement" => Ok(SETTLEMENT_ELF),
        "bridge" => Ok(BRIDGE_ELF),
        "withdraw" => Ok(WITHDRAW_ELF),
        _ => anyhow::bail!("unsupported proof kind: {kind}"),
    }
}

fn stdin_for(kind: &str, input: &Value, settlement_vk: &str) -> Result<(sp1_sdk::Elf, SP1Stdin)> {
    match kind {
        "settlement" => {
            let graphql = input
                .get("graphql")
                .and_then(Value::as_str)
                .context("settlement graphql is required")?;
            Ok((SETTLEMENT_ELF, settlement_stdin(graphql, settlement_vk)?))
        }
        "bridge" => {
            let input: BridgeTransitionInput = serde_json::from_value(input.clone())?;
            let mut stdin = SP1Stdin::new();
            stdin.write(&input);
            Ok((BRIDGE_ELF, stdin))
        }
        "withdraw" => {
            let input: WithdrawTransitionInput = serde_json::from_value(input.clone())?;
            let mut stdin = SP1Stdin::new();
            stdin.write(&input);
            Ok((WITHDRAW_ELF, stdin))
        }
        _ => anyhow::bail!("unsupported proof kind: {kind}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore = "slow SP1 execution test"]
    async fn executes_bridge_preflight_in_process() {
        let input: Value =
            serde_json::from_str(include_str!("../../proofs/bridge-input.json")).unwrap();
        assert!(matches!(
            preflight("bridge", &input, "").await.unwrap(),
            Preflight::Bridge { .. }
        ));
    }

    #[tokio::test]
    #[ignore = "slow SP1 execution test"]
    async fn executes_withdraw_preflight_in_process() {
        let input: Value =
            serde_json::from_str(include_str!("../../proofs/withdraw-input.json")).unwrap();
        assert!(matches!(
            preflight("withdraw", &input, "").await.unwrap(),
            Preflight::Withdraw { .. }
        ));
    }
}
