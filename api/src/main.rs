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
use sqlx::{postgres::PgPoolOptions, FromRow, PgPool};
use std::{env, net::SocketAddr, sync::Arc, time::Duration};
use tokio::time::sleep;
use tower_http::trace::TraceLayer;
use uuid::Uuid;
use zeko_sp1_lib::{BridgeTransitionInput, WithdrawTransitionInput};

mod ethereum;
mod prover;

#[derive(Clone)]
struct AppState {
    pool: PgPool,
    api_key: Arc<str>,
    ethereum: ethereum::Ethereum,
    settlement_vk: Arc<str>,
    proof_system: Arc<str>,
}

#[derive(Debug, Deserialize, Serialize)]
struct SettlementRequest {
    graphql: String,
    expected: Option<SettlementExpectedState>,
}

#[derive(Debug, Deserialize, Serialize)]
struct SettlementExpectedState {
    vk_hash: String,
    action_state: String,
    current_root: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CreatedJob {
    id: Uuid,
    status: &'static str,
    status_url: String,
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
        env::var("SETTLEMENT_PRIVATE_KEY").unwrap_or_else(|_| default_key.clone()),
        env::var("BRIDGE_PRIVATE_KEY").unwrap_or_else(|_| default_key.clone()),
        env::var("WITHDRAW_PRIVATE_KEY").unwrap_or(default_key),
    )?;
    let settlement_vk: Arc<str> = std::fs::read_to_string(required_env("SETTLEMENT_VK_PATH")?)?
        .trim()
        .to_owned()
        .into();
    let proof_system: Arc<str> = env::var("PROOF_SYSTEM")
        .unwrap_or_else(|_| "groth16".to_owned())
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
    sqlx::query(
        "UPDATE proof_jobs
         SET status = 'queued', error = 'worker restarted before completion', updated_at = NOW()
         WHERE status IN ('validating', 'proving', 'submitting')",
    )
    .execute(&pool)
    .await
    .context("recover interrupted jobs")?;
    let state = AppState {
        pool,
        api_key,
        ethereum,
        settlement_vk,
        proof_system,
    };
    let worker_state = state.clone();
    tokio::spawn(async move { worker_loop(worker_state).await });

    let protected = Router::new()
        .route("/v1/proofs/settlement", post(create_settlement))
        .route("/v1/proofs/bridge", post(create_bridge))
        .route("/v1/proofs/withdraw", post(create_withdraw))
        .route("/v1/proofs", get(list_jobs))
        .route("/v1/proofs/:id", get(get_job))
        .route_layer(middleware::from_fn_with_state(state.clone(), authenticate));

    let app = Router::new()
        .route(
            "/health",
            get(|| async { Json(serde_json::json!({"status": "ok"})) }),
        )
        .merge(protected)
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(bind).await?;
    tracing::info!(%bind, "proof API listening");
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
    Json(request): Json<SettlementRequest>,
) -> Response {
    if request.graphql.trim().is_empty() {
        return api_error(StatusCode::BAD_REQUEST, "graphql must not be empty");
    }
    create_job(
        &state,
        &headers,
        "settlement",
        serde_json::to_value(request).unwrap(),
    )
    .await
}

async fn create_bridge(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<BridgeTransitionInput>,
) -> Response {
    create_job(
        &state,
        &headers,
        "bridge",
        serde_json::to_value(input).unwrap(),
    )
    .await
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
    let idempotency_key = headers
        .get("idempotency-key")
        .and_then(|value| value.to_str().ok());
    let result = sqlx::query_scalar::<_, Uuid>(
        "INSERT INTO proof_jobs (id, kind, input, idempotency_key)
         VALUES ($1, $2::proof_kind, $3, $4)
         ON CONFLICT (idempotency_key) WHERE idempotency_key IS NOT NULL
         DO UPDATE SET idempotency_key = EXCLUDED.idempotency_key
         RETURNING id",
    )
    .bind(id)
    .bind(kind)
    .bind(input)
    .bind(idempotency_key)
    .fetch_one(&state.pool)
    .await;

    match result {
        Ok(id) => (
            StatusCode::ACCEPTED,
            Json(CreatedJob {
                id,
                status: "queued",
                status_url: format!("/v1/proofs/{id}"),
            }),
        )
            .into_response(),
        Err(error) => {
            tracing::error!(%error, "create proof job");
            api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "could not create proof job",
            )
        }
    }
}

async fn get_job(State(state): State<AppState>, Path(id): Path<Uuid>) -> Response {
    let job = sqlx::query_as::<_, ProofJob>(
        "SELECT id, kind::text AS kind, status::text AS status, input, public_values,
                proof_request_id, transaction_hash, error, attempts, created_at,
                updated_at, started_at, completed_at
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
                updated_at, started_at, completed_at
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
    let job = sqlx::query_as::<_, ClaimedJob>(
        "SELECT id, kind::text AS kind, input, proof_request_id
         FROM proof_jobs
         WHERE status = 'queued'
         ORDER BY created_at
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

async fn process_job(state: &AppState, job: ClaimedJob) {
    let result = async {
        let preflight = prover::preflight(&job.kind, &job.input, &state.settlement_vk).await?;
        validate_preflight(state, &job.kind, &job.input, &preflight).await?;
        set_status(&state.pool, job.id, "proving").await?;

        let request_id = match job.proof_request_id {
            Some(request_id) => request_id,
            None => {
                let request_id = prover::request_proof(
                    &job.kind,
                    &job.input,
                    &state.settlement_vk,
                    &state.proof_system,
                )
                .await?;
                sqlx::query(
                    "UPDATE proof_jobs SET proof_request_id = $2, updated_at = NOW() WHERE id = $1",
                )
                .bind(job.id)
                .bind(&request_id)
                .execute(&state.pool)
                .await?;
                request_id
            }
        };
        let proof = prover::wait_proof(&job.kind, &request_id).await?;
        set_status(&state.pool, job.id, "submitting").await?;
        let transaction_hash = state
            .ethereum
            .submit(&job.kind, proof.public_values.clone(), proof.proof.bytes())
            .await?;
        sqlx::query(
            "UPDATE proof_jobs SET status = 'confirmed', public_values = $2,
                    proof_request_id = $3, transaction_hash = $4,
                    completed_at = NOW(), updated_at = NOW()
             WHERE id = $1",
        )
        .bind(job.id)
        .bind(format!("0x{}", hex::encode(proof.public_values)))
        .bind(request_id)
        .bind(transaction_hash)
        .execute(&state.pool)
        .await?;
        Result::<()>::Ok(())
    }
    .await;

    if let Err(error) = result {
        tracing::error!(job_id = %job.id, %error, "proof job failed");
        let _ = sqlx::query(
            "UPDATE proof_jobs SET status = 'failed', error = $2,
                    completed_at = NOW(), updated_at = NOW() WHERE id = $1",
        )
        .bind(job.id)
        .bind(format!("{error:#}"))
        .execute(&state.pool)
        .await;
    }
}

async fn validate_preflight(
    state: &AppState,
    kind: &str,
    input: &Value,
    preflight: &prover::Preflight,
) -> Result<()> {
    let local_vkey = prover::program_vkey(kind).await?;
    match preflight {
        prover::Preflight::Settlement(values) => {
            let chain = state.ethereum.settlement_state().await?;
            ensure_hex_eq(
                &local_vkey,
                &chain.program_vkey.to_string(),
                "settlement program vkey",
            )?;
            ensure_bytes_eq(values.vk_hash, chain.vk_hash, "vk hash")?;
            ensure_bytes_eq(
                values.action_state_before,
                chain.action_state,
                "action state",
            )?;
            ensure_bytes_eq(values.state_before[3], chain.current_root, "current root")?;
        }
        prover::Preflight::Bridge(values) => {
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
        }
        prover::Preflight::Withdraw(values) => {
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
    sqlx::query(
        "UPDATE proof_jobs SET status = $2::proof_status, updated_at = NOW() WHERE id = $1",
    )
    .bind(id)
    .bind(status)
    .execute(pool)
    .await?;
    Ok(())
}

fn api_error(status: StatusCode, message: &str) -> Response {
    (status, Json(serde_json::json!({"error": message}))).into_response()
}

fn required_env(name: &str) -> Result<String> {
    env::var(name).with_context(|| format!("{name} is required"))
}
