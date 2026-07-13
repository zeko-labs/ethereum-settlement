//! Read-only Succinct Network quote helper. This does not register a program,
//! submit a request, reserve funds, execute, or prove anything.

use anyhow::{Context, Result};
use sp1_sdk::{
    network::{proto::GetProofRequestParamsResponse, NetworkMode},
    ProverClient, SP1ProofMode,
};

#[tokio::main]
async fn main() -> Result<()> {
    let pgu = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "52146595101".to_owned())
        .parse::<u64>()
        .context("usage: network_quote [estimated-pgu]")?;
    let client = ProverClient::builder()
        .network_for(NetworkMode::Mainnet)
        // SP1 6.1 requires a signer even for this public read-only RPC. Use a
        // fixed throwaway key so quoting never touches the requester's key.
        .private_key("0x0000000000000000000000000000000000000000000000000000000000000001")
        .build()
        .await;

    for (name, mode) in [
        ("groth16", SP1ProofMode::Groth16),
        ("plonk", SP1ProofMode::Plonk),
    ] {
        let response = client.get_proof_request_params(mode).await?;
        let GetProofRequestParamsResponse::Auction(params) = response else {
            anyhow::bail!("auction quotes are unavailable");
        };
        let base_fee = params.base_fee.parse::<u128>()?;
        let max_price_per_pgu = params.max_price_per_pgu.parse::<u128>()?;
        let maximum = base_fee + u128::from(pgu) * max_price_per_pgu;
        println!("proof_system={name}");
        println!("estimated_pgu={pgu}");
        println!("base_fee_atto_prove={base_fee}");
        println!("max_price_per_pgu_atto_prove={max_price_per_pgu}");
        println!("maximum_cost_prove={}", format_prove(maximum));
    }
    Ok(())
}

fn format_prove(value: u128) -> String {
    let whole = value / 1_000_000_000_000_000_000;
    let fraction = value % 1_000_000_000_000_000_000;
    format!("{whole}.{fraction:018}")
        .trim_end_matches('0')
        .trim_end_matches('.')
        .to_owned()
}
