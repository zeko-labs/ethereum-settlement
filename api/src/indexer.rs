use anyhow::{Context, Result};
use sqlx::{PgPool, Row};
use std::time::Duration;
use tokio::time::sleep;

use crate::ethereum::{BlockRef, Ethereum};
use serde_json::{json, Value};
use zeko_sp1_lib::SettlementPublicValuesV1;

#[derive(Clone, Debug)]
pub struct Config {
    pub start_block: Option<u64>,
    pub confirmations: u64,
    pub poll_interval: Duration,
}

pub async fn run(pool: PgPool, ethereum: Ethereum, config: Config) {
    loop {
        if let Err(error) = tick(&pool, &ethereum, &config).await {
            tracing::error!(%error, "Ethereum indexer tick failed");
        }
        sleep(config.poll_interval).await;
    }
}

async fn tick(pool: &PgPool, ethereum: &Ethereum, config: &Config) -> Result<()> {
    let head = ethereum
        .block_number()
        .await
        .context("read Ethereum head")?;
    index_blocks(pool, ethereum, config, head).await?;
    reconcile_jobs(pool, ethereum, config.confirmations, head).await?;
    Ok(())
}

async fn index_blocks(
    pool: &PgPool,
    ethereum: &Ethereum,
    config: &Config,
    head: u64,
) -> Result<()> {
    let latest = sqlx::query(
        "SELECT block_number, block_hash FROM gateway_blocks
         WHERE canonical ORDER BY block_number DESC LIMIT 1",
    )
    .fetch_optional(pool)
    .await?;

    let mut next = match latest {
        Some(row) => {
            let number: i64 = row.try_get("block_number")?;
            u64::try_from(number).context("negative indexed Ethereum block")? + 1
        }
        None => config.start_block.unwrap_or(head).min(head),
    };

    while next <= head {
        let block = ethereum.block(next).await?;
        ensure_parent(pool, ethereum, &block).await?;
        insert_block(pool, &block).await?;
        next = block.number + 1;
    }

    let finalized_through = head.saturating_sub(config.confirmations);
    sqlx::query(
        "UPDATE gateway_blocks
         SET finalized = canonical AND block_number <= $1",
    )
    .bind(i64::try_from(finalized_through)?)
    .execute(pool)
    .await?;

    let head_block = ethereum.block(head).await?;
    sqlx::query(
        "UPDATE gateway_config
         SET block_height = $1, state_hash = $2, updated_at = NOW()
         WHERE id = TRUE",
    )
    .bind(i64::try_from(head)?)
    .bind(head_block.hash.to_string())
    .execute(pool)
    .await?;
    Ok(())
}

async fn ensure_parent(pool: &PgPool, ethereum: &Ethereum, block: &BlockRef) -> Result<()> {
    if block.number == 0 {
        return Ok(());
    }
    let local_parent = sqlx::query_scalar::<_, String>(
        "SELECT block_hash FROM gateway_blocks
         WHERE block_number = $1 AND canonical",
    )
    .bind(i64::try_from(block.number - 1)?)
    .fetch_optional(pool)
    .await?;
    if local_parent.as_deref() == Some(block.parent_hash.to_string().as_str()) {
        return Ok(());
    }
    if local_parent.is_none() {
        return Ok(());
    }

    let mut candidate = block.number - 1;
    let ancestor = loop {
        let remote = ethereum.block(candidate).await?;
        let local = sqlx::query_scalar::<_, String>(
            "SELECT block_hash FROM gateway_blocks
             WHERE block_number = $1 AND canonical",
        )
        .bind(i64::try_from(candidate)?)
        .fetch_optional(pool)
        .await?;
        if local.as_deref() == Some(remote.hash.to_string().as_str()) {
            break candidate;
        }
        if candidate == 0 {
            break 0;
        }
        candidate -= 1;
    };
    rollback_after(pool, ancestor).await?;
    anyhow::ensure!(
        ancestor + 1 == block.number,
        "Ethereum reorg rolled back to {ancestor}; caller must refetch from the new tip"
    );
    Ok(())
}

async fn rollback_after(pool: &PgPool, ancestor: u64) -> Result<()> {
    let ancestor = i64::try_from(ancestor)?;
    let mut tx = pool.begin().await?;
    sqlx::query("SELECT id FROM gateway_config WHERE id = TRUE FOR UPDATE")
        .fetch_one(&mut *tx)
        .await?;
    // A deep reorg can orphan several already-confirmed settlements while a
    // newer settlement is still proving. Only the earliest orphaned receipt
    // can be replayed against the rolled-back contract state. Preserve its
    // paid proof request and invalidate dependent/later work; the sequencer
    // will export those commits again after the first receipt is canonical.
    let retry_settlement = sqlx::query_scalar::<_, uuid::Uuid>(
        "SELECT id FROM proof_jobs
         WHERE kind = 'settlement'
           AND submitted_block_number > $1
           AND status IN ('submitted', 'confirmed')
         ORDER BY submitted_block_number, created_at
         LIMIT 1",
    )
    .bind(ancestor)
    .fetch_optional(&mut *tx)
    .await?;
    let histories = sqlx::query(
        "SELECT job_id, public_key, token_id, account_before
         FROM gateway_account_history
         WHERE ethereum_block_number > $1
         ORDER BY ethereum_block_number DESC, created_at DESC",
    )
    .bind(ancestor)
    .fetch_all(&mut *tx)
    .await?;
    for history in histories {
        let public_key: String = history.try_get("public_key")?;
        let token_id: String = history.try_get("token_id")?;
        let account_before: Value = history.try_get("account_before")?;
        sqlx::query(
            "UPDATE gateway_accounts
             SET account_json = $3, ethereum_block_number = NULL,
                 ethereum_block_hash = NULL, updated_at = NOW()
             WHERE public_key = $1 AND token_id = $2",
        )
        .bind(public_key)
        .bind(token_id)
        .bind(account_before)
        .execute(&mut *tx)
        .await?;
    }
    sqlx::query("DELETE FROM gateway_account_history WHERE ethereum_block_number > $1")
        .bind(ancestor)
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "UPDATE gateway_blocks SET canonical = FALSE, finalized = FALSE
         WHERE canonical AND block_number > $1",
    )
    .bind(ancestor)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "UPDATE gateway_actions SET removed = TRUE
         WHERE NOT removed AND ethereum_block_number > $1",
    )
    .bind(ancestor)
    .execute(&mut *tx)
    .await?;
    if let Some(retry_id) = retry_settlement {
        sqlx::query(
            "UPDATE proof_jobs
             SET status = 'reorged', completed_at = NOW(),
                 error = 'Settlement depends on state removed by an Ethereum reorganization',
                 updated_at = NOW()
             WHERE kind = 'settlement' AND id <> $1
               AND status IN (
                 'queued', 'validating', 'proof_requested', 'proving',
                 'submitting', 'submitted', 'confirmed'
               )",
        )
        .bind(retry_id)
        .execute(&mut *tx)
        .await?;
    }
    sqlx::query(
        "UPDATE proof_jobs
         SET status = 'queued', transaction_hash = NULL,
             submitted_block_number = NULL, submitted_block_hash = NULL,
             confirmations = 0, completed_at = NULL,
             error = 'Ethereum submission was removed by a chain reorganization',
             updated_at = NOW()
         WHERE submitted_block_number > $1
           AND status IN ('submitted', 'confirmed')
           AND (kind <> 'settlement' OR id = $2)",
    )
    .bind(ancestor)
    .bind(retry_settlement)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "DELETE FROM gateway_pending_commands
         WHERE job_id IN (SELECT id FROM proof_jobs WHERE status = 'reorged')",
    )
    .execute(&mut *tx)
    .await?;
    if let Some(retry_id) = retry_settlement {
        sqlx::query(
            "INSERT INTO gateway_pending_commands
                (job_id, public_key, nonce, command_kind, command_base64)
             SELECT id,
                    input #>> '{submission,feePayerPublicKey}',
                    (input #>> '{submission,nonce}')::bigint,
                    'zkapp',
                    input #>> '{submission,commandBase64}'
             FROM proof_jobs
             WHERE id = $1 AND input ? 'submission'
             ON CONFLICT (job_id, command_kind) DO NOTHING",
        )
        .bind(retry_id)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    tracing::warn!(ancestor, "rolled back orphaned Ethereum gateway state");
    Ok(())
}

async fn insert_block(pool: &PgPool, block: &BlockRef) -> Result<()> {
    sqlx::query(
        "INSERT INTO gateway_blocks
            (block_number, block_hash, parent_hash, canonical, finalized)
         VALUES ($1, $2, $3, TRUE, FALSE)
         ON CONFLICT (block_number) DO UPDATE SET
            block_hash = EXCLUDED.block_hash,
            parent_hash = EXCLUDED.parent_hash,
            canonical = TRUE,
            finalized = FALSE,
            indexed_at = NOW()",
    )
    .bind(i64::try_from(block.number)?)
    .bind(block.hash.to_string())
    .bind(block.parent_hash.to_string())
    .execute(pool)
    .await?;
    Ok(())
}

async fn reconcile_jobs(
    pool: &PgPool,
    ethereum: &Ethereum,
    required_confirmations: u64,
    head: u64,
) -> Result<()> {
    let rows = sqlx::query(
        "SELECT id, kind::text AS kind, input, public_values, status::text AS status,
                transaction_hash FROM proof_jobs
         WHERE transaction_hash IS NOT NULL
           AND status IN ('submitted', 'confirmed')",
    )
    .fetch_all(pool)
    .await?;
    for row in rows {
        let id: uuid::Uuid = row.try_get("id")?;
        let kind: String = row.try_get("kind")?;
        let input: Value = row.try_get("input")?;
        let public_values: Option<String> = row.try_get("public_values")?;
        let previous_status: String = row.try_get("status")?;
        let transaction_hash: String = row.try_get("transaction_hash")?;
        let Some(receipt) = ethereum.transaction_receipt(&transaction_hash).await? else {
            continue;
        };
        let confirmations = head.saturating_sub(receipt.block_number) + 1;
        if !receipt.succeeded {
            sqlx::query(
                "UPDATE proof_jobs SET status = 'ethereum_reverted',
                        ethereum_gas_used = $2, confirmations = $3,
                        error = 'Ethereum transaction reverted',
                        completed_at = NOW(), updated_at = NOW()
                 WHERE id = $1",
            )
            .bind(id)
            .bind(i64::try_from(receipt.gas_used)?)
            .bind(i32::try_from(confirmations.min(i32::MAX as u64))?)
            .execute(pool)
            .await?;
            sqlx::query("DELETE FROM gateway_pending_commands WHERE job_id = $1")
                .bind(id)
                .execute(pool)
                .await?;
            continue;
        }
        let canonical_hash = sqlx::query_scalar::<_, String>(
            "SELECT block_hash FROM gateway_blocks
             WHERE block_number = $1 AND canonical",
        )
        .bind(i64::try_from(receipt.block_number)?)
        .fetch_optional(pool)
        .await?;
        if canonical_hash.as_deref() != Some(receipt.block_hash.to_string().as_str()) {
            continue;
        }
        let confirmed = confirmations >= required_confirmations.max(1);
        if confirmed && previous_status != "confirmed" && kind == "settlement" {
            apply_confirmed_settlement(
                pool,
                id,
                &input,
                public_values
                    .as_deref()
                    .context("settlement public values missing")?,
                receipt.block_number,
                &receipt.block_hash.to_string(),
                &transaction_hash,
            )
            .await?;
        }
        sqlx::query(
            "UPDATE proof_jobs SET status = $2::proof_status,
                    submitted_block_number = $3, submitted_block_hash = $4,
                    ethereum_gas_used = $5, confirmations = $6,
                    completed_at = CASE WHEN $7 THEN NOW() ELSE NULL END,
                    updated_at = NOW()
             WHERE id = $1",
        )
        .bind(id)
        .bind(if confirmed { "confirmed" } else { "submitted" })
        .bind(i64::try_from(receipt.block_number)?)
        .bind(receipt.block_hash.to_string())
        .bind(i64::try_from(receipt.gas_used)?)
        .bind(i32::try_from(confirmations.min(i32::MAX as u64))?)
        .bind(confirmed)
        .execute(pool)
        .await?;
    }
    Ok(())
}

async fn apply_confirmed_settlement(
    pool: &PgPool,
    job_id: uuid::Uuid,
    input: &Value,
    public_values_hex: &str,
    block_number: u64,
    block_hash: &str,
    transaction_hash: &str,
) -> Result<()> {
    if sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM gateway_account_history WHERE job_id = $1)",
    )
    .bind(job_id)
    .fetch_one(pool)
    .await?
    {
        return Ok(());
    }
    let bytes = hex::decode(
        public_values_hex
            .strip_prefix("0x")
            .unwrap_or(public_values_hex),
    )?;
    let receipt = SettlementPublicValuesV1::decode(&bytes).map_err(anyhow::Error::msg)?;
    let Some(submission) = input.get("submission") else {
        tracing::warn!(%job_id, "confirmed direct settlement has no Mina account metadata");
        return Ok(());
    };
    let outer_public_key = submission
        .get("outerAccountPublicKey")
        .and_then(Value::as_str)
        .context("outerAccountPublicKey missing")?;
    let fee_payer_public_key = submission
        .get("feePayerPublicKey")
        .and_then(Value::as_str)
        .context("feePayerPublicKey missing")?;
    let fee_payer_nonce = submission
        .get("nonce")
        .and_then(Value::as_u64)
        .context("fee payer nonce missing")?;
    let actions = input
        .pointer("/proof/binding/actions")
        .cloned()
        .context("settlement actions missing")?;

    let mut tx = pool.begin().await?;
    update_outer_account(
        &mut tx,
        job_id,
        outer_public_key,
        &receipt,
        block_number,
        block_hash,
    )
    .await?;
    update_fee_payer(
        &mut tx,
        job_id,
        fee_payer_public_key,
        fee_payer_nonce,
        block_number,
        block_hash,
    )
    .await?;
    sqlx::query(
        "INSERT INTO gateway_actions
            (address, sequence, state_before, state_after, action_data,
             ethereum_block_number, ethereum_block_hash, ethereum_tx_hash,
             ethereum_log_index)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, 0)
         ON CONFLICT (ethereum_tx_hash, ethereum_log_index, sequence)
         DO UPDATE SET removed = FALSE",
    )
    .bind(outer_public_key)
    .bind(i64::from(receipt.outer_action_state_length_after))
    .bind(field_decimal(receipt.outer_action_state_before))
    .bind(field_decimal(receipt.outer_action_state_after))
    .bind(fields_to_decimal(actions)?)
    .bind(i64::try_from(block_number)?)
    .bind(block_hash)
    .bind(transaction_hash)
    .execute(&mut *tx)
    .await?;
    sqlx::query("DELETE FROM gateway_pending_commands WHERE job_id = $1")
        .bind(job_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

async fn update_outer_account(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    job_id: uuid::Uuid,
    public_key: &str,
    receipt: &SettlementPublicValuesV1,
    block_number: u64,
    block_hash: &str,
) -> Result<()> {
    let Some(mut account) = sqlx::query_scalar::<_, Value>(
        "SELECT account_json FROM gateway_accounts
         WHERE public_key = $1 AND token_id = '1' FOR UPDATE",
    )
    .bind(public_key)
    .fetch_optional(&mut **tx)
    .await?
    else {
        anyhow::bail!("outer virtual Mina account {public_key} is not configured");
    };
    snapshot_account(tx, job_id, public_key, &account, block_number, block_hash).await?;
    let object = account
        .as_object_mut()
        .context("virtual Mina account must be a JSON object")?;
    object.insert(
        "zkappState".to_owned(),
        Value::Array(
            receipt
                .state_after
                .fields
                .iter()
                .copied()
                .map(field_decimal)
                .map(Value::String)
                .collect(),
        ),
    );
    let old_actions = object
        .get("actionState")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| vec![Value::String("0".to_owned()); 5]);
    let mut action_state = vec![
        Value::String(field_decimal(receipt.outer_action_state_after)),
        Value::String(field_decimal(receipt.outer_action_state_before)),
    ];
    action_state.extend(old_actions.into_iter().take(3));
    object.insert("actionState".to_owned(), Value::Array(action_state));
    object.insert("provedState".to_owned(), Value::Bool(true));
    store_account(tx, public_key, account, block_number, block_hash).await
}

async fn update_fee_payer(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    job_id: uuid::Uuid,
    public_key: &str,
    nonce: u64,
    block_number: u64,
    block_hash: &str,
) -> Result<()> {
    let Some(mut account) = sqlx::query_scalar::<_, Value>(
        "SELECT account_json FROM gateway_accounts
         WHERE public_key = $1 AND token_id = '1' FOR UPDATE",
    )
    .bind(public_key)
    .fetch_optional(&mut **tx)
    .await?
    else {
        anyhow::bail!("fee-payer virtual Mina account {public_key} is not configured");
    };
    snapshot_account(tx, job_id, public_key, &account, block_number, block_hash).await?;
    account
        .as_object_mut()
        .context("virtual Mina account must be a JSON object")?
        .insert(
            "nonce".to_owned(),
            json!(nonce
                .checked_add(1)
                .context("fee-payer nonce overflow")?
                .to_string()),
        );
    store_account(tx, public_key, account, block_number, block_hash).await
}

async fn snapshot_account(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    job_id: uuid::Uuid,
    public_key: &str,
    account: &Value,
    block_number: u64,
    block_hash: &str,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO gateway_account_history
            (job_id, public_key, token_id, account_before,
             ethereum_block_number, ethereum_block_hash)
         VALUES ($1, $2, '1', $3, $4, $5)
         ON CONFLICT (job_id, public_key, token_id) DO NOTHING",
    )
    .bind(job_id)
    .bind(public_key)
    .bind(account)
    .bind(i64::try_from(block_number)?)
    .bind(block_hash)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn store_account(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    public_key: &str,
    account: Value,
    block_number: u64,
    block_hash: &str,
) -> Result<()> {
    sqlx::query(
        "UPDATE gateway_accounts SET account_json = $2,
                ethereum_block_number = $3, ethereum_block_hash = $4,
                updated_at = NOW()
         WHERE public_key = $1 AND token_id = '1'",
    )
    .bind(public_key)
    .bind(account)
    .bind(i64::try_from(block_number)?)
    .bind(block_hash)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

fn field_decimal(bytes: [u8; 32]) -> String {
    alloy::primitives::U256::from_be_bytes(bytes).to_string()
}

fn fields_to_decimal(value: Value) -> Result<Value> {
    let events = value
        .as_array()
        .context("settlement actions must be an array")?;
    Ok(Value::Array(
        events
            .iter()
            .map(|event| {
                event
                    .as_array()
                    .context("settlement action must be an array")?
                    .iter()
                    .map(|field| {
                        let encoded = field.as_str().context("action field must be hex")?;
                        let bytes: [u8; 32] =
                            hex::decode(encoded.strip_prefix("0x").unwrap_or(encoded))?
                                .try_into()
                                .map_err(|_| anyhow::anyhow!("action field must be 32 bytes"))?;
                        Ok(Value::String(field_decimal(bytes)))
                    })
                    .collect::<Result<Vec<_>>>()
                    .map(Value::Array)
            })
            .collect::<Result<Vec<_>>>()?,
    ))
}

#[cfg(test)]
mod tests {
    #[test]
    fn confirmation_count_includes_submission_block() {
        assert_eq!(12_u64.saturating_sub(12) + 1, 1);
        assert_eq!(15_u64.saturating_sub(12) + 1, 4);
    }
}
