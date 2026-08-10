use anyhow::{Context, Result};
use clap::Parser;
use prost::Message;
use serde_json::json;
use sha2::{Digest, Sha256};
use sp1_sdk::{
    network::{
        get_default_cycle_limit_for_mode, get_default_rpc_url_for_mode,
        proto::{
            artifact::{
                artifact_store_client::ArtifactStoreClient, ArtifactType, CreateArtifactRequest,
            },
            auction_network::prover_network_client::ProverNetworkClient,
            auction_types::{
                FulfillmentStrategy, GetProversByUptimeRequest, MessageFormat, ProofMode,
                ProofRequest, RequestProofRequest, RequestProofRequestBody, RequestProofResponse,
                TransactionVariant,
            },
            GetFilteredProofRequestsResponse, GetProofRequestParamsResponse,
        },
        signer::NetworkSigner,
        Address, NetworkClient, NetworkMode, B256,
    },
    HashableKey, Prover, ProverClient, ProvingKey, SP1ProofMode, SP1Stdin, SP1_CIRCUIT_VERSION,
};
use std::{
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tonic::transport::{Channel, ClientTlsConfig, Endpoint};
use zeko_sp1_lib::ZkappPublicValues;
use zkapp_script::{
    execute_minimal, settlement_stdin_from_bundle, SettlementProofBundle, SETTLEMENT_ELF,
};

const PROVE_SCALE: u128 = 1_000_000_000_000_000_000;
const FIXTURE_FILES: [&str; 4] = [
    "vk.serde.json",
    "proof.serde.json",
    "public_input_skeleton.json",
    "app_statement.json",
];
const RECOVERY_ATTEMPTS: usize = 5;

struct FixtureSnapshot {
    bundle: SettlementProofBundle,
    input_sha256: String,
}

struct SubmittedRequest {
    body: RequestProofRequestBody,
    requester: Address,
    created_after: u64,
}

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

    let fixture = fixture_snapshot(&args.fixture_dir)?;
    let input_sha256 = fixture.input_sha256.clone();
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

    let stdin = settlement_stdin_from_bundle(&fixture.bundle)?;
    let request_stdin = stdin.clone();
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

    let network_mode = NetworkMode::Mainnet;
    let private_key = std::env::var("NETWORK_PRIVATE_KEY")
        .context("NETWORK_PRIVATE_KEY is required with --request")?;
    let signer = NetworkSigner::local(&private_key).context("parse NETWORK_PRIVATE_KEY")?;
    let rpc_url = std::env::var("NETWORK_RPC_URL")
        .unwrap_or_else(|_| get_default_rpc_url_for_mode(network_mode));
    let client = ProverClient::builder()
        .network_for(network_mode)
        .signer(signer.clone())
        .rpc_url(&rpc_url)
        .build()
        .await;
    let request_client = NetworkClient::new(signer.clone(), &rpc_url, network_mode);
    let balance_before = parse_u128(client.get_balance().await?.to_string(), "network balance")?;
    let GetProofRequestParamsResponse::Auction(params) = client
        .get_proof_request_params(SP1ProofMode::Groth16)
        .await?
    else {
        anyhow::bail!("auction pricing is unavailable")
    };
    let base_fee = parse_u64(&params.base_fee, "network base fee")?;
    let network_max_price = parse_u64(&params.max_price_per_pgu, "network max price per PGU")?;
    let maximum_cost = maximum_auction_cost(base_fee, args.max_pgu, args.max_price_per_pgu);
    emit(json!({
        "event": "authorization_check",
        "timestampMs": unix_time_ms()?,
        "balanceAttoProve": balance_before.to_string(),
        "balanceProve": format_prove(balance_before),
        "baseFeeAttoProve": base_fee.to_string(),
        "baseFeeProve": format_prove(u128::from(base_fee)),
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

    let pk = client
        .setup(SETTLEMENT_ELF)
        .await
        .context("set up settlement program on network client")?;
    anyhow::ensure!(
        pk.verifying_key().bytes32().to_string() == program_vkey,
        "network client and local preflight program vkeys differ"
    );
    let vk_hash = client
        .register_program(pk.verifying_key(), pk.elf())
        .await
        .context("register settlement program")?;
    let auctioneer = parse_network_address(&params.auctioneer, "network auctioneer")?;
    let executor = parse_network_address(&params.executor, "network executor")?;
    let verifier = parse_network_address(&params.verifier, "network verifier")?;
    let treasury = parse_network_address(&params.treasury, "network treasury")?;
    let stdin_uri = upload_stdin(&rpc_url, &signer, &request_stdin).await?;
    let whitelist = get_prover_whitelist(&rpc_url).await?;
    let nonce = request_client
        .get_nonce()
        .await
        .context("read proof request nonce")?;

    let requested_at_ms = unix_time_ms()?;
    let deadline = unix_time_secs()?
        .checked_add(args.timeout_secs)
        .context("proof request deadline overflow")?;
    let request_started = Instant::now();
    let body = RequestProofRequestBody {
        nonce,
        vk_hash: vk_hash.to_vec(),
        version: format!("sp1-{SP1_CIRCUIT_VERSION}"),
        mode: ProofMode::Groth16.into(),
        strategy: FulfillmentStrategy::Auction.into(),
        stdin_uri,
        deadline,
        cycle_limit: get_default_cycle_limit_for_mode(network_mode),
        gas_limit: args.max_pgu,
        min_auction_period: args.min_auction_period_secs,
        whitelist,
        domain: params.domain,
        auctioneer: auctioneer.to_vec(),
        executor: executor.to_vec(),
        verifier: verifier.to_vec(),
        public_values_hash: None,
        base_fee: base_fee.to_string(),
        max_price_per_pgu: args.max_price_per_pgu.to_string(),
        variant: TransactionVariant::RequestVariant.into(),
        treasury: treasury.to_vec(),
    };
    let signature = signer
        .sign_message(&body.encode_to_vec())
        .await
        .context("sign fixed-nonce proof request")?
        .as_bytes()
        .to_vec();
    let signed_request = RequestProofRequest {
        format: MessageFormat::Binary.into(),
        signature,
        body: Some(body.clone()),
    };
    let submitted = SubmittedRequest {
        body,
        requester: signer.address(),
        created_after: (requested_at_ms / 1_000)
            .try_into()
            .context("request timestamp exceeds u64")?,
    };
    let (request_id, recovered_after_ambiguous_response) = match submit_proof_request(
        &rpc_url,
        signed_request,
    )
    .await
    {
        Ok(response) => (
            parse_request_id(
                response
                    .body
                    .as_ref()
                    .map(|body| body.request_id.as_slice())
                    .unwrap_or_default(),
            )?,
            false,
        ),
        Err(submission_error) => {
            let recovered = recover_request_id(&request_client, &submitted)
                .await
                .context("recover ambiguous proof request submission")?;
            let Some(request_id) = recovered else {
                return Err(submission_error).context(format!(
                        "proof request outcome is ambiguous for fixed nonce {nonce}; refusing to submit another paid request"
                    ));
            };
            (request_id, true)
        }
    };
    emit(json!({
        "event": "request_created",
        "timestampMs": unix_time_ms()?,
        "requestedAtMs": requested_at_ms,
        "requestId": request_id.to_string(),
        "requestNonce": nonce.to_string(),
        "recoveredAfterAmbiguousResponse": recovered_after_ambiguous_response,
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

fn fixture_snapshot(fixture_dir: &Path) -> Result<FixtureSnapshot> {
    let mut files = Vec::with_capacity(FIXTURE_FILES.len());
    for name in FIXTURE_FILES {
        let path = fixture_dir.join(name);
        files.push(fs::read(&path).with_context(|| format!("read {}", path.display()))?);
    }
    fixture_snapshot_from_bytes(files.try_into().expect("fixture file count is fixed"))
}

fn fixture_snapshot_from_bytes(files: [Vec<u8>; 4]) -> Result<FixtureSnapshot> {
    let mut hasher = Sha256::new();
    for (name, bytes) in FIXTURE_FILES.iter().zip(&files) {
        hasher.update((name.len() as u64).to_be_bytes());
        hasher.update(name.as_bytes());
        hasher.update((bytes.len() as u64).to_be_bytes());
        hasher.update(bytes);
    }
    let [vk, proof, skeleton, app_statement] = files;
    let decode = |bytes: Vec<u8>, name: &str| {
        String::from_utf8(bytes).with_context(|| format!("{name} is not valid UTF-8"))
    };
    Ok(FixtureSnapshot {
        bundle: SettlementProofBundle {
            vk_json: decode(vk, FIXTURE_FILES[0])?,
            proof_json: decode(proof, FIXTURE_FILES[1])?,
            public_input_skeleton_json: decode(skeleton, FIXTURE_FILES[2])?,
            app_statement_json: decode(app_statement, FIXTURE_FILES[3])?,
            binding: None,
            context: None,
            inner_action_batch: None,
            asset_registry_checkpoint: None,
            asset_registry_batch: None,
        },
        input_sha256: hex::encode(hasher.finalize()),
    })
}

fn network_endpoint(rpc_url: &str) -> Result<Endpoint> {
    let mut endpoint = Endpoint::from_shared(rpc_url.to_owned())?
        .timeout(Duration::from_secs(60))
        .connect_timeout(Duration::from_secs(15))
        .keep_alive_while_idle(true)
        .http2_keep_alive_interval(Duration::from_secs(15))
        .keep_alive_timeout(Duration::from_secs(15))
        .tcp_keepalive(Some(Duration::from_secs(60)))
        .tcp_nodelay(true);
    if rpc_url.starts_with("https://") {
        endpoint = endpoint.tls_config(ClientTlsConfig::new().with_enabled_roots())?;
    }
    Ok(endpoint)
}

async fn network_channel(rpc_url: &str) -> Result<Channel> {
    network_endpoint(rpc_url)?
        .connect()
        .await
        .context("connect to SP1 network")
}

async fn upload_stdin(rpc_url: &str, signer: &NetworkSigner, stdin: &SP1Stdin) -> Result<String> {
    let signature = signer
        .sign_message(b"create_artifact")
        .await
        .context("sign stdin artifact request")?;
    let signature = signature.as_bytes();
    anyhow::ensure!(signature.len() == 65, "invalid artifact signature length");
    let mut artifact_signature = signature[..64].to_vec();
    artifact_signature.push(
        signature[64]
            .checked_add(27)
            .context("invalid artifact recovery ID")?,
    );
    let mut client = ArtifactStoreClient::new(network_channel(rpc_url).await?);
    let response = client
        .create_artifact(CreateArtifactRequest {
            signature: artifact_signature,
            artifact_type: ArtifactType::Stdin.into(),
        })
        .await
        .context("create stdin artifact")?
        .into_inner();
    let serialized = bincode::serialize(stdin).context("serialize settlement stdin")?;
    let compressed =
        zstd::encode_all(serialized.as_slice(), 3).context("compress settlement stdin")?;
    let upload = reqwest::Client::new()
        .put(&response.artifact_presigned_url)
        .body(compressed)
        .send()
        .await
        .context("upload settlement stdin")?;
    anyhow::ensure!(
        upload.status().is_success(),
        "stdin upload failed with HTTP {}",
        upload.status()
    );
    Ok(response.artifact_uri)
}

async fn get_prover_whitelist(rpc_url: &str) -> Result<Vec<Vec<u8>>> {
    let mut client = ProverNetworkClient::new(network_channel(rpc_url).await?);
    Ok(client
        .get_provers_by_uptime(GetProversByUptimeRequest {
            high_availability_only: false,
        })
        .await
        .context("read SP1 prover whitelist")?
        .into_inner()
        .provers)
}

async fn submit_proof_request(
    rpc_url: &str,
    request: RequestProofRequest,
) -> Result<RequestProofResponse> {
    let mut client = ProverNetworkClient::new(network_channel(rpc_url).await?);
    Ok(client
        .request_proof(request)
        .await
        .context("submit fixed-nonce paid proof request")?
        .into_inner())
}

async fn recover_request_id(
    client: &NetworkClient,
    submitted: &SubmittedRequest,
) -> Result<Option<B256>> {
    for attempt in 0..RECOVERY_ATTEMPTS {
        let response = client
            .get_filtered_proof_requests(
                Some(submitted.body.version.clone()),
                None,
                None,
                Some(submitted.body.deadline.saturating_sub(1)),
                Some(submitted.body.vk_hash.clone()),
                Some(submitted.requester.to_vec()),
                None,
                Some(submitted.created_after.saturating_sub(300)),
                None,
                Some(100),
                Some(1),
                Some(submitted.body.mode),
                None,
                None,
                None,
                None,
            )
            .await?;
        let GetFilteredProofRequestsResponse::Auction(response) = response else {
            anyhow::bail!("auction request recovery is unavailable")
        };
        let matches = response
            .requests
            .iter()
            .filter(|request| request_matches_submission(request, submitted))
            .collect::<Vec<_>>();
        anyhow::ensure!(
            matches.len() <= 1,
            "multiple proof requests match fixed nonce {}",
            submitted.body.nonce
        );
        if let Some(request) = matches.first() {
            return parse_request_id(&request.request_id).map(Some);
        }
        if attempt + 1 < RECOVERY_ATTEMPTS {
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    }
    Ok(None)
}

fn request_matches_submission(request: &ProofRequest, submitted: &SubmittedRequest) -> bool {
    request.requester == submitted.requester.as_slice()
        && request.vk_hash == submitted.body.vk_hash
        && request.version == submitted.body.version
        && request.mode == submitted.body.mode
        && request.strategy == submitted.body.strategy
        && request.stdin_uri == submitted.body.stdin_uri
        && request.deadline == submitted.body.deadline
        && request.cycle_limit == submitted.body.cycle_limit
        && request.gas_limit == submitted.body.gas_limit
        && request.min_auction_period == submitted.body.min_auction_period
        && request.whitelist == submitted.body.whitelist
        && request.base_fee.as_deref() == Some(submitted.body.base_fee.as_str())
        && request.max_price_per_pgu.as_deref() == Some(submitted.body.max_price_per_pgu.as_str())
}

fn parse_request_id(bytes: &[u8]) -> Result<B256> {
    anyhow::ensure!(
        bytes.len() == 32,
        "network returned an invalid proof request ID"
    );
    Ok(B256::from_slice(bytes))
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

fn unix_time_secs() -> Result<u64> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system time is before Unix epoch")?
        .as_secs())
}

fn parse_u128(value: impl AsRef<str>, name: &str) -> Result<u128> {
    value
        .as_ref()
        .parse::<u128>()
        .with_context(|| format!("invalid {name}"))
}

fn parse_u64(value: impl AsRef<str>, name: &str) -> Result<u64> {
    value
        .as_ref()
        .parse::<u64>()
        .with_context(|| format!("invalid {name}"))
}

fn parse_network_address(bytes: &[u8], name: &str) -> Result<Address> {
    anyhow::ensure!(bytes.len() == 20, "invalid {name}");
    Ok(Address::from_slice(bytes))
}

fn maximum_auction_cost(base_fee: u64, max_pgu: u64, max_price_per_pgu: u64) -> u128 {
    u128::from(base_fee) + u128::from(max_pgu) * u128::from(max_price_per_pgu)
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
        let base_fee = 444_839_000_000_000_000u64;
        let maximum = maximum_auction_cost(base_fee, 4_783_740_216, 700_000_000);
        assert_eq!(format_prove(maximum), "3.7934571512");
        assert!(maximum <= 3_810_054_280_210_000_000u128);
    }

    #[test]
    fn fixture_snapshot_owns_the_approved_bytes() {
        let mut files = [
            b"vk-before".to_vec(),
            b"proof-before".to_vec(),
            b"skeleton-before".to_vec(),
            b"statement-before".to_vec(),
        ];
        let snapshot = fixture_snapshot_from_bytes(files.clone()).unwrap();
        files[1] = b"proof-after".to_vec();
        let mutated = fixture_snapshot_from_bytes(files).unwrap();

        assert_eq!(snapshot.bundle.proof_json, "proof-before");
        assert_ne!(snapshot.input_sha256, mutated.input_sha256);
    }

    #[test]
    fn recovery_matches_only_the_submitted_payload() {
        let body = RequestProofRequestBody {
            nonce: 7,
            vk_hash: vec![1; 32],
            version: "sp1-test".to_owned(),
            mode: ProofMode::Groth16.into(),
            strategy: FulfillmentStrategy::Auction.into(),
            stdin_uri: "stdin://approved".to_owned(),
            deadline: 99,
            cycle_limit: 100,
            gas_limit: 101,
            min_auction_period: 15,
            whitelist: vec![vec![2; 20]],
            domain: vec![3; 32],
            auctioneer: vec![4; 20],
            executor: vec![5; 20],
            verifier: vec![6; 20],
            public_values_hash: None,
            base_fee: "102".to_owned(),
            max_price_per_pgu: "103".to_owned(),
            variant: TransactionVariant::RequestVariant.into(),
            treasury: vec![7; 20],
        };
        let submitted = SubmittedRequest {
            body: body.clone(),
            requester: Address::repeat_byte(8),
            created_after: 1,
        };
        let mut request = ProofRequest::default();
        request.requester = submitted.requester.to_vec();
        request.vk_hash = body.vk_hash;
        request.version = body.version;
        request.mode = body.mode;
        request.strategy = body.strategy;
        request.stdin_uri = body.stdin_uri;
        request.deadline = body.deadline;
        request.cycle_limit = body.cycle_limit;
        request.gas_limit = body.gas_limit;
        request.min_auction_period = body.min_auction_period;
        request.whitelist = body.whitelist;
        request.base_fee = Some(body.base_fee);
        request.max_price_per_pgu = Some(body.max_price_per_pgu);

        assert!(request_matches_submission(&request, &submitted));
        request.stdin_uri = "stdin://unapproved".to_owned();
        assert!(!request_matches_submission(&request, &submitted));
    }
}
