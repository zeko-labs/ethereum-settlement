use anyhow::{Context, Result};
use sqlx::{PgPool, Row};
use std::time::Duration;
use tokio::time::sleep;

use crate::ethereum::{BlockRef, Ethereum};
use serde_json::{json, Value};
use zeko_sp1_lib::inner_action_commitment::{
    action_fields_hash as hash_action_fields, erc20_withdrawal_leaf as hash_erc20_withdrawal_leaf,
    native_withdrawal_leaf as hash_native_withdrawal_leaf,
    raw_inner_action_leaf as hash_raw_inner_action_leaf, root as inner_action_root,
};
use zeko_sp1_lib::{
    BridgeTransitionPublicValuesV2, Bytes32, InnerActionBatchWitnessV2, SettlementPublicValues,
    SettlementPublicValuesV1, SettlementPublicValuesV2,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FinalityMode {
    Finalized,
    Confirmations,
}

impl FinalityMode {
    pub fn parse(value: &str) -> Result<Self> {
        match value {
            "finalized" => Ok(Self::Finalized),
            "confirmations" => Ok(Self::Confirmations),
            _ => anyhow::bail!(
                "ETHEREUM_FINALITY_MODE must be `finalized` or `confirmations`, got {value}"
            ),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Finalized => "finalized",
            Self::Confirmations => "confirmations",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Config {
    pub start_block: Option<u64>,
    pub finality_mode: FinalityMode,
    pub confirmations: u64,
    pub poll_interval: Duration,
    pub fee_payer_public_key: Option<String>,
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
    let finalized_block = match config.finality_mode {
        FinalityMode::Finalized => Some(
            ethereum
                .finalized_block()
                .await
                .context("read Ethereum consensus-finalized head")?,
        ),
        FinalityMode::Confirmations => None,
    };
    index_blocks(pool, ethereum, config, head, finalized_block.as_ref()).await?;
    index_explorer_events_through(pool, ethereum, config, head).await?;
    recover_gateway_state(pool, ethereum, config).await?;
    reconcile_jobs(pool, ethereum, config, head, finalized_block.as_ref()).await?;
    sqlx::query(
        "UPDATE gateway_config SET recovery_ready = TRUE, updated_at = NOW()
         WHERE id = TRUE",
    )
    .execute(pool)
    .await?;
    Ok(())
}

async fn index_blocks(
    pool: &PgPool,
    ethereum: &Ethereum,
    config: &Config,
    head: u64,
    finalized_block: Option<&BlockRef>,
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
        ensure_parent(pool, ethereum, config.finality_mode, &block).await?;
        insert_block(pool, &block).await?;
        index_bridge_deposits(pool, ethereum, block.number).await?;
        next = block.number + 1;
    }

    let finalized_through = match finalized_block {
        Some(block) => {
            anyhow::ensure!(
                block.number <= head,
                "Ethereum finalized head {} is above latest head {head}",
                block.number
            );
            let indexed_hash = sqlx::query_scalar::<_, String>(
                "SELECT block_hash FROM gateway_blocks
                 WHERE block_number = $1 AND canonical",
            )
            .bind(i64::try_from(block.number)?)
            .fetch_optional(pool)
            .await?;
            if let Some(indexed_hash) = indexed_hash {
                anyhow::ensure!(
                    indexed_hash == block.hash.to_string(),
                    "Ethereum finalized head {} does not match the indexed canonical hash",
                    block.number
                );
            }
            let previous_finalized = sqlx::query_scalar::<_, Option<i64>>(
                "SELECT MAX(block_number) FROM gateway_blocks
                 WHERE canonical AND finalized",
            )
            .fetch_one(pool)
            .await?
            .map(u64::try_from)
            .transpose()
            .context("negative finalized Ethereum block")?;
            anyhow::ensure!(
                previous_finalized.is_none_or(|height| block.number >= height),
                "Ethereum finalized head regressed from {} to {}",
                previous_finalized.unwrap_or_default(),
                block.number
            );
            block.number
        }
        None => head.saturating_sub(config.confirmations),
    };
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

async fn ensure_parent(
    pool: &PgPool,
    ethereum: &Ethereum,
    finality_mode: FinalityMode,
    block: &BlockRef,
) -> Result<()> {
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
    if finality_mode == FinalityMode::Finalized {
        let would_reorg_finalized = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(
                SELECT 1 FROM gateway_blocks
                WHERE canonical AND finalized AND block_number > $1
             )",
        )
        .bind(i64::try_from(ancestor)?)
        .fetch_one(pool)
        .await?;
        anyhow::ensure!(
            !would_reorg_finalized,
            "Ethereum canonical chain conflicts with a consensus-finalized checkpoint above block {ancestor}"
        );
    }
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
    sqlx::query(
        "UPDATE gateway_config SET recovery_ready = FALSE, updated_at = NOW()
         WHERE id = TRUE",
    )
    .execute(&mut *tx)
    .await?;
    // A deep reorg can orphan several already-confirmed settlements while a
    // newer settlement is still proving. Only the earliest orphaned receipt
    // can be replayed against the rolled-back contract state. Preserve its
    // paid proof request and invalidate dependent/later work; the sequencer
    // will export those commits again after the first receipt is canonical.
    let retry_settlement = sqlx::query_scalar::<_, uuid::Uuid>(
        "SELECT id FROM proof_jobs
         WHERE kind = 'settlement'
           AND input->>'recoveredFromEthereum' IS DISTINCT FROM 'true'
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
        "UPDATE gateway_explorer_settlements SET removed = TRUE
         WHERE NOT removed AND ethereum_block_number > $1",
    )
    .bind(ancestor)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "UPDATE gateway_explorer_bridge_transitions SET removed = TRUE
         WHERE NOT removed AND ethereum_block_number > $1",
    )
    .bind(ancestor)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "UPDATE gateway_native_withdrawal_claims SET removed = TRUE
         WHERE NOT removed AND ethereum_block_number > $1",
    )
    .bind(ancestor)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "UPDATE gateway_explorer_index_state
         SET last_block = LEAST(last_block, $1)",
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
    sqlx::query(
        "DELETE FROM proof_jobs
         WHERE input->>'recoveredFromEthereum' = 'true'
           AND submitted_block_number > $1",
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
        let asset_id = deposit.asset_id.map(|value| value.to_string());
        let action_encoding_version = i32::try_from(deposit.action_encoding_version)?;
        let registry_index = deposit.registry_index.map(i64::from);
        let record_commitment = deposit.record_commitment.map(|value| value.to_string());
        sqlx::query(
            "INSERT INTO gateway_bridge_deposits
                (nonce, deposit_leaf, old_deposit_state, new_deposit_state,
                 token, asset_id, action_encoding_version, registry_index,
                 record_commitment, sender, zeko_recipient, ethereum_amount,
                 zeko_amount, timeout, ethereum_block_number,
                 ethereum_block_hash, ethereum_tx_hash, ethereum_log_index,
                 removed)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12::numeric,
                     $13::numeric, $14, $15, $16, $17, $18, FALSE)
             ON CONFLICT (nonce) DO UPDATE SET
                 deposit_leaf = EXCLUDED.deposit_leaf,
                 old_deposit_state = EXCLUDED.old_deposit_state,
                 new_deposit_state = EXCLUDED.new_deposit_state,
                 token = EXCLUDED.token,
                 asset_id = EXCLUDED.asset_id,
                 action_encoding_version = EXCLUDED.action_encoding_version,
                 registry_index = EXCLUDED.registry_index,
                 record_commitment = EXCLUDED.record_commitment,
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
        .bind(asset_id)
        .bind(action_encoding_version)
        .bind(registry_index)
        .bind(record_commitment)
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

async fn index_explorer_events_through(
    pool: &PgPool,
    ethereum: &Ethereum,
    config: &Config,
    head: u64,
) -> Result<()> {
    let last = sqlx::query_scalar::<_, i64>(
        "SELECT last_block FROM gateway_explorer_index_state WHERE id = TRUE",
    )
    .fetch_optional(pool)
    .await?;
    let first_indexed = sqlx::query_scalar::<_, Option<i64>>(
        "SELECT MIN(block_number) FROM gateway_blocks WHERE canonical",
    )
    .fetch_one(pool)
    .await?;
    let mut next = match last {
        Some(last) => u64::try_from(last).context("negative explorer index block")? + 1,
        None => config
            .start_block
            .or_else(|| first_indexed.and_then(|block| u64::try_from(block).ok()))
            .unwrap_or(head)
            .min(head),
    };
    while next <= head {
        let through = next.saturating_add(999).min(head);
        index_explorer_events(pool, ethereum, next, through).await?;
        sqlx::query(
            "INSERT INTO gateway_explorer_index_state (id, last_block)
             VALUES (TRUE, $1)
             ON CONFLICT (id) DO UPDATE SET last_block = EXCLUDED.last_block",
        )
        .bind(i64::try_from(through)?)
        .execute(pool)
        .await?;
        if through == head {
            break;
        }
        next = through + 1;
    }
    Ok(())
}

async fn index_explorer_events(
    pool: &PgPool,
    ethereum: &Ethereum,
    from_block: u64,
    to_block: u64,
) -> Result<()> {
    for transition in ethereum
        .bridge_transition_accepted_logs(from_block, to_block)
        .await?
    {
        sqlx::query(
            "INSERT INTO gateway_explorer_bridge_transitions
                (old_action_state, new_action_state, new_deposit_state,
                 new_withdraw_state, new_deposit_nonce,
                 ethereum_block_number, ethereum_block_hash,
                 ethereum_tx_hash, ethereum_log_index, removed)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, FALSE)
             ON CONFLICT (ethereum_tx_hash, ethereum_log_index) DO UPDATE SET
                 old_action_state = EXCLUDED.old_action_state,
                 new_action_state = EXCLUDED.new_action_state,
                 new_deposit_state = EXCLUDED.new_deposit_state,
                 new_withdraw_state = EXCLUDED.new_withdraw_state,
                 new_deposit_nonce = EXCLUDED.new_deposit_nonce,
                 ethereum_block_number = EXCLUDED.ethereum_block_number,
                 ethereum_block_hash = EXCLUDED.ethereum_block_hash,
                 removed = FALSE,
                 indexed_at = NOW()",
        )
        .bind(transition.old_action_state.to_string())
        .bind(transition.new_action_state.to_string())
        .bind(transition.new_deposit_state.to_string())
        .bind(transition.new_withdraw_state.to_string())
        .bind(i64::try_from(transition.new_deposit_nonce)?)
        .bind(i64::try_from(transition.block_number)?)
        .bind(transition.block_hash.to_string())
        .bind(transition.transaction_hash.to_string())
        .bind(i64::try_from(transition.log_index)?)
        .execute(pool)
        .await?;
    }

    for settlement in ethereum
        .settlement_accepted_logs(from_block, to_block)
        .await?
    {
        sqlx::query(
            "INSERT INTO gateway_explorer_settlements
                (batch_sequence, mina_transaction_hash, ledger_hash,
                 outer_action_state, outer_action_state_length,
                 inner_action_state, inner_action_state_length,
                 slot_lower, slot_upper, ethereum_block_number,
                 ethereum_block_hash, ethereum_tx_hash, ethereum_log_index,
                 removed)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12,
                     $13, FALSE)
             ON CONFLICT (ethereum_tx_hash, ethereum_log_index) DO UPDATE SET
                 batch_sequence = EXCLUDED.batch_sequence,
                 mina_transaction_hash = EXCLUDED.mina_transaction_hash,
                 ledger_hash = EXCLUDED.ledger_hash,
                 outer_action_state = EXCLUDED.outer_action_state,
                 outer_action_state_length = EXCLUDED.outer_action_state_length,
                 inner_action_state = EXCLUDED.inner_action_state,
                 inner_action_state_length = EXCLUDED.inner_action_state_length,
                 slot_lower = EXCLUDED.slot_lower,
                 slot_upper = EXCLUDED.slot_upper,
                 ethereum_block_number = EXCLUDED.ethereum_block_number,
                 ethereum_block_hash = EXCLUDED.ethereum_block_hash,
                 inner_action_root = NULL,
                 inner_action_start_index = NULL,
                 inner_action_count = NULL,
                 claimable_slot = NULL,
                 removed = FALSE,
                 indexed_at = NOW()",
        )
        .bind(i64::try_from(settlement.batch_sequence)?)
        .bind(settlement.mina_transaction_hash.to_string())
        .bind(settlement.ledger_hash.to_string())
        .bind(settlement.outer_action_state.to_string())
        .bind(i64::from(settlement.outer_action_state_length))
        .bind(settlement.inner_action_state.to_string())
        .bind(i64::from(settlement.inner_action_state_length))
        .bind(i64::from(settlement.slot_lower))
        .bind(i64::from(settlement.slot_upper))
        .bind(i64::try_from(settlement.block_number)?)
        .bind(settlement.block_hash.to_string())
        .bind(settlement.transaction_hash.to_string())
        .bind(i64::try_from(settlement.log_index)?)
        .execute(pool)
        .await?;
    }

    for batch in ethereum
        .inner_action_batch_logs(from_block, to_block)
        .await?
    {
        let updated = sqlx::query(
            "UPDATE gateway_explorer_settlements
             SET inner_action_root = $1, inner_action_start_index = $2,
                 inner_action_count = $3, claimable_slot = $4
             WHERE batch_sequence = $5 AND ethereum_tx_hash = $6
               AND inner_action_state = $7 AND NOT removed",
        )
        .bind(batch.root.to_string())
        .bind(i64::from(batch.start_index))
        .bind(i64::from(batch.count))
        .bind(i64::from(batch.claimable_slot))
        .bind(i64::try_from(batch.batch_sequence)?)
        .bind(batch.transaction_hash.to_string())
        .bind(batch.state_after.to_string())
        .execute(pool)
        .await?;
        anyhow::ensure!(
            updated.rows_affected() == 1,
            "inner-action batch event did not match its settlement event"
        );
    }

    for claim in ethereum
        .native_withdrawal_claimed_logs(from_block, to_block)
        .await?
    {
        sqlx::query(
            "INSERT INTO gateway_native_withdrawal_claims
                (settlement_sequence, global_action_index, recipient,
                 zeko_amount, ethereum_amount, action_fields_hash,
                 ethereum_block_number, ethereum_block_hash, ethereum_tx_hash,
                 ethereum_log_index, removed)
             VALUES ($1, $2, $3, $4::numeric, $5::numeric, $6, $7, $8, $9,
                     $10, FALSE)
             ON CONFLICT (settlement_sequence, global_action_index) DO UPDATE SET
                 recipient = EXCLUDED.recipient,
                 zeko_amount = EXCLUDED.zeko_amount,
                 ethereum_amount = EXCLUDED.ethereum_amount,
                 action_fields_hash = EXCLUDED.action_fields_hash,
                 ethereum_block_number = EXCLUDED.ethereum_block_number,
                 ethereum_block_hash = EXCLUDED.ethereum_block_hash,
                 ethereum_tx_hash = EXCLUDED.ethereum_tx_hash,
                 ethereum_log_index = EXCLUDED.ethereum_log_index,
                 removed = FALSE,
                 indexed_at = NOW()",
        )
        .bind(i64::try_from(claim.settlement_sequence)?)
        .bind(i64::from(claim.global_action_index))
        .bind(claim.recipient.to_string())
        .bind(claim.zeko_amount.to_string())
        .bind(claim.ethereum_amount.to_string())
        .bind(claim.action_fields_hash.to_string())
        .bind(i64::try_from(claim.block_number)?)
        .bind(claim.block_hash.to_string())
        .bind(claim.transaction_hash.to_string())
        .bind(i64::try_from(claim.log_index)?)
        .execute(pool)
        .await?;
    }
    Ok(())
}

async fn recover_gateway_state(pool: &PgPool, ethereum: &Ethereum, config: &Config) -> Result<()> {
    let events = sqlx::query(
        "SELECT kind, ethereum_block_number, ethereum_block_hash,
                ethereum_tx_hash, ethereum_log_index
         FROM (
           SELECT 'bridge'::text AS kind, transitions.ethereum_block_number,
                  transitions.ethereum_block_hash,
                  transitions.ethereum_tx_hash,
                  transitions.ethereum_log_index
           FROM gateway_explorer_bridge_transitions transitions
           JOIN gateway_blocks blocks
             ON blocks.block_number = transitions.ethereum_block_number
            AND blocks.block_hash = transitions.ethereum_block_hash
            AND blocks.canonical AND blocks.finalized
           WHERE NOT transitions.removed
           UNION ALL
           SELECT 'settlement', settlements.ethereum_block_number,
                  settlements.ethereum_block_hash,
                  settlements.ethereum_tx_hash,
                  settlements.ethereum_log_index
           FROM gateway_explorer_settlements settlements
           JOIN gateway_blocks blocks
             ON blocks.block_number = settlements.ethereum_block_number
            AND blocks.block_hash = settlements.ethereum_block_hash
            AND blocks.canonical AND blocks.finalized
           WHERE NOT settlements.removed
         ) accepted
         ORDER BY ethereum_block_number, ethereum_log_index",
    )
    .fetch_all(pool)
    .await?;

    for event in events {
        let kind: String = event.try_get("kind")?;
        let block_number = u64::try_from(event.try_get::<i64, _>("ethereum_block_number")?)?;
        let block_hash: String = event.try_get("ethereum_block_hash")?;
        let transaction_hash: String = event.try_get("ethereum_tx_hash")?;
        let already_applied = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(
               SELECT 1 FROM gateway_account_history history
               JOIN proof_jobs jobs ON jobs.id = history.job_id
               WHERE lower(jobs.transaction_hash) = lower($1)
             )",
        )
        .bind(&transaction_hash)
        .fetch_one(pool)
        .await?;
        if already_applied {
            continue;
        }

        let public_values = ethereum
            .accepted_public_values(&kind, &transaction_hash)
            .await?;
        let public_values_hex = format!("0x{}", hex::encode(&public_values));
        let existing = sqlx::query(
            "SELECT id, input FROM proof_jobs
             WHERE lower(transaction_hash) = lower($1)
             ORDER BY created_at LIMIT 1",
        )
        .bind(&transaction_hash)
        .fetch_optional(pool)
        .await?;
        let (job_id, input) = match existing {
            Some(row) => {
                let original: Value = row.try_get("input")?;
                let input = if kind == "settlement" && original.get("submission").is_none() {
                    recovered_settlement_input(pool, config, &public_values).await?
                } else {
                    original
                };
                (row.try_get("id")?, input)
            }
            None => {
                let input = if kind == "settlement" {
                    recovered_settlement_input(pool, config, &public_values).await?
                } else {
                    json!({ "recoveredFromEthereum": true })
                };
                let id = uuid::Uuid::new_v4();
                sqlx::query(
                    "INSERT INTO proof_jobs
                        (id, kind, status, idempotency_key, input,
                         public_values, transaction_hash,
                         submitted_block_number, submitted_block_hash,
                         confirmations, completed_at)
                     VALUES ($1, $2::proof_kind, 'confirmed', $3, $4, $5, $6,
                             $7, $8, 1, NOW())",
                )
                .bind(id)
                .bind(&kind)
                .bind(format!(
                    "recovered:{kind}:{}",
                    transaction_hash.to_lowercase()
                ))
                .bind(&input)
                .bind(&public_values_hex)
                .bind(&transaction_hash)
                .bind(i64::try_from(block_number)?)
                .bind(&block_hash)
                .execute(pool)
                .await?;
                (id, input)
            }
        };

        match kind.as_str() {
            "bridge" => {
                apply_confirmed_bridge(
                    pool,
                    job_id,
                    &public_values_hex,
                    block_number,
                    &block_hash,
                    &transaction_hash,
                    config.fee_payer_public_key.as_deref(),
                )
                .await?;
            }
            "settlement" => {
                apply_confirmed_settlement(
                    pool,
                    job_id,
                    &input,
                    &public_values_hex,
                    block_number,
                    &block_hash,
                    &transaction_hash,
                )
                .await?;
            }
            _ => unreachable!(),
        }
        sqlx::query(
            "UPDATE proof_jobs SET status = 'confirmed', public_values = $2,
                    submitted_block_number = $3, submitted_block_hash = $4,
                    confirmations = GREATEST(confirmations, 1),
                    completed_at = COALESCE(completed_at, NOW()),
                    updated_at = NOW()
             WHERE id = $1",
        )
        .bind(job_id)
        .bind(&public_values_hex)
        .bind(i64::try_from(block_number)?)
        .bind(&block_hash)
        .execute(pool)
        .await?;
        tracing::info!(%kind, %transaction_hash, "recovered finalized gateway state from Ethereum");
    }
    Ok(())
}

fn u32_word(value: u32) -> Bytes32 {
    let mut word = [0u8; 32];
    word[28..].copy_from_slice(&value.to_be_bytes());
    word
}

async fn recovered_settlement_input(
    pool: &PgPool,
    config: &Config,
    public_values: &[u8],
) -> Result<Value> {
    let decoded = SettlementPublicValues::decode(public_values).map_err(anyhow::Error::msg)?;
    let receipt = decoded.settlement();
    let outer_public_key = sqlx::query_scalar::<_, Option<String>>(
        "SELECT outer_public_key FROM gateway_config WHERE id = TRUE",
    )
    .fetch_one(pool)
    .await?
    .context("VIRTUAL_MINA_OUTER_PUBLIC_KEY is required for gateway recovery")?;
    let fee_payer_public_key = config
        .fee_payer_public_key
        .as_deref()
        .context("VIRTUAL_MINA_FEE_PAYER_PUBLIC_KEY is required for gateway recovery")?;
    let nonce = sqlx::query_scalar::<_, Option<String>>(
        "SELECT account_json->>'nonce' FROM gateway_accounts
         WHERE public_key = $1 AND token_id = '1'",
    )
    .bind(fee_payer_public_key)
    .fetch_one(pool)
    .await?
    .unwrap_or_else(|| "0".to_owned())
    .parse::<u64>()
    .context("virtual Mina fee-payer nonce is invalid")?;
    let action = [
        [0u8; 32],
        receipt.state_after.fields[2],
        receipt.state_after.fields[3],
        receipt.state_after.fields[4],
        receipt.synchronized_outer_action_state,
        u32_word(receipt.synchronized_outer_action_state_length),
        u32_word(receipt.slot_lower),
        u32_word(receipt.slot_upper),
    ]
    .into_iter()
    .map(|field| Value::String(format!("0x{}", hex::encode(field))))
    .collect::<Vec<_>>();
    Ok(json!({
        "recoveredFromEthereum": true,
        "submission": {
            "outerAccountPublicKey": outer_public_key,
            "feePayerPublicKey": fee_payer_public_key,
            "nonce": nonce
        },
        "proof": { "binding": { "actions": [action] } }
    }))
}

async fn reconcile_jobs(
    pool: &PgPool,
    ethereum: &Ethereum,
    config: &Config,
    head: u64,
    finalized_block: Option<&BlockRef>,
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
        let confirmed = transaction_is_finalized(
            config.finality_mode,
            receipt.block_number,
            confirmations,
            config.confirmations,
            finalized_block.map(|block| block.number),
        );
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
                config.fee_payer_public_key.as_deref(),
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

pub(crate) async fn apply_confirmed_settlement(
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
    if input.pointer("/proof/innerActionBatch").is_some() {
        let inner_action_batch = decoded
            .inner_action_batch()
            .context("settlement receipt does not bind an inner-action batch")?;
        store_inner_action_leaves(
            &mut tx,
            input,
            inner_action_batch,
            block_number,
            block_hash,
            transaction_hash,
        )
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

pub(crate) async fn apply_confirmed_bridge(
    pool: &PgPool,
    job_id: uuid::Uuid,
    public_values_hex: &str,
    block_number: u64,
    block_hash: &str,
    transaction_hash: &str,
    fee_payer_public_key: Option<&str>,
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
    if let Some(fee_payer_public_key) = fee_payer_public_key {
        advance_fee_payer(
            &mut tx,
            job_id,
            fee_payer_public_key,
            None,
            u64::try_from(decoded.actions.len())?,
            block_number,
            block_hash,
        )
        .await?;
    }

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
        let leaf = match (&action.withdrawal, &action.token_withdrawal) {
            (Some(withdrawal), None) => hash_native_withdrawal_leaf(
                receipt.settlement.chain_id,
                batch.bridge_address,
                global_index,
                withdrawal,
                action_fields_hash,
            ),
            (None, Some(withdrawal)) => hash_erc20_withdrawal_leaf(
                receipt.settlement.chain_id,
                batch.bridge_address,
                global_index,
                withdrawal,
                action_fields_hash,
            ),
            (None, None) => hash_raw_inner_action_leaf(
                receipt.settlement.chain_id,
                batch.bridge_address,
                global_index,
                action_fields_hash,
            ),
            (Some(_), Some(_)) => anyhow::bail!("inner action has multiple withdrawal preimages"),
        };
        leaves.push(leaf);
        rows.push((offset, global_index, action, action_fields_hash, leaf));
    }
    anyhow::ensure!(
        inner_action_root(&leaves) == receipt.inner_action_root,
        "stored inner actions do not reproduce settlement root"
    );

    for (offset, global_index, action, action_fields_hash, leaf) in rows {
        let (
            token,
            asset_id,
            action_encoding_version,
            registry_index,
            record_commitment,
            recipient,
            amount,
        ) = match (&action.withdrawal, &action.token_withdrawal) {
            (Some(withdrawal), None) => (
                None,
                None,
                None,
                None,
                None,
                Some(format!("0x{}", hex::encode(withdrawal.recipient))),
                Some(withdrawal.amount.to_string()),
            ),
            (None, Some(withdrawal)) => (
                Some(format!("0x{}", hex::encode(withdrawal.token))),
                Some(format!("0x{}", hex::encode(withdrawal.asset_id))),
                Some(i32::try_from(withdrawal.encoding_version)?),
                (withdrawal.encoding_version == 2).then_some(i64::from(withdrawal.registry_index)),
                (withdrawal.encoding_version == 2)
                    .then(|| format!("0x{}", hex::encode(withdrawal.record_commitment))),
                Some(format!("0x{}", hex::encode(withdrawal.recipient))),
                Some(withdrawal.amount.to_string()),
            ),
            (None, None) => (None, None, None, None, None, None, None),
            (Some(_), Some(_)) => {
                anyhow::bail!("inner action has multiple withdrawal preimages")
            }
        };
        sqlx::query(
            "INSERT INTO gateway_inner_action_leaves
                (settlement_sequence, action_offset, global_action_index,
                 action_fields, action_fields_hash, leaf, token, asset_id,
                 action_encoding_version, registry_index, record_commitment,
                 recipient, zeko_amount, inner_action_root, commit_slot_upper,
                 ethereum_block_number, ethereum_block_hash, ethereum_tx_hash,
                 removed)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12,
                     $13::numeric, $14, $15, $16, $17, $18, FALSE)
             ON CONFLICT (settlement_sequence, action_offset) DO UPDATE SET
                 global_action_index = EXCLUDED.global_action_index,
                 action_fields = EXCLUDED.action_fields,
                 action_fields_hash = EXCLUDED.action_fields_hash,
                 leaf = EXCLUDED.leaf,
                 token = EXCLUDED.token,
                 asset_id = EXCLUDED.asset_id,
                 action_encoding_version = EXCLUDED.action_encoding_version,
                 registry_index = EXCLUDED.registry_index,
                 record_commitment = EXCLUDED.record_commitment,
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
        .bind(token)
        .bind(asset_id)
        .bind(action_encoding_version)
        .bind(registry_index)
        .bind(record_commitment)
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
    advance_fee_payer(
        tx,
        job_id,
        public_key,
        Some(nonce),
        1,
        block_number,
        block_hash,
    )
    .await
}

async fn advance_fee_payer(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    job_id: uuid::Uuid,
    public_key: &str,
    expected_nonce: Option<u64>,
    increment: u64,
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
    let current_nonce = account
        .get("nonce")
        .and_then(Value::as_str)
        .unwrap_or("0")
        .parse::<u64>()
        .context("virtual Mina fee-payer nonce is invalid")?;
    if let Some(expected_nonce) = expected_nonce {
        anyhow::ensure!(
            current_nonce == expected_nonce,
            "virtual Mina fee-payer nonce {current_nonce} does not match expected nonce \
             {expected_nonce}"
        );
    }
    snapshot_account(tx, job_id, public_key, &account, block_number, block_hash).await?;
    account
        .as_object_mut()
        .context("virtual Mina account must be a JSON object")?
        .insert(
            "nonce".to_owned(),
            json!(fee_payer_nonce_after(current_nonce, increment)?.to_string()),
        );
    store_account(tx, public_key, account, block_number, block_hash).await
}

fn fee_payer_nonce_after(current_nonce: u64, increment: u64) -> Result<u64> {
    current_nonce
        .checked_add(increment)
        .context("fee-payer nonce overflow")
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

fn transaction_is_finalized(
    mode: FinalityMode,
    block_number: u64,
    confirmations: u64,
    required_confirmations: u64,
    finalized_height: Option<u64>,
) -> bool {
    match mode {
        FinalityMode::Finalized => finalized_height.is_some_and(|height| block_number <= height),
        FinalityMode::Confirmations => confirmations >= required_confirmations.max(1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn confirmation_count_includes_submission_block() {
        assert_eq!(12_u64.saturating_sub(12) + 1, 1);
        assert_eq!(15_u64.saturating_sub(12) + 1, 4);
    }

    #[test]
    fn consensus_finality_ignores_confirmation_depth() {
        assert!(!transaction_is_finalized(
            FinalityMode::Finalized,
            100,
            100,
            12,
            Some(99)
        ));
        assert!(transaction_is_finalized(
            FinalityMode::Finalized,
            100,
            1,
            12,
            Some(100)
        ));
        assert!(!transaction_is_finalized(
            FinalityMode::Finalized,
            100,
            100,
            12,
            None
        ));
    }

    #[test]
    fn confirmation_mode_preserves_local_boundary() {
        assert!(!transaction_is_finalized(
            FinalityMode::Confirmations,
            100,
            11,
            12,
            None
        ));
        assert!(transaction_is_finalized(
            FinalityMode::Confirmations,
            100,
            12,
            12,
            None
        ));
    }

    #[test]
    fn finality_mode_is_explicit() {
        assert_eq!(
            FinalityMode::parse("finalized").unwrap(),
            FinalityMode::Finalized
        );
        assert_eq!(
            FinalityMode::parse("confirmations").unwrap(),
            FinalityMode::Confirmations
        );
        assert!(FinalityMode::parse("safe").is_err());
    }

    #[test]
    fn bridge_actions_advance_the_virtual_fee_payer_nonce() {
        assert_eq!(fee_payer_nonce_after(2, 2).unwrap(), 4);
        assert!(fee_payer_nonce_after(u64::MAX, 1).is_err());
    }
}
