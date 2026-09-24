//! Durable Ethereum submission. Persist the proof and signed bytes before any
//! broadcast, so a lost RPC response never causes a fresh nonce/proof attempt.
use anyhow::{Context, Result};
use sqlx::Row;
use std::time::Duration;
use uuid::Uuid;

use crate::{ethereum::PreparedSubmission, proof_kind::ProofKind, AppState};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FailureClass {
    RateLimited,
    Transport,
    InsufficientFunds,
    StaleCheckpoint,
    Expired,
    Rejected,
    Unknown,
}

impl FailureClass {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::RateLimited => "rate_limited",
            Self::Transport => "transport",
            Self::InsufficientFunds => "insufficient_funds",
            Self::StaleCheckpoint => "stale_checkpoint",
            Self::Expired => "expired",
            Self::Rejected => "rejected",
            Self::Unknown => "unknown",
        }
    }

    pub(crate) fn retryable(self) -> bool {
        matches!(
            self,
            Self::RateLimited | Self::Transport | Self::InsufficientFunds
        )
    }
}

// Errors at the validation boundary already carry these specific invariants.
// RPC classification also covers JSON-RPC errors nested inside HTTP responses.
pub(crate) fn classify(error: &anyhow::Error) -> FailureClass {
    let message = format!("{error:#}").to_ascii_lowercase();
    if crate::rpc::classify(error) == crate::rpc::FailureKind::InsufficientFunds
        || message.contains("insufficient funds")
    {
        FailureClass::InsufficientFunds
    } else if crate::rpc::is_rate_limited(error)
        || message.contains("429")
        || message.contains("-32007")
        || message.contains("rate limit")
        || message.contains("too many requests")
    {
        FailureClass::RateLimited
    } else if message.contains("action state mismatch")
        || message.contains("outer action-state length mismatch")
        || message.contains("settlement batch sequence mismatch")
        || message.contains("stale settlement")
        || message.contains("stale checkpoint")
        || message.contains("outer state field") && message.contains("mismatch")
        || crate::outer_writer::error_code(error) == Some("STALE_CHECKPOINT")
    {
        FailureClass::StaleCheckpoint
    } else if message.contains("expired")
        || message.contains("remaining slots")
        || message.contains("slots remaining")
    {
        FailureClass::Expired
    } else if transient_database_error(error)
        || crate::rpc::classify(error) == crate::rpc::FailureKind::Transient
        || message.contains("timed out")
        || message.contains("timeout")
        || message.contains("connection reset")
        || message.contains("connection refused")
        || message.contains("connection closed")
        || message.contains("error sending request")
        || message.contains("temporarily unavailable")
        || message.contains("http error 502")
        || message.contains("http error 503")
        || message.contains("http error 504")
    {
        FailureClass::Transport
    } else if message.contains("cancelled") || message.contains("rejected") {
        FailureClass::Rejected
    } else {
        FailureClass::Unknown
    }
}

fn transient_database_error(error: &anyhow::Error) -> bool {
    error
        .chain()
        .any(|cause| match cause.downcast_ref::<sqlx::Error>() {
            Some(
                sqlx::Error::Io(_)
                | sqlx::Error::PoolTimedOut
                | sqlx::Error::PoolClosed
                | sqlx::Error::WorkerCrashed,
            ) => true,
            Some(sqlx::Error::Database(error)) => error.code().is_some_and(|code| {
                code.starts_with("08")
                    || matches!(
                        code.as_ref(),
                        "40001" | "40P01" | "53300" | "55P03" | "57P01" | "57P02" | "57P03"
                    )
            }),
            _ => false,
        })
}

pub(crate) async fn recover_interrupted(pool: &sqlx::PgPool) -> Result<()> {
    sqlx::query(
        "UPDATE proof_jobs SET status = CASE
             WHEN proof_request_intent IS NOT NULL AND proof_request_id IS NULL
                 THEN 'queued'::proof_status
             WHEN transaction_hash IS NOT NULL AND prepared_transaction IS NULL
                 THEN 'submitted'::proof_status
             WHEN approved_at IS NOT NULL THEN 'approved'::proof_status
             ELSE 'queued'::proof_status END,
             error = CASE WHEN proof_request_intent IS NOT NULL AND proof_request_id IS NULL
                 THEN 'SP1 request outcome is unknown; reconcile the persisted intent before retrying'
                 ELSE 'worker restarted before completion' END,
             error_class = CASE WHEN proof_request_intent IS NOT NULL AND proof_request_id IS NULL
                 THEN 'proof_request_ambiguous' ELSE error_class END,
             attempts = attempts + 1, updated_at = NOW()
         WHERE status IN ('validating', 'proof_requested', 'proving', 'submitting')",
    )
    .execute(pool)
    .await?;
    Ok(())
}

/// Seal one immutable proof request before crossing the non-idempotent SP1 API.
/// An unknown outcome is deliberately blocked rather than purchasing again.
pub(crate) async fn begin_proof_request(
    pool: &sqlx::PgPool,
    job_id: Uuid,
    attempts: i32,
) -> Result<Uuid> {
    let intent = Uuid::new_v4();
    let result = sqlx::query(
        "UPDATE proof_jobs SET proof_request_intent = $2, updated_at = NOW()
         WHERE id = $1 AND attempts = $3 AND status = 'proving'
           AND proof_request_intent IS NULL AND proof_request_id IS NULL",
    )
    .bind(job_id)
    .bind(intent)
    .bind(attempts)
    .execute(pool)
    .await?;
    anyhow::ensure!(
        result.rows_affected() == 1,
        "proof request already sealed or worker claim was fenced"
    );
    Ok(intent)
}

/// A response from a previous worker may resolve its own unique intent. Keep a
/// recovered queued job queued so the current owner can resume that same ID.
pub(crate) async fn finish_proof_request(
    pool: &sqlx::PgPool,
    job_id: Uuid,
    intent: Uuid,
    request_id: &str,
) -> Result<()> {
    let result = sqlx::query(
        "UPDATE proof_jobs SET proof_request_id = $3,
             status = CASE WHEN status = 'proving' THEN 'proof_requested'::proof_status ELSE status END,
             error = NULL, error_class = NULL, next_attempt_at = NULL, updated_at = NOW()
         WHERE id = $1 AND proof_request_intent = $2
           AND (proof_request_id IS NULL OR proof_request_id = $3)
           AND status IN ('queued', 'approved', 'proving', 'proof_requested')",
    )
    .bind(job_id)
    .bind(intent)
    .bind(request_id)
    .execute(pool)
    .await?;
    anyhow::ensure!(
        result.rows_affected() == 1,
        "proof request response does not match its durable intent"
    );
    Ok(())
}

pub(crate) fn retry_delay(attempt: u32, class: FailureClass, jitter: u64) -> Duration {
    let base = if class == FailureClass::InsufficientFunds {
        30
    } else {
        2
    };
    let seconds = (base * (1_u64 << attempt.min(8))).min(300);
    Duration::from_secs(seconds + jitter % (seconds / 4 + 1))
}

pub(crate) async fn send(
    state: &AppState,
    job_id: Uuid,
    attempts: i32,
    kind: ProofKind,
    public_values: &[u8],
    proof: &[u8],
) -> Result<()> {
    let row = sqlx::query(
        "UPDATE proof_jobs SET public_values = $2, proof_bytes = $3,
                transaction_hash = COALESCE(prepared_transaction->>'transaction_hash', transaction_hash),
                status = 'submitting', updated_at = NOW()
         WHERE id = $1 AND attempts = $4 AND status IN ('validating', 'proving', 'proof_requested', 'submitting')
         RETURNING prepared_transaction",
    )
    .bind(job_id)
    .bind(format!("0x{}", hex::encode(public_values)))
    .bind(format!("0x{}", hex::encode(proof)))
    .bind(attempts)
    .fetch_optional(&state.pool)
    .await?
    .context("proof job was cancelled before preparing submission")?;

    let prepared: PreparedSubmission = match row
        .try_get::<Option<serde_json::Value>, _>("prepared_transaction")?
    {
        Some(value) => {
            serde_json::from_value(value).context("decode persisted signed transaction")?
        }
        None => {
            let prepared = state
                .ethereum
                .prepare_submission(kind, public_values.to_vec(), proof.to_vec())
                .await?;
            let result = sqlx::query(
                    "UPDATE proof_jobs SET prepared_transaction = $2, transaction_hash = $3,
                        updated_at = NOW()
                 WHERE id = $1 AND attempts = $4 AND status = 'submitting' AND prepared_transaction IS NULL",
                )
                .bind(job_id)
                .bind(serde_json::to_value(&prepared)?)
                .bind(prepared.transaction_hash.to_string())
                .bind(attempts)
                .execute(&state.pool)
                .await?;
            anyhow::ensure!(
                result.rows_affected() == 1,
                "proof job was cancelled before persisting submission"
            );
            prepared
        }
    };

    // No long-lived database transaction is held during RPC retries. The
    // durable outer-writer reservation protects this job's source checkpoint.
    let hash = state.ethereum.broadcast_submission(&prepared).await?;
    anyhow::ensure!(
        hash.to_string() == prepared.transaction_hash.to_string(),
        "broadcast returned a different transaction hash"
    );
    sqlx::query(
        "UPDATE proof_jobs SET status = 'submitted', error = NULL, error_class = NULL,
                next_attempt_at = NULL, updated_at = NOW()
         WHERE id = $1 AND attempts = $2 AND status = 'submitting'",
    )
    .bind(job_id)
    .bind(attempts)
    .execute(&state.pool)
    .await?;
    Ok(())
}

pub(crate) async fn record_failure(
    state: &AppState,
    job_id: Uuid,
    attempts: i32,
    error: &anyhow::Error,
) -> Result<()> {
    let class = classify(error);
    let mut tx = state.pool.begin().await?;
    let row = sqlx::query(
        "SELECT status::text AS status, prepared_transaction IS NOT NULL AS prepared,
                transaction_hash IS NOT NULL AS has_hash, retry_count, attempts,
                proof_request_intent IS NOT NULL AND proof_request_id IS NULL AS unknown_proof_request
         FROM proof_jobs WHERE id = $1 FOR UPDATE",
    )
    .bind(job_id)
    .fetch_one(&mut *tx)
    .await?;
    let status: String = row.try_get("status")?;
    if row.try_get::<i32, _>("attempts")? != attempts
        || matches!(
            status.as_str(),
            "confirmed"
                | "reorged"
                | "rejected"
                | "ethereum_reverted"
                | "failed"
                | "proof_failed"
                | "executed"
        )
    {
        tx.commit().await?;
        return Ok(());
    }
    let may_have_broadcast =
        row.try_get::<bool, _>("prepared")? || row.try_get::<bool, _>("has_hash")?;
    let unknown_proof_request: bool = row.try_get("unknown_proof_request")?;
    let retry_count = u32::try_from(row.try_get::<i32, _>("retry_count")?).unwrap_or(u32::MAX);
    let retry = class.retryable() || may_have_broadcast;
    let delay = retry_delay(retry_count, class, job_id.as_u128() as u64);
    let message = crate::database_safe_error(error);
    if unknown_proof_request {
        sqlx::query(
            "UPDATE proof_jobs SET status = 'queued', error = $2,
                 error_class = 'proof_request_ambiguous', next_attempt_at = NULL,
                 completed_at = NULL, updated_at = NOW()
             WHERE id = $1",
        )
        .bind(job_id)
        .bind(format!("SP1 request outcome is unknown; reconcile the persisted intent before retrying: {message}"))
        .execute(&mut *tx)
        .await?;
    } else if retry {
        sqlx::query(
            "UPDATE proof_jobs SET status = CASE WHEN approved_at IS NOT NULL
                     THEN 'approved'::proof_status ELSE 'queued'::proof_status END,
                    error = $2, error_class = $3,
                    next_attempt_at = NOW() + ($4 * INTERVAL '1 second'),
                    retry_count = LEAST(retry_count::bigint + 1, 2147483647)::integer,
                    completed_at = NULL, updated_at = NOW()
             WHERE id = $1",
        )
        .bind(job_id)
        .bind(message)
        .bind(class.as_str())
        .bind(delay.as_secs() as f64)
        .execute(&mut *tx)
        .await?;
    } else {
        sqlx::query(
            "UPDATE proof_jobs SET status = 'failed', error = $2, error_class = $3,
                    completed_at = NOW(), next_attempt_at = NULL, updated_at = NOW()
             WHERE id = $1",
        )
        .bind(job_id)
        .bind(message)
        .bind(class.as_str())
        .execute(&mut *tx)
        .await?;
        sqlx::query("DELETE FROM gateway_pending_commands WHERE job_id = $1")
            .bind(job_id)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    tracing::warn!(%job_id, error_class = class.as_str(), retry, may_have_broadcast, unknown_proof_request,
        retry_after_secs = delay.as_secs(), "settlement attempt requires recovery");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::keccak256;
    use axum::{
        extract::{Path, State},
        http::StatusCode,
        response::IntoResponse,
        Json,
    };
    use serde_json::{json, Value};
    use sqlx::PgPool;
    use std::sync::{Arc, Mutex};

    fn state(pool: PgPool, url: &str) -> AppState {
        AppState {
            pool,
            archive_pool: None,
            api_key: "test".into(),
            ethereum: crate::ethereum::Ethereum::new(
                url.into(),
                format!("0x{}", "11".repeat(20)),
                format!("0x{}", "22".repeat(20)),
                "01".repeat(32),
                "02".repeat(32),
            )
            .unwrap(),
            proof_system: "groth16".into(),
            prover_config: crate::prover::NetworkRequestConfig {
                timeout: Duration::from_secs(1),
                min_auction_period: 1,
                gas_limit: None,
                max_price_per_pgu: None,
            },
            network_explorer_base: "http://unused".into(),
            execute_only: false,
            local_mock_submit: false,
            require_proof_approval: false,
            min_remaining_slots: 1,
            ethereum_finality_mode: crate::indexer::FinalityMode::Finalized,
            ethereum_confirmations: 12,
            sequencer_graphql_url: None,
            inner_public_key: None,
            fee_payer_public_key: None,
            http_client: reqwest::Client::new(),
        }
    }

    async fn job(pool: &PgPool) -> Uuid {
        let id = Uuid::new_v4();
        sqlx::query("INSERT INTO proof_jobs (id, kind, status, input) VALUES ($1, 'settlement', 'validating', '{}')")
            .bind(id).execute(pool).await.unwrap();
        sqlx::query("INSERT INTO gateway_pending_commands (job_id, public_key, nonce, command_kind, command_base64) VALUES ($1, 'outer', 0, 'zkapp', 'command')")
            .bind(id).execute(pool).await.unwrap();
        id
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn retry_and_restart_reuse_the_persisted_signed_transaction(pool: PgPool) {
        sqlx::query("INSERT INTO gateway_config (genesis_timestamp, fork_slot, account_creation_fee, state_hash, recovery_ready) VALUES ('0', 0, '1', 'test', TRUE)")
            .execute(&pool).await.unwrap();
        let id = job(&pool).await;
        let sent = Arc::new(Mutex::new(Vec::<String>::new()));
        let requests = Arc::new(Mutex::new(Vec::<String>::new()));
        let sent_observed = sent.clone();
        let requests_observed = requests.clone();
        let rpc_pool = pool.clone();
        let server = crate::rpc::test_support::serve(move |request| {
            let pool = rpc_pool.clone();
            let sent = sent_observed.clone();
            let requests = requests_observed.clone();
            async move {
                let method = request["method"].as_str().unwrap();
                requests.lock().unwrap().push(method.to_owned());
                let result = match method {
                    "eth_call" => json!("0x"),
                    "eth_estimateGas" => json!("0x5208"),
                    "eth_feeHistory" => json!({"oldestBlock":"0x0","baseFeePerGas":["0x1","0x1"],"gasUsedRatio":[0.5],"reward":[["0x1"]]}),
                    "eth_chainId" => json!("0x7a69"),
                    "eth_getTransactionCount" => json!("0x2"),
                    "eth_getTransactionByHash" | "eth_getTransactionReceipt" => Value::Null,
                    "eth_sendRawTransaction" => {
                        let raw = request["params"][0].as_str().unwrap().to_owned();
                        let saved: Value = sqlx::query_scalar("SELECT prepared_transaction FROM proof_jobs WHERE id = $1")
                            .bind(id).fetch_one(&pool).await.unwrap();
                        assert_eq!(saved["raw_transaction"], raw, "signed bytes must be durable before broadcast");
                        let hash = keccak256(hex::decode(raw.strip_prefix("0x").unwrap()).unwrap());
                        let mut attempts = sent.lock().unwrap();
                        attempts.push(raw);
                        if attempts.len() == 1 {
                            return StatusCode::TOO_MANY_REQUESTS.into_response();
                        }
                        json!(hash.to_string())
                    }
                    _ => panic!("unexpected RPC {method}"),
                };
                Json(json!({"jsonrpc":"2.0", "id":request["id"], "result":result})).into_response()
            }
        }).await;
        let state = state(pool.clone(), &server.url);
        let error = send(&state, id, 0, ProofKind::Settlement, &[1], &[2])
            .await
            .unwrap_err();
        assert_eq!(classify(&error), FailureClass::RateLimited);
        record_failure(&state, id, 0, &error).await.unwrap();
        assert!(
            crate::claim_job(&pool).await.unwrap().is_none(),
            "retry must respect backoff"
        );
        let row = sqlx::query("SELECT status::text AS status, proof_bytes, prepared_transaction, transaction_hash, completed_at IS NULL AS pending FROM proof_jobs WHERE id = $1")
            .bind(id).fetch_one(&pool).await.unwrap();
        assert_eq!(row.get::<String, _>("status"), "queued");
        assert_eq!(row.get::<String, _>("proof_bytes"), "0x02");
        assert!(row.get::<bool, _>("pending"));
        let saved: Value = row.get("prepared_transaction");
        assert_eq!(
            row.get::<String, _>("transaction_hash"),
            saved["transaction_hash"]
        );
        let cancel = crate::cancel_proof(State(state.clone()), Path(id)).await;
        assert_eq!(cancel.status(), StatusCode::CONFLICT);
        // Reorg rollback clears inclusion/hash metadata. A subsequent crash
        // must still resume the exact signed bytes and restore their hash.
        sqlx::query("UPDATE proof_jobs SET status = 'submitting', transaction_hash = NULL, next_attempt_at = NOW() - INTERVAL '1 second' WHERE id = $1")
            .bind(id).execute(&pool).await.unwrap();
        recover_interrupted(&pool).await.unwrap();
        let resumed = crate::claim_job(&pool).await.unwrap().unwrap();
        assert!(resumed.prepared);
        crate::process_job(&state, resumed).await.unwrap();
        let status: String =
            sqlx::query_scalar("SELECT status::text FROM proof_jobs WHERE id = $1")
                .bind(id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(status, "submitted");
        let hash: String =
            sqlx::query_scalar("SELECT transaction_hash FROM proof_jobs WHERE id = $1")
                .bind(id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(hash, saved["transaction_hash"]);
        {
            let attempts = sent.lock().unwrap();
            assert_eq!(attempts.len(), 2);
            assert_eq!(attempts[0], attempts[1]);
            let requests = requests.lock().unwrap();
            assert_eq!(
                requests
                    .iter()
                    .filter(|m| m.as_str() == "eth_getTransactionCount")
                    .count(),
                1
            );
            assert_eq!(
                requests.iter().filter(|m| m.as_str() == "eth_call").count(),
                1
            );
        }
        let pending: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM gateway_pending_commands WHERE job_id = $1")
                .bind(id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            pending, 1,
            "only canonical finality may remove an ambiguous submission"
        );
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn transient_failure_keeps_pending_but_stale_unsigned_proof_is_terminal(pool: PgPool) {
        let state = state(pool.clone(), "http://127.0.0.1:1");
        let id = job(&pool).await;
        let disconnected = sqlx::Error::Io(std::io::Error::from(std::io::ErrorKind::BrokenPipe));
        let disconnected =
            anyhow::Error::from(disconnected).context("save preflight on disconnected backend");
        assert_eq!(classify(&disconnected), FailureClass::Transport);
        record_failure(&state, id, 0, &disconnected).await.unwrap();
        let status: String =
            sqlx::query_scalar("SELECT status::text FROM proof_jobs WHERE id = $1")
                .bind(id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            status, "queued",
            "losing one database connection cannot reject accepted work"
        );
        record_failure(&state, id, 0, &anyhow::anyhow!("insufficient funds"))
            .await
            .unwrap();
        let pending: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM gateway_pending_commands WHERE job_id = $1")
                .bind(id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(pending, 1);
        record_failure(&state, id, 0, &anyhow::anyhow!("action state mismatch"))
            .await
            .unwrap();
        let row =
            sqlx::query("SELECT status::text AS status, error_class FROM proof_jobs WHERE id = $1")
                .bind(id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(row.get::<String, _>("status"), "failed");
        assert_eq!(row.get::<String, _>("error_class"), "stale_checkpoint");
        let pending: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM gateway_pending_commands WHERE job_id = $1")
                .bind(id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(pending, 0);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn proof_intent_has_one_winner_and_late_response_resumes_after_restart(pool: PgPool) {
        sqlx::query("INSERT INTO gateway_config (genesis_timestamp, fork_slot, account_creation_fee, state_hash, recovery_ready) VALUES ('0', 0, '1', 'test', TRUE)")
            .execute(&pool).await.unwrap();
        let id = job(&pool).await;
        let state = state(pool.clone(), "http://127.0.0.1:1");
        sqlx::query("UPDATE gateway_outer_writer SET reservation_id = $1, owner_id = 'sequencer', job_id = $2 WHERE id = TRUE")
            .bind(Uuid::new_v4()).bind(id).execute(&pool).await.unwrap();
        crate::set_status(&pool, id, 0, "proving").await.unwrap();

        // Independent pooled connections race at the actual admission CAS.
        let (first, second) = tokio::join!(
            begin_proof_request(&pool, id, 0),
            begin_proof_request(&pool, id, 0),
        );
        assert_ne!(
            first.is_ok(),
            second.is_ok(),
            "only one worker may purchase the proof"
        );
        let intent = first.or(second).unwrap();
        record_failure(&state, id, 0, &anyhow::anyhow!("request response lost"))
            .await
            .unwrap();
        let row =
            sqlx::query("SELECT status::text AS status, error_class FROM proof_jobs WHERE id = $1")
                .bind(id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(row.get::<String, _>("status"), "queued");
        assert_eq!(
            row.get::<String, _>("error_class"),
            "proof_request_ambiguous"
        );
        assert!(crate::claim_job(&pool).await.unwrap().is_none());
        assert_eq!(
            crate::cancel_proof(State(state.clone()), Path(id))
                .await
                .status(),
            StatusCode::CONFLICT
        );
        let pending: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM gateway_pending_commands WHERE job_id = $1")
                .bind(id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(pending, 1);
        let owner: Uuid =
            sqlx::query_scalar("SELECT job_id FROM gateway_outer_writer WHERE id = TRUE")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(owner, id);

        // Recovery must also recognize a crash before recording the ambiguity.
        sqlx::query("UPDATE proof_jobs SET status = 'proving', error_class = NULL WHERE id = $1")
            .bind(id)
            .execute(&pool)
            .await
            .unwrap();
        recover_interrupted(&pool).await.unwrap();
        assert!(crate::claim_job(&pool).await.unwrap().is_none());
        assert!(begin_proof_request(&pool, id, 1).await.is_err());
        assert!(finish_proof_request(&pool, id, Uuid::new_v4(), "wrong")
            .await
            .is_err());

        // A late response resolves only its own intent and leaves the queued
        // job claimable; it cannot strand a request in an unowned active stage.
        finish_proof_request(&pool, id, intent, "known-request")
            .await
            .unwrap();
        finish_proof_request(&pool, id, intent, "known-request")
            .await
            .unwrap();
        assert!(finish_proof_request(&pool, id, intent, "different-request")
            .await
            .is_err());
        let row =
            sqlx::query("SELECT status::text AS status, error_class FROM proof_jobs WHERE id = $1")
                .bind(id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(row.get::<String, _>("status"), "queued");
        assert!(row.get::<Option<String>, _>("error_class").is_none());
        let resumed = crate::claim_job(&pool).await.unwrap().unwrap();
        assert_eq!(resumed.proof_request_id.as_deref(), Some("known-request"));
        assert!(begin_proof_request(&pool, id, resumed.attempts)
            .await
            .is_err());
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn restarted_claim_fences_stale_status_failure_and_submission_writes(pool: PgPool) {
        sqlx::query("INSERT INTO gateway_config (genesis_timestamp, fork_slot, account_creation_fee, state_hash, recovery_ready) VALUES ('0', 0, '1', 'test', TRUE)")
            .execute(&pool).await.unwrap();
        let id = job(&pool).await;
        let state = state(pool.clone(), "http://127.0.0.1:1");
        recover_interrupted(&pool).await.unwrap();
        // Recovery itself fences the old generation before another claim.
        assert!(crate::set_status(&pool, id, 0, "proving").await.is_err());
        let resumed = crate::claim_job(&pool).await.unwrap().unwrap();
        assert!(resumed.attempts > 0);
        record_failure(&state, id, 0, &anyhow::anyhow!("invalid verification key"))
            .await
            .unwrap();
        assert!(send(&state, id, 0, ProofKind::Settlement, &[1], &[2])
            .await
            .is_err());
        let row = sqlx::query("SELECT status::text AS status, attempts, error_class, proof_bytes FROM proof_jobs WHERE id = $1")
            .bind(id).fetch_one(&pool).await.unwrap();
        assert_eq!(row.get::<String, _>("status"), "validating");
        assert_eq!(row.get::<i32, _>("attempts"), resumed.attempts);
        assert!(row.get::<Option<String>, _>("error_class").is_none());
        assert!(row.get::<Option<String>, _>("proof_bytes").is_none());
        let pending: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM gateway_pending_commands WHERE job_id = $1")
                .bind(id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            pending, 1,
            "an old worker cannot remove the current attempt's command"
        );
        record_failure(
            &state,
            id,
            resumed.attempts,
            &anyhow::anyhow!("connection reset"),
        )
        .await
        .unwrap();
        let status: String =
            sqlx::query_scalar("SELECT status::text FROM proof_jobs WHERE id = $1")
                .bind(id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            status, "queued",
            "the current generation can still schedule recovery"
        );
    }

    #[test]
    fn transient_provider_errors_do_not_become_stale_proofs() {
        for text in [
            "HTTP error 429",
            "error code -32007: 125/second request limit",
            "connection reset by peer",
        ] {
            assert!(classify(&anyhow::anyhow!(text)).retryable());
        }
        assert_eq!(
            classify(&anyhow::anyhow!("action state mismatch")),
            FailureClass::StaleCheckpoint
        );
        assert!(!FailureClass::StaleCheckpoint.retryable());
        assert!(!classify(&anyhow::anyhow!("invalid verification key")).retryable());
    }

    #[cfg(feature = "fake-prover-tests")]
    #[sqlx::test(migrations = "./migrations")]
    async fn interrupted_prover_request_is_polled_without_repeating_preflight(pool: PgPool) {
        use crate::prover::testing::{Fixture, Operation};
        use zeko_sp1_lib::{BridgeOuterActionV2, BridgeTransitionPublicValuesV2};
        let values = BridgeTransitionPublicValuesV2 {
            ethereum_state_before: [1; 32],
            ethereum_state_after: [2; 32],
            ethereum_nonce_before: 0,
            ethereum_nonce_after: 1,
            zeko_action_state_before: [3; 32],
            zeko_action_state_after: [4; 32],
            zeko_action_state_length_before: 0,
            zeko_action_state_length_after: 1,
            actions: vec![BridgeOuterActionV2 {
                fields: [[5; 32]; 5],
                state_after: [4; 32],
            }],
        }
        .encode();
        let input = json!({"fixture": "durable-proof-request"});
        let fixture = Fixture::new(
            ProofKind::Bridge,
            input.clone(),
            values.clone(),
            "0x00".into(),
        )
        .unwrap();
        let state = state(pool.clone(), "http://127.0.0.1:1");
        let id = Uuid::new_v4();
        sqlx::query("INSERT INTO gateway_config (genesis_timestamp, fork_slot, account_creation_fee, state_hash, recovery_ready) VALUES ('0', 0, '1', 'test', TRUE)")
            .execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO proof_jobs (id, kind, status, input, public_values, proof_request_id) VALUES ($1, 'bridge', 'proof_requested', $2, $3, $4)")
            .bind(id).bind(input).bind(format!("0x{}", hex::encode(values))).bind(fixture.request_id())
            .execute(&pool).await.unwrap();
        recover_interrupted(&pool).await.unwrap();
        for _ in 0..2 {
            fixture.fail_next(Operation::WaitProof, "temporary connection reset");
            let claimed = crate::claim_job(&pool).await.unwrap().unwrap();
            fixture
                .run(crate::process_job(&state, claimed))
                .await
                .unwrap();
            sqlx::query(
                "UPDATE proof_jobs SET next_attempt_at = NOW() - INTERVAL '1 second' WHERE id = $1",
            )
            .bind(id)
            .execute(&pool)
            .await
            .unwrap();
        }
        assert_eq!(fixture.counts().preflight, 0);
        assert_eq!(fixture.counts().request_proof, 0);
        assert_eq!(fixture.counts().wait_proof, 2);
        let saved: String =
            sqlx::query_scalar("SELECT proof_request_id FROM proof_jobs WHERE id = $1")
                .bind(id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(saved, fixture.request_id());
    }

    #[test]
    fn prolonged_outage_retries_are_bounded_and_do_not_busy_loop() {
        assert_eq!(
            retry_delay(0, FailureClass::InsufficientFunds, 0),
            Duration::from_secs(30)
        );
        for attempt in [0, 1, 5, 20, u32::MAX] {
            let delay = retry_delay(attempt, FailureClass::Transport, u64::MAX);
            assert!(delay >= Duration::from_secs(2));
            assert!(delay <= Duration::from_secs(375));
        }
    }
}
