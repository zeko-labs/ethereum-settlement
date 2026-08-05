use anyhow::{Context, Result};
use clap::Parser;
use serde_json::json;
use sha2::{Digest, Sha256};
use sp1_sdk::{
    network::{proto::GetProofRequestParamsResponse, NetworkMode},
    HashableKey, ProveRequest, Prover, ProverClient, ProvingKey, SP1ProofMode,
};
use std::{
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use zeko_sp1_lib::ZkappPublicValues;
use zkapp_script::{execute_minimal, settlement_stdin, SETTLEMENT_ELF};

const PROVE_SCALE: u128 = 1_000_000_000_000_000_000;
const FIXTURE_FILES: [&str; 4] = [
    "vk.serde.json",
    "proof.serde.json",
    "public_input_skeleton.json",
    "app_statement.json",
];

#[derive(Debug, Parser)]
#[command(about = "Preflight or request one capped fixture proof from the Succinct network")]
struct Args {
    #[arg(long, default_value = "proofs/mainnet-blockchain-snark")]
    fixture_dir: PathBuf,
    /// Create the paid request. Without this flag the command is read-only.
    #[arg(long)]
    request: bool,
    /// Required with --request and must match the fixture manifest SHA-256.
    #[arg(long, requires = "request")]
    approved_input_sha256: Option<String>,
    /// Retained local SP1 gas simulation for this exact guest and fixture.
    #[arg(long, default_value_t = 4_736_376_451)]
    simulated_pgu: u64,
    /// One-percent PGU headroom over the retained local simulation.
    #[arg(long, default_value_t = 4_783_740_216)]
    max_pgu: u64,
    /// Slightly below the current network maximum so the PGU buffer remains
    /// within the previously approved 3.81005428021 PROVE total ceiling.
    #[arg(long, default_value_t = 700_000_000)]
    max_price_per_pgu: u64,
    #[arg(long, default_value_t = 3_810_054_280_210_000_000_u128)]
    max_total_atto_prove: u128,
    #[arg(long, default_value_t = 7_200)]
    timeout_secs: u64,
    #[arg(long, default_value_t = 15)]
    min_auction_period_secs: u64,
    #[arg(
        long,
        default_value = "build/network-proof/optimized-fixture-groth16-20260731.bin"
    )]
    output: PathBuf,
}

#[tokio::main]
async fn main() -> Result<()> {
    dotenv::dotenv().ok();
    let args = Args::parse();
    anyhow::ensure!(
        args.max_pgu >= args.simulated_pgu,
        "approved PGU cap is below the retained simulation"
    );

    let input_sha256 = fixture_manifest_sha256(&args.fixture_dir)?;
    if args.request {
        let approved = args
            .approved_input_sha256
            .as_deref()
            .context("--approved-input-sha256 is required with --request")?;
        anyhow::ensure!(
            approved.eq_ignore_ascii_case(&input_sha256),
            "approved fixture SHA-256 does not match the requested input"
        );
    }

    let fixture_dir = args
        .fixture_dir
        .to_str()
        .context("fixture path is not valid UTF-8")?;
    let stdin = settlement_stdin(fixture_dir)?;
    let preflight_started = Instant::now();
    let (expected_public_values, cycles) = execute_minimal(SETTLEMENT_ELF, stdin)
        .context("execute low-memory settlement preflight")?;
    let decoded: ZkappPublicValues =
        bincode::deserialize(&expected_public_values).context("decode public values")?;
    anyhow::ensure!(
        decoded.proof_valid,
        "local Pickles preflight rejected the proof"
    );
    let public_values_sha256 = hex::encode(Sha256::digest(&expected_public_values));

    let local_client = ProverClient::builder().mock().build().await;
    let local_pk = local_client
        .setup(SETTLEMENT_ELF)
        .await
        .context("set up settlement program")?;
    let program_vkey = local_pk.verifying_key().bytes32().to_string();
    emit(json!({
        "event": "preflight_complete",
        "timestampMs": unix_time_ms()?,
        "fixtureDir": args.fixture_dir,
        "inputSha256": input_sha256,
        "programVkey": program_vkey,
        "cycles": cycles,
        "simulatedPgu": args.simulated_pgu,
        "publicValuesBytes": expected_public_values.len(),
        "publicValuesSha256": public_values_sha256,
        "preflightElapsedSeconds": preflight_started.elapsed().as_secs_f64(),
        "paidRequestAuthorized": args.request,
    }))?;
    if !args.request {
        return Ok(());
    }

    let client = ProverClient::builder()
        .network_for(NetworkMode::Mainnet)
        .build()
        .await;
    let balance_before = parse_u128(client.get_balance().await?.to_string(), "network balance")?;
    let GetProofRequestParamsResponse::Auction(params) = client
        .get_proof_request_params(SP1ProofMode::Groth16)
        .await?
    else {
        anyhow::bail!("auction pricing is unavailable")
    };
    let base_fee = parse_u128(params.base_fee, "network base fee")?;
    let network_max_price = parse_u128(params.max_price_per_pgu, "network max price per PGU")?;
    let maximum_cost = base_fee.saturating_add(
        u128::from(args.max_pgu).saturating_mul(u128::from(args.max_price_per_pgu)),
    );
    emit(json!({
        "event": "authorization_check",
        "timestampMs": unix_time_ms()?,
        "balanceAttoProve": balance_before.to_string(),
        "balanceProve": format_prove(balance_before),
        "baseFeeAttoProve": base_fee.to_string(),
        "baseFeeProve": format_prove(base_fee),
        "networkMaxPricePerPgu": network_max_price.to_string(),
        "approvedMaxPgu": args.max_pgu.to_string(),
        "approvedMaxPricePerPgu": args.max_price_per_pgu.to_string(),
        "maximumCostAttoProve": maximum_cost.to_string(),
        "maximumCostProve": format_prove(maximum_cost),
        "operatorCeilingAttoProve": args.max_total_atto_prove.to_string(),
        "operatorCeilingProve": format_prove(args.max_total_atto_prove),
    }))?;
    anyhow::ensure!(
        maximum_cost <= args.max_total_atto_prove,
        "live maximum cost {} PROVE exceeds operator ceiling {} PROVE",
        format_prove(maximum_cost),
        format_prove(args.max_total_atto_prove)
    );
    anyhow::ensure!(
        balance_before >= maximum_cost,
        "network balance {} PROVE is below live maximum cost {} PROVE",
        format_prove(balance_before),
        format_prove(maximum_cost)
    );

    let stdin = settlement_stdin(fixture_dir)?;
    let pk = client
        .setup(SETTLEMENT_ELF)
        .await
        .context("set up settlement program on network client")?;
    anyhow::ensure!(
        pk.verifying_key().bytes32().to_string() == program_vkey,
        "network client and local preflight program vkeys differ"
    );

    let requested_at_ms = unix_time_ms()?;
    let request_started = Instant::now();
    let request_id = client
        .prove(&pk, stdin)
        .groth16()
        .timeout(Duration::from_secs(args.timeout_secs))
        .min_auction_period(args.min_auction_period_secs)
        .gas_limit(args.max_pgu)
        .max_price_per_pgu(args.max_price_per_pgu)
        .skip_simulation(true)
        .request()
        .await
        .context("create paid SP1 proof request")?;
    emit(json!({
        "event": "request_created",
        "timestampMs": unix_time_ms()?,
        "requestedAtMs": requested_at_ms,
        "requestId": request_id.to_string(),
        "proofSystem": "groth16",
        "explorerUrl": format!("https://explorer.succinct.xyz/request/{request_id}"),
    }))?;

    let proof = client
        .wait_proof(request_id, None, None)
        .await
        .context("wait for SP1 network proof")?;
    client
        .verify(&proof, pk.verifying_key(), None)
        .context("verify returned SP1 proof")?;
    anyhow::ensure!(
        proof.public_values.as_slice() == expected_public_values,
        "network proof public values differ from local preflight"
    );
    if let Some(parent) = args.output.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("create proof output directory {}", parent.display()))?;
    }
    proof
        .save(&args.output)
        .with_context(|| format!("save proof to {}", args.output.display()))?;

    let request = client
        .get_proof_request(request_id)
        .await?
        .context("completed proof request is unavailable")?;
    let deduction = parse_optional_u128(request.deduction_amount.as_deref(), "deduction amount")?;
    let refund = parse_optional_u128(request.refund_amount.as_deref(), "refund amount")?;
    let accounted_cost = deduction.map(|value| value.saturating_sub(refund.unwrap_or_default()));
    let balance_after = parse_u128(client.get_balance().await?.to_string(), "network balance")?;
    let balance_cost = balance_before.saturating_sub(balance_after);
    let actual_cost = accounted_cost
        .filter(|value| *value > 0)
        .unwrap_or(balance_cost);
    emit(json!({
        "event": "proof_complete",
        "timestampMs": unix_time_ms()?,
        "requestId": request_id.to_string(),
        "elapsedSeconds": request_started.elapsed().as_secs_f64(),
        "cycles": request.cycles,
        "proverGas": request.gas_used,
        "deductionAttoProve": deduction.map(|value| value.to_string()),
        "refundAttoProve": refund.map(|value| value.to_string()),
        "balanceBeforeProve": format_prove(balance_before),
        "balanceAfterProve": format_prove(balance_after),
        "actualCostAttoProve": actual_cost.to_string(),
        "actualCostProve": format_prove(actual_cost),
        "proofPath": args.output,
        "proofBytes": fs::metadata(&args.output)?.len(),
        "locallyVerified": true,
        "publicValuesMatchPreflight": true,
        "ethereumSubmitted": false,
    }))?;
    Ok(())
}

fn fixture_manifest_sha256(fixture_dir: &Path) -> Result<String> {
    let mut hasher = Sha256::new();
    for name in FIXTURE_FILES {
        let path = fixture_dir.join(name);
        let bytes = fs::read(&path).with_context(|| format!("read {}", path.display()))?;
        hasher.update((name.len() as u64).to_be_bytes());
        hasher.update(name.as_bytes());
        hasher.update((bytes.len() as u64).to_be_bytes());
        hasher.update(bytes);
    }
    Ok(hex::encode(hasher.finalize()))
}

fn emit(value: serde_json::Value) -> Result<()> {
    println!("{}", serde_json::to_string(&value)?);
    io::stdout().flush()?;
    Ok(())
}

fn unix_time_ms() -> Result<u128> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system time is before Unix epoch")?
        .as_millis())
}

fn parse_u128(value: impl AsRef<str>, name: &str) -> Result<u128> {
    value
        .as_ref()
        .parse::<u128>()
        .with_context(|| format!("invalid {name}"))
}

fn parse_optional_u128(value: Option<&str>, name: &str) -> Result<Option<u128>> {
    value
        .filter(|value| !value.is_empty())
        .map(|value| parse_u128(value, name))
        .transpose()
}

fn format_prove(value: u128) -> String {
    let whole = value / PROVE_SCALE;
    let fractional = format!("{:018}", value % PROVE_SCALE);
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
    fn buffered_defaults_stay_within_the_approved_ceiling() {
        let base_fee = 444_839_000_000_000_000u128;
        let maximum = base_fee + 4_783_740_216u128 * 700_000_000u128;
        assert_eq!(format_prove(maximum), "3.7934571512");
        assert!(maximum <= 3_810_054_280_210_000_000u128);
    }
}
