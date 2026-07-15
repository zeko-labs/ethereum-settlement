use alloy::primitives::{keccak256, Address, B256, U256};
use anyhow::{Context, Result};
use axum::{
    extract::{Path, Query, State},
    http::{header, HeaderMap, HeaderValue, Method, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use sqlx::{postgres::PgPoolOptions, FromRow, PgPool, Row};
use std::{env, net::SocketAddr, sync::Arc, time::Duration};
use tokio::time::sleep;
use tower_http::{
    cors::{AllowOrigin, Any, CorsLayer},
    trace::TraceLayer,
};
use uuid::Uuid;
use zeko_sp1_lib::{
    BridgeDeposit, BridgeTransitionInput, EthereumBridgeState, SettlementContextV1,
    SettlementPublicValues, WithdrawTransitionInput, ZekoBridgeState,
};
use zkapp_script::SettlementProofBundle;

mod ethereum;
mod graphql;
mod indexer;
mod prover;

#[derive(Clone)]
struct AppState {
    pool: PgPool,
    api_key: Arc<str>,
    ethereum: ethereum::Ethereum,
    proof_system: Arc<str>,
    prover_config: prover::NetworkRequestConfig,
    network_explorer_base: Arc<str>,
    execute_only: bool,
    local_mock_submit: bool,
    require_proof_approval: bool,
    min_remaining_slots: u64,
    ethereum_finality_mode: indexer::FinalityMode,
    ethereum_confirmations: u64,
}

#[derive(Debug, Deserialize, Serialize)]
struct SettlementRequest {
    #[serde(rename = "schemaVersion")]
    schema_version: u16,
    #[serde(rename = "minaTransactionHash")]
    mina_transaction_hash: String,
    proof: SettlementProofBundle,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CreatedJob {
    id: Uuid,
    status: &'static str,
    status_url: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApproveProofRequest {
    input_digest: String,
    max_pgu: String,
    max_price_per_pgu: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProofQuoteResponse {
    id: Uuid,
    kind: String,
    status: String,
    input_digest: String,
    cycle_count: u64,
    remaining_slots: Option<u64>,
    quote: prover::AuctionQuote,
    note: &'static str,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct NativeWithdrawalProof {
    settlement_sequence: u64,
    offset: u32,
    global_action_index: u32,
    recipient: String,
    amount: String,
    action_fields_hash: String,
    siblings: Vec<String>,
    inner_action_root: String,
    commit_slot_upper: u32,
    claimable_slot: u64,
    current_virtual_slot: u64,
    recipient_cursor: u32,
    status: String,
    next_action: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct NativeDepositStatus {
    nonce: u64,
    token: String,
    sender: String,
    zeko_recipient: String,
    ethereum_amount: String,
    zeko_amount: String,
    timeout: u64,
    ethereum_transaction_hash: String,
    ethereum_finalized: bool,
    bridge_job_id: Option<Uuid>,
    bridge_job_status: Option<String>,
    outer_action_sequence: Option<u32>,
    outer_action_state_after: Option<String>,
    synchronized_settlement_sequence: Option<u64>,
    status: String,
    next_action: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListDepositsQuery {
    zeko_recipient: Option<String>,
    after: Option<u64>,
    limit: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct ListWithdrawalsQuery {
    recipient: Option<String>,
    after: Option<u64>,
    limit: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct ListJobsQuery {
    kind: Option<String>,
    status: Option<String>,
    limit: Option<i64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProofQuoteQuery {
    max_pgu: Option<String>,
    max_price_per_pgu: Option<String>,
}

#[derive(Debug, Serialize, FromRow)]
#[serde(rename_all = "camelCase")]
struct ProofJob {
    id: Uuid,
    kind: String,
    status: String,
    input: Value,
    public_values: Option<String>,
    proof_request_id: Option<String>,
    transaction_hash: Option<String>,
    error: Option<String>,
    attempts: i32,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    started_at: Option<DateTime<Utc>>,
    completed_at: Option<DateTime<Utc>>,
    input_digest: String,
    preflight_input_digest: Option<String>,
    cycle_count: Option<i64>,
    prover_gas: Option<i64>,
    base_fee_prove: Option<String>,
    max_price_per_pgu: Option<String>,
    actual_cost_prove: Option<String>,
    ethereum_gas_used: Option<i64>,
    confirmations: i32,
    explorer_url: Option<String>,
    approval_input_digest: Option<String>,
    approval_max_pgu: Option<i64>,
    approval_max_price_per_pgu: Option<i64>,
    approval_base_fee_atto_prove: Option<String>,
    approval_network_max_price_per_pgu: Option<String>,
    approval_max_cost_atto_prove: Option<String>,
    approved_at: Option<DateTime<Utc>>,
}

#[derive(Debug, FromRow)]
struct ClaimedJob {
    id: Uuid,
    kind: String,
    input: Value,
    proof_request_id: Option<String>,
    claimed_status: String,
    public_values: Option<String>,
    cycle_count: Option<i64>,
    approval_max_pgu: Option<i64>,
    approval_max_price_per_pgu: Option<i64>,
}

#[tokio::main]
async fn main() -> Result<()> {
    dotenv::dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "zeko_proof_api=info,tower_http=info".into()),
        )
        .init();

    let database_url = required_env("DATABASE_URL")?;
    let api_key: Arc<str> = required_env("PROOF_API_KEY")?.into();
    let default_key = env::var("ETHEREUM_PRIVATE_KEY").unwrap_or_default();
    let ethereum = ethereum::Ethereum::new(
        required_env("RPC_URL")?,
        required_env("SETTLEMENT_CONTRACT_ADDRESS")?,
        required_env("BRIDGE_CONTRACT_ADDRESS")?,
        nonempty_env("SETTLEMENT_PRIVATE_KEY").unwrap_or_else(|| default_key.clone()),
        nonempty_env("BRIDGE_PRIVATE_KEY").unwrap_or_else(|| default_key.clone()),
        nonempty_env("WITHDRAW_PRIVATE_KEY").unwrap_or(default_key),
    )?;
    let ethereum_finality_mode = indexer::FinalityMode::parse(
        &env::var("ETHEREUM_FINALITY_MODE").unwrap_or_else(|_| "finalized".to_owned()),
    )?;
    validate_finality_mode(ethereum_finality_mode, ethereum.chain_id().await?)?;
    let proof_system: Arc<str> = env::var("PROOF_SYSTEM")
        .unwrap_or_else(|_| "groth16".to_owned())
        .into();
    let execute_only = bool_env("API_EXECUTE_ONLY")?;
    let local_mock_submit = bool_env("API_LOCAL_MOCK_SUBMIT")?;
    let require_proof_approval = bool_env("API_REQUIRE_PROOF_APPROVAL")?;
    anyhow::ensure!(
        !(execute_only && local_mock_submit),
        "API_EXECUTE_ONLY and API_LOCAL_MOCK_SUBMIT are mutually exclusive"
    );
    anyhow::ensure!(
        !(require_proof_approval && (execute_only || local_mock_submit)),
        "API_REQUIRE_PROOF_APPROVAL is only valid for network proving"
    );
    if local_mock_submit {
        ethereum.ensure_local_mock_verifiers().await?;
    }
    validate_program_vkeys(&ethereum).await?;
    let prover_config = prover::NetworkRequestConfig {
        timeout: Duration::from_secs(u64_env("PROVER_TIMEOUT_SECS", 21_600)?),
        min_auction_period: u64_env("PROVER_MIN_AUCTION_PERIOD_SECS", 15)?,
        gas_limit: optional_u64_env("PROVER_GAS_LIMIT")?,
        max_price_per_pgu: optional_u64_env("PROVER_MAX_PRICE_PER_PGU")?,
    };
    if require_proof_approval {
        anyhow::ensure!(
            prover_config.gas_limit.is_some() && prover_config.max_price_per_pgu.is_some(),
            "API_REQUIRE_PROOF_APPROVAL requires PROVER_GAS_LIMIT and PROVER_MAX_PRICE_PER_PGU hard caps"
        );
    }
    let network_explorer_base: Arc<str> = env::var("PROVER_EXPLORER_BASE_URL")
        .unwrap_or_else(|_| "https://explorer.succinct.xyz/request".to_owned())
        .trim_end_matches('/')
        .to_owned()
        .into();
    let bind: SocketAddr = env::var("API_BIND")
        .unwrap_or_else(|_| "127.0.0.1:8080".to_owned())
        .parse()
        .context("invalid API_BIND")?;

    let pool = PgPoolOptions::new()
        .max_connections(10)
        .connect(&database_url)
        .await
        .context("connect to PostgreSQL")?;
    sqlx::migrate!()
        .run(&pool)
        .await
        .context("run migrations")?;
    initialize_gateway_config(&pool).await?;
    sqlx::query(
        "UPDATE proof_jobs
         SET status = CASE
               WHEN approved_at IS NOT NULL THEN 'approved'::proof_status
               ELSE 'queued'::proof_status
             END,
             error = 'worker restarted before completion', updated_at = NOW()
         WHERE status IN ('validating', 'proof_requested', 'proving', 'submitting')",
    )
    .execute(&pool)
    .await
    .context("recover interrupted jobs")?;
    let ethereum_confirmations = u64_env("ETHEREUM_CONFIRMATIONS", 12)?;
    let state = AppState {
        pool,
        api_key,
        ethereum,
        proof_system,
        prover_config,
        network_explorer_base,
        execute_only,
        local_mock_submit,
        require_proof_approval,
        min_remaining_slots: u64_env("PROVER_MIN_REMAINING_SLOTS", 1_900)?,
        ethereum_finality_mode,
        ethereum_confirmations,
    };
    let worker_state = state.clone();
    tokio::spawn(async move { worker_loop(worker_state).await });
    let indexer_config = indexer::Config {
        start_block: optional_u64_env("ETHEREUM_INDEXER_START_BLOCK")?,
        finality_mode: ethereum_finality_mode,
        confirmations: ethereum_confirmations,
        poll_interval: Duration::from_secs(u64_env("ETHEREUM_POLL_INTERVAL_SECS", 3)?),
    };
    let indexer_pool = state.pool.clone();
    let indexer_ethereum = state.ethereum.clone();
    tokio::spawn(async move { indexer::run(indexer_pool, indexer_ethereum, indexer_config).await });
    if bool_env("BRIDGE_AUTO_PROVE_DEPOSITS")? {
        let auto_bridge_state = state.clone();
        let interval = Duration::from_secs(u64_env("BRIDGE_AUTO_PROVE_POLL_SECS", 5)?);
        tokio::spawn(
            async move { automatic_deposit_batch_loop(auto_bridge_state, interval).await },
        );
    }

    let protected = Router::new()
        .route("/v1/proofs/settlement", post(create_settlement))
        .route("/v1/settlements", post(create_settlement))
        .route("/v1/proofs/bridge", post(create_bridge))
        .route("/v1/bridge/deposits/prove", post(create_deposit_batch))
        .route("/v1/proofs/withdraw", post(create_withdraw))
        .route("/v1/proofs/:id/quote", get(get_proof_quote))
        .route("/v1/proofs/:id/approve", post(approve_proof))
        .route("/v1/proofs/:id/cancel", post(cancel_proof))
        .route("/v1/proofs", get(list_jobs))
        .route("/v1/proofs/:id", get(get_job))
        .route_layer(middleware::from_fn_with_state(state.clone(), authenticate));

    let app = Router::new()
        .route("/health", get(health))
        .route("/graphql", post(graphql::handle))
        .route("/v1/bridge/config", get(get_bridge_config))
        .route(
            "/v1/bridge/withdrawals/:sequence/:offset",
            get(get_native_withdrawal_proof),
        )
        .route("/v1/bridge/withdrawals", get(list_native_withdrawals))
        .route("/v1/bridge/deposits", get(list_native_deposits))
        .route("/v1/bridge/deposits/:nonce", get(get_native_deposit))
        .merge(protected)
        .layer(cors_layer()?)
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(bind).await?;
    tracing::info!(
        %bind,
        execute_only,
        local_mock_submit,
        require_proof_approval,
        "proof API listening"
    );
    axum::serve(listener, app).await?;
    Ok(())
}

async fn health(State(state): State<AppState>) -> Response {
    let database = sqlx::query_scalar::<_, i32>("SELECT 1")
        .fetch_one(&state.pool)
        .await;
    let chain_id = state.ethereum.chain_id().await;
    let virtual_slot = state.ethereum.current_virtual_slot().await;
    match (database, chain_id, virtual_slot) {
        (Ok(1), Ok(chain_id), Ok(virtual_slot)) => Json(serde_json::json!({
            "status": "ok",
            "database": "ok",
            "ethereum": "ok",
            "chainId": chain_id,
            "currentVirtualSlot": virtual_slot
        }))
        .into_response(),
        (database, chain_id, virtual_slot) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({
                "status": "unavailable",
                "database": database.err().map(|error| error.to_string()),
                "ethereum": chain_id.err().map(|error| error.to_string()),
                "settlement": virtual_slot.err().map(|error| error.to_string())
            })),
        )
            .into_response(),
    }
}

async fn get_bridge_config(State(state): State<AppState>) -> Response {
    let values = tokio::join!(
        state.ethereum.chain_id(),
        state.ethereum.withdrawal_delay_slots(),
        state.ethereum.current_virtual_slot()
    );
    match values {
        (Ok(chain_id), Ok(withdrawal_delay_slots), Ok(current_virtual_slot)) => {
            Json(serde_json::json!({
                "schemaVersion": 1,
                "chainId": chain_id,
                "bridgeAddress": state.ethereum.bridge_address().to_string(),
                "settlementAddress": state.ethereum.settlement_address().to_string(),
                "ethereumDecimals": 18,
                "zekoNativeDecimals": 9,
                "ethereumFinalityMode": state.ethereum_finality_mode.as_str(),
                "ethereumConfirmations": state.ethereum_confirmations,
                "withdrawalDelaySlots": withdrawal_delay_slots,
                "currentVirtualSlot": current_virtual_slot
            }))
            .into_response()
        }
        (chain_id, delay, slot) => {
            tracing::error!(
                chain_id = ?chain_id.err(),
                withdrawal_delay = ?delay.err(),
                virtual_slot = ?slot.err(),
                "read public bridge configuration"
            );
            api_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "could not read bridge configuration",
            )
        }
    }
}

async fn validate_program_vkeys(ethereum: &ethereum::Ethereum) -> Result<()> {
    let configured = ethereum.configured_program_vkeys().await?;
    for (index, kind) in ["settlement", "bridge", "withdraw"].iter().enumerate() {
        let embedded = prover::program_vkey(kind)
            .await?
            .parse::<B256>()
            .with_context(|| format!("invalid embedded {kind} program vkey"))?;
        anyhow::ensure!(
            embedded == configured[index],
            "embedded {kind} program vkey {embedded} does not match contract {}",
            configured[index]
        );
    }
    Ok(())
}

async fn authenticate(
    State(state): State<AppState>,
    headers: HeaderMap,
    request: axum::extract::Request,
    next: Next,
) -> Response {
    let supplied = headers
        .get("x-api-key")
        .and_then(|value| value.to_str().ok());
    if supplied != Some(state.api_key.as_ref()) {
        return (StatusCode::UNAUTHORIZED, "invalid API key").into_response();
    }
    next.run(request).await
}

async fn create_settlement(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(mut request): Json<SettlementRequest>,
) -> Response {
    if request.schema_version != 1 {
        return api_error(
            StatusCode::BAD_REQUEST,
            "unsupported settlement schemaVersion",
        );
    }
    if !is_bytes32_hex(&request.mina_transaction_hash) {
        return api_error(
            StatusCode::BAD_REQUEST,
            "minaTransactionHash must be a 32-byte 0x-prefixed hex value",
        );
    }
    if request.proof.binding.is_none() {
        return api_error(
            StatusCode::BAD_REQUEST,
            "settlement proof must include the OCaml account-update binding",
        );
    }
    match conflicting_outer_writer(&state.pool, "settlement").await {
        Ok(false) => {}
        Ok(true) => {
            return api_error(
                StatusCode::CONFLICT,
                "a bridge batch is queued or active; retry after it is finalized",
            );
        }
        Err(error) => {
            tracing::error!(%error, "check bridge writer before settlement");
            return api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "could not check the outer action-state queue",
            );
        }
    }
    // Ethereum-domain context is assigned when the worker claims this job.
    // This lets the sequencer queue later OCaml commits while an earlier
    // settlement is still proving without binding them to a stale L1 batch.
    request.proof.context = None;
    if let Some(batch) = &mut request.proof.inner_action_batch {
        batch.bridge_address = state
            .ethereum
            .bridge_address()
            .as_slice()
            .try_into()
            .expect("Ethereum address length");
    }
    create_job(
        &state,
        &headers,
        "settlement",
        serde_json::to_value(request).unwrap(),
    )
    .await
}

async fn hydrate_settlement_context(
    state: &AppState,
    proof: &mut SettlementProofBundle,
    mina_transaction_hash: &str,
) -> Result<()> {
    let chain = state.ethereum.settlement_state().await?;
    let chain_id = state.ethereum.chain_id().await?;
    let hash: [u8; 32] = hex::decode(
        mina_transaction_hash
            .strip_prefix("0x")
            .context("mina transaction hash must be 0x-prefixed")?,
    )?
    .try_into()
    .map_err(|_| anyhow::anyhow!("mina transaction hash must be 32 bytes"))?;
    let settlement_contract: [u8; 20] = state
        .ethereum
        .settlement_address()
        .as_slice()
        .try_into()
        .expect("Ethereum address is 20 bytes");
    let context = SettlementContextV1 {
        chain_id,
        settlement_contract,
        batch_sequence: chain
            .batch_sequence
            .checked_add(1)
            .context("settlement batch sequence overflow")?,
        mina_transaction_hash: hash,
        outer_action_state_length_before: chain.outer_action_state_length,
    };
    if let Some(supplied) = &proof.context {
        anyhow::ensure!(supplied == &context, "stale settlement Ethereum context");
    }
    proof.context = Some(context);
    Ok(())
}

async fn create_bridge(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<BridgeTransitionInput>,
) -> Response {
    match conflicting_outer_writer(&state.pool, "bridge").await {
        Ok(false) => {}
        Ok(true) => {
            return api_error(
                StatusCode::CONFLICT,
                "a settlement is queued or active; retry after it is finalized",
            );
        }
        Err(error) => {
            tracing::error!(%error, "check settlement writer before bridge");
            return api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "could not check the outer action-state queue",
            );
        }
    }
    create_job(
        &state,
        &headers,
        "bridge",
        serde_json::to_value(input).unwrap(),
    )
    .await
}

/// Builds the bridge proof input exclusively from canonical Ethereum logs.
/// This is the PoC production endpoint; `/v1/proofs/bridge` is retained only
/// for explicit fixture/debug inputs.
async fn create_deposit_batch(State(state): State<AppState>, headers: HeaderMap) -> Response {
    match conflicting_outer_writer(&state.pool, "bridge").await {
        Ok(false) => {}
        Ok(true) => {
            return api_error(
                StatusCode::CONFLICT,
                "a settlement is queued or active; retry after it is finalized",
            );
        }
        Err(error) => {
            tracing::error!(%error, "check settlement writer before bridge");
            return api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "could not check the outer action-state queue",
            );
        }
    }
    match queue_canonical_deposit_batch(&state, &headers, true).await {
        Ok(job) => (StatusCode::ACCEPTED, Json(job)).into_response(),
        Err(error) => api_error(StatusCode::CONFLICT, &error.to_string()),
    }
}

async fn queue_canonical_deposit_batch(
    state: &AppState,
    headers: &HeaderMap,
    allow_terminal_retry: bool,
) -> Result<CreatedJob> {
    let input = canonical_deposit_batch(state).await?;
    let first_nonce = input
        .ethereum
        .deposit_nonce
        .checked_add(1)
        .context("bridge deposit nonce overflow")?;
    let last_nonce = input
        .ethereum
        .deposit_nonce
        .checked_add(u64::try_from(input.deposits.len())?)
        .context("bridge deposit nonce overflow")?;
    let input = serde_json::to_value(input).expect("serialize bridge input");
    let input_digest = proof_input_digest(&input);
    let idempotency_key = headers
        .get("idempotency-key")
        .and_then(|value| value.to_str().ok());
    let id = Uuid::new_v4();
    let mut tx = state.pool.begin().await?;
    let eligible = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*)
         FROM gateway_bridge_deposits deposits
         LEFT JOIN proof_jobs previous ON previous.id = deposits.bridge_job_id
         WHERE deposits.nonce BETWEEN $1 AND $2 AND NOT deposits.removed
           AND (
             deposits.bridge_job_id IS NULL
             OR deposits.bridge_job_id = $3
             OR ($4 AND previous.status IN (
               'executed', 'failed', 'proof_failed', 'ethereum_reverted',
               'reorged', 'rejected'
             ))
           )",
    )
    .bind(i64::try_from(first_nonce)?)
    .bind(i64::try_from(last_nonce)?)
    .bind(id)
    .bind(allow_terminal_retry)
    .fetch_one(&mut *tx)
    .await?;
    anyhow::ensure!(
        eligible == i64::try_from(last_nonce - first_nonce + 1)?,
        "deposit batch already has a proof job; an operator must retry a failed batch"
    );
    let actual_id = sqlx::query_scalar::<_, Uuid>(
        "INSERT INTO proof_jobs (id, kind, input, idempotency_key, input_digest)
         VALUES ($1, 'bridge', $2, $3, $4)
         ON CONFLICT (idempotency_key) WHERE idempotency_key IS NOT NULL
         DO UPDATE SET idempotency_key = EXCLUDED.idempotency_key
         WHERE proof_jobs.input_digest = EXCLUDED.input_digest
         RETURNING id",
    )
    .bind(id)
    .bind(&input)
    .bind(idempotency_key)
    .bind(input_digest)
    .fetch_optional(&mut *tx)
    .await?
    .context("idempotency key already exists with a different payload")?;
    let updated = sqlx::query(
        "UPDATE gateway_bridge_deposits
         SET bridge_job_id = $1
         WHERE nonce BETWEEN $2 AND $3 AND NOT removed",
    )
    .bind(actual_id)
    .bind(i64::try_from(first_nonce)?)
    .bind(i64::try_from(last_nonce)?)
    .execute(&mut *tx)
    .await?;
    anyhow::ensure!(
        updated.rows_affected() == last_nonce - first_nonce + 1,
        "canonical deposit batch changed while it was queued"
    );
    tx.commit().await?;
    Ok(CreatedJob {
        id: actual_id,
        status: "queued",
        status_url: format!("/v1/proofs/{actual_id}"),
    })
}

async fn automatic_deposit_batch_loop(state: AppState, interval: Duration) {
    loop {
        match conflicting_outer_writer(&state.pool, "bridge").await {
            Ok(false) => {
                match queue_canonical_deposit_batch(&state, &HeaderMap::new(), false).await {
                    Ok(job) => {
                        tracing::info!(job_id = %job.id, "automatically queued deposit proof batch")
                    }
                    Err(error) => {
                        tracing::debug!(%error, "no automatic deposit proof batch queued")
                    }
                }
            }
            Ok(true) => {}
            Err(error) => {
                tracing::warn!(%error, "could not check for an active outer-state writer")
            }
        }
        sleep(interval).await;
    }
}

async fn canonical_deposit_batch(state: &AppState) -> Result<BridgeTransitionInput> {
    let (bridge, _historical) = state.ethereum.bridge_state("bridge", None, None).await?;
    anyhow::ensure!(!bridge.paused, "bridge contract is paused");
    anyhow::ensure!(
        bridge.deposit_nonce > bridge.bridged_deposit_nonce,
        "there are no unbridged deposits"
    );
    let (_, historical) = state
        .ethereum
        .bridge_state("bridge", Some(bridge.bridged_deposit_nonce), None)
        .await?;
    let historical = historical.context("missing bridged deposit checkpoint")?;
    let settlement = state.ethereum.settlement_state().await?;
    let rows = sqlx::query(
        "SELECT nonce, old_deposit_state, new_deposit_state, token,
                zeko_recipient, ethereum_amount::text AS ethereum_amount,
                zeko_amount::text AS zeko_amount, timeout
         FROM gateway_bridge_deposits deposits
         JOIN gateway_blocks blocks
           ON blocks.block_number = deposits.ethereum_block_number
          AND blocks.block_hash = deposits.ethereum_block_hash
         WHERE NOT deposits.removed AND blocks.canonical AND blocks.finalized
           AND nonce > $1 AND nonce <= $2
         ORDER BY nonce",
    )
    .bind(i64::try_from(bridge.bridged_deposit_nonce)?)
    .bind(i64::try_from(bridge.deposit_nonce)?)
    .fetch_all(&state.pool)
    .await?;
    anyhow::ensure!(
        rows.len() == usize::try_from(bridge.deposit_nonce - bridge.bridged_deposit_nonce)?,
        "canonical deposit log range is incomplete"
    );

    let mut expected_nonce = bridge.bridged_deposit_nonce + 1;
    let mut expected_state = historical;
    let mut deposits = Vec::with_capacity(rows.len());
    for row in rows {
        let nonce = u64::try_from(row.try_get::<i64, _>("nonce")?)?;
        anyhow::ensure!(
            nonce == expected_nonce,
            "deposit nonce range is not contiguous"
        );
        let old_state: alloy::primitives::B256 = row
            .try_get::<String, _>("old_deposit_state")?
            .parse()
            .context("invalid indexed old deposit state")?;
        let new_state: alloy::primitives::B256 = row
            .try_get::<String, _>("new_deposit_state")?
            .parse()
            .context("invalid indexed new deposit state")?;
        anyhow::ensure!(
            old_state == expected_state,
            "deposit accumulator is discontinuous"
        );
        let token: alloy::primitives::Address = row
            .try_get::<String, _>("token")?
            .parse()
            .context("invalid indexed token")?;
        anyhow::ensure!(
            token.is_zero(),
            "only canonical native deposits are batchable"
        );
        let timeout = u64::try_from(row.try_get::<i64, _>("timeout")?)?;
        anyhow::ensure!(
            timeout == u64::from(u32::MAX),
            "deposit does not use the no-cancellation timeout"
        );
        let ethereum_amount =
            U256::from_str_radix(&row.try_get::<String, _>("ethereum_amount")?, 10)?;
        let zeko_amount = U256::from_str_radix(&row.try_get::<String, _>("zeko_amount")?, 10)?;
        anyhow::ensure!(
            ethereum_amount == zeko_amount * U256::from(1_000_000_000u64),
            "indexed native amount normalization mismatch"
        );
        let recipient: alloy::primitives::B256 = row
            .try_get::<String, _>("zeko_recipient")?
            .parse()
            .context("invalid indexed Zeko recipient")?;
        deposits.push(BridgeDeposit {
            amount: ethereum_amount.to_be_bytes(),
            zeko_recipient: recipient.0,
        });
        expected_nonce += 1;
        expected_state = new_state;
    }
    anyhow::ensure!(
        expected_state == bridge.current_deposit_state,
        "indexed deposit accumulator does not reach bridge state"
    );

    Ok(BridgeTransitionInput {
        ethereum: EthereumBridgeState {
            chain_id: state.ethereum.chain_id().await?,
            bridge_address: state
                .ethereum
                .bridge_address()
                .as_slice()
                .try_into()
                .expect("Ethereum address length"),
            deposit_nonce: bridge.bridged_deposit_nonce,
            deposit_state: historical.0,
            withdraw_state: bridge.current_withdraw_state.0,
        },
        zeko: ZekoBridgeState {
            action_state: settlement.action_state.0,
            action_state_length: settlement.outer_action_state_length,
        },
        deposits,
    })
}

async fn get_native_withdrawal_proof(
    State(state): State<AppState>,
    Path((sequence, offset)): Path<(u64, u32)>,
) -> Response {
    match load_native_withdrawal_proof(&state, sequence, offset).await {
        Ok(proof) => Json(proof).into_response(),
        Err(error) => api_error(StatusCode::NOT_FOUND, &error.to_string()),
    }
}

async fn get_native_deposit(State(state): State<AppState>, Path(nonce): Path<u64>) -> Response {
    let row = sqlx::query(
        "SELECT deposits.nonce, deposits.token, deposits.sender,
                deposits.zeko_recipient,
                deposits.ethereum_amount::text AS ethereum_amount,
                deposits.zeko_amount::text AS zeko_amount, deposits.timeout,
                deposits.ethereum_tx_hash, blocks.finalized,
                deposits.bridge_job_id, bridge_jobs.status::text AS bridge_job_status,
                deposits.outer_action_sequence, deposits.outer_action_state_after,
                deposits.synchronized_settlement_sequence
         FROM gateway_bridge_deposits deposits
         JOIN gateway_blocks blocks
           ON blocks.block_number = deposits.ethereum_block_number
          AND blocks.block_hash = deposits.ethereum_block_hash
         LEFT JOIN proof_jobs bridge_jobs ON bridge_jobs.id = deposits.bridge_job_id
         WHERE deposits.nonce = $1 AND NOT deposits.removed AND blocks.canonical",
    )
    .bind(match i64::try_from(nonce) {
        Ok(nonce) => nonce,
        Err(_) => return api_error(StatusCode::BAD_REQUEST, "deposit nonce is too large"),
    })
    .fetch_optional(&state.pool)
    .await;
    match row {
        Ok(Some(row)) => match native_deposit_from_row(row) {
            Ok(deposit) => Json(deposit).into_response(),
            Err(error) => {
                tracing::error!(%error, nonce, "decode indexed bridge deposit");
                api_error(StatusCode::INTERNAL_SERVER_ERROR, "invalid indexed deposit")
            }
        },
        Ok(None) => api_error(StatusCode::NOT_FOUND, "canonical deposit not found"),
        Err(error) => {
            tracing::error!(%error, nonce, "read indexed bridge deposit");
            api_error(StatusCode::INTERNAL_SERVER_ERROR, "could not read deposit")
        }
    }
}

async fn list_native_deposits(
    State(state): State<AppState>,
    Query(query): Query<ListDepositsQuery>,
) -> Response {
    let zeko_recipient = match query.zeko_recipient {
        Some(recipient) => match recipient.parse::<B256>() {
            Ok(recipient) => Some(recipient.to_string()),
            Err(_) => {
                return api_error(
                    StatusCode::BAD_REQUEST,
                    "zekoRecipient must be a 32-byte packed Zeko address",
                )
            }
        },
        None => None,
    };
    let after = match query.after.map(i64::try_from).transpose() {
        Ok(after) => after,
        Err(_) => return api_error(StatusCode::BAD_REQUEST, "after is too large"),
    };
    let limit = query.limit.unwrap_or(20).clamp(1, 100);
    let rows = sqlx::query(
        "SELECT deposits.nonce, deposits.token, deposits.sender,
                deposits.zeko_recipient,
                deposits.ethereum_amount::text AS ethereum_amount,
                deposits.zeko_amount::text AS zeko_amount, deposits.timeout,
                deposits.ethereum_tx_hash, blocks.finalized,
                deposits.bridge_job_id, bridge_jobs.status::text AS bridge_job_status,
                deposits.outer_action_sequence, deposits.outer_action_state_after,
                deposits.synchronized_settlement_sequence
         FROM gateway_bridge_deposits deposits
         JOIN gateway_blocks blocks
           ON blocks.block_number = deposits.ethereum_block_number
          AND blocks.block_hash = deposits.ethereum_block_hash
         LEFT JOIN proof_jobs bridge_jobs ON bridge_jobs.id = deposits.bridge_job_id
         WHERE NOT deposits.removed AND blocks.canonical
           AND ($1::text IS NULL OR lower(deposits.zeko_recipient) = lower($1))
           AND ($2::bigint IS NULL OR deposits.nonce > $2)
         ORDER BY deposits.nonce
         LIMIT $3",
    )
    .bind(zeko_recipient)
    .bind(after)
    .bind(limit)
    .fetch_all(&state.pool)
    .await;
    match rows {
        Ok(rows) => match rows
            .into_iter()
            .map(native_deposit_from_row)
            .collect::<Result<Vec<_>>>()
        {
            Ok(deposits) => Json(deposits).into_response(),
            Err(error) => {
                tracing::error!(%error, "decode indexed bridge deposits");
                api_error(StatusCode::INTERNAL_SERVER_ERROR, "invalid indexed deposit")
            }
        },
        Err(error) => {
            tracing::error!(%error, "list indexed bridge deposits");
            api_error(StatusCode::INTERNAL_SERVER_ERROR, "could not list deposits")
        }
    }
}

fn native_deposit_from_row(row: sqlx::postgres::PgRow) -> Result<NativeDepositStatus> {
    let finalized: bool = row.try_get("finalized")?;
    let bridge_job_id: Option<Uuid> = row.try_get("bridge_job_id")?;
    let bridge_job_status: Option<String> = row.try_get("bridge_job_status")?;
    let outer_action_sequence = row
        .try_get::<Option<i64>, _>("outer_action_sequence")?
        .map(u32::try_from)
        .transpose()?;
    let synchronized_settlement_sequence = row
        .try_get::<Option<i64>, _>("synchronized_settlement_sequence")?
        .map(u64::try_from)
        .transpose()?;
    let (status, next_action) = native_deposit_progress(
        finalized,
        bridge_job_status.as_deref(),
        outer_action_sequence.is_some(),
        synchronized_settlement_sequence.is_some(),
    );
    Ok(NativeDepositStatus {
        nonce: u64::try_from(row.try_get::<i64, _>("nonce")?)?,
        token: row.try_get("token")?,
        sender: row.try_get("sender")?,
        zeko_recipient: row.try_get("zeko_recipient")?,
        ethereum_amount: row.try_get("ethereum_amount")?,
        zeko_amount: row.try_get("zeko_amount")?,
        timeout: u64::try_from(row.try_get::<i64, _>("timeout")?)?,
        ethereum_transaction_hash: row.try_get("ethereum_tx_hash")?,
        ethereum_finalized: finalized,
        bridge_job_id,
        bridge_job_status,
        outer_action_sequence,
        outer_action_state_after: row.try_get("outer_action_state_after")?,
        synchronized_settlement_sequence,
        status: status.to_owned(),
        next_action: next_action.to_owned(),
    })
}

fn native_deposit_progress(
    finalized: bool,
    bridge_job_status: Option<&str>,
    bridge_proven: bool,
    synchronized: bool,
) -> (&'static str, &'static str) {
    if !finalized {
        ("confirming", "waitForEthereumFinality")
    } else if synchronized {
        ("synchronized", "finalizeDepositOnZeko")
    } else if bridge_proven {
        ("bridgeProven", "waitForSettlementSynchronization")
    } else {
        match bridge_job_status {
            None => ("locked", "requestBridgeProof"),
            Some("queued" | "validating") => ("proofQueued", "waitForBridgeProof"),
            Some("awaiting_approval") => ("awaitingProofApproval", "waitForOperatorApproval"),
            Some("approved" | "proof_requested" | "proving") => ("proving", "waitForBridgeProof"),
            Some("submitting" | "submitted") => ("submitting", "waitForEthereumSubmission"),
            Some("executed") => ("executed", "operatorSubmissionRequired"),
            Some("failed" | "proof_failed" | "ethereum_reverted" | "reorged" | "rejected") => {
                ("proofFailed", "retryBridgeProof")
            }
            Some("confirmed") => ("bridgeProven", "waitForSettlementSynchronization"),
            Some(_) => ("proofQueued", "waitForBridgeProof"),
        }
    }
}

async fn list_native_withdrawals(
    State(state): State<AppState>,
    Query(query): Query<ListWithdrawalsQuery>,
) -> Response {
    let recipient = match query.recipient {
        Some(recipient) => match recipient.parse::<Address>() {
            Ok(recipient) => Some(recipient.to_string()),
            Err(_) => {
                return api_error(
                    StatusCode::BAD_REQUEST,
                    "recipient must be a 20-byte Ethereum address",
                )
            }
        },
        None => None,
    };
    let after = match query.after.map(i64::try_from).transpose() {
        Ok(after) => after,
        Err(_) => return api_error(StatusCode::BAD_REQUEST, "after is too large"),
    };
    let limit = query.limit.unwrap_or(20).clamp(1, 100);
    let rows = sqlx::query(
        "SELECT settlement_sequence, action_offset
         FROM gateway_inner_action_leaves
         WHERE recipient IS NOT NULL AND NOT removed
           AND ($1::text IS NULL OR lower(recipient) = lower($1))
           AND ($2::bigint IS NULL OR global_action_index > $2)
         ORDER BY global_action_index
         LIMIT $3",
    )
    .bind(recipient)
    .bind(after)
    .bind(limit)
    .fetch_all(&state.pool)
    .await;
    let rows = match rows {
        Ok(rows) => rows,
        Err(error) => {
            tracing::error!(%error, "list native withdrawals");
            return api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "could not list withdrawals",
            );
        }
    };
    let current_slot = match state.ethereum.current_virtual_slot().await {
        Ok(slot) => slot,
        Err(error) => {
            tracing::error!(%error, "read current virtual slot");
            return api_error(
                StatusCode::BAD_GATEWAY,
                "could not read settlement virtual slot",
            );
        }
    };
    let delay = match state.ethereum.withdrawal_delay_slots().await {
        Ok(delay) => delay,
        Err(error) => {
            tracing::error!(%error, "read withdrawal delay");
            return api_error(StatusCode::BAD_GATEWAY, "could not read withdrawal delay");
        }
    };
    let mut withdrawals = Vec::with_capacity(rows.len());
    for row in rows {
        let location = (|| -> Result<(u64, u32)> {
            Ok((
                u64::try_from(row.try_get::<i64, _>("settlement_sequence")?)?,
                u32::try_from(row.try_get::<i32, _>("action_offset")?)?,
            ))
        })();
        let (sequence, offset) = match location {
            Ok(location) => location,
            Err(error) => {
                tracing::error!(%error, "decode indexed withdrawal location");
                return api_error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "invalid indexed withdrawal location",
                );
            }
        };
        match load_native_withdrawal_proof_at(&state, sequence, offset, current_slot, delay).await {
            Ok(proof) => withdrawals.push(proof),
            Err(error) => {
                tracing::error!(%error, sequence, offset, "build native withdrawal proof");
                return api_error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "could not build withdrawal proof",
                );
            }
        }
    }
    Json(withdrawals).into_response()
}

async fn load_native_withdrawal_proof(
    state: &AppState,
    sequence: u64,
    offset: u32,
) -> Result<NativeWithdrawalProof> {
    let current_slot = state.ethereum.current_virtual_slot().await?;
    let delay = state.ethereum.withdrawal_delay_slots().await?;
    load_native_withdrawal_proof_at(state, sequence, offset, current_slot, delay).await
}

async fn load_native_withdrawal_proof_at(
    state: &AppState,
    sequence: u64,
    offset: u32,
    current_slot: u64,
    delay: u32,
) -> Result<NativeWithdrawalProof> {
    let rows = sqlx::query(
        "SELECT action_offset, global_action_index, action_fields_hash, leaf,
                recipient, zeko_amount::text AS zeko_amount, inner_action_root,
                commit_slot_upper
         FROM gateway_inner_action_leaves
         WHERE settlement_sequence = $1 AND NOT removed
         ORDER BY action_offset",
    )
    .bind(i64::try_from(sequence)?)
    .fetch_all(&state.pool)
    .await?;
    anyhow::ensure!(!rows.is_empty(), "settlement inner-action batch not found");
    let target = rows
        .get(usize::try_from(offset)?)
        .context("withdrawal offset is outside the batch")?;
    anyhow::ensure!(
        target.try_get::<i32, _>("action_offset")? == i32::try_from(offset)?,
        "inner-action batch is not contiguous"
    );
    let recipient: String = target
        .try_get::<Option<String>, _>("recipient")?
        .context("inner action is not a claimable native withdrawal")?;
    let amount = target
        .try_get::<Option<String>, _>("zeko_amount")?
        .context("withdrawal amount missing")?;
    amount
        .parse::<u64>()
        .context("withdrawal amount is outside the supported native range")?;
    let leaves = rows
        .iter()
        .map(|row| -> Result<[u8; 32]> {
            Ok(row
                .try_get::<String, _>("leaf")?
                .parse::<alloy::primitives::B256>()?
                .0)
        })
        .collect::<Result<Vec<_>>>()?;
    let siblings = inner_action_merkle_proof(&leaves, usize::try_from(offset)?)
        .into_iter()
        .map(|hash| format!("0x{}", hex::encode(hash)))
        .collect();
    let commit_slot_upper = u32::try_from(target.try_get::<i64, _>("commit_slot_upper")?)?;
    let global_action_index = u32::try_from(target.try_get::<i64, _>("global_action_index")?)?;
    let recipient_address = recipient
        .parse::<Address>()
        .context("indexed withdrawal recipient is invalid")?;
    let recipient_cursor = state
        .ethereum
        .next_withdrawal_index(recipient_address)
        .await?;
    let claimable_slot = u64::from(commit_slot_upper) + u64::from(delay);
    let (status, next_action) = native_withdrawal_progress(
        global_action_index,
        recipient_cursor,
        current_slot,
        claimable_slot,
    );

    Ok(NativeWithdrawalProof {
        settlement_sequence: sequence,
        offset,
        global_action_index,
        recipient,
        amount,
        action_fields_hash: target.try_get("action_fields_hash")?,
        siblings,
        inner_action_root: target.try_get("inner_action_root")?,
        commit_slot_upper,
        claimable_slot,
        current_virtual_slot: current_slot,
        recipient_cursor,
        status: status.to_owned(),
        next_action: next_action.to_owned(),
    })
}

fn native_withdrawal_progress(
    global_action_index: u32,
    recipient_cursor: u32,
    current_slot: u64,
    claimable_slot: u64,
) -> (&'static str, &'static str) {
    if global_action_index < recipient_cursor {
        ("processed", "none")
    } else if current_slot < claimable_slot {
        ("waitingForDelay", "waitForWithdrawalDelay")
    } else {
        ("claimable", "claimNativeWithdrawal")
    }
}

fn inner_action_merkle_proof(leaves: &[[u8; 32]], target: usize) -> [[u8; 32]; 16] {
    let zero_hashes = inner_action_zero_hashes();
    let mut proof = [[0u8; 32]; 16];
    let mut nodes = leaves.to_vec();
    let mut index = target;
    for level in 0..16 {
        proof[level] = nodes.get(index ^ 1).copied().unwrap_or(zero_hashes[level]);
        nodes = nodes
            .chunks(2)
            .map(|pair| {
                hash_inner_action_node(pair[0], pair.get(1).copied().unwrap_or(zero_hashes[level]))
            })
            .collect();
        index >>= 1;
    }
    proof
}

fn inner_action_zero_hashes() -> [[u8; 32]; 17] {
    let mut hashes = [[0u8; 32]; 17];
    for level in 0..16 {
        hashes[level + 1] = hash_inner_action_node(hashes[level], hashes[level]);
    }
    hashes
}

fn hash_inner_action_node(left: [u8; 32], right: [u8; 32]) -> [u8; 32] {
    let mut encoded = Vec::with_capacity(96);
    encoded.extend_from_slice(&keccak256("ZEKO_INNER_ACTION_NODE_V2").0);
    encoded.extend_from_slice(&left);
    encoded.extend_from_slice(&right);
    keccak256(encoded).0
}

async fn create_withdraw(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<WithdrawTransitionInput>,
) -> Response {
    create_job(
        &state,
        &headers,
        "withdraw",
        serde_json::to_value(input).unwrap(),
    )
    .await
}

async fn create_job(state: &AppState, headers: &HeaderMap, kind: &str, input: Value) -> Response {
    let id = Uuid::new_v4();
    let input_digest = proof_input_digest(&input);
    let idempotency_key = headers
        .get("idempotency-key")
        .and_then(|value| value.to_str().ok());
    let result = sqlx::query_scalar::<_, Uuid>(
        "INSERT INTO proof_jobs (id, kind, input, idempotency_key, input_digest)
         VALUES ($1, $2::proof_kind, $3, $4, $5)
         ON CONFLICT (idempotency_key) WHERE idempotency_key IS NOT NULL
         DO UPDATE SET idempotency_key = EXCLUDED.idempotency_key
         WHERE proof_jobs.input_digest = EXCLUDED.input_digest
         RETURNING id",
    )
    .bind(id)
    .bind(kind)
    .bind(input)
    .bind(idempotency_key)
    .bind(input_digest)
    .fetch_optional(&state.pool)
    .await;

    match result {
        Ok(Some(id)) => (
            StatusCode::ACCEPTED,
            Json(CreatedJob {
                id,
                status: "queued",
                status_url: format!("/v1/proofs/{id}"),
            }),
        )
            .into_response(),
        Ok(None) => api_error(
            StatusCode::CONFLICT,
            "idempotency key already exists with a different payload",
        ),
        Err(error) => {
            if error
                .as_database_error()
                .and_then(|database| database.constraint())
                == Some("one_active_settlement")
            {
                return api_error(
                    StatusCode::CONFLICT,
                    "another settlement is still active; retry after it is finalized",
                );
            }
            if error
                .as_database_error()
                .and_then(|database| database.constraint())
                == Some("one_active_bridge_batch")
            {
                return api_error(
                    StatusCode::CONFLICT,
                    "another bridge batch is still active; retry after it is finalized",
                );
            }
            tracing::error!(%error, "create proof job");
            api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "could not create proof job",
            )
        }
    }
}

async fn conflicting_outer_writer(pool: &PgPool, requested_kind: &str) -> Result<bool> {
    let conflicting_kind = match requested_kind {
        "settlement" => "bridge",
        "bridge" => "settlement",
        _ => return Ok(false),
    };
    Ok(sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(
             SELECT 1 FROM proof_jobs
             WHERE kind::text = $1
               AND status IN (
                 'queued', 'validating', 'awaiting_approval', 'approved',
                 'proof_requested', 'proving', 'submitting', 'submitted'
               )
         )",
    )
    .bind(conflicting_kind)
    .fetch_one(pool)
    .await?)
}

async fn get_proof_quote(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Query(query): Query<ProofQuoteQuery>,
) -> Response {
    match load_proof_quote(&state, id, query).await {
        Ok(quote) => Json(quote).into_response(),
        Err(error) => api_error(StatusCode::CONFLICT, &error.to_string()),
    }
}

async fn load_proof_quote(
    state: &AppState,
    id: Uuid,
    query: ProofQuoteQuery,
) -> Result<ProofQuoteResponse> {
    let row = sqlx::query(
        "SELECT kind::text AS kind, status::text AS status,
                COALESCE(preflight_input_digest, input_digest) AS input_digest,
                public_values, cycle_count, approval_max_pgu,
                approval_max_price_per_pgu
         FROM proof_jobs WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(&state.pool)
    .await?
    .context("proof job not found")?;
    let kind: String = row.try_get("kind")?;
    let status: String = row.try_get("status")?;
    anyhow::ensure!(
        !matches!(
            status.as_str(),
            "queued" | "validating" | "failed" | "rejected"
        ),
        "proof job has not completed preflight"
    );
    let cycle_count = u64::try_from(
        row.try_get::<Option<i64>, _>("cycle_count")?
            .context("proof job has no cycle count")?,
    )?;
    let max_pgu = parse_optional_u64(query.max_pgu, "maxPgu")?
        .or(row
            .try_get::<Option<i64>, _>("approval_max_pgu")?
            .map(u64::try_from)
            .transpose()?)
        .or(state.prover_config.gas_limit)
        // SP1 executor cycles are not exact PGU. This fallback only makes the
        // read-only quote useful before an operator supplies a simulation cap.
        .unwrap_or(cycle_count);
    let max_price_per_pgu = parse_optional_u64(query.max_price_per_pgu, "maxPricePerPgu")?
        .or(row
            .try_get::<Option<i64>, _>("approval_max_price_per_pgu")?
            .map(u64::try_from)
            .transpose()?)
        .or(state.prover_config.max_price_per_pgu);
    let quote = prover::auction_quote(&state.proof_system, max_pgu, max_price_per_pgu).await?;
    let public_values: String = row
        .try_get::<Option<String>, _>("public_values")?
        .context("proof job has no public values")?;
    let remaining_slots = proof_remaining_slots(state, &kind, &public_values).await?;
    Ok(ProofQuoteResponse {
        id,
        kind,
        status,
        input_digest: row.try_get("input_digest")?,
        cycle_count,
        remaining_slots,
        quote,
        note: "Read-only auction parameters; executor cycles are not an exact PGU estimate and no proof request was created",
    })
}

async fn approve_proof(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(request): Json<ApproveProofRequest>,
) -> Response {
    match approve_proof_inner(&state, id, request).await {
        Ok(value) => Json(value).into_response(),
        Err(error) => api_error(StatusCode::CONFLICT, &error.to_string()),
    }
}

async fn approve_proof_inner(
    state: &AppState,
    id: Uuid,
    request: ApproveProofRequest,
) -> Result<Value> {
    anyhow::ensure!(
        state.require_proof_approval,
        "proof approval mode is disabled"
    );
    let max_pgu = parse_positive_u64(&request.max_pgu, "maxPgu")?;
    let max_price_per_pgu = parse_positive_u64(&request.max_price_per_pgu, "maxPricePerPgu")?;
    if let Some(hard_cap) = state.prover_config.gas_limit {
        anyhow::ensure!(
            max_pgu <= hard_cap,
            "maxPgu exceeds the configured hard cap"
        );
    }
    if let Some(hard_cap) = state.prover_config.max_price_per_pgu {
        anyhow::ensure!(
            max_price_per_pgu <= hard_cap,
            "maxPricePerPgu exceeds the configured hard cap"
        );
    }
    let max_pgu_i64 = i64::try_from(max_pgu).context("maxPgu exceeds PostgreSQL BIGINT")?;
    let max_price_i64 =
        i64::try_from(max_price_per_pgu).context("maxPricePerPgu exceeds PostgreSQL BIGINT")?;
    let row = sqlx::query(
        "SELECT kind::text AS kind, status::text AS status, input,
                COALESCE(preflight_input_digest, input_digest) AS input_digest,
                public_values, cycle_count
         FROM proof_jobs WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(&state.pool)
    .await?
    .context("proof job not found")?;
    let status: String = row.try_get("status")?;
    anyhow::ensure!(
        status == "awaiting_approval",
        "proof job is not awaiting approval"
    );
    let input_digest: String = row.try_get("input_digest")?;
    anyhow::ensure!(
        request.input_digest == input_digest,
        "approval input digest does not match the preflighted job"
    );
    let kind: String = row.try_get("kind")?;
    let public_values_hex: String = row
        .try_get::<Option<String>, _>("public_values")?
        .context("proof job has no public values")?;
    let public_values = decode_hex_bytes(&public_values_hex, "public values")?;
    let cycles = u64::try_from(
        row.try_get::<Option<i64>, _>("cycle_count")?
            .context("proof job has no cycle count")?,
    )?;
    let preflight = prover::Preflight::decode(&kind, public_values, cycles)?;
    validate_preflight(state, &kind, &row.try_get::<Value, _>("input")?, &preflight).await?;
    require_proof_lifetime(state, &kind, &public_values_hex).await?;
    let quote =
        prover::auction_quote(&state.proof_system, max_pgu, Some(max_price_per_pgu)).await?;
    let result = sqlx::query(
        "UPDATE proof_jobs
         SET status = 'approved', approval_input_digest = $2,
             approval_max_pgu = $3, approval_max_price_per_pgu = $4,
             approval_base_fee_atto_prove = $5,
             approval_network_max_price_per_pgu = $6,
             approval_max_cost_atto_prove = $7,
             approved_at = NOW(), error = NULL, updated_at = NOW()
         WHERE id = $1 AND status = 'awaiting_approval'
           AND COALESCE(preflight_input_digest, input_digest) = $2",
    )
    .bind(id)
    .bind(&input_digest)
    .bind(max_pgu_i64)
    .bind(max_price_i64)
    .bind(&quote.base_fee_atto_prove)
    .bind(&quote.network_max_price_per_pgu)
    .bind(&quote.maximum_cost_atto_prove)
    .execute(&state.pool)
    .await?;
    anyhow::ensure!(
        result.rows_affected() == 1,
        "proof job approval raced with another update"
    );
    Ok(serde_json::json!({
        "id": id,
        "status": "approved",
        "inputDigest": input_digest,
        "quote": quote,
        "statusUrl": format!("/v1/proofs/{id}")
    }))
}

async fn cancel_proof(State(state): State<AppState>, Path(id): Path<Uuid>) -> Response {
    let result = sqlx::query(
        "UPDATE proof_jobs SET status = 'rejected',
                error = 'cancelled by operator before network proof request',
                completed_at = NOW(), updated_at = NOW()
         WHERE id = $1 AND proof_request_id IS NULL
           AND status IN ('queued', 'validating', 'awaiting_approval', 'approved')",
    )
    .bind(id)
    .execute(&state.pool)
    .await;
    match result {
        Ok(result) if result.rows_affected() == 1 => {
            let _ = sqlx::query("DELETE FROM gateway_pending_commands WHERE job_id = $1")
                .bind(id)
                .execute(&state.pool)
                .await;
            Json(serde_json::json!({"id": id, "status": "rejected"})).into_response()
        }
        Ok(_) => api_error(
            StatusCode::CONFLICT,
            "proof job cannot be cancelled after a network request or terminal transition",
        ),
        Err(error) => api_error(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string()),
    }
}

async fn proof_remaining_slots(
    state: &AppState,
    kind: &str,
    public_values_hex: &str,
) -> Result<Option<u64>> {
    if kind != "settlement" {
        return Ok(None);
    }
    let public_values = decode_hex_bytes(public_values_hex, "settlement public values")?;
    let values = SettlementPublicValues::decode(&public_values).map_err(anyhow::Error::msg)?;
    let current = state.ethereum.current_virtual_slot().await?;
    Ok(Some(
        u64::from(values.settlement().slot_upper).saturating_sub(current),
    ))
}

async fn require_proof_lifetime(
    state: &AppState,
    kind: &str,
    public_values_hex: &str,
) -> Result<()> {
    if let Some(remaining) = proof_remaining_slots(state, kind, public_values_hex).await? {
        anyhow::ensure!(
            remaining >= state.min_remaining_slots,
            "settlement proof has {remaining} slots remaining; at least {} are required",
            state.min_remaining_slots
        );
    }
    Ok(())
}

fn decode_hex_bytes(value: &str, name: &str) -> Result<Vec<u8>> {
    let value = value
        .strip_prefix("0x")
        .with_context(|| format!("{name} must start with 0x"))?;
    hex::decode(value).with_context(|| format!("invalid {name} hex"))
}

fn proof_input_digest(input: &Value) -> String {
    format!("0x{}", hex::encode(Sha256::digest(input.to_string())))
}

fn parse_positive_u64(value: &str, name: &str) -> Result<u64> {
    let value = value
        .parse::<u64>()
        .with_context(|| format!("{name} must be an unsigned integer string"))?;
    anyhow::ensure!(value > 0, "{name} must be greater than zero");
    Ok(value)
}

fn parse_optional_u64(value: Option<String>, name: &str) -> Result<Option<u64>> {
    value
        .map(|value| parse_positive_u64(&value, name))
        .transpose()
}

fn database_safe_error(error: &anyhow::Error) -> String {
    // JSON-RPC revert diagnostics may contain decoded NUL bytes. PostgreSQL
    // TEXT rejects NUL, and losing the failure transition would strand a job
    // in its active state.
    format!("{error:#}").replace('\0', "\\0")
}

async fn get_job(State(state): State<AppState>, Path(id): Path<Uuid>) -> Response {
    let job = sqlx::query_as::<_, ProofJob>(
        "SELECT id, kind::text AS kind, status::text AS status, input, public_values,
                proof_request_id, transaction_hash, error, attempts, created_at,
                updated_at, started_at, completed_at, input_digest,
                preflight_input_digest, cycle_count,
                prover_gas, base_fee_prove, max_price_per_pgu,
                actual_cost_prove, ethereum_gas_used,
                confirmations, explorer_url, approval_input_digest,
                approval_max_pgu, approval_max_price_per_pgu,
                approval_base_fee_atto_prove,
                approval_network_max_price_per_pgu,
                approval_max_cost_atto_prove, approved_at
         FROM proof_jobs WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(&state.pool)
    .await;

    match job {
        Ok(Some(job)) => Json(job).into_response(),
        Ok(None) => api_error(StatusCode::NOT_FOUND, "proof job not found"),
        Err(error) => {
            tracing::error!(%error, "read proof job");
            api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "could not read proof job",
            )
        }
    }
}

async fn list_jobs(State(state): State<AppState>, Query(query): Query<ListJobsQuery>) -> Response {
    let limit = query.limit.unwrap_or(50).clamp(1, 200);
    let jobs = sqlx::query_as::<_, ProofJob>(
        "SELECT id, kind::text AS kind, status::text AS status, input, public_values,
                proof_request_id, transaction_hash, error, attempts, created_at,
                updated_at, started_at, completed_at, input_digest,
                preflight_input_digest, cycle_count,
                prover_gas, base_fee_prove, max_price_per_pgu,
                actual_cost_prove, ethereum_gas_used,
                confirmations, explorer_url, approval_input_digest,
                approval_max_pgu, approval_max_price_per_pgu,
                approval_base_fee_atto_prove,
                approval_network_max_price_per_pgu,
                approval_max_cost_atto_prove, approved_at
         FROM proof_jobs
         WHERE ($1::text IS NULL OR kind::text = $1)
           AND ($2::text IS NULL OR status::text = $2)
         ORDER BY created_at DESC
         LIMIT $3",
    )
    .bind(query.kind)
    .bind(query.status)
    .bind(limit)
    .fetch_all(&state.pool)
    .await;

    match jobs {
        Ok(jobs) => Json(jobs).into_response(),
        Err(error) => {
            tracing::error!(%error, "list proof jobs");
            api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "could not list proof jobs",
            )
        }
    }
}

async fn worker_loop(state: AppState) {
    loop {
        match claim_job(&state.pool).await {
            Ok(Some(job)) => process_job(&state, job).await,
            Ok(None) => sleep(Duration::from_secs(2)).await,
            Err(error) => {
                tracing::error!(%error, "claim proof job");
                sleep(Duration::from_secs(5)).await;
            }
        }
    }
}

async fn claim_job(pool: &PgPool) -> Result<Option<ClaimedJob>> {
    let mut tx = pool.begin().await?;
    // Serialize claims across gateway replicas. The lock is held only for the
    // short database transaction; the active-settlement index then guards the
    // much longer prove/submit lifecycle.
    sqlx::query("SELECT id FROM gateway_config WHERE id = TRUE FOR UPDATE")
        .fetch_one(&mut *tx)
        .await?;
    let job = sqlx::query_as::<_, ClaimedJob>(
        "SELECT queued.id, queued.kind::text AS kind, queued.input,
                queued.proof_request_id, queued.status::text AS claimed_status,
                queued.public_values, queued.cycle_count,
                queued.approval_max_pgu, queued.approval_max_price_per_pgu
         FROM proof_jobs queued
         WHERE queued.status IN ('queued', 'approved')
           AND (queued.kind <> 'settlement' OR NOT EXISTS (
             SELECT 1 FROM proof_jobs active
             WHERE active.id <> queued.id AND active.kind = 'settlement'
               AND active.status IN (
                 'validating', 'awaiting_approval', 'approved', 'proof_requested',
                 'proving', 'submitting', 'submitted'
               )
           ))
           AND (queued.kind NOT IN ('settlement', 'bridge') OR NOT EXISTS (
             SELECT 1 FROM proof_jobs active
             WHERE active.id <> queued.id AND active.kind <> queued.kind
               AND active.kind IN ('settlement', 'bridge')
               AND active.status IN (
                 'validating', 'awaiting_approval', 'approved', 'proof_requested',
                 'proving', 'submitting', 'submitted'
               )
           ))
         ORDER BY queued.created_at
         FOR UPDATE SKIP LOCKED
         LIMIT 1",
    )
    .fetch_optional(&mut *tx)
    .await?;
    if let Some(ref job) = job {
        sqlx::query(
            "UPDATE proof_jobs
             SET status = CASE
                   WHEN status = 'approved' THEN 'proving'::proof_status
                   ELSE 'validating'::proof_status
                 END,
                 attempts = attempts + 1,
                 started_at = COALESCE(started_at, NOW()), updated_at = NOW()
             WHERE id = $1",
        )
        .bind(job.id)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(job)
}

async fn process_job(state: &AppState, mut job: ClaimedJob) {
    let result = async {
        let (preflight, request_config) = if job.claimed_status == "approved" {
            let public_values_hex = job
                .public_values
                .as_deref()
                .context("approved proof job has no public values")?;
            require_proof_lifetime(state, &job.kind, public_values_hex).await?;
            let public_values = decode_hex_bytes(public_values_hex, "public values")?;
            let cycles = u64::try_from(
                job.cycle_count
                    .context("approved proof job has no cycle count")?,
            )?;
            let preflight = prover::Preflight::decode(&job.kind, public_values, cycles)?;
            validate_preflight(state, &job.kind, &job.input, &preflight).await?;
            let mut config = state.prover_config.clone();
            config.gas_limit = Some(u64::try_from(
                job.approval_max_pgu
                    .context("approved proof job has no max PGU")?,
            )?);
            config.max_price_per_pgu = Some(u64::try_from(
                job.approval_max_price_per_pgu
                    .context("approved proof job has no max price per PGU")?,
            )?);
            (preflight, config)
        } else {
            if job.kind == "settlement" {
                hydrate_queued_settlement(state, &mut job.input).await?;
                let result = sqlx::query(
                    "UPDATE proof_jobs SET input = $2, updated_at = NOW()
                     WHERE id = $1 AND status = 'validating'",
                )
                .bind(job.id)
                .bind(&job.input)
                .execute(&state.pool)
                .await?;
                anyhow::ensure!(result.rows_affected() == 1, "settlement job was cancelled");
            }
            let preflight_input_digest = proof_input_digest(&job.input);
            let digest_result = sqlx::query(
                "UPDATE proof_jobs SET preflight_input_digest = $2, updated_at = NOW()
                 WHERE id = $1 AND status = 'validating'",
            )
            .bind(job.id)
            .bind(preflight_input_digest)
            .execute(&state.pool)
            .await?;
            anyhow::ensure!(
                digest_result.rows_affected() == 1,
                "proof job was cancelled"
            );
            let preflight = prover::preflight(&job.kind, &job.input).await?;
            validate_preflight(state, &job.kind, &job.input, &preflight).await?;
            let cycle_count = i64::try_from(preflight.cycles())
                .context("SP1 preflight cycle count exceeds PostgreSQL BIGINT")?;
            let public_values_hex = format!("0x{}", hex::encode(preflight.public_values()));
            let result = sqlx::query(
                "UPDATE proof_jobs SET public_values = $2, cycle_count = $3,
                        updated_at = NOW()
                 WHERE id = $1 AND status = 'validating'",
            )
            .bind(job.id)
            .bind(&public_values_hex)
            .bind(cycle_count)
            .execute(&state.pool)
            .await?;
            anyhow::ensure!(result.rows_affected() == 1, "proof job was cancelled");
            if state.execute_only {
                let result = sqlx::query(
                    "UPDATE proof_jobs SET status = 'executed',
                            completed_at = NOW(), updated_at = NOW()
                     WHERE id = $1 AND status = 'validating'",
                )
                .bind(job.id)
                .execute(&state.pool)
                .await?;
                anyhow::ensure!(result.rows_affected() == 1, "proof job was cancelled");
                return Result::<()>::Ok(());
            }
            if state.local_mock_submit {
                submit_local_mock(state, job.id, &job.kind, &preflight).await?;
                return Result::<()>::Ok(());
            }
            if state.require_proof_approval {
                let result = sqlx::query(
                    "UPDATE proof_jobs SET status = 'awaiting_approval',
                            completed_at = NULL, updated_at = NOW()
                     WHERE id = $1 AND status = 'validating'",
                )
                .bind(job.id)
                .execute(&state.pool)
                .await?;
                anyhow::ensure!(result.rows_affected() == 1, "proof job was cancelled");
                return Result::<()>::Ok(());
            }
            (preflight, state.prover_config.clone())
        };

        set_status(&state.pool, job.id, "proving").await?;

        let request_id = match job.proof_request_id {
            Some(request_id) => request_id,
            None => {
                let request_id = prover::request_proof(
                    &job.kind,
                    &job.input,
                    &state.proof_system,
                    &request_config,
                )
                .await?;
                let result = sqlx::query(
                    "UPDATE proof_jobs SET proof_request_id = $2, updated_at = NOW()
                     WHERE id = $1 AND status = 'proving'",
                )
                .bind(job.id)
                .bind(&request_id)
                .execute(&state.pool)
                .await?;
                anyhow::ensure!(result.rows_affected() == 1, "proof job was cancelled");
                set_status(&state.pool, job.id, "proof_requested").await?;
                request_id
            }
        };
        let proof = prover::wait_proof(&job.kind, &request_id).await?;
        anyhow::ensure!(
            proof.public_values == preflight.public_values(),
            "network proof public values differ from local SP1 preflight"
        );
        let metrics = prover::request_metrics(&request_id)
            .await
            .unwrap_or_else(|error| {
                tracing::warn!(%error, %request_id, "could not read prover-network metrics");
                prover::RequestMetrics::default()
            });
        set_status(&state.pool, job.id, "submitting").await?;
        let mut submit_tx = state.pool.begin().await?;
        sqlx::query("SELECT id FROM gateway_config WHERE id = TRUE FOR UPDATE")
            .fetch_one(&mut *submit_tx)
            .await?;
        let still_submitting = sqlx::query_scalar::<_, bool>(
            "SELECT status = 'submitting' FROM proof_jobs WHERE id = $1",
        )
        .bind(job.id)
        .fetch_one(&mut *submit_tx)
        .await?;
        anyhow::ensure!(
            still_submitting,
            "proof job was cancelled before submission"
        );
        let transaction_hash = state
            .ethereum
            .submit(&job.kind, proof.public_values.clone(), proof.proof.bytes())
            .await?;
        let result = sqlx::query(
            "UPDATE proof_jobs SET status = 'submitted', public_values = $2,
                    proof_request_id = $3, transaction_hash = $4,
                    cycle_count = $5, prover_gas = $6, base_fee_prove = $7,
                    max_price_per_pgu = $8, actual_cost_prove = $9,
                    confirmations = 0, explorer_url = $10,
                    updated_at = NOW()
             WHERE id = $1 AND status = 'submitting'",
        )
        .bind(job.id)
        .bind(format!("0x{}", hex::encode(proof.public_values)))
        .bind(&request_id)
        .bind(transaction_hash.to_string())
        .bind(metrics.cycles.and_then(|value| i64::try_from(value).ok()))
        .bind(
            metrics
                .prover_gas
                .and_then(|value| i64::try_from(value).ok()),
        )
        .bind(metrics.base_fee_prove)
        .bind(metrics.max_price_per_pgu)
        .bind(metrics.actual_cost_prove)
        .bind(format!("{}/{}", state.network_explorer_base, request_id))
        .execute(&mut *submit_tx)
        .await?;
        anyhow::ensure!(
            result.rows_affected() == 1,
            "proof job was cancelled after submission"
        );
        submit_tx.commit().await?;
        Result::<()>::Ok(())
    }
    .await;

    if let Err(error) = result {
        tracing::error!(job_id = %job.id, %error, "proof job failed");
        let error_message = database_safe_error(&error);
        if let Err(update_error) = sqlx::query(
            "UPDATE proof_jobs SET status = 'failed', error = $2,
                    completed_at = NOW(), updated_at = NOW()
             WHERE id = $1 AND status NOT IN ('reorged', 'rejected')",
        )
        .bind(job.id)
        .bind(error_message)
        .execute(&state.pool)
        .await
        {
            tracing::error!(job_id = %job.id, %update_error, "could not persist proof failure");
        }
        let _ = sqlx::query("DELETE FROM gateway_pending_commands WHERE job_id = $1")
            .bind(job.id)
            .execute(&state.pool)
            .await;
    }
}

async fn submit_local_mock(
    state: &AppState,
    job_id: Uuid,
    kind: &str,
    preflight: &prover::Preflight,
) -> Result<()> {
    set_status(&state.pool, job_id, "submitting").await?;
    let mut tx = state.pool.begin().await?;
    sqlx::query("SELECT id FROM gateway_config WHERE id = TRUE FOR UPDATE")
        .fetch_one(&mut *tx)
        .await?;
    let still_submitting =
        sqlx::query_scalar::<_, bool>("SELECT status = 'submitting' FROM proof_jobs WHERE id = $1")
            .bind(job_id)
            .fetch_one(&mut *tx)
            .await?;
    anyhow::ensure!(
        still_submitting,
        "proof job was cancelled before submission"
    );

    let transaction_hash = state
        .ethereum
        .submit(kind, preflight.public_values().to_vec(), Vec::new())
        .await?;
    let cycle_count = i64::try_from(preflight.cycles())
        .context("SP1 preflight cycle count exceeds PostgreSQL BIGINT")?;
    let result = sqlx::query(
        "UPDATE proof_jobs SET status = 'submitted', public_values = $2,
                transaction_hash = $3, cycle_count = $4, confirmations = 0,
                updated_at = NOW()
         WHERE id = $1 AND status = 'submitting'",
    )
    .bind(job_id)
    .bind(format!("0x{}", hex::encode(preflight.public_values())))
    .bind(transaction_hash.to_string())
    .bind(cycle_count)
    .execute(&mut *tx)
    .await?;
    anyhow::ensure!(
        result.rows_affected() == 1,
        "proof job was cancelled after local submission"
    );
    tx.commit().await?;
    Ok(())
}

async fn hydrate_queued_settlement(state: &AppState, input: &mut Value) -> Result<()> {
    let mina_transaction_hash = input
        .get("minaTransactionHash")
        .and_then(Value::as_str)
        .context("queued settlement has no Mina transaction hash")?
        .to_owned();
    let mut proof: SettlementProofBundle = serde_json::from_value(
        input
            .get("proof")
            .cloned()
            .context("queued settlement has no proof bundle")?,
    )?;
    hydrate_settlement_context(state, &mut proof, &mina_transaction_hash).await?;
    input
        .as_object_mut()
        .context("queued settlement input must be an object")?
        .insert("proof".to_owned(), serde_json::to_value(proof)?);
    Ok(())
}

async fn validate_preflight(
    state: &AppState,
    kind: &str,
    input: &Value,
    preflight: &prover::Preflight,
) -> Result<()> {
    let local_vkey = prover::program_vkey(kind).await?;
    match preflight {
        prover::Preflight::Settlement { values, .. } => {
            let values = values.settlement();
            let chain = state.ethereum.settlement_state().await?;
            ensure_hex_eq(
                &local_vkey,
                &chain.program_vkey.to_string(),
                "settlement program vkey",
            )?;
            ensure_bytes_eq(values.vk_hash, chain.vk_hash, "vk hash")?;
            anyhow::ensure!(
                values.chain_id == state.ethereum.chain_id().await?,
                "settlement chain id mismatch"
            );
            anyhow::ensure!(
                values.settlement_contract.as_slice()
                    == state.ethereum.settlement_address().as_slice(),
                "settlement contract address mismatch"
            );
            anyhow::ensure!(
                values.batch_sequence == chain.batch_sequence + 1,
                "settlement batch sequence mismatch"
            );
            ensure_bytes_eq(
                values.outer_action_state_before,
                chain.action_state,
                "action state",
            )?;
            anyhow::ensure!(
                values.outer_action_state_length_before == chain.outer_action_state_length,
                "outer action-state length mismatch"
            );
            for (index, (actual, expected)) in values
                .state_before
                .fields
                .iter()
                .zip(chain.outer_state.iter())
                .enumerate()
            {
                anyhow::ensure!(
                    actual.as_slice() == expected.as_slice(),
                    "outer state field {index} mismatch"
                );
            }
            ensure_bytes_eq(
                values.state_before.fields[2],
                chain.current_root,
                "current root",
            )?;
        }
        prover::Preflight::Bridge { values, .. } => {
            let input: BridgeTransitionInput = serde_json::from_value(input.clone())?;
            let chain_id = state.ethereum.chain_id().await?;
            anyhow::ensure!(input.ethereum.chain_id == chain_id, "chain id mismatch");
            anyhow::ensure!(
                input.ethereum.bridge_address.as_slice()
                    == state.ethereum.bridge_address().as_slice(),
                "bridge address mismatch"
            );
            let (chain, historical) = state
                .ethereum
                .bridge_state(
                    "bridge",
                    Some(values.ethereum_nonce_before),
                    Some(values.zeko_action_state_after.into()),
                )
                .await?;
            anyhow::ensure!(!chain.paused, "bridge contract is paused");
            anyhow::ensure!(
                values.ethereum_nonce_before == chain.bridged_deposit_nonce,
                "bridge batch does not start at the next unbridged nonce"
            );
            anyhow::ensure!(
                chain.action_state_processed == Some(false),
                "bridge action state already processed"
            );
            ensure_hex_eq(
                &local_vkey,
                &chain.program_vkey.to_string(),
                "bridge program vkey",
            )?;
            ensure_bytes_eq(
                values.ethereum_state_before,
                historical.context("missing historical deposit state")?,
                "historical deposit state",
            )?;
            anyhow::ensure!(
                values.ethereum_nonce_after == chain.deposit_nonce,
                "deposit nonce after mismatch"
            );
            ensure_bytes_eq(
                values.ethereum_state_after,
                chain.current_deposit_state,
                "current deposit state",
            )?;
            let settlement = state.ethereum.settlement_state().await?;
            ensure_bytes_eq(
                values.zeko_action_state_before,
                settlement.action_state,
                "settlement outer action state",
            )?;
            anyhow::ensure!(
                values.zeko_action_state_length_before == settlement.outer_action_state_length,
                "settlement outer action-state length mismatch"
            );
            anyhow::ensure!(
                values.zeko_action_state_length_after
                    == values
                        .zeko_action_state_length_before
                        .checked_add(u32::try_from(values.actions.len())?)
                        .context("bridge action-state length overflow")?,
                "bridge action-state length transition mismatch"
            );
            anyhow::ensure!(
                values.actions.last().map(|action| action.state_after)
                    == Some(values.zeko_action_state_after),
                "bridge final action-state checkpoint mismatch"
            );
        }
        prover::Preflight::Withdraw { values, .. } => {
            let input: WithdrawTransitionInput = serde_json::from_value(input.clone())?;
            let chain_id = state.ethereum.chain_id().await?;
            anyhow::ensure!(input.ethereum.chain_id == chain_id, "chain id mismatch");
            anyhow::ensure!(
                input.ethereum.bridge_address.as_slice()
                    == state.ethereum.bridge_address().as_slice(),
                "bridge address mismatch"
            );
            let (chain, _) = state
                .ethereum
                .bridge_state(
                    "withdraw",
                    None,
                    Some(values.zeko_action_state_after.into()),
                )
                .await?;
            anyhow::ensure!(!chain.paused, "bridge contract is paused");
            anyhow::ensure!(
                chain.action_state_processed == Some(false),
                "withdraw action state already processed"
            );
            ensure_hex_eq(
                &local_vkey,
                &chain.program_vkey.to_string(),
                "withdraw program vkey",
            )?;
            ensure_bytes_eq(
                values.ethereum_withdraw_state_before,
                chain.current_withdraw_state,
                "current withdraw state",
            )?;
            let old_info = state
                .ethereum
                .l2_action_state_info(values.zeko_action_state_before.into())
                .await?;
            let new_info = state
                .ethereum
                .l2_action_state_info(values.zeko_action_state_after.into())
                .await?;
            anyhow::ensure!(
                old_info.1 && new_info.1,
                "withdraw action state is not settled"
            );
            anyhow::ensure!(
                old_info.0 == chain.current_withdraw_action_state_index
                    && new_info.0 == old_info.0 + 1,
                "invalid withdraw action state transition"
            );
        }
    }
    Ok(())
}

fn ensure_bytes_eq(actual: [u8; 32], expected: alloy::primitives::B256, name: &str) -> Result<()> {
    anyhow::ensure!(actual.as_slice() == expected.as_slice(), "{name} mismatch");
    Ok(())
}

fn ensure_hex_eq(actual: &str, expected: &str, name: &str) -> Result<()> {
    anyhow::ensure!(
        actual.eq_ignore_ascii_case(expected),
        "{name} mismatch: local={actual}, onchain={expected}"
    );
    Ok(())
}

async fn set_status(pool: &PgPool, id: Uuid, status: &str) -> Result<()> {
    let result = sqlx::query(
        "UPDATE proof_jobs SET status = $2::proof_status, updated_at = NOW()
         WHERE id = $1 AND status <> 'reorged'",
    )
    .bind(id)
    .bind(status)
    .execute(pool)
    .await?;
    anyhow::ensure!(result.rows_affected() == 1, "proof job was cancelled");
    Ok(())
}

fn api_error(status: StatusCode, message: &str) -> Response {
    (status, Json(serde_json::json!({"error": message}))).into_response()
}

fn cors_layer() -> Result<CorsLayer> {
    let layer = CorsLayer::new()
        .allow_methods([Method::GET, Method::POST])
        .allow_headers([header::CONTENT_TYPE]);
    let Some(configured) = nonempty_env("API_CORS_ALLOWED_ORIGINS") else {
        return Ok(layer);
    };
    if configured.trim() == "*" {
        return Ok(layer.allow_origin(Any));
    }
    let origins = configured
        .split(',')
        .map(str::trim)
        .filter(|origin| !origin.is_empty())
        .map(|origin| {
            origin
                .parse::<HeaderValue>()
                .with_context(|| format!("invalid API_CORS_ALLOWED_ORIGINS entry {origin}"))
        })
        .collect::<Result<Vec<_>>>()?;
    anyhow::ensure!(
        !origins.is_empty(),
        "API_CORS_ALLOWED_ORIGINS must contain an origin"
    );
    Ok(layer.allow_origin(AllowOrigin::list(origins)))
}

fn required_env(name: &str) -> Result<String> {
    env::var(name).with_context(|| format!("{name} is required"))
}

fn validate_finality_mode(mode: indexer::FinalityMode, chain_id: u64) -> Result<()> {
    anyhow::ensure!(
        mode != indexer::FinalityMode::Confirmations || chain_id == 31_337,
        "ETHEREUM_FINALITY_MODE=confirmations is restricted to local chain ID 31337"
    );
    Ok(())
}

fn nonempty_env(name: &str) -> Option<String> {
    env::var(name).ok().filter(|value| !value.is_empty())
}

fn bool_env(name: &str) -> Result<bool> {
    match nonempty_env(name)
        .unwrap_or_else(|| "false".to_owned())
        .to_ascii_lowercase()
        .as_str()
    {
        "1" | "true" | "yes" | "on" => Ok(true),
        "0" | "false" | "no" | "off" => Ok(false),
        value => anyhow::bail!("{name} must be a boolean, got {value}"),
    }
}

fn u64_env(name: &str, default: u64) -> Result<u64> {
    match nonempty_env(name) {
        Some(value) => value
            .parse()
            .with_context(|| format!("{name} must be an unsigned integer")),
        None => Ok(default),
    }
}

fn optional_u64_env(name: &str) -> Result<Option<u64>> {
    nonempty_env(name)
        .map(|value| {
            value
                .parse()
                .with_context(|| format!("{name} must be an unsigned integer"))
        })
        .transpose()
}

fn is_bytes32_hex(value: &str) -> bool {
    value.len() == 66
        && value.starts_with("0x")
        && value[2..].bytes().all(|byte| byte.is_ascii_hexdigit())
}

async fn initialize_gateway_config(pool: &PgPool) -> Result<()> {
    let genesis_timestamp =
        env::var("VIRTUAL_MINA_GENESIS_TIMESTAMP").unwrap_or_else(|_| Utc::now().to_rfc3339());
    let fork_slot = env::var("VIRTUAL_MINA_FORK_SLOT")
        .unwrap_or_else(|_| "0".to_owned())
        .parse::<i32>()
        .context("VIRTUAL_MINA_FORK_SLOT must fit int32")?;
    let account_creation_fee =
        env::var("VIRTUAL_MINA_ACCOUNT_CREATION_FEE").unwrap_or_else(|_| "1000000000".to_owned());
    let state_hash = env::var("VIRTUAL_MINA_INITIAL_STATE_HASH").unwrap_or_else(|_| {
        "0x0000000000000000000000000000000000000000000000000000000000000000".to_owned()
    });
    let outer_public_key = nonempty_env("VIRTUAL_MINA_OUTER_PUBLIC_KEY");
    sqlx::query(
        "INSERT INTO gateway_config
            (id, genesis_timestamp, fork_slot, account_creation_fee, state_hash,
             outer_public_key)
         VALUES (TRUE, $1, $2, $3, $4, $5)
         ON CONFLICT (id) DO UPDATE SET
            outer_public_key = COALESCE(
                gateway_config.outer_public_key,
                EXCLUDED.outer_public_key
            )",
    )
    .bind(genesis_timestamp)
    .bind(fork_slot)
    .bind(account_creation_fee)
    .bind(state_hash)
    .bind(outer_public_key)
    .execute(pool)
    .await?;
    if let Some(path) = nonempty_env("VIRTUAL_MINA_ACCOUNTS_PATH") {
        let contents = std::fs::read_to_string(&path)
            .with_context(|| format!("read virtual Mina accounts from {path}"))?;
        let accounts: Vec<Value> = serde_json::from_str(&contents)
            .with_context(|| format!("parse virtual Mina accounts from {path}"))?;
        for account in accounts {
            let public_key = account
                .get("publicKey")
                .and_then(Value::as_str)
                .context("virtual Mina account publicKey is required")?;
            let token_id = account
                .get("tokenId")
                .and_then(Value::as_str)
                .unwrap_or("1");
            sqlx::query(
                "INSERT INTO gateway_accounts (public_key, token_id, account_json)
                 VALUES ($1, $2, $3)
                 ON CONFLICT (public_key, token_id) DO NOTHING",
            )
            .bind(public_key)
            .bind(token_id)
            .bind(&account)
            .execute(pool)
            .await?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn approval_caps_are_positive_unsigned_integer_strings() {
        assert_eq!(parse_positive_u64("1", "maxPgu").unwrap(), 1);
        assert_eq!(
            parse_positive_u64("18446744073709551615", "maxPgu").unwrap(),
            u64::MAX
        );
        for value in ["0", "-1", "1.0", "", " 1"] {
            assert!(parse_positive_u64(value, "maxPgu").is_err(), "{value}");
        }
    }

    #[test]
    fn database_errors_escape_nul_bytes() {
        let error = anyhow::anyhow!("revert\0payload").context("submit");
        assert_eq!(database_safe_error(&error), "submit: revert\\0payload");
    }

    #[test]
    fn confirmation_finality_mode_is_local_only() {
        assert!(validate_finality_mode(indexer::FinalityMode::Confirmations, 31_337).is_ok());
        assert!(validate_finality_mode(indexer::FinalityMode::Confirmations, 11_155_111).is_err());
        assert!(validate_finality_mode(indexer::FinalityMode::Finalized, 11_155_111).is_ok());
    }

    #[test]
    fn withdrawal_proof_uses_fixed_depth_and_preserves_offsets() {
        let leaves = [
            keccak256("first").0,
            keccak256("second").0,
            keccak256("third").0,
        ];
        for target in 0..leaves.len() {
            let proof = inner_action_merkle_proof(&leaves, target);
            let mut computed = leaves[target];
            let mut index = target;
            for sibling in proof {
                computed = if index & 1 == 0 {
                    hash_inner_action_node(computed, sibling)
                } else {
                    hash_inner_action_node(sibling, computed)
                };
                index >>= 1;
            }

            let zero_hashes = inner_action_zero_hashes();
            let mut nodes = leaves.to_vec();
            for level in 0..16 {
                nodes = nodes
                    .chunks(2)
                    .map(|pair| {
                        hash_inner_action_node(
                            pair[0],
                            pair.get(1).copied().unwrap_or(zero_hashes[level]),
                        )
                    })
                    .collect();
            }
            assert_eq!(computed, nodes[0]);
        }
    }

    #[test]
    fn deposit_progress_requires_finality_proof_and_synchronization() {
        assert_eq!(
            native_deposit_progress(false, None, false, false),
            ("confirming", "waitForEthereumFinality")
        );
        assert_eq!(
            native_deposit_progress(true, None, false, false),
            ("locked", "requestBridgeProof")
        );
        assert_eq!(
            native_deposit_progress(true, Some("proving"), false, false),
            ("proving", "waitForBridgeProof")
        );
        assert_eq!(
            native_deposit_progress(true, Some("failed"), false, false),
            ("proofFailed", "retryBridgeProof")
        );
        assert_eq!(
            native_deposit_progress(true, Some("confirmed"), true, false),
            ("bridgeProven", "waitForSettlementSynchronization")
        );
        assert_eq!(
            native_deposit_progress(true, Some("confirmed"), true, true),
            ("synchronized", "finalizeDepositOnZeko")
        );
    }

    #[test]
    fn withdrawal_progress_observes_recipient_cursor_before_delay() {
        assert_eq!(
            native_withdrawal_progress(9, 10, 100, 90),
            ("processed", "none")
        );
        assert_eq!(
            native_withdrawal_progress(10, 10, 89, 90),
            ("waitingForDelay", "waitForWithdrawalDelay")
        );
        assert_eq!(
            native_withdrawal_progress(10, 10, 90, 90),
            ("claimable", "claimNativeWithdrawal")
        );
    }
}
