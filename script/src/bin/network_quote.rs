use anyhow::{Context, Result};
use clap::{Parser, ValueEnum};
use serde_json::json;
use sp1_sdk::{
    network::{proto::GetProofRequestParamsResponse, signer::NetworkSigner, NetworkMode},
    ProverClient, SP1ProofMode,
};

#[derive(Clone, Copy, Debug, ValueEnum)]
enum ProofSystem {
    Groth16,
    Plonk,
}

#[derive(Debug, Parser)]
#[command(about = "Read current Succinct auction pricing without requesting a proof")]
struct Args {
    #[arg(long, value_enum, default_value_t = ProofSystem::Groth16)]
    proof_system: ProofSystem,
    /// Optional prover gas units used to calculate the current maximum charge.
    #[arg(long)]
    pgu: Option<u64>,
    /// Include the credited balance for NETWORK_PRIVATE_KEY.
    #[arg(long)]
    include_balance: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    dotenv::dotenv().ok();
    let args = Args::parse();
    let mode = match args.proof_system {
        ProofSystem::Groth16 => SP1ProofMode::Groth16,
        ProofSystem::Plonk => SP1ProofMode::Plonk,
    };
    let builder = ProverClient::builder().network_for(NetworkMode::Mainnet);
    let client = if args.include_balance {
        // The builder reads NETWORK_PRIVATE_KEY. get_balance only derives its
        // address and performs a read-only network RPC.
        builder.build().await
    } else {
        // SP1 6.1 requires a signer object even for public pricing RPCs. This
        // fixed throwaway key is never funded and cannot create a paid request
        // in this utility.
        builder
            .private_key("0x0000000000000000000000000000000000000000000000000000000000000001")
            .build()
            .await
    };
    let GetProofRequestParamsResponse::Auction(params) =
        client.get_proof_request_params(mode).await?
    else {
        anyhow::bail!("auction pricing is unavailable")
    };
    let base_fee = params
        .base_fee
        .parse::<u128>()
        .context("invalid network base fee")?;
    let max_price_per_pgu = params
        .max_price_per_pgu
        .parse::<u128>()
        .context("invalid network max price per PGU")?;
    let maximum_cost = args
        .pgu
        .map(|pgu| base_fee.saturating_add(max_price_per_pgu.saturating_mul(u128::from(pgu))));
    let balance = if args.include_balance {
        Some(
            client
                .get_balance()
                .await?
                .to_string()
                .parse::<u128>()
                .context("network balance does not fit u128")?,
        )
    } else {
        None
    };
    let requester_address = if args.include_balance {
        let private_key = std::env::var("NETWORK_PRIVATE_KEY")
            .context("NETWORK_PRIVATE_KEY is required with --include-balance")?;
        Some(format!(
            "{:#x}",
            NetworkSigner::local(&private_key)?.address()
        ))
    } else {
        None
    };
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "proofSystem": match args.proof_system {
                ProofSystem::Groth16 => "groth16",
                ProofSystem::Plonk => "plonk",
            },
            "baseFeeAttoProve": base_fee.to_string(),
            "baseFeeProve": format_prove(base_fee),
            "maxPricePerPguAttoProve": max_price_per_pgu.to_string(),
            "pgu": args.pgu,
            "maximumCostAttoProve": maximum_cost.map(|value| value.to_string()),
            "maximumCostProve": maximum_cost.map(format_prove),
            "balanceAttoProve": balance.map(|value| value.to_string()),
            "balanceProve": balance.map(format_prove),
            "requesterAddress": requester_address,
            "note": "Read-only auction parameters and optional account balance; no proof request was created"
        }))?
    );
    Ok(())
}

fn format_prove(value: u128) -> String {
    const SCALE: u128 = 1_000_000_000_000_000_000;
    let whole = value / SCALE;
    let fractional = format!("{:018}", value % SCALE);
    let fractional = fractional.trim_end_matches('0');
    if fractional.is_empty() {
        whole.to_string()
    } else {
        format!("{whole}.{fractional}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_prove_without_losing_atto_units() {
        assert_eq!(format_prove(0), "0");
        assert_eq!(format_prove(1), "0.000000000000000001");
        assert_eq!(format_prove(1_500_000_000_000_000_000), "1.5");
    }
}
