use axum::{extract::State, response::IntoResponse, Json};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::Digest;
use sqlx::Row;
use uuid::Uuid;

use crate::AppState;

// Mina's OCaml GraphQL client serializes the default token as its base58
// TokenId, while the gateway stores the same token using the conventional
// decimal spelling used by the browser APIs.
const MINA_DEFAULT_TOKEN_ID: &str = "wSHV2S4qX9jFsLjQo8r1BsMLH2ZRKsZx6EJd1sbozGPieEC4Jf";

#[derive(Debug, Deserialize)]
pub struct GraphqlRequest {
    query: String,
    #[serde(default)]
    variables: Value,
}

pub async fn handle(
    State(state): State<AppState>,
    Json(request): Json<GraphqlRequest>,
) -> impl IntoResponse {
    match execute(&state, &request).await {
        Ok(data) => Json(json!({"data": data})),
        Err(error) => {
            tracing::warn!(%error, query = %request.query, "virtual Mina GraphQL query failed");
            Json(json!({"errors": [{"message": error.to_string()}]}))
        }
    }
}

async fn execute(state: &AppState, request: &GraphqlRequest) -> anyhow::Result<Value> {
    let query = request.query.as_str();
    if query.contains("sendZkapp") {
        return send_zkapp(state, request).await;
    }
    if query.contains("pooledZkappCommands") {
        return pooled_commands(state, request, "zkapp", "pooledZkappCommands").await;
    }
    if query.contains("pooledUserCommands") {
        return pooled_commands(state, request, "signed", "pooledUserCommands").await;
    }
    if query.contains("actions(") {
        return actions(state, request).await;
    }
    if query.contains("events(") {
        return Ok(json!({"events": []}));
    }
    if query.contains("account(") {
        return account(state, request).await;
    }
    if query.contains("genesisConstants") {
        let row = config(state).await?;
        let fee: String = row.try_get("account_creation_fee")?;
        let timestamp: String = row.try_get("genesis_timestamp")?;
        let mut constants = serde_json::Map::new();
        if query.contains("accountCreationFee") {
            constants.insert("accountCreationFee".to_owned(), Value::String(fee));
        }
        if query.contains("genesisTimestamp") {
            constants.insert("genesisTimestamp".to_owned(), Value::String(timestamp));
        }
        return Ok(json!({"genesisConstants": constants}));
    }
    if query.contains("runtimeConfig") {
        let row = config(state).await?;
        let fork_slot: i32 = row.try_get("fork_slot")?;
        return Ok(json!({
            "runtimeConfig": {
                "proof": {"fork": {"global_slot_since_genesis": fork_slot}}
            }
        }));
    }
    if query.contains("networkState") {
        return network_state(state).await;
    }
    if query.contains("bestChain") {
        return best_chain(state, request).await;
    }
    anyhow::bail!("operation is outside the supported Mina compatibility subset")
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GatewaySettlement {
    schema_version: u16,
    mina_transaction_hash: String,
    outer_account_public_key: String,
    fee_payer_public_key: String,
    nonce: u64,
    command_base64: String,
    proof: zkapp_script::SettlementProofBundle,
}

async fn send_zkapp(state: &AppState, request: &GraphqlRequest) -> anyhow::Result<Value> {
    let supplied_token = variable_string(request, "gatewayToken")?;
    anyhow::ensure!(
        supplied_token == state.api_key.as_ref(),
        "invalid Ethereum gateway token"
    );
    let mut settlement: GatewaySettlement = serde_json::from_value(
        request
            .variables
            .get("settlement")
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("missing settlement proof export"))?,
    )?;
    anyhow::ensure!(
        settlement.schema_version == 1,
        "unsupported settlement schema"
    );
    anyhow::ensure!(
        super::is_bytes32_hex(&settlement.mina_transaction_hash),
        "minaTransactionHash must be 32-byte hex"
    );
    anyhow::ensure!(
        settlement.proof.binding.is_some(),
        "settlement proof export has no OCaml account-update binding"
    );
    // Assign L1 batch/action context only when this queued job reaches the
    // worker. Earlier queued settlements may change those values first.
    settlement.proof.context = None;

    let job_id = Uuid::new_v4();
    let input = json!({
        "schemaVersion": settlement.schema_version,
        "minaTransactionHash": settlement.mina_transaction_hash,
        "proof": settlement.proof,
        "submission": {
            "outerAccountPublicKey": settlement.outer_account_public_key,
            "feePayerPublicKey": settlement.fee_payer_public_key,
            "nonce": settlement.nonce,
            "commandBase64": settlement.command_base64
        }
    });
    let input_digest = format!("0x{}", hex::encode(sha2::Sha256::digest(input.to_string())));
    let mut tx = state.pool.begin().await?;
    let actual_job_id = sqlx::query_scalar::<_, Uuid>(
        "INSERT INTO proof_jobs
            (id, kind, input, idempotency_key, input_digest)
         VALUES ($1, 'settlement', $2, $3, $4)
         ON CONFLICT (idempotency_key) WHERE idempotency_key IS NOT NULL
         DO UPDATE SET idempotency_key = EXCLUDED.idempotency_key
         WHERE proof_jobs.input_digest = EXCLUDED.input_digest
         RETURNING id",
    )
    .bind(job_id)
    .bind(&input)
    .bind(&settlement.mina_transaction_hash)
    .bind(input_digest)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| anyhow::anyhow!("transaction hash was reused for different proof input"))?;
    sqlx::query(
        "INSERT INTO gateway_pending_commands
            (job_id, public_key, nonce, command_kind, command_base64)
         VALUES ($1, $2, $3, 'zkapp', $4)
         ON CONFLICT (job_id, command_kind) DO NOTHING",
    )
    .bind(actual_job_id)
    .bind(&settlement.fee_payer_public_key)
    .bind(i64::try_from(settlement.nonce)?)
    .bind(&settlement.command_base64)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;

    Ok(json!({
        "sendZkapp": {
            "zkapp": {
                "id": settlement.command_base64,
                "failureReason": []
            }
        }
    }))
}

async fn config(state: &AppState) -> anyhow::Result<sqlx::postgres::PgRow> {
    Ok(sqlx::query(
        "SELECT genesis_timestamp, fork_slot, account_creation_fee, block_height, state_hash
         FROM gateway_config WHERE id = TRUE",
    )
    .fetch_one(&state.pool)
    .await?)
}

async fn account(state: &AppState, request: &GraphqlRequest) -> anyhow::Result<Value> {
    let public_key = variable_string(request, "pk")?;
    let token_id = request
        .variables
        .get("tokenId")
        .and_then(Value::as_str)
        .unwrap_or("1");
    let token_id = canonical_token_id(token_id);
    let account = sqlx::query_scalar::<_, Value>(
        "SELECT account_json FROM gateway_accounts
         WHERE public_key = $1 AND token_id = $2",
    )
    .bind(public_key)
    .bind(token_id)
    .fetch_optional(&state.pool)
    .await?;
    Ok(json!({"account": account}))
}

async fn pooled_commands(
    state: &AppState,
    request: &GraphqlRequest,
    kind: &str,
    response_field: &str,
) -> anyhow::Result<Value> {
    let public_key = variable_string(request, "pk")?;
    let commands = sqlx::query_scalar::<_, String>(
        "SELECT command_base64 FROM gateway_pending_commands
         WHERE public_key = $1 AND command_kind = $2
         ORDER BY nonce, created_at",
    )
    .bind(public_key)
    .bind(kind)
    .fetch_all(&state.pool)
    .await?
    .into_iter()
    .map(|id| json!({"id": id}))
    .collect::<Vec<_>>();
    let mut response = serde_json::Map::new();
    response.insert(response_field.to_owned(), Value::Array(commands));
    Ok(Value::Object(response))
}

async fn actions(state: &AppState, request: &GraphqlRequest) -> anyhow::Result<Value> {
    let public_key = variable_string(request, "pk")?;
    let from = variable_optional_string(request, "fromActionState");
    let end = variable_optional_string(request, "endActionState");
    let from_block = variable_optional_i64(request, "from");
    let to_block = variable_optional_i64(request, "to");
    let rows = sqlx::query(
        "SELECT actions.sequence, actions.state_before, actions.state_after,
                actions.action_data, actions.ethereum_block_number,
                actions.ethereum_tx_hash, blocks.finalized, blocks.indexed_at
         FROM gateway_actions actions
         JOIN gateway_blocks blocks
           ON blocks.block_number = actions.ethereum_block_number
          AND blocks.block_hash = actions.ethereum_block_hash
         WHERE actions.address = $1 AND NOT actions.removed
           AND blocks.canonical AND blocks.finalized
           AND ($2::text IS NULL OR sequence >= COALESCE((
                SELECT MIN(sequence) FROM gateway_actions
                WHERE address = $1 AND NOT removed AND state_after = $2
           ), (SELECT MIN(sequence) FROM gateway_actions
               WHERE address = $1 AND NOT removed AND state_before = $2)))
           AND ($3::text IS NULL OR sequence <= COALESCE((
                SELECT MIN(sequence) FROM gateway_actions
                WHERE address = $1 AND NOT removed AND state_after = $3
           ), 9223372036854775807))
           AND ($4::bigint IS NULL OR actions.ethereum_block_number >= $4)
           AND ($5::bigint IS NULL OR actions.ethereum_block_number < $5)
         ORDER BY actions.sequence",
    )
    .bind(public_key)
    .bind(from)
    .bind(end)
    .bind(from_block)
    .bind(to_block)
    .fetch_all(&state.pool)
    .await?;
    let tip = ethereum_tip(state).await?;
    let actions = rows
        .into_iter()
        .map(|row| {
            let before: String = row.try_get("state_before")?;
            let after: String = row.try_get("state_after")?;
            let action_data: Value = row.try_get("action_data")?;
            let height: i64 = row.try_get("ethereum_block_number")?;
            let transaction_hash: String = row.try_get("ethereum_tx_hash")?;
            let finalized: bool = row.try_get("finalized")?;
            let indexed_at: chrono::DateTime<chrono::Utc> = row.try_get("indexed_at")?;
            let action_data = action_data
                .as_array()
                .ok_or_else(|| anyhow::anyhow!("gateway action data must be an array"))?
                .iter()
                .map(|data| action_data_value(request, data, &transaction_hash))
                .collect::<anyhow::Result<Vec<_>>>()?;
            Ok(json!({
                "actionState": {
                    "actionStateOne": after,
                    "actionStateTwo": before
                },
                "actionData": action_data,
                "blockInfo": block_info_value(
                    request,
                    indexed_at.timestamp_millis(),
                    height,
                    tip,
                    finalized,
                )
            }))
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    Ok(json!({"actions": actions}))
}

fn action_data_value(
    request: &GraphqlRequest,
    data: &Value,
    transaction_hash: &str,
) -> anyhow::Result<Value> {
    let mut value = serde_json::Map::new();
    if request.query.contains("data") {
        value.insert(
            "data".to_owned(),
            Value::Array(
                data.as_array()
                    .ok_or_else(|| anyhow::anyhow!("gateway action must be an array"))?
                    .clone(),
            ),
        );
    }
    if request.query.contains("transactionInfo") {
        value.insert(
            "transactionInfo".to_owned(),
            json!({"hash": transaction_hash}),
        );
    }
    Ok(Value::Object(value))
}

fn block_info_value(
    request: &GraphqlRequest,
    timestamp_millis: i64,
    height: i64,
    tip: i64,
    finalized: bool,
) -> Value {
    let mut value = serde_json::Map::new();
    if request.query.contains("timestamp") {
        value.insert(
            "timestamp".to_owned(),
            Value::String(timestamp_millis.to_string()),
        );
    }
    if request.query.contains("height") {
        value.insert("height".to_owned(), Value::from(height));
    }
    if request.query.contains("distanceFromMaxBlockHeight") {
        value.insert(
            "distanceFromMaxBlockHeight".to_owned(),
            Value::from(tip.saturating_sub(height)),
        );
    }
    if request.query.contains("chainStatus") {
        value.insert(
            "chainStatus".to_owned(),
            Value::String(if finalized { "canonical" } else { "pending" }.to_owned()),
        );
    }
    Value::Object(value)
}

async fn network_state(state: &AppState) -> anyhow::Result<Value> {
    let pending = ethereum_tip(state).await?;
    let canonical = sqlx::query_scalar::<_, Option<i64>>(
        "SELECT MAX(block_number) FROM gateway_blocks WHERE canonical AND finalized",
    )
    .fetch_one(&state.pool)
    .await?
    .unwrap_or(0);
    Ok(json!({
        "networkState": {
            "maxBlockHeight": {
                "canonicalMaxBlockHeight": canonical,
                "pendingMaxBlockHeight": pending
            }
        }
    }))
}

async fn ethereum_tip(state: &AppState) -> anyhow::Result<i64> {
    Ok(sqlx::query_scalar::<_, Option<i64>>(
        "SELECT MAX(block_number) FROM gateway_blocks WHERE canonical",
    )
    .fetch_one(&state.pool)
    .await?
    .unwrap_or(0))
}

async fn best_chain(state: &AppState, request: &GraphqlRequest) -> anyhow::Result<Value> {
    let max_length = request
        .variables
        .get("maxLength")
        .and_then(Value::as_i64)
        .unwrap_or(1)
        .clamp(1, 100);
    let rows = sqlx::query(
        "SELECT block_number, block_hash FROM gateway_blocks
         WHERE canonical ORDER BY block_number DESC LIMIT $1",
    )
    .bind(max_length)
    .fetch_all(&state.pool)
    .await?;
    let mut blocks = rows
        .into_iter()
        .map(|row| {
            let height: i64 = row.try_get("block_number")?;
            let hash: String = row.try_get("block_hash")?;
            Ok(json!({
                "stateHash": hash,
                "protocolState": {"consensusState": {"blockHeight": height.to_string()}}
            }))
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    if blocks.is_empty() {
        let row = config(state).await?;
        let height: i64 = row.try_get("block_height")?;
        let hash: String = row.try_get("state_hash")?;
        blocks.push(json!({
            "stateHash": hash,
            "protocolState": {"consensusState": {"blockHeight": height.to_string()}}
        }));
    }
    Ok(json!({"bestChain": blocks}))
}

fn variable_string<'a>(request: &'a GraphqlRequest, name: &str) -> anyhow::Result<&'a str> {
    request
        .variables
        .get(name)
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("missing string variable `{name}`"))
}

fn variable_optional_string<'a>(request: &'a GraphqlRequest, name: &str) -> Option<&'a str> {
    request.variables.get(name).and_then(Value::as_str)
}

fn variable_optional_i64(request: &GraphqlRequest, name: &str) -> Option<i64> {
    request.variables.get(name).and_then(Value::as_i64)
}

fn canonical_token_id(token_id: &str) -> &str {
    if token_id == MINA_DEFAULT_TOKEN_ID {
        "1"
    } else {
        token_id
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_mina_default_token_id_for_gateway_storage() {
        assert_eq!(canonical_token_id(MINA_DEFAULT_TOKEN_ID), "1");
        assert_eq!(canonical_token_id("1"), "1");
        assert_eq!(canonical_token_id("custom-token"), "custom-token");
    }

    #[test]
    fn action_response_omits_fields_not_requested_by_ocaml_client() {
        let request = GraphqlRequest {
            query:
                "actions { actionData { data } blockInfo { height distanceFromMaxBlockHeight } }"
                    .to_owned(),
            variables: Value::Null,
        };
        assert_eq!(
            action_data_value(&request, &json!(["1", "2"]), "0x123").unwrap(),
            json!({"data": ["1", "2"]})
        );
        assert_eq!(
            block_info_value(&request, 1_234, 10, 12, true),
            json!({"height": 10, "distanceFromMaxBlockHeight": 2})
        );
    }
}
