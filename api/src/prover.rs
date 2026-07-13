use alloy::primitives::U256;
use anyhow::{Context, Result};
use serde_json::Value;
use sp1_sdk::{
    blocking::{Prover as BlockingProver, ProverClient as BlockingProverClient},
    network::{NetworkMode, B256},
    HashableKey, ProveRequest, Prover, ProverClient, ProvingKey, SP1ProofWithPublicValues,
    SP1Stdin,
};
use zeko_sp1_lib::{
    BridgeTransitionInput, BridgeTransitionPublicValues, SettlementPublicValuesV1,
    WithdrawTransitionInput, WithdrawTransitionPublicValues,
};
use zkapp_script::{
    execute_minimal, settlement_stdin_from_bundle, SettlementProofBundle, BRIDGE_ELF,
    SETTLEMENT_ELF, WITHDRAW_ELF,
};

pub struct ProofOutput {
    pub proof: SP1ProofWithPublicValues,
    pub public_values: Vec<u8>,
}

#[derive(Clone, Debug)]
pub struct NetworkRequestConfig {
    pub timeout: std::time::Duration,
    pub min_auction_period: u64,
    pub gas_limit: Option<u64>,
    pub max_price_per_pgu: Option<u64>,
}

#[derive(Clone, Debug, Default)]
pub struct RequestMetrics {
    pub cycles: Option<u64>,
    pub prover_gas: Option<u64>,
    pub base_fee_prove: Option<String>,
    pub max_price_per_pgu: Option<String>,
    pub actual_cost_prove: Option<String>,
}

pub enum Preflight {
    Settlement {
        values: SettlementPublicValuesV1,
        public_values: Vec<u8>,
        cycles: u64,
    },
    Bridge {
        values: BridgeTransitionPublicValues,
        public_values: Vec<u8>,
        cycles: u64,
    },
    Withdraw {
        values: WithdrawTransitionPublicValues,
        public_values: Vec<u8>,
        cycles: u64,
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

    pub fn cycles(&self) -> u64 {
        match self {
            Preflight::Settlement { cycles, .. }
            | Preflight::Bridge { cycles, .. }
            | Preflight::Withdraw { cycles, .. } => *cycles,
        }
    }
}

pub async fn preflight(kind: &str, input: &Value) -> Result<Preflight> {
    let kind = kind.to_owned();
    let input = input.clone();
    tokio::task::spawn_blocking(move || {
        let (elf, stdin) = stdin_for(&kind, &input)?;
        let (public_values, cycles) =
            execute_minimal(elf, stdin).context("execute SP1 preflight")?;
        match kind.as_str() {
            "settlement" => Ok(Preflight::Settlement {
                values: SettlementPublicValuesV1::decode(&public_values)
                    .map_err(anyhow::Error::msg)?,
                public_values,
                cycles,
            }),
            "bridge" => Ok(Preflight::Bridge {
                values: bincode::deserialize(&public_values)?,
                public_values,
                cycles,
            }),
            "withdraw" => Ok(Preflight::Withdraw {
                values: bincode::deserialize(&public_values)?,
                public_values,
                cycles,
            }),
            _ => anyhow::bail!("unsupported proof kind: {kind}"),
        }
    })
    .await?
}

pub async fn request_proof(
    kind: &str,
    input: &Value,
    system: &str,
    config: &NetworkRequestConfig,
) -> Result<String> {
    let (elf, stdin) = stdin_for(kind, input)?;
    let client = ProverClient::builder()
        .network_for(NetworkMode::Mainnet)
        .build()
        .await;
    let pk = client.setup(elf).await.context("setup SP1 program")?;
    let mut request = match system {
        "groth16" => client.prove(&pk, stdin).groth16(),
        "plonk" => client.prove(&pk, stdin).plonk(),
        _ => anyhow::bail!("unsupported proof system: {system}"),
    };
    request = request
        .timeout(config.timeout)
        .min_auction_period(config.min_auction_period);
    if let Some(gas_limit) = config.gas_limit {
        request = request.gas_limit(gas_limit).skip_simulation(true);
    }
    if let Some(max_price_per_pgu) = config.max_price_per_pgu {
        request = request.max_price_per_pgu(max_price_per_pgu);
    }
    let request_id = request
        .request()
        .await
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

pub async fn request_metrics(request_id: &str) -> Result<RequestMetrics> {
    let request_id: B256 = request_id.parse().context("invalid SP1 proof request id")?;
    let client = ProverClient::builder()
        .network_for(NetworkMode::Mainnet)
        .build()
        .await;
    let Some(request) = client.get_proof_request(request_id).await? else {
        return Ok(RequestMetrics::default());
    };

    let actual_cost_prove = match (
        request.deduction_amount.as_deref(),
        request.refund_amount.as_deref(),
    ) {
        (Some(deduction), refund) => {
            let deduction =
                U256::from_str_radix(deduction, 10).context("invalid network deduction amount")?;
            let refund = refund
                .map(|value| U256::from_str_radix(value, 10))
                .transpose()
                .context("invalid network refund amount")?
                .unwrap_or_default();
            Some(format_prove(deduction.saturating_sub(refund)))
        }
        _ => None,
    };

    Ok(RequestMetrics {
        cycles: request.cycles,
        prover_gas: request.gas_used,
        base_fee_prove: request.base_fee.as_deref().map(parse_prove).transpose()?,
        max_price_per_pgu: request
            .max_price_per_pgu
            .as_deref()
            .map(parse_prove)
            .transpose()?,
        actual_cost_prove,
    })
}

fn parse_prove(value: &str) -> Result<String> {
    Ok(format_prove(
        U256::from_str_radix(value, 10).context("invalid PROVE amount")?,
    ))
}

fn format_prove(value: U256) -> String {
    let digits = value.to_string();
    if digits.len() <= 18 {
        return format!("0.{:0>18}", digits)
            .trim_end_matches('0')
            .trim_end_matches('.')
            .to_owned();
    }
    let split = digits.len() - 18;
    let formatted = format!("{}.{}", &digits[..split], &digits[split..]);
    formatted
        .trim_end_matches('0')
        .trim_end_matches('.')
        .to_owned()
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

fn stdin_for(kind: &str, input: &Value) -> Result<(sp1_sdk::Elf, SP1Stdin)> {
    match kind {
        "settlement" => {
            let bundle: SettlementProofBundle = serde_json::from_value(
                input
                    .get("proof")
                    .cloned()
                    .context("settlement proof bundle is required")?,
            )?;
            Ok((SETTLEMENT_ELF, settlement_stdin_from_bundle(&bundle)?))
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
            preflight("bridge", &input).await.unwrap(),
            Preflight::Bridge { .. }
        ));
    }

    #[tokio::test]
    #[ignore = "slow SP1 execution test"]
    async fn executes_withdraw_preflight_in_process() {
        let input: Value =
            serde_json::from_str(include_str!("../../proofs/withdraw-input.json")).unwrap();
        assert!(matches!(
            preflight("withdraw", &input).await.unwrap(),
            Preflight::Withdraw { .. }
        ));
    }
}
