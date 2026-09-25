//! Durable ownership of the outer action-state writer. Preparation leases expire;
//! accepted jobs do not. All admission paths lock the same row before inserting.
use alloy::primitives::U256;
use anyhow::{Context, Result};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::{PgPool, Postgres, Row, Transaction};
use uuid::Uuid;

use crate::{
    ethereum::{BlockRef, SettlementState},
    AppState, ProofKind,
};

#[derive(Debug)]
pub struct CoordinationError {
    pub code: &'static str,
    message: &'static str,
}

impl std::fmt::Display for CoordinationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}
impl std::error::Error for CoordinationError {}

fn conflict(code: &'static str, message: &'static str) -> anyhow::Error {
    CoordinationError { code, message }.into()
}

pub fn error_code(error: &anyhow::Error) -> Option<&'static str> {
    error.downcast_ref::<CoordinationError>().map(|e| e.code)
}

pub fn error_response(error: anyhow::Error) -> Response {
    let code = error_code(&error).unwrap_or("COORDINATION_UNAVAILABLE");
    let status = if error_code(&error).is_some() {
        StatusCode::CONFLICT
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    tracing::warn!(%error, "outer writer coordination failed");
    (
        status,
        Json(json!({"errorCode": code, "message": error.to_string()})),
    )
        .into_response()
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ReservationToken {
    pub id: Uuid,
    pub fencing_token: String,
}

/// Mina hashes omit proof/signature bytes. A newly fenced proof attempt may
/// therefore keep the same Mina hash, while retries of its frozen payload must
/// still resolve to the original job.
pub fn settlement_idempotency_key(hash: &str, reservation: Option<&ReservationToken>) -> String {
    match reservation {
        Some(reservation) => format!("{hash}:reservation:{}", reservation.id),
        None => hash.to_owned(), // Retain idempotent replay of legacy accepted jobs.
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Checkpoint {
    pub outer_state_fields: Vec<String>,
    pub ledger_hash: String,
    pub action_state: String,
    pub action_state_length: u32,
    pub block_number: u64,
    pub block_hash: String,
}

impl Checkpoint {
    fn from_chain(block: &BlockRef, state: &SettlementState) -> Self {
        Self {
            outer_state_fields: state.outer_state.iter().map(|v| decimal(v.0)).collect(),
            ledger_hash: decimal(state.current_root.0),
            action_state: decimal(state.action_state.0),
            action_state_length: state.outer_action_state_length,
            block_number: block.number,
            block_hash: block.hash.to_string(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Reservation {
    pub reservation_id: Uuid,
    pub fencing_token: String,
    pub expires_at: Option<DateTime<Utc>>,
    pub checkpoint: Checkpoint,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcquireRequest {
    owner_id: String,
    ttl_seconds: Option<u32>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenRequest {
    fencing_token: String,
    ttl_seconds: Option<u32>,
}

#[derive(sqlx::FromRow)]
struct Owner {
    reservation_id: Option<Uuid>,
    owner_id: Option<String>,
    fencing_token: i64,
    expires_at: Option<DateTime<Utc>>,
    checkpoint: Option<Value>,
    job_id: Option<Uuid>,
}

impl Owner {
    fn response(self) -> Result<Reservation> {
        Ok(Reservation {
            reservation_id: self.reservation_id.context("writer has no reservation")?,
            fencing_token: self.fencing_token.to_string(),
            expires_at: self.expires_at,
            checkpoint: serde_json::from_value(
                self.checkpoint.context("writer has no checkpoint")?,
            )?,
        })
    }
    fn matches(&self, token: &ReservationToken) -> bool {
        self.reservation_id == Some(token.id)
            && token.fencing_token.parse::<i64>().ok() == Some(self.fencing_token)
    }
}

fn ttl(value: Option<u32>) -> Result<i64> {
    let seconds = value.unwrap_or(120);
    if !(30..=600).contains(&seconds) {
        return Err(conflict(
            "INVALID_RESERVATION",
            "ttlSeconds must be between 30 and 600",
        ));
    }
    Ok(i64::from(seconds))
}

/// The config row is also the worker's claim lock. Always take it first.
pub async fn lock(tx: &mut Transaction<'_, Postgres>) -> Result<()> {
    let ready = sqlx::query_scalar::<_, bool>(
        "SELECT recovery_ready FROM gateway_config WHERE id = TRUE FOR UPDATE",
    )
    .fetch_one(&mut **tx)
    .await?;
    if !ready {
        return Err(conflict(
            "OUTER_WRITER_BUSY",
            "gateway is replaying finalized state",
        ));
    }
    sqlx::query("SELECT id FROM gateway_outer_writer WHERE id = TRUE FOR UPDATE")
        .fetch_one(&mut **tx)
        .await?;
    // A tx hash is persisted before broadcasting. A failure with such a hash is
    // ambiguous until reconciliation proves finality or an Ethereum revert.
    sqlx::query(
        "UPDATE gateway_outer_writer SET reservation_id = NULL, owner_id = NULL,
             expires_at = NULL, checkpoint = NULL, job_id = NULL
         WHERE id = TRUE AND (
             (job_id IS NULL AND expires_at <= NOW()) OR
             job_id IN (SELECT id FROM proof_jobs WHERE
                 status IN ('confirmed', 'ethereum_reverted') OR
                 (transaction_hash IS NULL AND prepared_transaction IS NULL AND status IN
                    ('executed', 'failed', 'proof_failed', 'rejected', 'reorged')))
         )",
    )
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn owner(tx: &mut Transaction<'_, Postgres>) -> Result<Owner> {
    Ok(sqlx::query_as::<_, Owner>(
        "SELECT reservation_id, owner_id, fencing_token, expires_at, checkpoint, job_id
         FROM gateway_outer_writer WHERE id = TRUE",
    )
    .fetch_one(&mut **tx)
    .await?)
}

async fn ensure_no_jobs(tx: &mut Transaction<'_, Postgres>, except: Option<Uuid>) -> Result<()> {
    let busy = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM proof_jobs WHERE kind IN ('settlement', 'bridge')
         AND ($1::uuid IS NULL OR id <> $1) AND NOT (
            status IN ('confirmed', 'ethereum_reverted') OR
            (transaction_hash IS NULL AND prepared_transaction IS NULL AND status IN
                ('executed', 'failed', 'proof_failed', 'rejected', 'reorged'))))",
    )
    .bind(except)
    .fetch_one(&mut **tx)
    .await?;
    if busy {
        return Err(conflict(
            "OUTER_WRITER_BUSY",
            "an outer-state job is queued or active",
        ));
    }
    Ok(())
}

async fn virtual_account(tx: &mut Transaction<'_, Postgres>) -> Result<Value> {
    sqlx::query_scalar::<_, Value>(
        "SELECT account_json FROM gateway_accounts
         WHERE public_key = (SELECT outer_public_key FROM gateway_config WHERE id = TRUE)
           AND token_id = '1'",
    )
    .fetch_optional(&mut **tx)
    .await?
    .context("virtual outer account is not configured")
}

/// Cheap advisory precheck for the automatic bridge loop. Admission still repeats
/// the check while holding the writer lock, so this query is never authoritative.
pub async fn busy(pool: &PgPool) -> Result<bool> {
    Ok(sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM gateway_outer_writer
            WHERE id = TRUE AND job_id IS NULL AND expires_at > NOW())
         OR EXISTS(SELECT 1 FROM proof_jobs WHERE kind IN ('settlement', 'bridge')
            AND NOT (status IN ('confirmed', 'ethereum_reverted') OR
                (transaction_hash IS NULL AND prepared_transaction IS NULL AND status IN
                    ('executed', 'failed', 'proof_failed', 'rejected', 'reorged'))))",
    )
    .fetch_one(pool)
    .await?)
}

fn account_matches(account: &Value, checkpoint: &Checkpoint) -> bool {
    account.get("zkappState") == Some(&json!(checkpoint.outer_state_fields))
        && account.pointer("/actionState/0").and_then(Value::as_str)
            == Some(checkpoint.action_state.as_str())
}

async fn acquire_with_checkpoint(
    pool: &PgPool,
    request: &AcquireRequest,
    checkpoint: Checkpoint,
) -> Result<Reservation> {
    let seconds = ttl(request.ttl_seconds)?;
    if request.owner_id.is_empty() || request.owner_id.len() > 200 {
        return Err(conflict(
            "INVALID_RESERVATION",
            "ownerId must contain 1 to 200 bytes",
        ));
    }
    let mut tx = pool.begin().await?;
    lock(&mut tx).await?;
    let current = owner(&mut tx).await?;
    if current.reservation_id.is_some() {
        if current.job_id.is_some()
            || current.owner_id.as_deref() != Some(request.owner_id.as_str())
        {
            return Err(conflict(
                "OUTER_WRITER_BUSY",
                "another writer owns preparation or submission",
            ));
        }
        sqlx::query("UPDATE gateway_outer_writer SET expires_at = NOW() + $1 * INTERVAL '1 second' WHERE id = TRUE")
            .bind(seconds).execute(&mut *tx).await?;
    } else {
        ensure_no_jobs(&mut tx, None).await?;
        if !account_matches(&virtual_account(&mut tx).await?, &checkpoint) {
            return Err(conflict(
                "STALE_CHECKPOINT",
                "gateway has not caught up to the finalized checkpoint",
            ));
        }
        sqlx::query(
            "UPDATE gateway_outer_writer SET reservation_id = $1, owner_id = $2,
                 fencing_token = fencing_token + 1, expires_at = NOW() + $3 * INTERVAL '1 second',
                 checkpoint = $4, job_id = NULL WHERE id = TRUE",
        )
        .bind(Uuid::new_v4())
        .bind(&request.owner_id)
        .bind(seconds)
        .bind(serde_json::to_value(checkpoint)?)
        .execute(&mut *tx)
        .await?;
    }
    let response = owner(&mut tx).await?.response()?;
    tx.commit().await?;
    Ok(response)
}

pub async fn acquire(
    State(state): State<AppState>,
    Json(request): Json<AcquireRequest>,
) -> Response {
    let result = async {
        let (block, checkpoint) = match state.ethereum_finality_mode {
            crate::indexer::FinalityMode::Finalized => {
                state.ethereum.settlement_state_at_finalized().await?
            }
            crate::indexer::FinalityMode::Confirmations => {
                // Local Anvil's consensus-finalized tag does not advance. Use the
                // same explicitly configured boundary as the local indexer.
                let number = sqlx::query_scalar::<_, Option<i64>>(
                    "SELECT MAX(block_number) FROM gateway_blocks WHERE canonical AND finalized",
                )
                .fetch_one(&state.pool)
                .await?
                .ok_or_else(|| {
                    conflict("OUTER_WRITER_BUSY", "no indexed finalized checkpoint yet")
                })?;
                let block = state.ethereum.block(u64::try_from(number)?).await?;
                let checkpoint = state.ethereum.settlement_state_at(&block).await?;
                (block, checkpoint)
            }
        };
        acquire_with_checkpoint(
            &state.pool,
            &request,
            Checkpoint::from_chain(&block, &checkpoint),
        )
        .await
    }
    .await;
    match result {
        Ok(value) => Json(value).into_response(),
        Err(e) => error_response(e),
    }
}

async fn renew_token(pool: &PgPool, token: &ReservationToken, seconds: i64) -> Result<Reservation> {
    let mut tx = pool.begin().await?;
    // Do not clear a terminal attached row before answering an in-flight renew.
    sqlx::query("SELECT id FROM gateway_config WHERE id = TRUE FOR UPDATE")
        .fetch_one(&mut *tx)
        .await?;
    let current = owner(&mut tx).await?;
    if !current.matches(token) {
        return Err(conflict(
            "RESERVATION_EXPIRED",
            "reservation was released or fenced",
        ));
    }
    if current.job_id.is_none() {
        let result = sqlx::query(
            "UPDATE gateway_outer_writer SET expires_at = NOW() + $1 * INTERVAL '1 second'
             WHERE id = TRUE AND expires_at > NOW()",
        )
        .bind(seconds)
        .execute(&mut *tx)
        .await?;
        if result.rows_affected() != 1 {
            return Err(conflict("RESERVATION_EXPIRED", "preparation lease expired"));
        }
    }
    let response = owner(&mut tx).await?.response()?;
    tx.commit().await?;
    Ok(response)
}

pub async fn renew(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(request): Json<TokenRequest>,
) -> Response {
    let result = async {
        renew_token(
            &state.pool,
            &ReservationToken {
                id,
                fencing_token: request.fencing_token,
            },
            ttl(request.ttl_seconds)?,
        )
        .await
    }
    .await;
    match result {
        Ok(value) => Json(value).into_response(),
        Err(e) => error_response(e),
    }
}

pub async fn release(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(request): Json<TokenRequest>,
) -> Response {
    let result = async {
        let mut tx = state.pool.begin().await?;
        lock(&mut tx).await?;
        let current = owner(&mut tx).await?;
        if current.reservation_id.is_none() { tx.commit().await?; return Ok(()); }
        if !current.matches(&ReservationToken { id, fencing_token: request.fencing_token }) {
            return Err(conflict("RESERVATION_EXPIRED", "reservation was released or fenced"));
        }
        if current.job_id.is_some() {
            return Err(conflict("OUTER_WRITER_BUSY", "accepted job ownership lasts until its outcome is reconciled"));
        }
        sqlx::query("UPDATE gateway_outer_writer SET reservation_id = NULL, owner_id = NULL, expires_at = NULL, checkpoint = NULL WHERE id = TRUE")
            .execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(())
    }.await;
    match result {
        Ok(()) => Json(json!({"released": true})).into_response(),
        Err(e) => error_response(e),
    }
}

pub fn decimal(bytes: [u8; 32]) -> String {
    U256::from_be_bytes(bytes).to_string()
}

fn field(value: &Value) -> Result<String> {
    let encoded = value
        .as_str()
        .context("binding field must be a hex string")?;
    let bytes: [u8; 32] = hex::decode(
        encoded
            .strip_prefix("0x")
            .context("binding field must be 0x-prefixed")?,
    )?
    .try_into()
    .map_err(|_| anyhow::anyhow!("binding field must be 32 bytes"))?;
    Ok(decimal(bytes))
}

pub fn ledger_hashes(input: &Value) -> Result<(String, String)> {
    Ok((
        field(
            input
                .pointer("/proof/binding/stateBefore/fields/2")
                .context("missing source ledger binding")?,
        )?,
        field(
            input
                .pointer("/proof/binding/actions/0/1")
                .context("missing target ledger binding")?,
        )?,
    ))
}

fn proof_matches_checkpoint(input: &Value, checkpoint: &Checkpoint) -> Result<bool> {
    let fields = input
        .pointer("/proof/binding/stateBefore/fields")
        .and_then(Value::as_array)
        .context("missing outer state binding")?;
    // Index 36 is the action-state precondition in the OCaml V1 body wire.
    // This is an early freshness check, never a replacement for guest verification.
    let action = input
        .pointer("/proof/binding/accountUpdateBody/fieldElements/36")
        .context("missing action state binding")?;
    Ok(
        fields.iter().map(field).collect::<Result<Vec<_>>>()? == checkpoint.outer_state_fields
            && field(action)? == checkpoint.action_state,
    )
}

/// Call only under `lock`, inside the transaction inserting this new job.
pub async fn attach_settlement(
    tx: &mut Transaction<'_, Postgres>,
    job_id: Uuid,
    token: Option<&ReservationToken>,
    input: &Value,
) -> Result<()> {
    let token = token.ok_or_else(|| {
        conflict(
            "RESERVATION_REQUIRED",
            "acquire a preparation reservation before proving",
        )
    })?;
    let current = owner(tx).await?;
    if !current.matches(token) || current.job_id.is_some() {
        return Err(conflict(
            "RESERVATION_EXPIRED",
            "preparation token is no longer the current owner",
        ));
    }
    ensure_no_jobs(tx, Some(job_id)).await?;
    let checkpoint: Checkpoint = serde_json::from_value(
        current
            .checkpoint
            .context("reservation checkpoint missing")?,
    )?;
    if !account_matches(&virtual_account(tx).await?, &checkpoint)
        || !proof_matches_checkpoint(input, &checkpoint)?
    {
        return Err(conflict(
            "STALE_CHECKPOINT",
            "settlement proof does not match its reserved finalized checkpoint",
        ));
    }
    ledger_hashes(input)?;
    let result = sqlx::query(
        "UPDATE gateway_outer_writer SET job_id = $1, expires_at = NULL
         WHERE id = TRUE AND expires_at > NOW()",
    )
    .bind(job_id)
    .execute(&mut **tx)
    .await?;
    if result.rows_affected() != 1 {
        return Err(conflict(
            "RESERVATION_EXPIRED",
            "preparation lease expired before admission",
        ));
    }
    Ok(())
}

/// Bridges are prepared by the gateway, then atomically rechecked at admission.
pub async fn attach_bridge(
    tx: &mut Transaction<'_, Postgres>,
    job_id: Uuid,
    input: &Value,
) -> Result<()> {
    if owner(tx).await?.reservation_id.is_some() {
        return Err(conflict(
            "OUTER_WRITER_BUSY",
            "another writer owns preparation or submission",
        ));
    }
    ensure_no_jobs(tx, Some(job_id)).await?;
    let expected = field(
        input
            .pointer("/zeko/action_state")
            .context("missing bridge action state")?,
    )?;
    if virtual_account(tx)
        .await?
        .pointer("/actionState/0")
        .and_then(Value::as_str)
        != Some(expected.as_str())
    {
        return Err(conflict(
            "STALE_CHECKPOINT",
            "bridge input does not match finalized gateway state",
        ));
    }
    sqlx::query(
        "UPDATE gateway_outer_writer SET reservation_id = $1, owner_id = $2,
            fencing_token = fencing_token + 1, expires_at = NULL, checkpoint = NULL,
            job_id = $3 WHERE id = TRUE",
    )
    .bind(Uuid::new_v4())
    .bind(format!("bridge:{job_id}"))
    .bind(job_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

pub async fn attach_job(
    tx: &mut Transaction<'_, Postgres>,
    job_id: Uuid,
    kind: ProofKind,
    input: &Value,
) -> Result<()> {
    match kind {
        ProofKind::Bridge => attach_bridge(tx, job_id, input).await,
        ProofKind::Settlement => {
            let token = input
                .get("reservation")
                .filter(|value| !value.is_null())
                .cloned()
                .map(serde_json::from_value::<ReservationToken>)
                .transpose()?;
            attach_settlement(tx, job_id, token.as_ref(), input).await
        }
    }
}

pub async fn outcome(State(state): State<AppState>, Path(hash): Path<String>) -> Response {
    let result = async {
        anyhow::ensure!(
            crate::is_bytes32_hex(&hash),
            "invalid Mina transaction hash"
        );
        let row = sqlx::query(
            "SELECT id, status::text AS status, transaction_hash, prepared_transaction,
                    error, error_class, input
             FROM proof_jobs WHERE kind = 'settlement' AND input->>'minaTransactionHash' = $1
             ORDER BY created_at DESC LIMIT 1",
        )
        .bind(&hash)
        .fetch_optional(&state.pool)
        .await?;
        let Some(row) = row else {
            return Ok(None);
        };
        let status: String = row.try_get("status")?;
        let tx_hash: Option<String> = row.try_get("transaction_hash")?;
        let input: Value = row.try_get("input")?;
        let hashes = ledger_hashes(&input).ok();
        let prepared: Option<Value> = row.try_get("prepared_transaction")?;
        let error_class: Option<String> = row.try_get("error_class")?;
        let retryable = status == "ethereum_reverted"
            || (tx_hash.is_none()
                && prepared.is_none()
                && matches!(status.as_str(), "failed" | "proof_failed")
                && matches!(error_class.as_deref(), Some("stale_checkpoint" | "expired")))
            || (matches!(status.as_str(), "queued" | "approved" | "submitting")
                && error_class.as_deref() != Some("proof_request_ambiguous"));
        Ok::<_, anyhow::Error>(Some(json!({
            "jobId": row.try_get::<Uuid, _>("id")?, "status": status,
            "finalized": status == "confirmed", "retryable": retryable,
            "transactionHash": tx_hash, "minaTransactionHash": hash,
            "error": row.try_get::<Option<String>, _>("error")?,
            "errorClass": error_class,
            "sourceLedgerHash": hashes.as_ref().map(|v| &v.0),
            "targetLedgerHash": hashes.as_ref().map(|v| &v.1)
        })))
    }
    .await;
    match result {
        Ok(Some(value)) => Json(value).into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(json!({"errorCode":"UNKNOWN_SETTLEMENT"})),
        )
            .into_response(),
        Err(e) => error_response(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::to_bytes, http::HeaderMap, response::IntoResponse};
    use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
    use std::{str::FromStr, sync::Arc};
    use tokio::sync::Barrier;

    fn checkpoint() -> Checkpoint {
        Checkpoint {
            outer_state_fields: (0..8).map(|i| i.to_string()).collect(),
            ledger_hash: "2".into(),
            action_state: "11".into(),
            action_state_length: 3,
            block_number: 100,
            block_hash: format!("0x{:064x}", 100),
        }
    }

    fn acquire_request(owner: &str) -> AcquireRequest {
        AcquireRequest {
            owner_id: owner.into(),
            ttl_seconds: Some(120),
        }
    }

    fn token(reservation: &Reservation) -> ReservationToken {
        ReservationToken {
            id: reservation.reservation_id,
            fencing_token: reservation.fencing_token.clone(),
        }
    }

    fn proof(checkpoint: &Checkpoint) -> Value {
        let fields: Vec<String> = checkpoint
            .outer_state_fields
            .iter()
            .map(|s| format!("0x{:064x}", s.parse::<u64>().unwrap()))
            .collect();
        let mut body = vec![format!("0x{:064x}", 0); 37];
        body[36] = format!("0x{:064x}", checkpoint.action_state.parse::<u64>().unwrap());
        json!({"proof":{"binding":{
            "stateBefore":{"fields": fields}, "accountUpdateBody":{"fieldElements": body},
            "actions":[[format!("0x{:064x}", 0), format!("0x{:064x}", 9)]]
        }}})
    }

    fn app_state(pool: PgPool) -> AppState {
        AppState {
            pool,
            archive_pool: None,
            api_key: "test".into(),
            ethereum: crate::ethereum::Ethereum::new(
                "http://127.0.0.1:1".into(),
                format!("0x{}", "11".repeat(20)),
                format!("0x{}", "22".repeat(20)),
                "01".repeat(32),
                "02".repeat(32),
            )
            .unwrap(),
            proof_system: "groth16".into(),
            prover_config: crate::prover::NetworkRequestConfig {
                timeout: std::time::Duration::from_secs(1),
                min_auction_period: 1,
                gas_limit: None,
                max_price_per_pgu: None,
            },
            network_explorer_base: "http://unused".into(),
            execute_only: false,
            local_mock_submit: false,
            require_proof_approval: false,
            min_remaining_slots: 1,
            submission_min_remaining_slots: 1,
            ethereum_finality_mode: crate::indexer::FinalityMode::Finalized,
            ethereum_confirmations: 12,
            sequencer_graphql_url: None,
            inner_public_key: None,
            fee_payer_public_key: Some("payer".into()),
            http_client: reqwest::Client::new(),
        }
    }

    async fn json_response(response: Response) -> Value {
        let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    async fn admit(state: &AppState, graphql: bool, input: &Value) -> Value {
        if graphql {
            let request = serde_json::from_value(json!({
                "query":"mutation { sendZkapp }", "variables":{
                    "gatewayToken":"test", "settlement":input,
                },
            }))
            .unwrap();
            json_response(
                crate::graphql::handle(State(state.clone()), Json(request))
                    .await
                    .into_response(),
            )
            .await
        } else {
            let request = serde_json::from_value(input.clone()).unwrap();
            json_response(
                crate::create_settlement(State(state.clone()), HeaderMap::new(), Json(request))
                    .await,
            )
            .await
        }
    }

    async fn reservation_scoped_attempts(pool: PgPool, graphql: bool) {
        sqlx::query("INSERT INTO gateway_config (genesis_timestamp, fork_slot, account_creation_fee, state_hash, recovery_ready, outer_public_key) VALUES ('0', 0, '1', 'test', TRUE, 'outer')")
            .execute(&pool).await.unwrap();
        let initial = checkpoint();
        sqlx::query("INSERT INTO gateway_accounts (public_key, token_id, account_json) VALUES ('outer', '1', $1), ('payer', '1', '{\"nonce\":\"0\"}')")
            .bind(json!({"zkappState":initial.outer_state_fields,"actionState":[initial.action_state]}))
            .execute(&pool).await.unwrap();
        let state = app_state(pool.clone());
        let first = acquire_with_checkpoint(&pool, &acquire_request("sequencer"), initial.clone())
            .await
            .unwrap();
        let hash = format!("0x{:064x}", 123);
        let mut input = proof(&initial);
        input["schemaVersion"] = json!(1);
        input["minaTransactionHash"] = json!(hash);
        input["reservation"] = serde_json::to_value(token(&first)).unwrap();
        input["outerAccountPublicKey"] = json!("outer");
        input["feePayerPublicKey"] = json!("payer");
        input["nonce"] = json!(0);
        input["commandBase64"] = json!("fixture-command");
        for name in [
            "vkJson",
            "proofJson",
            "publicInputSkeletonJson",
            "appStatementJson",
        ] {
            input["proof"][name] = json!("fixture");
        }
        input["proof"]["binding"]["minaSignatureKind"] = json!("testnet");
        input["proof"]["binding"]["accountUpdateBody"]["packed"] = json!([]);
        let accepted = admit(&state, graphql, &input).await;
        assert!(
            accepted.get("errors").is_none() && accepted.get("errorCode").is_none(),
            "{accepted}"
        );
        let repeated = admit(&state, graphql, &input).await;
        assert_eq!(
            accepted, repeated,
            "frozen retries must return the same job/result"
        );
        let first_id: Uuid = sqlx::query_scalar("SELECT id FROM proof_jobs")
            .fetch_one(&pool)
            .await
            .unwrap();

        sqlx::query(
            "UPDATE proof_jobs SET status = 'failed', error_class = 'expired' WHERE id = $1",
        )
        .bind(first_id)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("DELETE FROM gateway_pending_commands WHERE job_id = $1")
            .bind(first_id)
            .execute(&pool)
            .await
            .unwrap();
        let expired = json_response(outcome(State(state.clone()), Path(hash.clone())).await).await;
        assert_eq!(
            expired["retryable"], true,
            "unsigned expired receipts can rebuild from a valid witness"
        );
        sqlx::query("UPDATE proof_jobs SET prepared_transaction = '{}' WHERE id = $1")
            .bind(first_id)
            .execute(&pool)
            .await
            .unwrap();
        let ambiguous =
            json_response(outcome(State(state.clone()), Path(hash.clone())).await).await;
        assert_eq!(
            ambiguous["retryable"], false,
            "signed expiry must reconcile the same transaction"
        );
        sqlx::query("UPDATE proof_jobs SET prepared_transaction = NULL WHERE id = $1")
            .bind(first_id)
            .execute(&pool)
            .await
            .unwrap();

        let second = acquire_with_checkpoint(&pool, &acquire_request("sequencer"), initial)
            .await
            .unwrap();
        assert_ne!(first.reservation_id, second.reservation_id);
        input["reservation"] = serde_json::to_value(token(&second)).unwrap();
        input["proof"]["proofJson"] = json!("fresh-proof-same-Mina-hash");
        let retried = admit(&state, graphql, &input).await;
        assert!(
            retried.get("errors").is_none() && retried.get("errorCode").is_none(),
            "{retried}"
        );
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM proof_jobs")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 2, "a fresh fenced attempt may retain the Mina hash");
        let latest = json_response(outcome(State(state.clone()), Path(hash.clone())).await).await;
        assert_ne!(latest["jobId"], json!(first_id));
        let second_id: Uuid = serde_json::from_value(latest["jobId"].clone()).unwrap();
        sqlx::query("UPDATE proof_jobs SET status = 'confirmed' WHERE id = $1")
            .bind(second_id)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM gateway_pending_commands WHERE job_id = $1")
            .bind(second_id)
            .execute(&pool)
            .await
            .unwrap();
        let mut finalized = checkpoint();
        finalized.ledger_hash = "9".into();
        finalized.outer_state_fields[2] = "9".into();
        finalized.action_state = "12".into();
        sqlx::query("UPDATE gateway_accounts SET account_json = $1 WHERE public_key = 'outer'")
            .bind(json!({"zkappState":finalized.outer_state_fields,"actionState":[finalized.action_state]}))
            .execute(&pool).await.unwrap();
        let third = acquire_with_checkpoint(&pool, &acquire_request("sequencer"), finalized)
            .await
            .unwrap();
        let frozen_success = input.clone();
        input["reservation"] = serde_json::to_value(token(&third)).unwrap();
        let stale = admit(&state, graphql, &input).await;
        let code = if graphql {
            &stale["errors"][0]["extensions"]["code"]
        } else {
            &stale["errorCode"]
        };
        assert_eq!(
            code, "STALE_CHECKPOINT",
            "confirmed state cannot be applied twice: {stale}"
        );
        let replay = admit(&state, graphql, &frozen_success).await;
        assert_eq!(
            replay, retried,
            "an exact confirmed replay remains idempotent"
        );
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM proof_jobs")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 2);
        let pending: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM gateway_pending_commands")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(
            pending, 0,
            "confirmed replay must not resurrect the pool command"
        );
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn graphql_reproof_scopes_idempotency_to_the_reservation(pool: PgPool) {
        reservation_scoped_attempts(pool, true).await;
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn rest_reproof_scopes_default_idempotency_to_the_reservation(pool: PgPool) {
        reservation_scoped_attempts(pool, false).await;
    }

    #[cfg(feature = "fake-prover-tests")]
    #[sqlx::test(migrations = "./migrations")]
    async fn bridge_preparation_and_validation_each_use_one_block_snapshot(pool: PgPool) {
        use alloy::primitives::B256;
        use std::sync::Mutex;
        use zeko_sp1_lib::{BridgeOuterActionV2, BridgeTransitionPublicValuesV2};
        let requests = Arc::new(Mutex::new(Vec::<Value>::new()));
        let observed = requests.clone();
        let server = crate::rpc::test_support::serve(move |request| {
            observed.lock().unwrap().push(request.clone());
            async move {
                let selector = |name: &str| {
                    format!("0x{}", hex::encode(&alloy::primitives::keccak256(name.as_bytes())[..4]))
                };
                let result = match request["method"].as_str().unwrap() {
                    "eth_getBlockByNumber" => {
                        let empty: alloy::rpc::types::Block = Default::default();
                        let mut block = serde_json::to_value(empty).unwrap();
                        block["hash"] = json!(B256::repeat_byte(7).to_string());
                        block["number"] = json!("0x64");
                        block
                    }
                    "eth_chainId" => json!("0x7a69"),
                    "eth_call" => {
                        assert_eq!(request["params"][1], json!({
                            "blockHash":B256::repeat_byte(7).to_string(), "requireCanonical":true,
                        }));
                        let data = request["params"][0]["input"].as_str()
                            .or_else(|| request["params"][0]["data"].as_str()).unwrap();
                        if data == selector("depositNonce()") { json!(format!("0x{:064x}", 1)) }
                        else { json!(format!("0x{}", "00".repeat(if data == selector("outerState()") { 256 } else { 32 }))) }
                    }
                    method => panic!("unexpected RPC {method}"),
                };
                Json(json!({"jsonrpc":"2.0", "id":request["id"], "result":result})).into_response()
            }
        }).await;
        let hash = B256::repeat_byte(7).to_string();
        let zero = B256::ZERO.to_string();
        sqlx::query("INSERT INTO gateway_blocks (block_number, block_hash, parent_hash, finalized) VALUES (100, $1, $2, TRUE)")
            .bind(&hash).bind(&zero).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO gateway_bridge_deposits (nonce, deposit_leaf, old_deposit_state, new_deposit_state, token, sender, zeko_recipient, ethereum_amount, zeko_amount, timeout, ethereum_block_number, ethereum_block_hash, ethereum_tx_hash, ethereum_log_index)
            VALUES (1, $1, $1, $1, $2, $2, $1, 1000000000, 1, 4294967295, 100, $3, $1, 0)")
            .bind(&zero).bind(alloy::primitives::Address::ZERO.to_string()).bind(&hash)
            .execute(&pool).await.unwrap();
        let mut state = app_state(pool);
        state.ethereum = crate::ethereum::Ethereum::new(
            server.url.clone(),
            format!("0x{}", "11".repeat(20)),
            format!("0x{}", "22".repeat(20)),
            "01".repeat(32),
            "02".repeat(32),
        )
        .unwrap();
        let input = crate::canonical_deposit_batch(&state).await.unwrap();
        assert_eq!(input.deposits.len(), 1);
        assert_eq!(
            requests
                .lock()
                .unwrap()
                .iter()
                .filter(|r| r["method"] == "eth_getBlockByNumber")
                .count(),
            1
        );
        let values = BridgeTransitionPublicValuesV2 {
            ethereum_state_before: [0; 32],
            ethereum_state_after: [0; 32],
            ethereum_nonce_before: 0,
            ethereum_nonce_after: 1,
            zeko_action_state_before: [0; 32],
            zeko_action_state_after: [4; 32],
            zeko_action_state_length_before: 0,
            zeko_action_state_length_after: 1,
            actions: vec![BridgeOuterActionV2 {
                fields: [[0; 32]; 5],
                state_after: [4; 32],
            }],
        }
        .encode();
        let input = serde_json::to_value(input).unwrap();
        let fixture = crate::prover::testing::Fixture::new(
            ProofKind::Bridge,
            input.clone(),
            values.clone(),
            zero,
        )
        .unwrap();
        let preflight = crate::prover::Preflight::decode(ProofKind::Bridge, values, None).unwrap();
        fixture
            .run(crate::validate_preflight(&state, &input, &preflight))
            .await
            .unwrap();
        assert_eq!(
            requests
                .lock()
                .unwrap()
                .iter()
                .filter(|r| r["method"] == "eth_getBlockByNumber")
                .count(),
            2
        );
    }

    struct Database {
        pool: PgPool,
        admin: PgPool,
        schema: String,
    }

    impl Database {
        async fn create() -> Result<Self> {
            let url = std::env::var("OUTER_WRITER_TEST_DATABASE_URL").context(
                "set OUTER_WRITER_TEST_DATABASE_URL to an isolated local PostgreSQL database",
            )?;
            let admin = PgPoolOptions::new()
                .max_connections(1)
                .connect(&url)
                .await?;
            let schema = format!("outer_writer_test_{}", Uuid::new_v4().simple());
            sqlx::query(&format!("CREATE SCHEMA {schema}"))
                .execute(&admin)
                .await?;
            let options =
                PgConnectOptions::from_str(&url)?.options([("search_path", schema.as_str())]);
            let pool = PgPoolOptions::new()
                .max_connections(4)
                .connect_with(options)
                .await?;
            sqlx::raw_sql(
                "CREATE TABLE gateway_config (id BOOLEAN PRIMARY KEY, recovery_ready BOOLEAN NOT NULL, outer_public_key TEXT);
                 INSERT INTO gateway_config VALUES (TRUE, TRUE, 'outer');
                 CREATE TABLE gateway_accounts (public_key TEXT, token_id TEXT, account_json JSONB);
                 CREATE TABLE proof_jobs (id UUID PRIMARY KEY, kind TEXT, status TEXT DEFAULT 'queued',
                    transaction_hash TEXT, prepared_transaction JSONB, input JSONB DEFAULT '{}');"
            ).execute(&pool).await?;
            sqlx::raw_sql(include_str!(
                "../migrations/0020_outer_writer_reservation.sql"
            ))
            .execute(&pool)
            .await?;
            let c = checkpoint();
            sqlx::query("INSERT INTO gateway_accounts VALUES ('outer', '1', $1)")
                .bind(json!({"zkappState":c.outer_state_fields,"actionState":[c.action_state]}))
                .execute(&pool)
                .await?;
            Ok(Self {
                pool,
                admin,
                schema,
            })
        }

        async fn close(self) -> Result<()> {
            self.pool.close().await;
            sqlx::query(&format!("DROP SCHEMA {} CASCADE", self.schema))
                .execute(&self.admin)
                .await?;
            self.admin.close().await;
            Ok(())
        }
    }

    #[test]
    fn source_checkpoint_is_bound_to_the_immutable_proof() {
        let c = checkpoint();
        let input = proof(&c);
        assert!(proof_matches_checkpoint(&input, &c).unwrap());
        assert_eq!(ledger_hashes(&input).unwrap(), ("2".into(), "9".into()));
        let mut moved = c.clone();
        moved.action_state = "12".into();
        assert!(!proof_matches_checkpoint(&input, &moved).unwrap());
        moved = c;
        moved.outer_state_fields[2] = "9".into();
        assert!(!proof_matches_checkpoint(&input, &moved).unwrap());
    }

    #[tokio::test]
    #[ignore = "requires OUTER_WRITER_TEST_DATABASE_URL; uses its own temporary schema, no SP1"]
    async fn concurrent_preparation_and_bridge_admission_have_one_owner() -> Result<()> {
        let db = Database::create().await?;
        let barrier = Arc::new(Barrier::new(2));
        let a_pool = db.pool.clone();
        let a_barrier = barrier.clone();
        let preparation = tokio::spawn(async move {
            a_barrier.wait().await;
            acquire_with_checkpoint(&a_pool, &acquire_request("sequencer"), checkpoint()).await
        });
        let b_pool = db.pool.clone();
        let bridge = tokio::spawn(async move {
            barrier.wait().await;
            let mut tx = b_pool.begin().await?;
            lock(&mut tx).await?;
            let id = Uuid::new_v4();
            sqlx::query("INSERT INTO proof_jobs (id, kind) VALUES ($1, 'bridge')")
                .bind(id)
                .execute(&mut *tx)
                .await?;
            attach_bridge(
                &mut tx,
                id,
                &json!({"zeko":{"action_state":format!("0x{:064x}",11)}}),
            )
            .await?;
            tx.commit().await?;
            Ok::<_, anyhow::Error>(())
        });
        let a = preparation.await?;
        let b = bridge.await?;
        assert_ne!(a.is_ok(), b.is_ok());
        let error = a.as_ref().err().or_else(|| b.as_ref().err()).unwrap();
        assert_eq!(error_code(error), Some("OUTER_WRITER_BUSY"));
        let writer_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM gateway_outer_writer WHERE reservation_id IS NOT NULL",
        )
        .fetch_one(&db.pool)
        .await?;
        assert_eq!(writer_count, 1);
        db.close().await
    }

    #[tokio::test]
    #[ignore = "requires OUTER_WRITER_TEST_DATABASE_URL; uses its own temporary schema, no SP1"]
    async fn expiry_fences_old_tokens_but_never_expires_an_accepted_job() -> Result<()> {
        let db = Database::create().await?;
        let first =
            acquire_with_checkpoint(&db.pool, &acquire_request("first"), checkpoint()).await?;
        let repeated =
            acquire_with_checkpoint(&db.pool, &acquire_request("first"), checkpoint()).await?;
        assert_eq!(first.reservation_id, repeated.reservation_id);
        assert_eq!(first.fencing_token, repeated.fencing_token);
        sqlx::query("UPDATE gateway_outer_writer SET expires_at = NOW() - INTERVAL '1 second'")
            .execute(&db.pool)
            .await?;
        let second =
            acquire_with_checkpoint(&db.pool, &acquire_request("second"), checkpoint()).await?;
        assert!(second.fencing_token.parse::<i64>()? > first.fencing_token.parse::<i64>()?);
        assert_eq!(
            error_code(
                &renew_token(&db.pool, &token(&first), 120)
                    .await
                    .unwrap_err()
            ),
            Some("RESERVATION_EXPIRED")
        );
        let mut tx = db.pool.begin().await?;
        lock(&mut tx).await?;
        let id = Uuid::new_v4();
        sqlx::query("INSERT INTO proof_jobs (id, kind) VALUES ($1, 'settlement')")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        assert_eq!(
            error_code(
                &attach_settlement(&mut tx, id, Some(&token(&first)), &proof(&checkpoint()))
                    .await
                    .unwrap_err()
            ),
            Some("RESERVATION_EXPIRED")
        );
        attach_settlement(&mut tx, id, Some(&token(&second)), &proof(&checkpoint())).await?;
        tx.commit().await?;
        assert!(renew_token(&db.pool, &token(&second), 120)
            .await?
            .expires_at
            .is_none());
        // A failed job with a durably staged transaction remains ambiguous.
        sqlx::query(
            "UPDATE proof_jobs SET status = 'failed', prepared_transaction = '{}' WHERE id = $1",
        )
        .bind(id)
        .execute(&db.pool)
        .await?;
        let error = acquire_with_checkpoint(&db.pool, &acquire_request("third"), checkpoint())
            .await
            .unwrap_err();
        assert_eq!(error_code(&error), Some("OUTER_WRITER_BUSY"));
        sqlx::query("UPDATE proof_jobs SET status = 'confirmed' WHERE id = $1")
            .bind(id)
            .execute(&db.pool)
            .await?;
        let third =
            acquire_with_checkpoint(&db.pool, &acquire_request("third"), checkpoint()).await?;
        assert_ne!(second.reservation_id, third.reservation_id);
        db.close().await
    }

    #[tokio::test]
    #[ignore = "requires OUTER_WRITER_TEST_DATABASE_URL; uses its own temporary schema, no SP1"]
    async fn stale_proof_rejection_rolls_back_admission_without_consuming_the_lease() -> Result<()>
    {
        let db = Database::create().await?;
        let reservation =
            acquire_with_checkpoint(&db.pool, &acquire_request("sequencer"), checkpoint()).await?;
        let mut stale = checkpoint();
        stale.action_state = "10".into();
        let mut tx = db.pool.begin().await?;
        lock(&mut tx).await?;
        let id = Uuid::new_v4();
        sqlx::query("INSERT INTO proof_jobs (id, kind) VALUES ($1, 'settlement')")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        let error = attach_settlement(&mut tx, id, Some(&token(&reservation)), &proof(&stale))
            .await
            .unwrap_err();
        assert_eq!(error_code(&error), Some("STALE_CHECKPOINT"));
        tx.rollback().await?;
        let jobs: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM proof_jobs")
            .fetch_one(&db.pool)
            .await?;
        assert_eq!(jobs, 0);
        let renewed = renew_token(&db.pool, &token(&reservation), 120).await?;
        assert_eq!(renewed.reservation_id, reservation.reservation_id);
        assert!(renewed.expires_at.is_some());
        db.close().await
    }

    #[tokio::test]
    #[ignore = "requires OUTER_WRITER_TEST_DATABASE_URL; uses its own temporary schema, no SP1"]
    async fn reorg_releases_unsigned_jobs_but_retains_every_ambiguous_submission() -> Result<()> {
        let db = Database::create().await?;
        for (prepared, hash) in [
            (None, None),
            (Some(json!({"raw_transaction": "0x01"})), None),
            (None, Some("0x02")),
        ] {
            let reservation =
                acquire_with_checkpoint(&db.pool, &acquire_request("sequencer"), checkpoint())
                    .await?;
            let id = Uuid::new_v4();
            let mut tx = db.pool.begin().await?;
            lock(&mut tx).await?;
            sqlx::query("INSERT INTO proof_jobs (id, kind) VALUES ($1, 'settlement')")
                .bind(id)
                .execute(&mut *tx)
                .await?;
            attach_settlement(
                &mut tx,
                id,
                Some(&token(&reservation)),
                &proof(&checkpoint()),
            )
            .await?;
            tx.commit().await?;
            sqlx::query("UPDATE proof_jobs SET status = 'reorged', prepared_transaction = $2, transaction_hash = $3 WHERE id = $1")
                .bind(id).bind(&prepared).bind(hash).execute(&db.pool).await?;
            let ambiguous = prepared.is_some() || hash.is_some();
            assert_eq!(busy(&db.pool).await?, ambiguous);
            let replacement =
                acquire_with_checkpoint(&db.pool, &acquire_request("replacement"), checkpoint())
                    .await;
            if ambiguous {
                assert_eq!(
                    error_code(&replacement.unwrap_err()),
                    Some("OUTER_WRITER_BUSY")
                );
                let attached: Uuid =
                    sqlx::query_scalar("SELECT job_id FROM gateway_outer_writer WHERE id = TRUE")
                        .fetch_one(&db.pool)
                        .await?;
                assert_eq!(
                    attached, id,
                    "reorg alone cannot release signed or hashed work"
                );
                // Only a known canonical outcome may release an ambiguous job.
                sqlx::query("UPDATE proof_jobs SET status = 'ethereum_reverted' WHERE id = $1")
                    .bind(id)
                    .execute(&db.pool)
                    .await?;
            } else {
                let replacement = replacement?;
                assert_ne!(replacement.reservation_id, reservation.reservation_id);
                sqlx::query("UPDATE gateway_outer_writer SET expires_at = NOW() - INTERVAL '1 second' WHERE id = TRUE")
                    .execute(&db.pool).await?;
            }
        }
        db.close().await
    }
}
