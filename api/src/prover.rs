use alloy::primitives::U256;
use anyhow::{Context, Result};
use serde_json::Value;
use sp1_sdk::{
    blocking::{Prover as BlockingProver, ProverClient as BlockingProverClient},
    network::{proto::GetProofRequestParamsResponse, NetworkMode, B256},
    HashableKey, ProveRequest, Prover, ProverClient, ProvingKey, SP1ProofMode,
    SP1ProofWithPublicValues, SP1Stdin,
};
use zeko_sp1_lib::{
    BridgeTransitionInput, BridgeTransitionPublicValuesV2, SettlementPublicValues,
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

#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuctionQuote {
    pub proof_system: String,
    pub base_fee_atto_prove: String,
    pub base_fee_prove: String,
    pub network_max_price_per_pgu: String,
    pub approved_max_pgu: String,
    pub approved_max_price_per_pgu: String,
    pub maximum_cost_atto_prove: String,
    pub maximum_cost_prove: String,
}

pub enum Preflight {
    Settlement {
        values: SettlementPublicValues,
        public_values: Vec<u8>,
        cycles: u64,
    },
    Bridge {
        values: BridgeTransitionPublicValuesV2,
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

    pub fn decode(kind: &str, public_values: Vec<u8>, cycles: u64) -> Result<Self> {
        match kind {
            "settlement" => Ok(Self::Settlement {
                values: SettlementPublicValues::decode(&public_values)
                    .map_err(anyhow::Error::msg)?,
                public_values,
                cycles,
            }),
            "bridge" => Ok(Self::Bridge {
                values: BridgeTransitionPublicValuesV2::decode(&public_values)
                    .map_err(anyhow::Error::msg)?,
                public_values,
                cycles,
            }),
            "withdraw" => Ok(Self::Withdraw {
                values: bincode::deserialize(&public_values)?,
                public_values,
                cycles,
            }),
            _ => anyhow::bail!("unsupported proof kind: {kind}"),
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
        Preflight::decode(&kind, public_values, cycles)
    })
    .await?
}

pub async fn auction_quote(
    system: &str,
    max_pgu: u64,
    approved_max_price_per_pgu: Option<u64>,
) -> Result<AuctionQuote> {
    let mode = match system {
        "groth16" => SP1ProofMode::Groth16,
        "plonk" => SP1ProofMode::Plonk,
        _ => anyhow::bail!("unsupported proof system: {system}"),
    };
    let client = ProverClient::builder()
        .network_for(NetworkMode::Mainnet)
        // Pricing is public and this fixed throwaway signer never creates a
        // request. Paid proving uses NETWORK_PRIVATE_KEY in request_proof.
        .private_key("0x0000000000000000000000000000000000000000000000000000000000000001")
        .build()
        .await;
    let GetProofRequestParamsResponse::Auction(params) =
        client.get_proof_request_params(mode).await?
    else {
        anyhow::bail!("auction pricing is unavailable")
    };
    let base_fee =
        U256::from_str_radix(&params.base_fee, 10).context("invalid network base fee")?;
    let network_max_price = U256::from_str_radix(&params.max_price_per_pgu, 10)
        .context("invalid network max price per PGU")?;
    let approved_max_price_per_pgu = approved_max_price_per_pgu.unwrap_or(
        params
            .max_price_per_pgu
            .parse()
            .context("network max price per PGU does not fit the SP1 SDK u64 cap")?,
    );
    let maximum_cost = base_fee
        .saturating_add(U256::from(max_pgu).saturating_mul(U256::from(approved_max_price_per_pgu)));
    Ok(AuctionQuote {
        proof_system: system.to_owned(),
        base_fee_atto_prove: base_fee.to_string(),
        base_fee_prove: format_prove(base_fee),
        network_max_price_per_pgu: network_max_price.to_string(),
        approved_max_pgu: max_pgu.to_string(),
        approved_max_price_per_pgu: approved_max_price_per_pgu.to_string(),
        maximum_cost_atto_prove: maximum_cost.to_string(),
        maximum_cost_prove: format_prove(maximum_cost),
    })
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

    #[test]
    fn prove_amounts_are_formatted_without_float_rounding() {
        assert_eq!(format_prove(U256::ZERO), "0");
        assert_eq!(format_prove(U256::from(1_u64)), "0.000000000000000001");
        assert_eq!(
            format_prove(U256::from(1_500_000_000_000_000_000_u64)),
            "1.5"
        );
        assert_eq!(
            format_prove(U256::from(12_345_678_901_234_567_890_u128)),
            "12.34567890123456789"
        );
    }

    #[test]
    fn prove_amount_parser_rejects_non_decimal_values() {
        assert_eq!(parse_prove("1000000000000000000").unwrap(), "1");
        assert!(parse_prove("1.5").is_err());
        assert!(parse_prove("0x1").is_err());
    }

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
