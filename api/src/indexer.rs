use alloy::primitives::keccak256;
use anyhow::{Context, Result};
use sqlx::{PgPool, Row};
use std::time::Duration;
use tokio::time::sleep;

use crate::ethereum::{BlockRef, Ethereum};
use serde_json::{json, Value};
use zeko_sp1_lib::{
    Address, BridgeTransitionPublicValuesV2, Bytes32, InnerActionBatchWitnessV2,
    SettlementPublicValues, SettlementPublicValuesV1, SettlementPublicValuesV2,
};

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
        index_bridge_deposits(pool, ethereum, block.number).await?;
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
    sqlx::query(
        "UPDATE gateway_bridge_deposits SET removed = TRUE
         WHERE NOT removed AND ethereum_block_number > $1",
    )
    .bind(ancestor)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "UPDATE gateway_inner_action_leaves SET removed = TRUE
         WHERE NOT removed AND ethereum_block_number > $1",
    )
    .bind(ancestor)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "UPDATE gateway_bridge_deposits deposits
         SET synchronized_settlement_job_id = NULL,
             synchronized_settlement_sequence = NULL
         FROM proof_jobs jobs
         WHERE deposits.synchronized_settlement_job_id = jobs.id
           AND jobs.submitted_block_number > $1",
    )
    .bind(ancestor)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "UPDATE gateway_bridge_deposits deposits
         SET bridge_job_id = NULL, outer_action_sequence = NULL,
             outer_action_state_after = NULL,
             synchronized_settlement_job_id = NULL,
             synchronized_settlement_sequence = NULL
         FROM proof_jobs jobs
         WHERE deposits.bridge_job_id = jobs.id
           AND jobs.submitted_block_number > $1",
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
                 'queued', 'validating', 'awaiting_approval', 'approved',
                 'proof_requested', 'proving',
                 'submitting', 'submitted', 'confirmed'
               )",
        )
        .bind(retry_id)
        .execute(&mut *tx)
        .await?;
    }
    sqlx::query(
        "UPDATE proof_jobs
         SET status = CASE
               WHEN approved_at IS NOT NULL THEN 'approved'::proof_status
               ELSE 'queued'::proof_status
             END,
             transaction_hash = NULL,
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

async fn index_bridge_deposits(
    pool: &PgPool,
    ethereum: &Ethereum,
    block_number: u64,
) -> Result<()> {
    for deposit in ethereum
        .bridge_deposit_logs(block_number, block_number)
        .await?
    {
        sqlx::query(
            "INSERT INTO gateway_bridge_deposits
                (nonce, deposit_leaf, old_deposit_state, new_deposit_state,
                 token, sender, zeko_recipient, ethereum_amount, zeko_amount,
                 timeout, ethereum_block_number, ethereum_block_hash,
                 ethereum_tx_hash, ethereum_log_index, removed)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8::numeric, $9::numeric,
                     $10, $11, $12, $13, $14, FALSE)
             ON CONFLICT (nonce) DO UPDATE SET
                 deposit_leaf = EXCLUDED.deposit_leaf,
                 old_deposit_state = EXCLUDED.old_deposit_state,
                 new_deposit_state = EXCLUDED.new_deposit_state,
                 token = EXCLUDED.token,
                 sender = EXCLUDED.sender,
                 zeko_recipient = EXCLUDED.zeko_recipient,
                 ethereum_amount = EXCLUDED.ethereum_amount,
                 zeko_amount = EXCLUDED.zeko_amount,
                 timeout = EXCLUDED.timeout,
                 ethereum_block_number = EXCLUDED.ethereum_block_number,
                 ethereum_block_hash = EXCLUDED.ethereum_block_hash,
                 ethereum_tx_hash = EXCLUDED.ethereum_tx_hash,
                 ethereum_log_index = EXCLUDED.ethereum_log_index,
                 removed = FALSE",
        )
        .bind(i64::try_from(deposit.nonce)?)
        .bind(deposit.deposit_leaf.to_string())
        .bind(deposit.old_deposit_state.to_string())
        .bind(deposit.new_deposit_state.to_string())
        .bind(deposit.token.to_string())
        .bind(deposit.sender.to_string())
        .bind(deposit.zeko_recipient.to_string())
        .bind(deposit.amount.to_string())
        .bind(deposit.zeko_amount.to_string())
        .bind(i64::try_from(deposit.timeout)?)
        .bind(i64::try_from(deposit.block_number)?)
        .bind(deposit.block_hash.to_string())
        .bind(deposit.transaction_hash.to_string())
        .bind(i64::try_from(deposit.log_index)?)
        .execute(pool)
        .await?;
    }
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
        if confirmed && previous_status != "confirmed" && kind == "bridge" {
            apply_confirmed_bridge(
                pool,
                id,
                public_values
                    .as_deref()
                    .context("bridge public values missing")?,
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
    let bytes = hex::decode(
        public_values_hex
            .strip_prefix("0x")
            .unwrap_or(public_values_hex),
    )?;
    let decoded = SettlementPublicValues::decode(&bytes).map_err(anyhow::Error::msg)?;
    let receipt = decoded.settlement();
    let mut tx = pool.begin().await?;
    sqlx::query(
        "UPDATE gateway_bridge_deposits
         SET synchronized_settlement_job_id = $1,
             synchronized_settlement_sequence = $2
         WHERE NOT removed AND bridge_job_id IS NOT NULL
           AND outer_action_sequence <= $3
           AND synchronized_settlement_job_id IS NULL",
    )
    .bind(job_id)
    .bind(i64::try_from(receipt.batch_sequence)?)
    .bind(i64::from(receipt.synchronized_outer_action_state_length))
    .execute(&mut *tx)
    .await?;
    if sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM gateway_account_history WHERE job_id = $1)",
    )
    .bind(job_id)
    .fetch_one(&mut *tx)
    .await?
    {
        tx.commit().await?;
        return Ok(());
    }
    let Some(submission) = input.get("submission") else {
        tracing::warn!(%job_id, "confirmed direct settlement has no Mina account metadata");
        tx.commit().await?;
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

    sqlx::query(
        "UPDATE gateway_config SET outer_public_key = $1, updated_at = NOW()
         WHERE id = TRUE",
    )
    .bind(outer_public_key)
    .execute(&mut *tx)
    .await?;
    update_outer_account(
        &mut tx,
        job_id,
        outer_public_key,
        receipt,
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
    if let SettlementPublicValues::V2(v2) = &decoded {
        store_inner_action_leaves(
            &mut tx,
            input,
            v2,
            block_number,
            block_hash,
            transaction_hash,
        )
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

async fn apply_confirmed_bridge(
    pool: &PgPool,
    job_id: uuid::Uuid,
    public_values_hex: &str,
    block_number: u64,
    block_hash: &str,
    transaction_hash: &str,
) -> Result<()> {
    let bytes = hex::decode(
        public_values_hex
            .strip_prefix("0x")
            .unwrap_or(public_values_hex),
    )?;
    let decoded = BridgeTransitionPublicValuesV2::decode(&bytes).map_err(anyhow::Error::msg)?;
    anyhow::ensure!(
        !decoded.actions.is_empty(),
        "bridge receipt contains no actions"
    );
    anyhow::ensure!(
        decoded.zeko_action_state_length_after
            == decoded
                .zeko_action_state_length_before
                .checked_add(u32::try_from(decoded.actions.len())?)
                .context("bridge action-state length overflow")?,
        "bridge receipt action-state length mismatch"
    );
    anyhow::ensure!(
        decoded.actions.last().map(|action| action.state_after)
            == Some(decoded.zeko_action_state_after),
        "bridge receipt final action state mismatch"
    );

    let mut tx = pool.begin().await?;
    for (offset, action) in decoded.actions.iter().enumerate() {
        let nonce = decoded
            .ethereum_nonce_before
            .checked_add(u64::try_from(offset)?)
            .and_then(|value| value.checked_add(1))
            .context("bridge deposit nonce overflow")?;
        let sequence = decoded
            .zeko_action_state_length_before
            .checked_add(u32::try_from(offset)?)
            .and_then(|value| value.checked_add(1))
            .context("bridge action sequence overflow")?;
        let updated = sqlx::query(
            "UPDATE gateway_bridge_deposits
             SET bridge_job_id = $1, outer_action_sequence = $2,
                 outer_action_state_after = $3
             WHERE nonce = $4 AND NOT removed
               AND (bridge_job_id IS NULL OR bridge_job_id = $1)",
        )
        .bind(job_id)
        .bind(i64::from(sequence))
        .bind(field_decimal(action.state_after))
        .bind(i64::try_from(nonce)?)
        .execute(&mut *tx)
        .await?;
        anyhow::ensure!(
            updated.rows_affected() == 1,
            "confirmed bridge receipt does not map to canonical deposit nonce {nonce}"
        );
    }
    if sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM gateway_account_history WHERE job_id = $1)",
    )
    .bind(job_id)
    .fetch_one(&mut *tx)
    .await?
    {
        tx.commit().await?;
        return Ok(());
    }
    let outer_public_key = sqlx::query_scalar::<_, Option<String>>(
        "SELECT outer_public_key FROM gateway_config WHERE id = TRUE FOR UPDATE",
    )
    .fetch_one(&mut *tx)
    .await?
    .context("VIRTUAL_MINA_OUTER_PUBLIC_KEY is required before applying a bridge receipt")?;

    update_outer_account_actions(
        &mut tx,
        job_id,
        &outer_public_key,
        decoded.zeko_action_state_before,
        &decoded
            .actions
            .iter()
            .map(|action| action.state_after)
            .collect::<Vec<_>>(),
        block_number,
        block_hash,
    )
    .await?;

    let mut state_before = decoded.zeko_action_state_before;
    for (offset, action) in decoded.actions.iter().enumerate() {
        let sequence = decoded
            .zeko_action_state_length_before
            .checked_add(u32::try_from(offset)?)
            .and_then(|value| value.checked_add(1))
            .context("bridge action sequence overflow")?;
        let action_data = Value::Array(vec![Value::Array(
            action
                .fields
                .iter()
                .copied()
                .map(field_decimal)
                .map(Value::String)
                .collect(),
        )]);
        sqlx::query(
            "INSERT INTO gateway_actions
                (address, sequence, state_before, state_after, action_data,
                 ethereum_block_number, ethereum_block_hash, ethereum_tx_hash,
                 ethereum_log_index, removed)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, FALSE)
             ON CONFLICT (ethereum_tx_hash, ethereum_log_index, sequence)
             DO UPDATE SET
                state_before = EXCLUDED.state_before,
                state_after = EXCLUDED.state_after,
                action_data = EXCLUDED.action_data,
                ethereum_block_number = EXCLUDED.ethereum_block_number,
                ethereum_block_hash = EXCLUDED.ethereum_block_hash,
                removed = FALSE",
        )
        .bind(&outer_public_key)
        .bind(i64::from(sequence))
        .bind(field_decimal(state_before))
        .bind(field_decimal(action.state_after))
        .bind(action_data)
        .bind(i64::try_from(block_number)?)
        .bind(block_hash)
        .bind(transaction_hash)
        .bind(i64::try_from(offset)?)
        .execute(&mut *tx)
        .await?;
        state_before = action.state_after;
    }
    tx.commit().await?;
    Ok(())
}

async fn store_inner_action_leaves(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    input: &Value,
    receipt: &SettlementPublicValuesV2,
    block_number: u64,
    block_hash: &str,
    transaction_hash: &str,
) -> Result<()> {
    let batch: InnerActionBatchWitnessV2 = serde_json::from_value(
        input
            .pointer("/proof/innerActionBatch")
            .cloned()
            .context("V2 settlement is missing innerActionBatch")?,
    )?;
    anyhow::ensure!(
        batch.bridge_address == receipt.bridge_address,
        "inner-action bridge address does not match receipt"
    );
    anyhow::ensure!(
        batch.actions.len() == receipt.inner_action_count as usize,
        "inner-action count does not match receipt"
    );

    let mut rows = Vec::with_capacity(batch.actions.len());
    let mut leaves = Vec::with_capacity(batch.actions.len());
    for (offset, action) in batch.actions.iter().enumerate() {
        let action_fields_hash = hash_action_fields(&action.fields);
        let global_index = receipt
            .inner_action_start_index
            .checked_add(u32::try_from(offset)?)
            .context("inner action index overflow")?;
        let leaf = match &action.withdrawal {
            Some(withdrawal) => hash_native_withdrawal_leaf(
                receipt.settlement.chain_id,
                batch.bridge_address,
                global_index,
                withdrawal.recipient,
                withdrawal.amount,
                action_fields_hash,
            ),
            None => hash_raw_inner_action_leaf(
                receipt.settlement.chain_id,
                batch.bridge_address,
                global_index,
                action_fields_hash,
            ),
        };
        leaves.push(leaf);
        rows.push((offset, global_index, action, action_fields_hash, leaf));
    }
    anyhow::ensure!(
        inner_action_root(&leaves) == receipt.inner_action_root,
        "stored inner actions do not reproduce settlement root"
    );

    for (offset, global_index, action, action_fields_hash, leaf) in rows {
        let (recipient, amount) = match &action.withdrawal {
            Some(withdrawal) => (
                Some(format!("0x{}", hex::encode(withdrawal.recipient))),
                Some(withdrawal.amount.to_string()),
            ),
            None => (None, None),
        };
        sqlx::query(
            "INSERT INTO gateway_inner_action_leaves
                (settlement_sequence, action_offset, global_action_index,
                 action_fields, action_fields_hash, leaf, recipient,
                 zeko_amount, inner_action_root, commit_slot_upper,
                 ethereum_block_number, ethereum_block_hash, ethereum_tx_hash,
                 removed)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8::numeric, $9, $10, $11, $12,
                     $13, FALSE)
             ON CONFLICT (settlement_sequence, action_offset) DO UPDATE SET
                 global_action_index = EXCLUDED.global_action_index,
                 action_fields = EXCLUDED.action_fields,
                 action_fields_hash = EXCLUDED.action_fields_hash,
                 leaf = EXCLUDED.leaf,
                 recipient = EXCLUDED.recipient,
                 zeko_amount = EXCLUDED.zeko_amount,
                 inner_action_root = EXCLUDED.inner_action_root,
                 commit_slot_upper = EXCLUDED.commit_slot_upper,
                 ethereum_block_number = EXCLUDED.ethereum_block_number,
                 ethereum_block_hash = EXCLUDED.ethereum_block_hash,
                 ethereum_tx_hash = EXCLUDED.ethereum_tx_hash,
                 removed = FALSE",
        )
        .bind(i64::try_from(receipt.settlement.batch_sequence)?)
        .bind(i32::try_from(offset)?)
        .bind(i64::from(global_index))
        .bind(serde_json::to_value(&action.fields)?)
        .bind(format!("0x{}", hex::encode(action_fields_hash)))
        .bind(format!("0x{}", hex::encode(leaf)))
        .bind(recipient)
        .bind(amount)
        .bind(format!("0x{}", hex::encode(receipt.inner_action_root)))
        .bind(i64::from(receipt.settlement.slot_upper))
        .bind(i64::try_from(block_number)?)
        .bind(block_hash)
        .bind(transaction_hash)
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

fn hash_action_fields(fields: &[Bytes32]) -> Bytes32 {
    let mut encoded = Vec::with_capacity(64 + fields.len() * 32);
    encoded.extend_from_slice(&keccak256("ZEKO_INNER_ACTION_FIELDS_V2").0);
    encoded.extend_from_slice(&u32_word(fields.len() as u32));
    for field in fields {
        encoded.extend_from_slice(field);
    }
    keccak256(encoded).0
}

fn hash_native_withdrawal_leaf(
    chain_id: u64,
    bridge: Address,
    global_index: u32,
    recipient: Address,
    amount: u64,
    action_fields_hash: Bytes32,
) -> Bytes32 {
    let mut encoded = Vec::with_capacity(224);
    encoded.extend_from_slice(&keccak256("ZEKO_NATIVE_WITHDRAWAL_LEAF_V2").0);
    encoded.extend_from_slice(&u64_word(chain_id));
    encoded.extend_from_slice(&address_word(bridge));
    encoded.extend_from_slice(&u32_word(global_index));
    encoded.extend_from_slice(&address_word(recipient));
    encoded.extend_from_slice(&u64_word(amount));
    encoded.extend_from_slice(&action_fields_hash);
    keccak256(encoded).0
}

fn hash_raw_inner_action_leaf(
    chain_id: u64,
    bridge: Address,
    global_index: u32,
    action_fields_hash: Bytes32,
) -> Bytes32 {
    let mut encoded = Vec::with_capacity(160);
    encoded.extend_from_slice(&keccak256("ZEKO_RAW_INNER_ACTION_LEAF_V2").0);
    encoded.extend_from_slice(&u64_word(chain_id));
    encoded.extend_from_slice(&address_word(bridge));
    encoded.extend_from_slice(&u32_word(global_index));
    encoded.extend_from_slice(&action_fields_hash);
    keccak256(encoded).0
}

fn inner_action_root(leaves: &[Bytes32]) -> Bytes32 {
    let zero_hashes = inner_action_zero_hashes();
    if leaves.is_empty() {
        return zero_hashes[16];
    }
    let mut nodes = leaves.to_vec();
    for level in 0..16 {
        nodes = nodes
            .chunks(2)
            .map(|pair| {
                hash_inner_action_node(pair[0], pair.get(1).copied().unwrap_or(zero_hashes[level]))
            })
            .collect();
    }
    nodes[0]
}

fn inner_action_zero_hashes() -> [Bytes32; 17] {
    let mut hashes = [[0u8; 32]; 17];
    for level in 0..16 {
        hashes[level + 1] = hash_inner_action_node(hashes[level], hashes[level]);
    }
    hashes
}

fn hash_inner_action_node(left: Bytes32, right: Bytes32) -> Bytes32 {
    let mut encoded = Vec::with_capacity(96);
    encoded.extend_from_slice(&keccak256("ZEKO_INNER_ACTION_NODE_V2").0);
    encoded.extend_from_slice(&left);
    encoded.extend_from_slice(&right);
    keccak256(encoded).0
}

fn u64_word(value: u64) -> Bytes32 {
    let mut word = [0u8; 32];
    word[24..].copy_from_slice(&value.to_be_bytes());
    word
}

fn u32_word(value: u32) -> Bytes32 {
    let mut word = [0u8; 32];
    word[28..].copy_from_slice(&value.to_be_bytes());
    word
}

fn address_word(value: Address) -> Bytes32 {
    let mut word = [0u8; 32];
    word[12..].copy_from_slice(&value);
    word
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

async fn update_outer_account_actions(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    job_id: uuid::Uuid,
    public_key: &str,
    expected_state_before: Bytes32,
    states_after: &[Bytes32],
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
    let old_actions = object
        .get("actionState")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| vec![Value::String("0".to_owned()); 5]);
    let expected_state_before = field_decimal(expected_state_before);
    anyhow::ensure!(
        old_actions.first().and_then(Value::as_str) == Some(expected_state_before.as_str()),
        "virtual Mina outer account action state is stale"
    );
    let mut action_state = states_after
        .iter()
        .rev()
        .copied()
        .map(field_decimal)
        .map(Value::String)
        .collect::<Vec<_>>();
    action_state.extend(old_actions);
    action_state.truncate(5);
    object.insert("actionState".to_owned(), Value::Array(action_state));
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
