use alloy::primitives::{keccak256, U256};
use anyhow::{Context, Result};
use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
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
use tower_http::trace::TraceLayer;
use uuid::Uuid;
use zeko_sp1_lib::{
    BridgeDeposit, BridgeTransitionInput, EthereumBridgeState, SettlementContextV1,
    WithdrawTransitionInput, ZekoBridgeState,
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

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct NativeWithdrawalProof {
    settlement_sequence: u64,
    offset: u32,
    global_action_index: u32,
    recipient: String,
    amount: u64,
    action_fields_hash: String,
    siblings: Vec<String>,
    inner_action_root: String,
    commit_slot_upper: u32,
    claimable_slot: u64,
}

#[derive(Debug, Deserialize)]
struct ListJobsQuery {
    kind: Option<String>,
    status: Option<String>,
    limit: Option<i64>,
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
    cycle_count: Option<i64>,
    prover_gas: Option<i64>,
    base_fee_prove: Option<String>,
    max_price_per_pgu: Option<String>,
    actual_cost_prove: Option<String>,
    ethereum_gas_used: Option<i64>,
    confirmations: i32,
    explorer_url: Option<String>,
}

#[derive(Debug, FromRow)]
struct ClaimedJob {
    id: Uuid,
    kind: String,
    input: Value,
    proof_request_id: Option<String>,
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
    let proof_system: Arc<str> = env::var("PROOF_SYSTEM")
        .unwrap_or_else(|_| "groth16".to_owned())
        .into();
    let execute_only = bool_env("API_EXECUTE_ONLY")?;
    let prover_config = prover::NetworkRequestConfig {
        timeout: Duration::from_secs(u64_env("PROVER_TIMEOUT_SECS", 21_600)?),
        min_auction_period: u64_env("PROVER_MIN_AUCTION_PERIOD_SECS", 15)?,
        gas_limit: optional_u64_env("PROVER_GAS_LIMIT")?,
        max_price_per_pgu: optional_u64_env("PROVER_MAX_PRICE_PER_PGU")?,
    };
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
         SET status = 'queued', error = 'worker restarted before completion', updated_at = NOW()
         WHERE status IN ('validating', 'proof_requested', 'proving', 'submitting')",
    )
    .execute(&pool)
    .await
    .context("recover interrupted jobs")?;
    let state = AppState {
        pool,
        api_key,
        ethereum,
        proof_system,
        prover_config,
        network_explorer_base,
        execute_only,
    };
    let worker_state = state.clone();
    tokio::spawn(async move { worker_loop(worker_state).await });
    let indexer_config = indexer::Config {
        start_block: optional_u64_env("ETHEREUM_INDEXER_START_BLOCK")?,
        confirmations: u64_env("ETHEREUM_CONFIRMATIONS", 12)?,
        poll_interval: Duration::from_secs(u64_env("ETHEREUM_POLL_INTERVAL_SECS", 3)?),
    };
    let indexer_pool = state.pool.clone();
    let indexer_ethereum = state.ethereum.clone();
    tokio::spawn(async move { indexer::run(indexer_pool, indexer_ethereum, indexer_config).await });

    let protected = Router::new()
        .route("/v1/proofs/settlement", post(create_settlement))
        .route("/v1/settlements", post(create_settlement))
        .route("/v1/proofs/bridge", post(create_bridge))
        .route("/v1/bridge/deposits/prove", post(create_deposit_batch))
        .route("/v1/proofs/withdraw", post(create_withdraw))
        .route("/v1/proofs", get(list_jobs))
        .route("/v1/proofs/:id", get(get_job))
        .route_layer(middleware::from_fn_with_state(state.clone(), authenticate));

    let app = Router::new()
        .route(
            "/health",
            get(|| async { Json(serde_json::json!({"status": "ok"})) }),
        )
        .route("/graphql", post(graphql::handle))
        .route(
            "/v1/bridge/withdrawals/:sequence/:offset",
            get(get_native_withdrawal_proof),
        )
        .merge(protected)
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(bind).await?;
    tracing::info!(%bind, execute_only, "proof API listening");
    axum::serve(listener, app).await?;
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
    match canonical_deposit_batch(&state).await {
        Ok(input) => {
            create_job(
                &state,
                &headers,
                "bridge",
                serde_json::to_value(input).expect("serialize bridge input"),
            )
            .await
        }
        Err(error) => api_error(StatusCode::CONFLICT, &error.to_string()),
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

async fn load_native_withdrawal_proof(
    state: &AppState,
    sequence: u64,
    offset: u32,
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
        .context("withdrawal amount missing")?
        .parse::<u64>()?;
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
    let delay = state.ethereum.withdrawal_delay_slots().await?;

    Ok(NativeWithdrawalProof {
        settlement_sequence: sequence,
        offset,
        global_action_index: u32::try_from(target.try_get::<i64, _>("global_action_index")?)?,
        recipient,
        amount,
        action_fields_hash: target.try_get("action_fields_hash")?,
        siblings,
        inner_action_root: target.try_get("inner_action_root")?,
        commit_slot_upper,
        claimable_slot: u64::from(commit_slot_upper) + u64::from(delay),
    })
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
    let input_digest = format!("0x{}", hex::encode(Sha256::digest(input.to_string())));
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
                 'queued', 'validating', 'proof_requested', 'proving',
                 'submitting', 'submitted'
               )
         )",
    )
    .bind(conflicting_kind)
    .fetch_one(pool)
    .await?)
}

async fn get_job(State(state): State<AppState>, Path(id): Path<Uuid>) -> Response {
    let job = sqlx::query_as::<_, ProofJob>(
        "SELECT id, kind::text AS kind, status::text AS status, input, public_values,
                proof_request_id, transaction_hash, error, attempts, created_at,
                updated_at, started_at, completed_at, input_digest, cycle_count,
                prover_gas, base_fee_prove, max_price_per_pgu,
                actual_cost_prove, ethereum_gas_used,
                confirmations, explorer_url
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
                updated_at, started_at, completed_at, input_digest, cycle_count,
                prover_gas, base_fee_prove, max_price_per_pgu,
                actual_cost_prove, ethereum_gas_used,
                confirmations, explorer_url
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
                queued.proof_request_id
         FROM proof_jobs queued
         WHERE queued.status = 'queued'
           AND (queued.kind <> 'settlement' OR NOT EXISTS (
             SELECT 1 FROM proof_jobs active
             WHERE active.kind = 'settlement'
               AND active.status IN (
                 'validating', 'proof_requested', 'proving',
                 'submitting', 'submitted'
               )
           ))
           AND (queued.kind NOT IN ('settlement', 'bridge') OR NOT EXISTS (
             SELECT 1 FROM proof_jobs active
             WHERE active.kind <> queued.kind
               AND active.kind IN ('settlement', 'bridge')
               AND active.status IN (
                 'validating', 'proof_requested', 'proving',
                 'submitting', 'submitted'
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
             SET status = 'validating', attempts = attempts + 1,
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
        let preflight = prover::preflight(&job.kind, &job.input).await?;
        validate_preflight(state, &job.kind, &job.input, &preflight).await?;
        if state.execute_only {
            let cycle_count = i64::try_from(preflight.cycles())
                .context("SP1 preflight cycle count exceeds PostgreSQL BIGINT")?;
            let result = sqlx::query(
                "UPDATE proof_jobs SET status = 'executed', public_values = $2,
                        cycle_count = $3, completed_at = NOW(), updated_at = NOW()
                 WHERE id = $1 AND status = 'validating'",
            )
            .bind(job.id)
            .bind(format!("0x{}", hex::encode(preflight.public_values())))
            .bind(cycle_count)
            .execute(&state.pool)
            .await?;
            anyhow::ensure!(result.rows_affected() == 1, "proof job was cancelled");
            return Result::<()>::Ok(());
        }

        set_status(&state.pool, job.id, "proving").await?;

        let request_id = match job.proof_request_id {
            Some(request_id) => request_id,
            None => {
                let request_id = prover::request_proof(
                    &job.kind,
                    &job.input,
                    &state.proof_system,
                    &state.prover_config,
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
        let _ = sqlx::query(
            "UPDATE proof_jobs SET status = 'failed', error = $2,
                    completed_at = NOW(), updated_at = NOW()
             WHERE id = $1 AND status <> 'reorged'",
        )
        .bind(job.id)
        .bind(format!("{error:#}"))
        .execute(&state.pool)
        .await;
        let _ = sqlx::query("DELETE FROM gateway_pending_commands WHERE job_id = $1")
            .bind(job.id)
            .execute(&state.pool)
            .await;
    }
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

fn required_env(name: &str) -> Result<String> {
    env::var(name).with_context(|| format!("{name} is required"))
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
}
