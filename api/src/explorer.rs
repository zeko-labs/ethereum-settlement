use alloy::primitives::Address as EthereumAddress;
use anyhow::{Context, Result};
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::AppState;

const DEFAULT_LIMIT: i64 = 20;
const MAX_LIMIT: i64 = 100;

const COMMIT_SCHEDULE_QUERY: &str =
    "query ExplorerCommitSchedule { commitSchedule { periodSeconds phase lastAttemptStartedAt nextAttemptAt } }";

#[derive(Debug, Deserialize)]
struct CommitScheduleGraphqlResponse {
    data: Option<CommitScheduleGraphqlData>,
    #[serde(default)]
    errors: Vec<Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CommitScheduleGraphqlData {
    commit_schedule: CommitSchedule,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct CommitSchedule {
    period_seconds: f64,
    phase: String,
    last_attempt_started_at: Option<DateTime<Utc>>,
    next_attempt_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListQuery {
    cursor: Option<String>,
    limit: Option<i64>,
    status: Option<String>,
    kind: Option<String>,
    account: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SearchQuery {
    q: String,
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/v1/explorer/summary", get(summary))
        .route("/v1/explorer/search", get(search))
        .route("/v1/explorer/blocks", get(list_blocks))
        .route("/v1/explorer/blocks/:identifier", get(get_block))
        .route("/v1/explorer/transactions", get(list_transactions))
        .route("/v1/explorer/transactions/:hash", get(get_transaction))
        .route("/v1/explorer/accounts/:public_key", get(get_account))
        .route("/v1/explorer/settlements", get(list_settlements))
        .route("/v1/explorer/settlements/:identifier", get(get_settlement))
        .route("/v1/explorer/deposits", get(list_deposits))
        .route("/v1/explorer/deposits/:nonce", get(get_deposit))
        .route("/v1/explorer/withdrawals", get(list_withdrawals))
        .route(
            "/v1/explorer/withdrawals/:sequence/:offset",
            get(get_withdrawal),
        )
}

pub async fn validate_archive_schema(pool: &PgPool) -> Result<()> {
    let required = [
        "blocks",
        "public_keys",
        "tokens",
        "account_identifiers",
        "user_commands",
        "blocks_user_commands",
        "zkapp_commands",
        "zkapp_fee_payer_body",
        "zkapp_account_update_body",
        "blocks_zkapp_commands",
        "accounts_accessed",
    ];
    for table in required {
        let present =
            sqlx::query_scalar::<_, bool>("SELECT to_regclass('public.' || $1) IS NOT NULL")
                .bind(table)
                .fetch_one(pool)
                .await?;
        anyhow::ensure!(present, "archive schema is missing table {table}");
    }
    Ok(())
}

async fn summary(State(state): State<AppState>) -> Response {
    let gateway_query = sqlx::query(
        "SELECT
             (SELECT COUNT(*)::text FROM gateway_bridge_deposits WHERE NOT removed) AS deposit_count,
             (SELECT COUNT(*)::text FROM gateway_inner_action_leaves WHERE recipient IS NOT NULL AND NOT removed) AS withdrawal_count,
             (SELECT COALESCE(SUM(ethereum_amount), 0)::text FROM gateway_bridge_deposits WHERE NOT removed) AS deposited_amount,
             (SELECT batch_sequence::text FROM gateway_explorer_settlements WHERE NOT removed ORDER BY batch_sequence DESC LIMIT 1) AS latest_settlement",
    )
    .fetch_one(&state.pool);
    let archive_query = async {
        match state.archive_pool.as_ref() {
            Some(pool) => sqlx::query(
                "SELECT
                 (SELECT MAX(height)::text FROM blocks WHERE chain_status = 'canonical') AS height,
                 ((SELECT COUNT(*) FROM blocks_user_commands buc JOIN blocks b ON b.id = buc.block_id WHERE b.chain_status = 'canonical') +
                  (SELECT COUNT(*) FROM blocks_zkapp_commands bzc JOIN blocks b ON b.id = bzc.block_id WHERE b.chain_status = 'canonical'))::text AS transaction_count,
                 (SELECT COUNT(DISTINCT account_identifier_id)::text FROM accounts_accessed aa JOIN blocks b ON b.id = aa.block_id WHERE b.chain_status = 'canonical') AS account_count",
            )
            .fetch_one(pool)
            .await
            .ok(),
            None => None,
        }
    };
    let (gateway, archive, commit_schedule) =
        tokio::join!(gateway_query, archive_query, fetch_commit_schedule(&state));
    match gateway {
        Ok(gateway) => Json(json!({
            "schemaVersion": 1,
            "asOf": Utc::now(),
            "sources": {
                "archive": archive.is_some(),
                "gateway": true,
                "ethereum": true,
                "sequencer": commit_schedule.is_some()
            },
            "l2": archive.as_ref().map(|row| json!({
                "blockHeight": row.try_get::<Option<String>, _>("height").ok().flatten(),
                "transactionCount": row.try_get::<String, _>("transaction_count").unwrap_or_else(|_| "0".into()),
                "accountCount": row.try_get::<String, _>("account_count").unwrap_or_else(|_| "0".into())
            })),
            "settlement": {
                "latestSequence": gateway.try_get::<Option<String>, _>("latest_settlement").ok().flatten(),
                "commitSchedule": commit_schedule
            },
            "bridge": {
                "depositCount": gateway.try_get::<String, _>("deposit_count").unwrap_or_else(|_| "0".into()),
                "withdrawalCount": gateway.try_get::<String, _>("withdrawal_count").unwrap_or_else(|_| "0".into()),
                "depositedAmount": gateway.try_get::<String, _>("deposited_amount").unwrap_or_else(|_| "0".into())
            }
        }))
        .into_response(),
        Err(error) => explorer_error(StatusCode::INTERNAL_SERVER_ERROR, error),
    }
}

async fn fetch_commit_schedule(state: &AppState) -> Option<CommitSchedule> {
    let url = state.sequencer_graphql_url.as_deref()?;
    let result = async {
        let response = state
            .http_client
            .post(url)
            .json(&json!({ "query": COMMIT_SCHEDULE_QUERY }))
            .send()
            .await?
            .error_for_status()?;
        let response: CommitScheduleGraphqlResponse = response.json().await?;
        anyhow::ensure!(
            response.errors.is_empty(),
            "sequencer returned GraphQL errors"
        );
        let schedule = response
            .data
            .context("sequencer response has no data")?
            .commit_schedule;
        validate_commit_schedule(&schedule)?;
        Result::<CommitSchedule>::Ok(schedule)
    }
    .await;
    match result {
        Ok(schedule) => Some(schedule),
        Err(error) => {
            tracing::debug!(%error, %url, "commit schedule is unavailable");
            None
        }
    }
}

fn validate_commit_schedule(schedule: &CommitSchedule) -> Result<()> {
    anyhow::ensure!(
        schedule.period_seconds.is_finite() && schedule.period_seconds >= 0.0,
        "sequencer returned an invalid commit period"
    );
    anyhow::ensure!(
        matches!(
            schedule.phase.as_str(),
            "WAITING" | "COMMITTING" | "DISABLED"
        ),
        "sequencer returned an invalid commit phase"
    );
    Ok(())
}

async fn list_blocks(State(state): State<AppState>, Query(query): Query<ListQuery>) -> Response {
    let pool = match archive_pool(&state) {
        Ok(pool) => pool,
        Err(response) => return response,
    };
    let cursor = match decode_i64_cursor(query.cursor.as_deref()) {
        Ok(value) => value,
        Err(error) => return explorer_error(StatusCode::BAD_REQUEST, error),
    };
    let limit = list_limit(query.limit);
    let rows = sqlx::query(
        "SELECT b.height::text AS height, b.state_hash, b.parent_hash,
                b.timestamp, b.chain_status::text AS chain_status,
                creator.value AS creator,
                COALESCE(uc.hash, zkc.hash) AS transaction_hash,
                CASE WHEN uc.id IS NOT NULL THEN uc.command_type::text
                     WHEN zkc.id IS NOT NULL THEN 'zkapp' END AS transaction_kind,
                COALESCE(buc.status::text, bzc.status::text) AS transaction_status,
                ((SELECT COUNT(*) FROM blocks_user_commands x WHERE x.block_id = b.id) +
                 (SELECT COUNT(*) FROM blocks_zkapp_commands x WHERE x.block_id = b.id))::bigint AS transaction_count
         FROM blocks b
         JOIN public_keys creator ON creator.id = b.creator_id
         LEFT JOIN LATERAL (
             SELECT x.user_command_id, x.status FROM blocks_user_commands x
             WHERE x.block_id = b.id ORDER BY x.sequence_no LIMIT 1
         ) buc ON TRUE
         LEFT JOIN user_commands uc ON uc.id = buc.user_command_id
         LEFT JOIN LATERAL (
             SELECT x.zkapp_command_id, x.status FROM blocks_zkapp_commands x
             WHERE x.block_id = b.id ORDER BY x.sequence_no LIMIT 1
         ) bzc ON TRUE
         LEFT JOIN zkapp_commands zkc ON zkc.id = bzc.zkapp_command_id
         WHERE ($1::bigint IS NULL OR b.height < $1)
           AND ($2::text IS NULL OR b.chain_status::text = $2)
         ORDER BY b.height DESC, b.id DESC
         LIMIT $3",
    )
    .bind(cursor)
    .bind(query.status)
    .bind(limit + 1)
    .fetch_all(pool)
    .await;
    match rows {
        Ok(mut rows) => {
            let more = rows.len() as i64 > limit;
            rows.truncate(limit as usize);
            let next = more
                .then(|| rows.last())
                .flatten()
                .and_then(|row| row.try_get::<String, _>("height").ok())
                .map(|height| encode_cursor(&height));
            let items = rows.iter().map(block_json).collect::<Result<Vec<_>>>();
            json_result(items.map(|items| page(items, next)))
        }
        Err(error) => explorer_error(StatusCode::INTERNAL_SERVER_ERROR, error),
    }
}

async fn get_block(State(state): State<AppState>, Path(identifier): Path<String>) -> Response {
    let pool = match archive_pool(&state) {
        Ok(pool) => pool,
        Err(response) => return response,
    };
    let height = identifier.replace(',', "").parse::<i64>().ok();
    let row = sqlx::query(
        "SELECT b.height::text AS height, b.state_hash, b.parent_hash,
                b.timestamp, b.chain_status::text AS chain_status,
                creator.value AS creator, winner.value AS block_winner,
                b.ledger_hash, b.global_slot_since_genesis::text AS global_slot,
                COALESCE(uc.hash, zkc.hash) AS transaction_hash,
                CASE WHEN uc.id IS NOT NULL THEN uc.command_type::text
                     WHEN zkc.id IS NOT NULL THEN 'zkapp' END AS transaction_kind,
                COALESCE(buc.status::text, bzc.status::text) AS transaction_status,
                ((SELECT COUNT(*) FROM blocks_user_commands x WHERE x.block_id = b.id) +
                 (SELECT COUNT(*) FROM blocks_zkapp_commands x WHERE x.block_id = b.id))::bigint AS transaction_count
         FROM blocks b
         JOIN public_keys creator ON creator.id = b.creator_id
         JOIN public_keys winner ON winner.id = b.block_winner_id
         LEFT JOIN blocks_user_commands buc ON buc.block_id = b.id
         LEFT JOIN user_commands uc ON uc.id = buc.user_command_id
         LEFT JOIN blocks_zkapp_commands bzc ON bzc.block_id = b.id
         LEFT JOIN zkapp_commands zkc ON zkc.id = bzc.zkapp_command_id
         WHERE ($1::bigint IS NOT NULL AND b.height = $1)
            OR ($1::bigint IS NULL AND b.state_hash = $2)
         ORDER BY buc.sequence_no, bzc.sequence_no LIMIT 1",
    )
    .bind(height)
    .bind(&identifier)
    .fetch_optional(pool)
    .await;
    match row {
        Ok(Some(row)) => match block_json(&row) {
            Ok(mut value) => {
                if let Value::Object(ref mut object) = value {
                    object.insert(
                        "blockWinner".into(),
                        json!(row.try_get::<String, _>("block_winner").ok()),
                    );
                    object.insert(
                        "ledgerHash".into(),
                        json!(row.try_get::<String, _>("ledger_hash").ok()),
                    );
                    object.insert(
                        "globalSlot".into(),
                        json!(row.try_get::<String, _>("global_slot").ok()),
                    );
                }
                Json(value).into_response()
            }
            Err(error) => explorer_error(StatusCode::INTERNAL_SERVER_ERROR, error),
        },
        Ok(None) => explorer_message(StatusCode::NOT_FOUND, "block not found"),
        Err(error) => explorer_error(StatusCode::INTERNAL_SERVER_ERROR, error),
    }
}

async fn list_transactions(
    State(state): State<AppState>,
    Query(query): Query<ListQuery>,
) -> Response {
    let pool = match archive_pool(&state) {
        Ok(pool) => pool,
        Err(response) => return response,
    };
    let cursor = match decode_i64_cursor(query.cursor.as_deref()) {
        Ok(value) => value,
        Err(error) => return explorer_error(StatusCode::BAD_REQUEST, error),
    };
    match load_transactions(
        pool,
        cursor,
        query.kind,
        query.status,
        query.account,
        list_limit(query.limit) + 1,
    )
    .await
    {
        Ok(mut items) => {
            annotate_withdrawal_requests(&state, &mut items).await;
            let limit = list_limit(query.limit) as usize;
            let more = items.len() > limit;
            items.truncate(limit);
            let next = more
                .then(|| items.last())
                .flatten()
                .and_then(|item| item.get("blockHeight"))
                .and_then(Value::as_str)
                .map(encode_cursor);
            Json(page(items, next)).into_response()
        }
        Err(error) => explorer_error(StatusCode::INTERNAL_SERVER_ERROR, error),
    }
}

async fn get_transaction(State(state): State<AppState>, Path(hash): Path<String>) -> Response {
    let pool = match archive_pool(&state) {
        Ok(pool) => pool,
        Err(response) => return response,
    };
    let row = sqlx::query(transaction_detail_sql())
        .bind(&hash)
        .fetch_optional(pool)
        .await;
    match row {
        Ok(Some(row)) => match transaction_json(&row) {
            Ok(mut transaction) => {
                annotate_withdrawal_requests(&state, std::slice::from_mut(&mut transaction)).await;
                if transaction.get("kind").and_then(Value::as_str) == Some("zkapp") {
                    match zkapp_updates(pool, &hash).await {
                        Ok(updates) => transaction["accountUpdates"] = Value::Array(updates),
                        Err(error) => {
                            return explorer_error(StatusCode::INTERNAL_SERVER_ERROR, error)
                        }
                    }
                }
                Json(transaction).into_response()
            }
            Err(error) => explorer_error(StatusCode::INTERNAL_SERVER_ERROR, error),
        },
        Ok(None) => explorer_message(StatusCode::NOT_FOUND, "transaction not found"),
        Err(error) => explorer_error(StatusCode::INTERNAL_SERVER_ERROR, error),
    }
}

async fn get_account(State(state): State<AppState>, Path(public_key): Path<String>) -> Response {
    let pool = match archive_pool(&state) {
        Ok(pool) => pool,
        Err(response) => return response,
    };
    let account = sqlx::query(
        "SELECT pk.value AS public_key, t.value AS token_id, aa.balance,
                aa.nonce::text AS nonce, b.height::text AS block_height,
                b.state_hash, delegate.value AS delegate
         FROM accounts_accessed aa
         JOIN blocks b ON b.id = aa.block_id AND b.chain_status = 'canonical'
         JOIN account_identifiers ai ON ai.id = aa.account_identifier_id
         JOIN public_keys pk ON pk.id = ai.public_key_id
         JOIN tokens t ON t.id = ai.token_id
         LEFT JOIN public_keys delegate ON delegate.id = aa.delegate_id
         WHERE pk.value = $1
           AND t.value = '1'
         ORDER BY b.height DESC LIMIT 1",
    )
    .bind(&public_key)
    .fetch_optional(pool)
    .await;
    match account {
        Ok(Some(row)) => {
            match load_transactions(pool, None, None, None, Some(public_key.clone()), 20).await {
                Ok(mut transactions) => {
                    annotate_withdrawal_requests(&state, &mut transactions).await;
                    Json(json!({
                        "publicKey": row.try_get::<String, _>("public_key").ok(),
                        "tokenId": row.try_get::<String, _>("token_id").ok(),
                        "balance": row.try_get::<String, _>("balance").ok(),
                        "nonce": row.try_get::<String, _>("nonce").ok(),
                        "delegate": row.try_get::<Option<String>, _>("delegate").ok().flatten(),
                        "lastUpdatedBlock": row.try_get::<String, _>("block_height").ok(),
                        "lastUpdatedStateHash": row.try_get::<String, _>("state_hash").ok(),
                        "transactions": transactions
                    }))
                    .into_response()
                }
                Err(error) => explorer_error(StatusCode::INTERNAL_SERVER_ERROR, error),
            }
        }
        Ok(None) => explorer_message(
            StatusCode::NOT_FOUND,
            "account not found in canonical archive history",
        ),
        Err(error) => explorer_error(StatusCode::INTERNAL_SERVER_ERROR, error),
    }
}

async fn list_settlements(
    State(state): State<AppState>,
    Query(query): Query<ListQuery>,
) -> Response {
    let cursor = match decode_time_cursor(query.cursor.as_deref()) {
        Ok(value) => value,
        Err(error) => return explorer_error(StatusCode::BAD_REQUEST, error),
    };
    let limit = list_limit(query.limit);
    let rows = sqlx::query(
        "SELECT source, identifier, status, created_at, transaction_hash,
                batch_sequence, mina_transaction_hash, ledger_hash,
                outer_action_state, outer_action_state_length,
                inner_action_state, inner_action_state_length,
                slot_lower, slot_upper, inner_action_root,
                inner_action_start_index, inner_action_count, claimable_slot,
                confirmations, ethereum_gas_used, cycle_count
         FROM (
           SELECT 'job'::text AS source, jobs.id::text AS identifier,
                  jobs.status::text AS status, jobs.created_at,
                  jobs.transaction_hash, events.batch_sequence::text,
                  COALESCE(events.mina_transaction_hash, jobs.input->>'minaTransactionHash') AS mina_transaction_hash,
                  events.ledger_hash, events.outer_action_state,
                  events.outer_action_state_length::text,
                  events.inner_action_state, events.inner_action_state_length::text,
                  events.slot_lower::text, events.slot_upper::text,
                  events.inner_action_root, events.inner_action_start_index::text,
                  events.inner_action_count::text, events.claimable_slot::text,
                  jobs.confirmations::text, jobs.ethereum_gas_used::text,
                  jobs.cycle_count::text
           FROM proof_jobs jobs
           LEFT JOIN gateway_explorer_settlements events
             ON lower(events.ethereum_tx_hash) = lower(jobs.transaction_hash) AND NOT events.removed
           WHERE jobs.kind = 'settlement'
             AND ($1::timestamptz IS NULL OR jobs.created_at < $1)
             AND ($2::text IS NULL OR jobs.status::text = $2)
           UNION ALL
           SELECT 'event', 'event-' || events.id::text,
                  CASE WHEN blocks.finalized THEN 'confirmed' ELSE 'submitted' END,
                  events.indexed_at,
                  events.ethereum_tx_hash, events.batch_sequence::text,
                  events.mina_transaction_hash, events.ledger_hash,
                  events.outer_action_state, events.outer_action_state_length::text,
                  events.inner_action_state, events.inner_action_state_length::text,
                  events.slot_lower::text, events.slot_upper::text,
                  events.inner_action_root, events.inner_action_start_index::text,
                  events.inner_action_count::text, events.claimable_slot::text,
                  NULL, NULL, NULL
           FROM gateway_explorer_settlements events
           JOIN gateway_blocks blocks
             ON blocks.block_number = events.ethereum_block_number
            AND blocks.block_hash = events.ethereum_block_hash
            AND blocks.canonical
           WHERE NOT events.removed
             AND ($1::timestamptz IS NULL OR events.indexed_at < $1)
             AND ($2::text IS NULL OR $2 = CASE
                   WHEN blocks.finalized THEN 'confirmed' ELSE 'submitted' END)
             AND NOT EXISTS (
                 SELECT 1 FROM proof_jobs jobs
                 WHERE lower(jobs.transaction_hash) = lower(events.ethereum_tx_hash)
             )
         ) settlements
         ORDER BY created_at DESC, identifier DESC LIMIT $3",
    )
    .bind(cursor)
    .bind(query.status)
    .bind(limit + 1)
    .fetch_all(&state.pool)
    .await;
    match rows {
        Ok(mut rows) => {
            let more = rows.len() as i64 > limit;
            rows.truncate(limit as usize);
            let next = more
                .then(|| rows.last())
                .flatten()
                .and_then(|row| row.try_get::<DateTime<Utc>, _>("created_at").ok())
                .map(|time| encode_cursor(&time.to_rfc3339()));
            let items = rows.iter().map(settlement_json).collect::<Result<Vec<_>>>();
            json_result(items.map(|items| page(items, next)))
        }
        Err(error) => explorer_error(StatusCode::INTERNAL_SERVER_ERROR, error),
    }
}

async fn get_settlement(State(state): State<AppState>, Path(identifier): Path<String>) -> Response {
    let row = if let Some(id) = identifier
        .strip_prefix("event-")
        .and_then(|id| id.parse::<i64>().ok())
    {
        sqlx::query(
            "SELECT 'event'::text AS source, 'event-' || events.id::text AS identifier,
                    CASE WHEN blocks.finalized THEN 'confirmed' ELSE 'submitted' END AS status,
                    events.indexed_at AS created_at,
                    events.ethereum_tx_hash AS transaction_hash, events.batch_sequence::text,
                    events.mina_transaction_hash, events.ledger_hash, events.outer_action_state,
                    events.outer_action_state_length::text, events.inner_action_state,
                    events.inner_action_state_length::text, events.slot_lower::text,
                    events.slot_upper::text, events.inner_action_root,
                    events.inner_action_start_index::text, events.inner_action_count::text,
                    events.claimable_slot::text, NULL::text AS confirmations,
                    NULL::text AS ethereum_gas_used, NULL::text AS cycle_count
             FROM gateway_explorer_settlements events
             JOIN gateway_blocks blocks
               ON blocks.block_number = events.ethereum_block_number
              AND blocks.block_hash = events.ethereum_block_hash
              AND blocks.canonical
             WHERE events.id = $1 AND NOT events.removed",
        )
        .bind(id)
        .fetch_optional(&state.pool)
        .await
    } else if let Ok(id) = Uuid::parse_str(&identifier) {
        sqlx::query(
            "SELECT 'job'::text AS source, jobs.id::text AS identifier,
                    jobs.status::text AS status, jobs.created_at,
                    jobs.transaction_hash, events.batch_sequence::text,
                    COALESCE(events.mina_transaction_hash, jobs.input->>'minaTransactionHash') AS mina_transaction_hash,
                    events.ledger_hash, events.outer_action_state,
                    events.outer_action_state_length::text, events.inner_action_state,
                    events.inner_action_state_length::text, events.slot_lower::text,
                    events.slot_upper::text, events.inner_action_root,
                    events.inner_action_start_index::text, events.inner_action_count::text,
                    events.claimable_slot::text, jobs.confirmations::text,
                    jobs.ethereum_gas_used::text, jobs.cycle_count::text
             FROM proof_jobs jobs
             LEFT JOIN gateway_explorer_settlements events
               ON lower(events.ethereum_tx_hash) = lower(jobs.transaction_hash) AND NOT events.removed
             WHERE jobs.id = $1 AND jobs.kind = 'settlement'",
        )
        .bind(id)
        .fetch_optional(&state.pool)
        .await
    } else if let Ok(sequence) = identifier.parse::<i64>() {
        sqlx::query(
            "SELECT 'event'::text AS source, 'event-' || events.id::text AS identifier,
                    CASE WHEN blocks.finalized THEN 'confirmed' ELSE 'submitted' END AS status,
                    events.indexed_at AS created_at,
                    events.ethereum_tx_hash AS transaction_hash, events.batch_sequence::text,
                    events.mina_transaction_hash, events.ledger_hash, events.outer_action_state,
                    events.outer_action_state_length::text, events.inner_action_state,
                    events.inner_action_state_length::text, events.slot_lower::text,
                    events.slot_upper::text, events.inner_action_root,
                    events.inner_action_start_index::text, events.inner_action_count::text,
                    events.claimable_slot::text, NULL::text AS confirmations,
                    NULL::text AS ethereum_gas_used, NULL::text AS cycle_count
             FROM gateway_explorer_settlements events
             JOIN gateway_blocks blocks
               ON blocks.block_number = events.ethereum_block_number
              AND blocks.block_hash = events.ethereum_block_hash
              AND blocks.canonical
             WHERE events.batch_sequence = $1 AND NOT events.removed
             ORDER BY events.ethereum_block_number DESC LIMIT 1",
        )
        .bind(sequence)
        .fetch_optional(&state.pool)
        .await
    } else {
        return explorer_message(StatusCode::BAD_REQUEST, "invalid settlement identifier");
    };
    match row {
        Ok(Some(row)) => match settlement_json(&row) {
            Ok(value) => Json(value).into_response(),
            Err(error) => explorer_error(StatusCode::INTERNAL_SERVER_ERROR, error),
        },
        Ok(None) => explorer_message(StatusCode::NOT_FOUND, "settlement not found"),
        Err(error) => explorer_error(StatusCode::INTERNAL_SERVER_ERROR, error),
    }
}

async fn list_deposits(State(state): State<AppState>, Query(query): Query<ListQuery>) -> Response {
    let cursor = match decode_i64_cursor(query.cursor.as_deref()) {
        Ok(value) => value,
        Err(error) => return explorer_error(StatusCode::BAD_REQUEST, error),
    };
    let limit = list_limit(query.limit);
    let rows = deposit_query(&state.pool, None, cursor, query.status, limit + 1).await;
    match rows {
        Ok(mut rows) => {
            let more = rows.len() as i64 > limit;
            rows.truncate(limit as usize);
            let next = more
                .then(|| rows.last())
                .flatten()
                .and_then(|row| row.try_get::<i64, _>("nonce").ok())
                .map(|nonce| encode_cursor(&nonce.to_string()));
            let items = rows.iter().map(deposit_json).collect::<Result<Vec<_>>>();
            json_result(items.map(|items| page(items, next)))
        }
        Err(error) => explorer_error(StatusCode::INTERNAL_SERVER_ERROR, error),
    }
}

async fn get_deposit(State(state): State<AppState>, Path(nonce): Path<u64>) -> Response {
    let nonce = match i64::try_from(nonce) {
        Ok(value) => value,
        Err(_) => return explorer_message(StatusCode::BAD_REQUEST, "deposit nonce is too large"),
    };
    match deposit_query(&state.pool, Some(nonce), None, None, 1).await {
        Ok(rows) if !rows.is_empty() => match deposit_json(&rows[0]) {
            Ok(value) => Json(value).into_response(),
            Err(error) => explorer_error(StatusCode::INTERNAL_SERVER_ERROR, error),
        },
        Ok(_) => explorer_message(StatusCode::NOT_FOUND, "deposit not found"),
        Err(error) => explorer_error(StatusCode::INTERNAL_SERVER_ERROR, error),
    }
}

async fn list_withdrawals(
    State(state): State<AppState>,
    Query(query): Query<ListQuery>,
) -> Response {
    let cursor = match decode_i64_cursor(query.cursor.as_deref()) {
        Ok(value) => value,
        Err(error) => return explorer_error(StatusCode::BAD_REQUEST, error),
    };
    let limit = list_limit(query.limit);
    let locations = sqlx::query(
        "SELECT settlement_sequence, action_offset, global_action_index
         FROM gateway_inner_action_leaves
         WHERE recipient IS NOT NULL AND NOT removed
           AND ($1::bigint IS NULL OR global_action_index < $1)
         ORDER BY global_action_index DESC LIMIT $2",
    )
    .bind(cursor)
    .bind(limit + 1)
    .fetch_all(&state.pool)
    .await;
    match locations {
        Ok(mut rows) => {
            let more = rows.len() as i64 > limit;
            rows.truncate(limit as usize);
            let next = more
                .then(|| rows.last())
                .flatten()
                .and_then(|row| row.try_get::<i64, _>("global_action_index").ok())
                .map(|index| encode_cursor(&index.to_string()));
            let mut items = Vec::with_capacity(rows.len());
            for row in rows {
                let sequence = match row
                    .try_get::<i64, _>("settlement_sequence")
                    .ok()
                    .and_then(|v| u64::try_from(v).ok())
                {
                    Some(value) => value,
                    None => {
                        return explorer_message(
                            StatusCode::INTERNAL_SERVER_ERROR,
                            "invalid indexed settlement sequence",
                        )
                    }
                };
                let offset = match row
                    .try_get::<i32, _>("action_offset")
                    .ok()
                    .and_then(|v| u32::try_from(v).ok())
                {
                    Some(value) => value,
                    None => {
                        return explorer_message(
                            StatusCode::INTERNAL_SERVER_ERROR,
                            "invalid indexed withdrawal offset",
                        )
                    }
                };
                match withdrawal_json(&state, sequence, offset).await {
                    Ok(value) => items.push(value),
                    Err(error) => return explorer_error(StatusCode::BAD_GATEWAY, error),
                }
            }
            Json(page(items, next)).into_response()
        }
        Err(error) => explorer_error(StatusCode::INTERNAL_SERVER_ERROR, error),
    }
}

async fn get_withdrawal(
    State(state): State<AppState>,
    Path((sequence, offset)): Path<(u64, u32)>,
) -> Response {
    let exists = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (
           SELECT 1 FROM gateway_inner_action_leaves
           WHERE settlement_sequence = $1 AND action_offset = $2
             AND recipient IS NOT NULL AND NOT removed
         )",
    )
    .bind(match i64::try_from(sequence) {
        Ok(value) => value,
        Err(_) => return explorer_message(StatusCode::BAD_REQUEST, "settlement is too large"),
    })
    .bind(match i32::try_from(offset) {
        Ok(value) => value,
        Err(_) => return explorer_message(StatusCode::BAD_REQUEST, "offset is too large"),
    })
    .fetch_one(&state.pool)
    .await;
    match exists {
        Ok(false) => return explorer_message(StatusCode::NOT_FOUND, "withdrawal not found"),
        Err(error) => return explorer_error(StatusCode::INTERNAL_SERVER_ERROR, error),
        Ok(true) => {}
    }
    match withdrawal_json(&state, sequence, offset).await {
        Ok(value) => Json(value).into_response(),
        Err(error) => explorer_error(StatusCode::BAD_GATEWAY, error),
    }
}

async fn search(State(state): State<AppState>, Query(query): Query<SearchQuery>) -> Response {
    let needle = query.q.trim();
    if needle.len() < 2 || needle.len() > 180 {
        return explorer_message(
            StatusCode::BAD_REQUEST,
            "search query must be between 2 and 180 characters",
        );
    }
    let mut groups = json!({ "blocks": [], "transactions": [], "accounts": [], "settlements": [], "deposits": [], "withdrawals": [] });
    if let Some(pool) = state.archive_pool.as_ref() {
        let height = needle.replace(',', "").parse::<i64>().ok();
        if let Ok(rows) = sqlx::query(
            "SELECT height::text, state_hash FROM blocks
             WHERE state_hash = $1 OR ($2::bigint IS NOT NULL AND height = $2)
             ORDER BY height DESC LIMIT 5",
        )
        .bind(needle)
        .bind(height)
        .fetch_all(pool)
        .await
        {
            groups["blocks"] = Value::Array(rows.into_iter().map(|row| json!({ "height": row.try_get::<String, _>("height").ok(), "stateHash": row.try_get::<String, _>("state_hash").ok() })).collect());
        }
        if let Ok(rows) = sqlx::query(
            "SELECT hash, kind FROM (
                SELECT hash, command_type::text AS kind FROM user_commands WHERE hash = $1
                UNION ALL SELECT hash, 'zkapp' FROM zkapp_commands WHERE hash = $1
             ) matches LIMIT 5",
        )
        .bind(needle)
        .fetch_all(pool)
        .await
        {
            groups["transactions"] = Value::Array(rows.into_iter().map(|row| json!({ "hash": row.try_get::<String, _>("hash").ok(), "kind": row.try_get::<String, _>("kind").ok() })).collect());
        }
        if needle.starts_with("B62") {
            if let Ok(rows) = sqlx::query("SELECT value FROM public_keys WHERE value = $1 LIMIT 5")
                .bind(needle)
                .fetch_all(pool)
                .await
            {
                groups["accounts"] = Value::Array(
                    rows.into_iter()
                        .map(|row| json!({ "publicKey": row.try_get::<String, _>("value").ok() }))
                        .collect(),
                );
            }
        }
    }
    let numeric = needle.parse::<i64>().ok();
    if let Ok(rows) = sqlx::query(
        "SELECT batch_sequence::text, ethereum_tx_hash FROM gateway_explorer_settlements
         WHERE NOT removed AND (ethereum_tx_hash = $1 OR mina_transaction_hash = $1
           OR ledger_hash = $1 OR ($2::bigint IS NOT NULL AND batch_sequence = $2))
         ORDER BY batch_sequence DESC LIMIT 5",
    )
    .bind(needle)
    .bind(numeric)
    .fetch_all(&state.pool)
    .await
    {
        groups["settlements"] = Value::Array(rows.into_iter().map(|row| json!({ "sequence": row.try_get::<String, _>("batch_sequence").ok(), "ethereumTransactionHash": row.try_get::<String, _>("ethereum_tx_hash").ok() })).collect());
    }
    if let Ok(rows) = sqlx::query(
        "SELECT nonce::text, ethereum_tx_hash, sender FROM gateway_bridge_deposits
         WHERE NOT removed AND (ethereum_tx_hash = $1 OR lower(sender) = lower($1)
           OR zeko_recipient = $1 OR ($2::bigint IS NOT NULL AND nonce = $2))
         ORDER BY nonce DESC LIMIT 5",
    )
    .bind(needle)
    .bind(numeric)
    .fetch_all(&state.pool)
    .await
    {
        groups["deposits"] = Value::Array(rows.into_iter().map(|row| json!({ "nonce": row.try_get::<String, _>("nonce").ok(), "ethereumTransactionHash": row.try_get::<String, _>("ethereum_tx_hash").ok(), "sender": row.try_get::<String, _>("sender").ok() })).collect());
    }
    if needle.starts_with("0x") && matches!(needle.len(), 42 | 66) {
        if let Ok(rows) = sqlx::query(
            "SELECT leaves.settlement_sequence::text, leaves.action_offset,
                    leaves.recipient
             FROM gateway_inner_action_leaves leaves
             LEFT JOIN gateway_native_withdrawal_claims claims
               ON claims.settlement_sequence = leaves.settlement_sequence
              AND claims.global_action_index = leaves.global_action_index
              AND NOT claims.removed
             WHERE leaves.recipient IS NOT NULL AND NOT leaves.removed
               AND (lower(leaves.recipient) = lower($1)
                    OR lower(claims.ethereum_tx_hash) = lower($1))
             ORDER BY leaves.global_action_index DESC LIMIT 5",
        )
        .bind(needle)
        .fetch_all(&state.pool)
        .await
        {
            groups["withdrawals"] = Value::Array(rows.into_iter().map(|row| json!({ "settlementSequence": row.try_get::<String, _>("settlement_sequence").ok(), "offset": row.try_get::<i32, _>("action_offset").ok(), "recipient": row.try_get::<String, _>("recipient").ok() })).collect());
        }
    }
    Json(json!({ "query": needle, "groups": groups })).into_response()
}

async fn load_transactions(
    pool: &PgPool,
    cursor: Option<i64>,
    kind: Option<String>,
    status: Option<String>,
    account: Option<String>,
    limit: i64,
) -> Result<Vec<Value>> {
    let rows = sqlx::query(transaction_list_sql())
        .bind(cursor)
        .bind(kind)
        .bind(status)
        .bind(account)
        .bind(limit)
        .fetch_all(pool)
        .await?;
    rows.iter().map(transaction_json).collect()
}

fn transaction_list_sql() -> &'static str {
    "WITH transactions AS (
       SELECT uc.hash, uc.command_type::text AS kind, buc.status::text AS status,
              buc.failure_reason, b.height, b.state_hash, b.timestamp,
              fee.value AS fee_payer, source.value AS source,
              receiver.value AS receiver, uc.amount, uc.fee,
              uc.nonce::text AS nonce, uc.memo, 0::bigint AS account_update_count
       FROM blocks_user_commands buc
       JOIN blocks b ON b.id = buc.block_id
       JOIN user_commands uc ON uc.id = buc.user_command_id
       JOIN public_keys fee ON fee.id = uc.fee_payer_id
       JOIN public_keys source ON source.id = uc.source_id
       JOIN public_keys receiver ON receiver.id = uc.receiver_id
       WHERE b.chain_status IN ('canonical', 'pending')
       UNION ALL
       SELECT zkc.hash, 'zkapp', bzc.status::text, NULL::text,
              b.height, b.state_hash, b.timestamp, fee.value,
              NULL::text, NULL::text, NULL::text, payer.fee,
              payer.nonce::text, zkc.memo,
              cardinality(zkc.zkapp_account_updates_ids)::bigint
       FROM blocks_zkapp_commands bzc
       JOIN blocks b ON b.id = bzc.block_id
       JOIN zkapp_commands zkc ON zkc.id = bzc.zkapp_command_id
       JOIN zkapp_fee_payer_body payer ON payer.id = zkc.zkapp_fee_payer_body_id
       JOIN public_keys fee ON fee.id = payer.public_key_id
       WHERE b.chain_status IN ('canonical', 'pending')
     )
     SELECT hash, kind, status, failure_reason, height::text AS block_height,
            state_hash, timestamp, fee_payer, source, receiver, amount, fee,
            nonce, memo, account_update_count
     FROM transactions
     WHERE ($1::bigint IS NULL OR height < $1)
       AND ($2::text IS NULL OR kind = $2)
       AND ($3::text IS NULL OR status = $3)
       AND ($4::text IS NULL OR fee_payer = $4 OR source = $4 OR receiver = $4
            OR EXISTS (
                SELECT 1 FROM zkapp_commands command
                CROSS JOIN LATERAL unnest(command.zkapp_account_updates_ids) body_id
                JOIN zkapp_account_update_body body ON body.id = body_id
                JOIN account_identifiers ai ON ai.id = body.account_identifier_id
                JOIN public_keys pk ON pk.id = ai.public_key_id
                WHERE command.hash = transactions.hash AND pk.value = $4
            ))
     ORDER BY height DESC, hash DESC LIMIT $5"
}

fn transaction_detail_sql() -> &'static str {
    "SELECT * FROM (
       SELECT uc.hash, uc.command_type::text AS kind, buc.status::text AS status,
              buc.failure_reason, b.height::text AS block_height, b.state_hash,
              b.timestamp, fee.value AS fee_payer, source.value AS source,
              receiver.value AS receiver, uc.amount, uc.fee,
              uc.nonce::text AS nonce, uc.memo, 0::bigint AS account_update_count
       FROM blocks_user_commands buc
       JOIN blocks b ON b.id = buc.block_id
       JOIN user_commands uc ON uc.id = buc.user_command_id
       JOIN public_keys fee ON fee.id = uc.fee_payer_id
       JOIN public_keys source ON source.id = uc.source_id
       JOIN public_keys receiver ON receiver.id = uc.receiver_id
       WHERE uc.hash = $1
       UNION ALL
       SELECT zkc.hash, 'zkapp', bzc.status::text, NULL::text,
              b.height::text, b.state_hash, b.timestamp, fee.value,
              NULL::text, NULL::text, NULL::text, payer.fee,
              payer.nonce::text, zkc.memo,
              cardinality(zkc.zkapp_account_updates_ids)::bigint
       FROM blocks_zkapp_commands bzc
       JOIN blocks b ON b.id = bzc.block_id
       JOIN zkapp_commands zkc ON zkc.id = bzc.zkapp_command_id
       JOIN zkapp_fee_payer_body payer ON payer.id = zkc.zkapp_fee_payer_body_id
       JOIN public_keys fee ON fee.id = payer.public_key_id
       WHERE zkc.hash = $1
     ) transaction LIMIT 1"
}

fn transaction_json(row: &sqlx::postgres::PgRow) -> Result<Value> {
    Ok(json!({
        "hash": row.try_get::<String, _>("hash")?,
        "kind": row.try_get::<String, _>("kind")?,
        "status": row.try_get::<String, _>("status")?,
        "failureReason": row.try_get::<Option<String>, _>("failure_reason")?,
        "blockHeight": row.try_get::<String, _>("block_height")?,
        "stateHash": row.try_get::<String, _>("state_hash")?,
        "timestamp": row.try_get::<String, _>("timestamp")?,
        "feePayer": row.try_get::<String, _>("fee_payer")?,
        "source": row.try_get::<Option<String>, _>("source")?,
        "receiver": row.try_get::<Option<String>, _>("receiver")?,
        "amount": row.try_get::<Option<String>, _>("amount")?,
        "fee": row.try_get::<String, _>("fee")?,
        "nonce": row.try_get::<String, _>("nonce")?,
        "memo": row.try_get::<String, _>("memo")?,
        "accountUpdateCount": row.try_get::<i64, _>("account_update_count")?.to_string()
    }))
}

async fn annotate_withdrawal_requests(state: &AppState, transactions: &mut [Value]) {
    let (Some(archive), Some(inner_public_key)) = (
        state.archive_pool.as_ref(),
        state.inner_public_key.as_deref(),
    ) else {
        return;
    };
    let actions =
        match crate::withdrawal_activity::load_archive_inner_actions(archive, inner_public_key)
            .await
        {
            Ok(actions) => actions,
            Err(error) => {
                tracing::debug!(%error, "could not annotate explorer withdrawal requests");
                return;
            }
        };
    let settled = sqlx::query_scalar::<_, i64>(
        "SELECT global_action_index FROM gateway_inner_action_leaves WHERE NOT removed",
    )
    .fetch_all(&state.pool)
    .await
    .unwrap_or_default()
    .into_iter()
    .collect::<std::collections::HashSet<_>>();
    let by_hash = actions
        .into_iter()
        .filter_map(|action| {
            let withdrawal = action.withdrawal?;
            let phase = if settled.contains(&i64::from(action.global_action_index)) {
                "settled"
            } else {
                "pendingSettlement"
            };
            Some((
                action.transaction_hash,
                json!({
                    "kind": "nativeWithdrawalRequest",
                    "phase": phase,
                    "globalActionIndex": action.global_action_index,
                    "recipient": EthereumAddress::from(withdrawal.recipient).to_string(),
                    "amount": withdrawal.amount.to_string()
                }),
            ))
        })
        .collect::<std::collections::HashMap<_, _>>();
    for transaction in transactions {
        let Some(hash) = transaction.get("hash").and_then(Value::as_str) else {
            continue;
        };
        if let Some(operation) = by_hash.get(hash) {
            transaction["bridgeOperation"] = operation.clone();
        }
    }
}

async fn zkapp_updates(pool: &PgPool, hash: &str) -> Result<Vec<Value>> {
    let rows = sqlx::query(
        "SELECT (updates.ordinality - 1)::text AS update_index,
                pk.value AS public_key, token.value AS token_id,
                body.balance_change, body.increment_nonce,
                body.call_depth::text AS call_depth,
                body.authorization_kind::text AS authorization_kind,
                body.use_full_commitment, body.may_use_token::text AS may_use_token
         FROM zkapp_commands command
         CROSS JOIN LATERAL unnest(command.zkapp_account_updates_ids)
             WITH ORDINALITY AS updates(body_id, ordinality)
         JOIN zkapp_account_update_body body ON body.id = updates.body_id
         JOIN account_identifiers ai ON ai.id = body.account_identifier_id
         JOIN public_keys pk ON pk.id = ai.public_key_id
         JOIN tokens token ON token.id = ai.token_id
         WHERE command.hash = $1 ORDER BY updates.ordinality",
    )
    .bind(hash)
    .fetch_all(pool)
    .await?;
    rows.into_iter()
        .map(|row| {
            Ok(json!({
                "index": row.try_get::<String, _>("update_index")?,
                "publicKey": row.try_get::<String, _>("public_key")?,
                "tokenId": row.try_get::<String, _>("token_id")?,
                "balanceChange": row.try_get::<String, _>("balance_change")?,
                "incrementNonce": row.try_get::<bool, _>("increment_nonce")?,
                "callDepth": row.try_get::<String, _>("call_depth")?,
                "authorizationKind": row.try_get::<String, _>("authorization_kind")?,
                "useFullCommitment": row.try_get::<bool, _>("use_full_commitment")?,
                "mayUseToken": row.try_get::<String, _>("may_use_token")?
            }))
        })
        .collect()
}

fn block_json(row: &sqlx::postgres::PgRow) -> Result<Value> {
    let count = row.try_get::<i64, _>("transaction_count")?;
    anyhow::ensure!(count <= 1, "block contains more than one user transaction");
    Ok(json!({
        "height": row.try_get::<String, _>("height")?,
        "stateHash": row.try_get::<String, _>("state_hash")?,
        "parentHash": row.try_get::<String, _>("parent_hash")?,
        "timestamp": row.try_get::<String, _>("timestamp")?,
        "chainStatus": row.try_get::<String, _>("chain_status")?,
        "creator": row.try_get::<String, _>("creator")?,
        "transactionCount": count.to_string(),
        "transaction": row.try_get::<Option<String>, _>("transaction_hash")?.map(|hash| json!({
            "hash": hash,
            "kind": row.try_get::<Option<String>, _>("transaction_kind").ok().flatten(),
            "status": row.try_get::<Option<String>, _>("transaction_status").ok().flatten()
        }))
    }))
}

fn settlement_json(row: &sqlx::postgres::PgRow) -> Result<Value> {
    Ok(json!({
        "id": row.try_get::<String, _>("identifier")?,
        "source": row.try_get::<String, _>("source")?,
        "status": row.try_get::<String, _>("status")?,
        "createdAt": row.try_get::<DateTime<Utc>, _>("created_at")?,
        "batchSequence": row.try_get::<Option<String>, _>("batch_sequence")?,
        "settlementCommandDigest": row.try_get::<Option<String>, _>("mina_transaction_hash")?,
        "ethereumTransactionHash": row.try_get::<Option<String>, _>("transaction_hash")?,
        "ledgerHash": row.try_get::<Option<String>, _>("ledger_hash")?,
        "outerActionState": row.try_get::<Option<String>, _>("outer_action_state")?,
        "outerActionStateLength": row.try_get::<Option<String>, _>("outer_action_state_length")?,
        "innerActionState": row.try_get::<Option<String>, _>("inner_action_state")?,
        "innerActionStateLength": row.try_get::<Option<String>, _>("inner_action_state_length")?,
        "slotLower": row.try_get::<Option<String>, _>("slot_lower")?,
        "slotUpper": row.try_get::<Option<String>, _>("slot_upper")?,
        "innerActionRoot": row.try_get::<Option<String>, _>("inner_action_root")?,
        "innerActionStartIndex": row.try_get::<Option<String>, _>("inner_action_start_index")?,
        "innerActionCount": row.try_get::<Option<String>, _>("inner_action_count")?,
        "claimableSlot": row.try_get::<Option<String>, _>("claimable_slot")?,
        "confirmations": row.try_get::<Option<String>, _>("confirmations")?,
        "ethereumGasUsed": row.try_get::<Option<String>, _>("ethereum_gas_used")?,
        "cycleCount": row.try_get::<Option<String>, _>("cycle_count")?
    }))
}

async fn deposit_query(
    pool: &PgPool,
    nonce: Option<i64>,
    cursor: Option<i64>,
    status: Option<String>,
    limit: i64,
) -> Result<Vec<sqlx::postgres::PgRow>> {
    Ok(sqlx::query(
        "SELECT deposits.nonce, deposits.token, deposits.sender,
                deposits.zeko_recipient,
                deposits.ethereum_amount::text AS ethereum_amount,
                deposits.zeko_amount::text AS zeko_amount, deposits.timeout,
                deposits.ethereum_tx_hash, deposits.ethereum_block_number,
                blocks.finalized, deposits.bridge_job_id,
                bridge_jobs.status::text AS bridge_job_status,
                deposits.outer_action_sequence, deposits.outer_action_state_after,
                deposits.synchronized_settlement_sequence
         FROM gateway_bridge_deposits deposits
         JOIN gateway_blocks blocks
           ON blocks.block_number = deposits.ethereum_block_number
          AND blocks.block_hash = deposits.ethereum_block_hash
         LEFT JOIN proof_jobs bridge_jobs ON bridge_jobs.id = deposits.bridge_job_id
         WHERE NOT deposits.removed AND blocks.canonical
           AND ($1::bigint IS NULL OR deposits.nonce = $1)
           AND ($2::bigint IS NULL OR deposits.nonce < $2)
           AND ($3::text IS NULL OR
                CASE
                  WHEN NOT blocks.finalized THEN 'confirming'
                  WHEN deposits.synchronized_settlement_sequence IS NOT NULL THEN 'synchronized'
                  WHEN deposits.outer_action_sequence IS NOT NULL THEN 'bridgeProven'
                  WHEN bridge_jobs.status::text IN ('failed','proof_failed','ethereum_reverted','reorged','rejected') THEN 'proofFailed'
                  WHEN bridge_jobs.status::text IN ('queued','validating') THEN 'proofQueued'
                  WHEN bridge_jobs.status::text = 'awaiting_approval' THEN 'awaitingProofApproval'
                  WHEN bridge_jobs.status::text IN ('approved','proof_requested','proving') THEN 'proving'
                  WHEN bridge_jobs.status::text IN ('submitting','submitted') THEN 'submitting'
                  WHEN bridge_jobs.status::text = 'executed' THEN 'executed'
                  WHEN bridge_jobs.status::text = 'confirmed' THEN 'bridgeProven'
                  ELSE 'locked'
                END = $3)
         ORDER BY deposits.nonce DESC LIMIT $4",
    )
    .bind(nonce)
    .bind(cursor)
    .bind(status)
    .bind(limit)
    .fetch_all(pool)
    .await?)
}

fn deposit_json(row: &sqlx::postgres::PgRow) -> Result<Value> {
    let finalized = row.try_get::<bool, _>("finalized")?;
    let bridge_status = row.try_get::<Option<String>, _>("bridge_job_status")?;
    let bridge_proven = row
        .try_get::<Option<i64>, _>("outer_action_sequence")?
        .is_some();
    let synchronized = row
        .try_get::<Option<i64>, _>("synchronized_settlement_sequence")?
        .is_some();
    let (status, next_action) = crate::native_deposit_progress(
        finalized,
        bridge_status.as_deref(),
        bridge_proven,
        synchronized,
    );
    Ok(json!({
        "nonce": row.try_get::<i64, _>("nonce")?.to_string(),
        "token": row.try_get::<String, _>("token")?,
        "sender": row.try_get::<String, _>("sender")?,
        "zekoRecipient": row.try_get::<String, _>("zeko_recipient")?,
        "ethereumAmount": row.try_get::<String, _>("ethereum_amount")?,
        "zekoAmount": row.try_get::<String, _>("zeko_amount")?,
        "timeout": row.try_get::<i64, _>("timeout")?.to_string(),
        "ethereumTransactionHash": row.try_get::<String, _>("ethereum_tx_hash")?,
        "ethereumBlockNumber": row.try_get::<i64, _>("ethereum_block_number")?.to_string(),
        "ethereumFinalized": finalized,
        "bridgeJobId": row.try_get::<Option<Uuid>, _>("bridge_job_id")?,
        "bridgeJobStatus": bridge_status,
        "outerActionSequence": row.try_get::<Option<i64>, _>("outer_action_sequence")?.map(|v| v.to_string()),
        "outerActionStateAfter": row.try_get::<Option<String>, _>("outer_action_state_after")?,
        "synchronizedSettlementSequence": row.try_get::<Option<i64>, _>("synchronized_settlement_sequence")?.map(|v| v.to_string()),
        "status": status,
        "nextAction": next_action,
        "accuracyNote": if synchronized { Some("Synchronization is authoritative; the archive does not persist a canonical deposit-nonce to L2-finalization mapping.") } else { None }
    }))
}

async fn withdrawal_json(state: &AppState, sequence: u64, offset: u32) -> Result<Value> {
    let proof = crate::load_native_withdrawal_proof(state, sequence, offset).await?;
    let claim = sqlx::query(
        "SELECT ethereum_tx_hash, ethereum_block_number::text AS ethereum_block_number,
                ethereum_amount::text AS ethereum_amount
         FROM gateway_native_withdrawal_claims
         WHERE settlement_sequence = $1 AND global_action_index = $2 AND NOT removed",
    )
    .bind(i64::try_from(sequence)?)
    .bind(i64::from(proof.global_action_index))
    .fetch_optional(&state.pool)
    .await?;
    let mut value = serde_json::to_value(&proof)?;
    value["settlementSequence"] = json!(sequence.to_string());
    value["globalActionIndex"] = json!(proof.global_action_index.to_string());
    value["claimableSlot"] = json!(proof.claimable_slot.to_string());
    value["currentVirtualSlot"] = json!(proof.current_virtual_slot.to_string());
    value["recipientCursor"] = json!(proof.recipient_cursor.to_string());
    if let Some(claim) = claim {
        value["claimEthereumTransactionHash"] =
            json!(claim.try_get::<String, _>("ethereum_tx_hash")?);
        value["claimEthereumBlockNumber"] =
            json!(claim.try_get::<String, _>("ethereum_block_number")?);
        value["claimEthereumAmount"] = json!(claim.try_get::<String, _>("ethereum_amount")?);
    }
    Ok(value)
}

fn page(items: Vec<Value>, next_cursor: Option<String>) -> Value {
    json!({ "items": items, "nextCursor": next_cursor })
}

fn list_limit(limit: Option<i64>) -> i64 {
    limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT)
}

fn encode_cursor(value: &str) -> String {
    hex::encode(value.as_bytes())
}

fn decode_cursor(value: &str) -> Result<String> {
    Ok(
        String::from_utf8(hex::decode(value).context("cursor must be hexadecimal")?)
            .context("cursor must contain UTF-8")?,
    )
}

fn decode_i64_cursor(value: Option<&str>) -> Result<Option<i64>> {
    value
        .map(|cursor| {
            decode_cursor(cursor)?
                .parse::<i64>()
                .context("cursor is not numeric")
        })
        .transpose()
}

fn decode_time_cursor(value: Option<&str>) -> Result<Option<DateTime<Utc>>> {
    value
        .map(|cursor| {
            DateTime::parse_from_rfc3339(&decode_cursor(cursor)?)
                .map(|time| time.with_timezone(&Utc))
                .context("cursor is not an RFC3339 timestamp")
        })
        .transpose()
}

fn archive_pool(state: &AppState) -> std::result::Result<&PgPool, Response> {
    state.archive_pool.as_ref().ok_or_else(|| {
        explorer_message(
            StatusCode::SERVICE_UNAVAILABLE,
            "Zeko archive database is not configured",
        )
    })
}

fn json_result(result: Result<Value>) -> Response {
    match result {
        Ok(value) => Json(value).into_response(),
        Err(error) => explorer_error(StatusCode::INTERNAL_SERVER_ERROR, error),
    }
}

fn explorer_error(status: StatusCode, error: impl std::fmt::Display) -> Response {
    tracing::error!(%error, "explorer request failed");
    if status.is_server_error() {
        explorer_message(status, "explorer source is temporarily unavailable")
    } else {
        explorer_message(status, &error.to_string())
    }
}

fn explorer_message(status: StatusCode, message: &str) -> Response {
    (status, Json(json!({ "error": message }))).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursors_are_opaque_and_preserve_full_i64_precision() {
        let value = i64::MAX.to_string();
        let encoded = encode_cursor(&value);
        assert_ne!(encoded, value);
        assert_eq!(decode_i64_cursor(Some(&encoded)).unwrap(), Some(i64::MAX));
    }

    #[test]
    fn list_limits_are_bounded() {
        assert_eq!(list_limit(None), 20);
        assert_eq!(list_limit(Some(0)), 1);
        assert_eq!(list_limit(Some(1_000)), 100);
    }

    #[test]
    fn public_pages_do_not_embed_proof_job_input() {
        let fields = [
            "id",
            "source",
            "status",
            "createdAt",
            "batchSequence",
            "settlementCommandDigest",
            "ethereumTransactionHash",
            "ledgerHash",
            "outerActionState",
            "innerActionState",
            "slotLower",
            "slotUpper",
            "confirmations",
            "ethereumGasUsed",
            "cycleCount",
        ];
        assert!(!fields.contains(&"input"));
        assert!(!fields.contains(&"proofRequestId"));
        assert!(!fields.contains(&"approvalInputDigest"));
    }

    #[test]
    fn commit_schedule_schema_accepts_expected_phases_and_rejects_bad_data() {
        let response: CommitScheduleGraphqlResponse = serde_json::from_value(json!({
            "data": {
                "commitSchedule": {
                    "periodSeconds": 900,
                    "phase": "WAITING",
                    "lastAttemptStartedAt": "2026-07-15T15:00:00Z",
                    "nextAttemptAt": "2026-07-15T15:15:00Z"
                }
            }
        }))
        .unwrap();
        let schedule = response.data.unwrap().commit_schedule;
        validate_commit_schedule(&schedule).unwrap();

        let invalid = CommitSchedule {
            phase: "UNKNOWN".to_owned(),
            ..schedule
        };
        assert!(validate_commit_schedule(&invalid).is_err());
    }
}
