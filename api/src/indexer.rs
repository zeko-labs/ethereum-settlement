use anyhow::{Context, Result};
use sqlx::{PgPool, Row};
use std::time::Duration;
use tokio::time::sleep;

use crate::ethereum::{BlockRef, Ethereum};
use crate::proof_kind::ProofKind;
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
    let mut latest = sqlx::query(
        "SELECT block_number, block_hash FROM gateway_blocks
         WHERE canonical ORDER BY block_number DESC LIMIT 1",
    )
    .fetch_optional(pool)
    .await?;

    if let Some(row) = &latest {
        let number = u64::try_from(row.try_get::<i64, _>("block_number")?)?;
        if number >= head {
            // No new child block will call ensure_parent at a stationary or
            // regressed head (notably after evm_revert). Check the tip itself.
            let remote = ethereum.block(head).await?;
            let hash: String = row.try_get("block_hash")?;
            if number > head || hash != remote.hash.to_string() {
                let ancestor = common_ancestor(pool, ethereum, remote).await?;
                rollback_canonical_chain(pool, config.finality_mode, ancestor).await?;
                latest = sqlx::query(
                    "SELECT block_number, block_hash FROM gateway_blocks
                     WHERE canonical ORDER BY block_number DESC LIMIT 1",
                )
                .fetch_optional(pool)
                .await?;
            }
        }
    }

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
        index_bridge_deposits(pool, ethereum, &block).await?;
        next = block.number + 1;
    }

    let head_block = ethereum.block(head).await?;
    let indexed_head = sqlx::query_scalar::<_, String>(
        "SELECT block_hash FROM gateway_blocks WHERE block_number = $1 AND canonical",
    )
    .bind(i64::try_from(head)?)
    .fetch_optional(pool)
    .await?;
    anyhow::ensure!(
        indexed_head.as_deref() == Some(head_block.hash.to_string().as_str()),
        "Ethereum head changed while indexing; retry before advancing finality"
    );
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
            Some(block.number)
        }
        None => head.checked_sub(config.confirmations.max(1) - 1),
    };
    sqlx::query(
        "UPDATE gateway_blocks
         SET finalized = canonical AND COALESCE(block_number <= $1, FALSE)",
    )
    .bind(finalized_through.map(i64::try_from).transpose()?)
    .execute(pool)
    .await?;

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

    let parent = ethereum.block(block.number - 1).await?;
    anyhow::ensure!(
        parent.hash == block.parent_hash,
        "Ethereum parent changed while indexing; refetch the child block"
    );
    let ancestor = common_ancestor(pool, ethereum, parent).await?;
    rollback_canonical_chain(pool, finality_mode, ancestor).await?;
    anyhow::ensure!(
        ancestor + 1 == block.number,
        "Ethereum reorg rolled back to {ancestor}; caller must refetch from the new tip"
    );
    Ok(())
}

async fn common_ancestor(pool: &PgPool, ethereum: &Ethereum, mut remote: BlockRef) -> Result<u64> {
    loop {
        let local = sqlx::query_scalar::<_, String>(
            "SELECT block_hash FROM gateway_blocks
             WHERE block_number = $1 AND canonical",
        )
        .bind(i64::try_from(remote.number)?)
        .fetch_optional(pool)
        .await?;
        if local.as_deref() == Some(remote.hash.to_string().as_str()) {
            return Ok(remote.number);
        }
        // No indexed state exists below the configured start height. Stop at
        // that boundary instead of querying all the way back to genesis.
        if local.is_none() {
            return Ok(remote.number);
        }
        anyhow::ensure!(remote.number > 0, "Ethereum genesis hash changed");
        remote = ethereum.block(remote.number - 1).await?;
    }
}

async fn rollback_canonical_chain(
    pool: &PgPool,
    finality_mode: FinalityMode,
    ancestor: u64,
) -> Result<()> {
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
    rollback_after(pool, ancestor).await
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
    // Recovered history normally has no submission owner. If a retained job
    // does have an owner or pending command, its known hash remains ambiguous
    // after a reorg. Keep that identity and ownership until canonical event
    // recovery observes its outcome, rather than deleting a referenced job or
    // clearing the writer reservation merely to satisfy its foreign key.
    sqlx::query(
        "UPDATE proof_jobs j SET status = 'reorged', completed_at = NOW(),
                error = 'Recovered Ethereum submission was orphaned; awaiting canonical reconciliation',
                updated_at = NOW()
         WHERE j.input->>'recoveredFromEthereum' = 'true'
           AND j.submitted_block_number > $1
           AND (j.prepared_transaction IS NOT NULL
                OR EXISTS(SELECT 1 FROM gateway_outer_writer w WHERE w.job_id = j.id)
                OR EXISTS(SELECT 1 FROM gateway_pending_commands p WHERE p.job_id = j.id))",
    )
    .bind(ancestor)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "DELETE FROM proof_jobs j
         WHERE j.input->>'recoveredFromEthereum' = 'true'
           AND j.submitted_block_number > $1
           AND j.prepared_transaction IS NULL
           AND NOT EXISTS(SELECT 1 FROM gateway_outer_writer w WHERE w.job_id = j.id)
           AND NOT EXISTS(SELECT 1 FROM gateway_pending_commands p WHERE p.job_id = j.id)",
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
         WHERE job_id IN (SELECT id FROM proof_jobs WHERE status = 'reorged'
                         AND transaction_hash IS NULL AND prepared_transaction IS NULL)",
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

async fn insert_block<'e, E>(executor: E, block: &BlockRef) -> Result<()>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
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
    .execute(executor)
    .await?;
    Ok(())
}

async fn index_bridge_deposits(pool: &PgPool, ethereum: &Ethereum, block: &BlockRef) -> Result<()> {
    // Fetch before opening a database transaction. The block row is the next
    // tick's cursor, so persist it atomically with every deposit only after a
    // successful RPC response. Exhausted 429 retries must not skip this block.
    let deposits = ethereum.bridge_deposit_logs(block).await?;
    let mut tx = pool.begin().await?;
    for deposit in deposits {
        anyhow::ensure!(
            deposit.block_number == block.number && deposit.block_hash == block.hash,
            "bridge deposit RPC returned logs outside the requested block"
        );
        let asset_id = deposit
            .erc20_identity
            .map(|identity| identity.asset_id().to_string());
        let action_encoding_version = i32::try_from(
            deposit
                .erc20_identity
                .map_or(0, |identity| identity.action_encoding_version()),
        )?;
        let registry_index = deposit
            .erc20_identity
            .and_then(|identity| identity.registry_index())
            .map(i64::from);
        let record_commitment = deposit
            .erc20_identity
            .and_then(|identity| identity.record_commitment())
            .map(|value| value.to_string());
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
        .execute(&mut *tx)
        .await?;
    }
    insert_block(&mut *tx, block).await?;
    tx.commit().await?;
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
                 new_deposit_nonce,
                 ethereum_block_number, ethereum_block_hash,
                 ethereum_tx_hash, ethereum_log_index, removed)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, FALSE)
             ON CONFLICT (ethereum_tx_hash, ethereum_log_index) DO UPDATE SET
                 old_action_state = EXCLUDED.old_action_state,
                 new_action_state = EXCLUDED.new_action_state,
                 new_deposit_state = EXCLUDED.new_deposit_state,
                 new_deposit_nonce = EXCLUDED.new_deposit_nonce,
                 ethereum_block_number = EXCLUDED.ethereum_block_number,
                 ethereum_block_hash = EXCLUDED.ethereum_block_hash,
                 removed = FALSE,
                 indexed_at = NOW()",
        )
        .bind(transition.old_action_state.to_string())
        .bind(transition.new_action_state.to_string())
        .bind(transition.new_deposit_state.to_string())
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
        let kind: ProofKind = event.try_get::<String, _>("kind")?.parse()?;
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
            .accepted_public_values(kind, &transaction_hash)
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
                let input = if kind == ProofKind::Settlement && original.get("submission").is_none()
                {
                    recovered_settlement_input(pool, config, &public_values).await?
                } else {
                    original
                };
                (row.try_get("id")?, input)
            }
            None => {
                let input = if kind == ProofKind::Settlement {
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
                .bind(kind.as_str())
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

        match kind {
            ProofKind::Bridge => {
                apply_confirmed_bridge(
                    pool,
                    job_id,
                    &public_values_hex,
                    block_number,
                    &block_hash,
                    &transaction_hash,
                )
                .await?;
            }
            ProofKind::Settlement => {
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
        "SELECT j.id, j.kind::text AS kind, j.input, j.public_values,
                j.status::text AS status, j.transaction_hash,
                (EXISTS(SELECT 1 FROM gateway_account_history h WHERE h.job_id = j.id)
                 OR (j.kind::text = 'settlement' AND NOT (j.input ? 'submission')))
                   AS state_applied
         FROM proof_jobs j
         WHERE j.transaction_hash IS NOT NULL
           AND j.status IN ('submitted', 'confirmed')
           AND j.kind::text IN ('settlement', 'bridge')
           AND ($1 OR j.status <> 'confirmed'
                OR NOT EXISTS(
                    SELECT 1 FROM gateway_blocks b
                    WHERE b.block_number = j.submitted_block_number
                      AND b.block_hash = j.submitted_block_hash
                      AND b.canonical AND b.finalized
                      AND b.block_number <= $2)
                OR ((j.kind::text = 'bridge' OR j.input ? 'submission')
                    AND NOT EXISTS(SELECT 1 FROM gateway_account_history h
                                   WHERE h.job_id = j.id)))",
    )
    // A confirmation threshold is reversible, so retain receipt reconciliation
    // in that mode. Consensus-finalized, applied jobs need no more RPC traffic.
    .bind(config.finality_mode == FinalityMode::Confirmations)
    .bind(
        finalized_block
            .map(|block| i64::try_from(block.number))
            .transpose()?,
    )
    .fetch_all(pool)
    .await?;
    for row in rows {
        let id: uuid::Uuid = row.try_get("id")?;
        let kind: ProofKind = row.try_get::<String, _>("kind")?.parse()?;
        let input: Value = row.try_get("input")?;
        let public_values: Option<String> = row.try_get("public_values")?;
        let previous_status: String = row.try_get("status")?;
        let state_applied: bool = row.try_get("state_applied")?;
        let transaction_hash: String = row.try_get("transaction_hash")?;
        let Some(receipt) = ethereum.transaction_receipt(&transaction_hash).await? else {
            continue;
        };
        let confirmations = head.saturating_sub(receipt.block_number) + 1;
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
        if !receipt.succeeded && confirmed {
            let mut tx = pool.begin().await?;
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
            .execute(&mut *tx)
            .await?;
            sqlx::query("DELETE FROM gateway_pending_commands WHERE job_id = $1")
                .bind(id)
                .execute(&mut *tx)
                .await?;
            tx.commit().await?;
            continue;
        }
        if confirmed
            && (previous_status != "confirmed" || !state_applied)
            && kind == ProofKind::Settlement
        {
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
        if confirmed
            && (previous_status != "confirmed" || !state_applied)
            && kind == ProofKind::Bridge
        {
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
                    completed_at = CASE WHEN $7 THEN COALESCE(completed_at, NOW()) ELSE NULL END,
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
    use alloy::primitives::B256;
    use axum::{http::StatusCode, response::IntoResponse, Json};
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    fn test_ethereum(url: &str) -> Ethereum {
        Ethereum::new(
            url.into(),
            format!("0x{}", "11".repeat(20)),
            format!("0x{}", "22".repeat(20)),
            "01".repeat(32),
            "02".repeat(32),
        )
        .unwrap()
    }

    fn test_config(finality_mode: FinalityMode) -> Config {
        Config {
            start_block: Some(0),
            finality_mode,
            confirmations: 12,
            poll_interval: Duration::from_secs(1),
            fee_payer_public_key: None,
        }
    }

    fn test_receipt(request: &Value, block: &BlockRef, succeeded: bool) -> Value {
        json!({"jsonrpc":"2.0", "id":request["id"], "result":{
            "transactionHash":request["params"][0], "transactionIndex":"0x0",
            "blockHash":block.hash.to_string(), "blockNumber":format!("0x{:x}", block.number),
            "from":format!("0x{}", "11".repeat(20)), "to":format!("0x{}", "22".repeat(20)),
            "cumulativeGasUsed":"0x5208", "gasUsed":"0x5208", "contractAddress":null,
            "logs":[], "logsBloom":format!("0x{}", "00".repeat(256)),
            "status":if succeeded {"0x1"} else {"0x0"}, "effectiveGasPrice":"0x1", "type":"0x2"
        }})
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn exhausted_deposit_read_does_not_advance_block_cursor(pool: PgPool) {
        sqlx::query("INSERT INTO gateway_config (genesis_timestamp, fork_slot, account_creation_fee, state_hash) VALUES ('0', 0, '1', 'test')")
            .execute(&pool).await.unwrap();
        let block = BlockRef {
            number: 100,
            hash: B256::repeat_byte(1),
            parent_hash: B256::repeat_byte(2),
        };
        let empty: alloy::rpc::types::Block = Default::default();
        let mut rpc_block = serde_json::to_value(empty).unwrap();
        rpc_block["number"] = json!("0x64");
        rpc_block["hash"] = json!(block.hash.to_string());
        rpc_block["parentHash"] = json!(block.parent_hash.to_string());
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = calls.clone();
        let retries = crate::rpc::RpcConfig::from_env().unwrap().max_retries as usize;
        let server = crate::rpc::test_support::serve(move |request| {
            let rpc_block = rpc_block.clone();
            let counter = counter.clone();
            async move {
                let result = match request["method"].as_str().unwrap() {
                    "eth_getBlockByNumber" => rpc_block,
                    "eth_getLogs" => {
                        assert_eq!(request["params"][0]["blockHash"], block.hash.to_string());
                        assert!(request["params"][0].get("fromBlock").is_none());
                        assert!(request["params"][0].get("toBlock").is_none());
                        if counter.fetch_add(1, Ordering::SeqCst) <= retries {
                            return (
                                StatusCode::TOO_MANY_REQUESTS,
                                [("retry-after", "0")],
                                "temporarily rate limited",
                            )
                                .into_response();
                        }
                        json!([])
                    }
                    method => panic!("unexpected RPC method {method}"),
                };
                Json(json!({"jsonrpc":"2.0", "id":request["id"], "result":result})).into_response()
            }
        })
        .await;
        let ethereum = test_ethereum(&server.url);
        let mut config = test_config(FinalityMode::Confirmations);
        config.start_block = Some(100);
        config.confirmations = 1;
        let error = index_blocks(&pool, &ethereum, &config, 100, None)
            .await
            .unwrap_err();
        assert!(crate::rpc::is_rate_limited(&error));
        let indexed: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM gateway_blocks")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(indexed, 0);
        assert_eq!(calls.load(Ordering::SeqCst), retries + 1);

        // The next tick must retry exactly the failed block, even when it has
        // no deposits. Only a successful log response permits cursor advance.
        index_blocks(&pool, &ethereum, &config, 100, None)
            .await
            .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), retries + 2);
        let indexed: (i64, String, bool) = sqlx::query_as(
            "SELECT block_number, block_hash, finalized FROM gateway_blocks WHERE canonical",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(indexed, (100, B256::repeat_byte(1).to_string(), true));
    }

    async fn check_changed_tip(pool: PgPool, head: u64, mode: FinalityMode) -> Result<()> {
        sqlx::query("INSERT INTO gateway_config (genesis_timestamp, fork_slot, account_creation_fee, state_hash, recovery_ready) VALUES ('0', 0, '1', 'test', TRUE)")
            .execute(&pool).await?;
        let anchor = BlockRef {
            number: 99,
            hash: B256::repeat_byte(9),
            parent_hash: B256::repeat_byte(8),
        };
        let orphan = BlockRef {
            number: 100,
            hash: B256::repeat_byte(10),
            parent_hash: anchor.hash,
        };
        insert_block(&pool, &anchor).await?;
        insert_block(&pool, &orphan).await?;
        sqlx::query("UPDATE gateway_blocks SET finalized = TRUE")
            .execute(&pool)
            .await?;
        let job = uuid::Uuid::new_v4();
        sqlx::query("INSERT INTO proof_jobs (id, kind, status, input, transaction_hash, submitted_block_number, submitted_block_hash) VALUES ($1, 'settlement', 'confirmed', '{}'::jsonb, $2, 100, $3)")
            .bind(job).bind(B256::repeat_byte(4).to_string()).bind(orphan.hash.to_string())
            .execute(&pool).await?;
        sqlx::query("INSERT INTO gateway_accounts (public_key, token_id, account_json, ethereum_block_number, ethereum_block_hash) VALUES ('outer', '1', '{\"nonce\":\"1\"}', 100, $1)")
            .bind(orphan.hash.to_string()).execute(&pool).await?;
        sqlx::query("INSERT INTO gateway_account_history (job_id, public_key, token_id, account_before, ethereum_block_number, ethereum_block_hash) VALUES ($1, 'outer', '1', '{\"nonce\":\"0\"}', 100, $2)")
            .bind(job).bind(orphan.hash.to_string()).execute(&pool).await?;
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = calls.clone();
        let replacement = BlockRef {
            hash: B256::repeat_byte(11),
            ..orphan.clone()
        };
        let remote_anchor = anchor.clone();
        let remote_replacement = replacement.clone();
        let server = crate::rpc::test_support::serve(move |request| {
            counter.fetch_add(1, Ordering::SeqCst);
            let anchor = remote_anchor.clone();
            let replacement = remote_replacement.clone();
            async move {
                let result = match request["method"].as_str().unwrap() {
                    "eth_getBlockByNumber" => {
                        let block = match request["params"][0].as_str().unwrap() {
                            "0x63" => anchor,
                            "0x64" => replacement,
                            number => panic!("unbounded ancestry query {number}"),
                        };
                        let empty: alloy::rpc::types::Block = Default::default();
                        let mut result = serde_json::to_value(empty).unwrap();
                        result["number"] = json!(format!("0x{:x}", block.number));
                        result["hash"] = json!(block.hash.to_string());
                        result["parentHash"] = json!(block.parent_hash.to_string());
                        result
                    }
                    "eth_getLogs" => {
                        assert_eq!(
                            request["params"][0]["blockHash"],
                            replacement.hash.to_string()
                        );
                        json!([])
                    }
                    method => panic!("unexpected RPC method {method}"),
                };
                Json(json!({"jsonrpc":"2.0", "id":request["id"], "result":result})).into_response()
            }
        })
        .await;
        let ethereum = test_ethereum(&server.url);
        let mut config = test_config(mode);
        config.confirmations = 1;
        let result = index_blocks(
            &pool,
            &ethereum,
            &config,
            head,
            Some(&anchor).filter(|_| mode == FinalityMode::Finalized),
        )
        .await;
        let expected_nonce = if mode == FinalityMode::Finalized {
            assert!(result
                .unwrap_err()
                .to_string()
                .contains("consensus-finalized checkpoint"));
            "1"
        } else {
            result?;
            let ready: bool = sqlx::query_scalar("SELECT recovery_ready FROM gateway_config")
                .fetch_one(&pool)
                .await?;
            assert!(
                !ready,
                "recovery must finish before writer admission resumes"
            );
            let status: String =
                sqlx::query_scalar("SELECT status::text FROM proof_jobs WHERE id = $1")
                    .bind(job)
                    .fetch_one(&pool)
                    .await?;
            assert_eq!(status, "queued");
            let indexed: (i64, String) = sqlx::query_as("SELECT block_number, block_hash FROM gateway_blocks WHERE canonical ORDER BY block_number DESC LIMIT 1")
                .fetch_one(&pool).await?;
            assert_eq!(
                indexed,
                (
                    head as i64,
                    if head == 99 {
                        anchor.hash
                    } else {
                        replacement.hash
                    }
                    .to_string()
                )
            );
            "0"
        };
        let account: Value = sqlx::query_scalar(
            "SELECT account_json FROM gateway_accounts WHERE public_key = 'outer'",
        )
        .fetch_one(&pool)
        .await?;
        assert_eq!(account["nonce"], expected_nonce);
        assert!(calls.load(Ordering::SeqCst) <= 5);
        Ok(())
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn confirmation_mode_rolls_back_same_height_reorg(pool: PgPool) -> Result<()> {
        check_changed_tip(pool, 100, FinalityMode::Confirmations).await
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn confirmation_mode_rolls_back_regressed_head(pool: PgPool) -> Result<()> {
        check_changed_tip(pool, 99, FinalityMode::Confirmations).await
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn same_height_reorg_cannot_roll_back_consensus_finality(pool: PgPool) -> Result<()> {
        check_changed_tip(pool, 100, FinalityMode::Finalized).await
    }

    fn rollback_test_state(pool: PgPool, url: &str) -> crate::AppState {
        crate::AppState {
            pool,
            archive_pool: None,
            api_key: "test".into(),
            ethereum: test_ethereum(url),
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
            ethereum_finality_mode: FinalityMode::Confirmations,
            ethereum_confirmations: 1,
            sequencer_graphql_url: None,
            inner_public_key: None,
            fee_payer_public_key: None,
            http_client: reqwest::Client::new(),
        }
    }

    async fn rollback_fixture(
        pool: &PgPool,
        signed_dependent: bool,
    ) -> (uuid::Uuid, uuid::Uuid, crate::ethereum::PreparedSubmission) {
        sqlx::query("INSERT INTO gateway_config (genesis_timestamp, fork_slot, account_creation_fee, state_hash, recovery_ready, outer_public_key) VALUES ('0', 0, '1', 'test', TRUE, 'outer')")
            .execute(pool).await.unwrap();
        let accepted = uuid::Uuid::new_v4();
        let dependent = uuid::Uuid::new_v4();
        let prepared = crate::ethereum::PreparedSubmission {
            transaction_hash: alloy::primitives::keccak256([1, 2]).to_string(),
            raw_transaction: "0x0102".into(),
            sender: format!("0x{}", "11".repeat(20)),
            nonce: 0,
        };
        let input = json!({"submission":{"feePayerPublicKey":"payer", "nonce":0, "commandBase64":"accepted-command"}});
        sqlx::query("INSERT INTO proof_jobs (id, kind, status, input, public_values, proof_bytes, proof_request_id, prepared_transaction, transaction_hash, submitted_block_number, submitted_block_hash)
                     VALUES ($1, 'settlement', 'confirmed', $2, '0x01', '0x02', 'paid-request', $3, $4, 101, $5)")
            .bind(accepted).bind(input).bind(serde_json::to_value(&prepared).unwrap())
            .bind(&prepared.transaction_hash).bind(B256::repeat_byte(1).to_string()).execute(pool).await.unwrap();
        sqlx::query("INSERT INTO proof_jobs (id, kind, status, input, prepared_transaction, transaction_hash)
                     VALUES ($1, 'settlement', 'validating', '{}', $2, $3)")
            .bind(dependent)
            .bind(signed_dependent.then(|| serde_json::to_value(&prepared).unwrap()))
            .bind(signed_dependent.then(|| B256::repeat_byte(2).to_string()))
            .execute(pool).await.unwrap();
        sqlx::query("INSERT INTO gateway_pending_commands (job_id, public_key, nonce, command_kind, command_base64) VALUES ($1, 'payer', 1, 'zkapp', 'dependent-command')")
            .bind(dependent).execute(pool).await.unwrap();
        sqlx::query("UPDATE gateway_outer_writer SET reservation_id = $1, owner_id = 'sequencer', fencing_token = 1, job_id = $2 WHERE id = TRUE")
            .bind(uuid::Uuid::new_v4()).bind(dependent).execute(pool).await.unwrap();
        (accepted, dependent, prepared)
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn rollback_releases_unsigned_successor_then_reuses_earliest_signed_attempt(
        pool: PgPool,
    ) {
        let (accepted, dependent, prepared) = rollback_fixture(&pool, false).await;
        rollback_after(&pool, 100).await.unwrap();
        assert!(
            crate::claim_job(&pool).await.unwrap().is_none(),
            "the indexer must finish canonical recovery first"
        );
        let row = sqlx::query("SELECT status::text AS status, proof_request_id, proof_bytes, prepared_transaction, transaction_hash FROM proof_jobs WHERE id = $1")
            .bind(accepted).fetch_one(&pool).await.unwrap();
        assert_eq!(row.get::<String, _>("status"), "queued");
        assert_eq!(row.get::<String, _>("proof_request_id"), "paid-request");
        assert_eq!(row.get::<String, _>("proof_bytes"), "0x02");
        assert_eq!(
            row.get::<Value, _>("prepared_transaction"),
            serde_json::to_value(&prepared).unwrap()
        );
        assert!(row.get::<Option<String>, _>("transaction_hash").is_none());
        let pending: Vec<uuid::Uuid> =
            sqlx::query_scalar("SELECT job_id FROM gateway_pending_commands")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(pending, vec![accepted]);
        let status: String =
            sqlx::query_scalar("SELECT status::text FROM proof_jobs WHERE id = $1")
                .bind(dependent)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(status, "reorged");

        sqlx::query("UPDATE gateway_config SET recovery_ready = TRUE")
            .execute(&pool)
            .await
            .unwrap();
        // claim_job calls the same writer lock used by competing API instances.
        let resumed = crate::claim_job(&pool)
            .await
            .unwrap()
            .expect("unsigned successor must release the writer");
        assert_eq!(resumed.id, accepted);
        assert!(resumed.prepared);
        let owner: Option<uuid::Uuid> =
            sqlx::query_scalar("SELECT job_id FROM gateway_outer_writer WHERE id = TRUE")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(owner.is_none());

        let sends = Arc::new(AtomicUsize::new(0));
        let observed = sends.clone();
        let expected = prepared.clone();
        let rpc_pool = pool.clone();
        let server = crate::rpc::test_support::serve(move |request| {
            let observed = observed.clone();
            let expected = expected.clone();
            let pool = rpc_pool.clone();
            async move {
                let result = match request["method"].as_str().unwrap() {
                    "eth_getTransactionByHash" | "eth_getTransactionReceipt" => Value::Null,
                    "eth_sendRawTransaction" => {
                        observed.fetch_add(1, Ordering::SeqCst);
                        assert_eq!(request["params"][0], expected.raw_transaction);
                        let hash: String = sqlx::query_scalar(
                            "SELECT transaction_hash FROM proof_jobs WHERE id = $1",
                        )
                        .bind(accepted)
                        .fetch_one(&pool)
                        .await
                        .unwrap();
                        assert_eq!(
                            hash, expected.transaction_hash,
                            "restored hash must be durable before rebroadcast"
                        );
                        json!(hash)
                    }
                    method => panic!(
                        "rollback must not request a new nonce, simulation, or proof: {method}"
                    ),
                };
                Json(json!({"jsonrpc":"2.0", "id":request["id"], "result":result})).into_response()
            }
        })
        .await;
        crate::process_job(&rollback_test_state(pool.clone(), &server.url), resumed)
            .await
            .unwrap();
        let row = sqlx::query("SELECT status::text AS status, transaction_hash, proof_request_id, proof_bytes FROM proof_jobs WHERE id = $1")
            .bind(accepted).fetch_one(&pool).await.unwrap();
        assert_eq!(row.get::<String, _>("status"), "submitted");
        assert_eq!(
            row.get::<String, _>("transaction_hash"),
            prepared.transaction_hash
        );
        assert_eq!(row.get::<String, _>("proof_request_id"), "paid-request");
        assert_eq!(row.get::<String, _>("proof_bytes"), "0x02");
        assert_eq!(sends.load(Ordering::SeqCst), 1);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn rollback_preserves_pending_and_owner_for_signed_successor(pool: PgPool) {
        let (accepted, dependent, _) = rollback_fixture(&pool, true).await;
        rollback_after(&pool, 100).await.unwrap();
        sqlx::query("UPDATE gateway_config SET recovery_ready = TRUE")
            .execute(&pool)
            .await
            .unwrap();
        let mut tx = pool.begin().await.unwrap();
        crate::outer_writer::lock(&mut tx).await.unwrap();
        tx.commit().await.unwrap();
        let pending: Vec<uuid::Uuid> =
            sqlx::query_scalar("SELECT job_id FROM gateway_pending_commands")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(pending.len(), 2);
        assert!(pending.contains(&accepted) && pending.contains(&dependent));
        let owner: Option<uuid::Uuid> =
            sqlx::query_scalar("SELECT job_id FROM gateway_outer_writer WHERE id = TRUE")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(owner, Some(dependent));
        assert!(
            crate::claim_job(&pool).await.unwrap().is_none(),
            "a signed dependent needs canonical reconciliation before releasing its writer"
        );
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn rollback_keeps_referenced_recovered_job_without_violating_writer_fk(pool: PgPool) {
        sqlx::query("INSERT INTO gateway_config (genesis_timestamp, fork_slot, account_creation_fee, state_hash, recovery_ready) VALUES ('0', 0, '1', 'test', TRUE)").execute(&pool).await.unwrap();
        let retained = uuid::Uuid::new_v4();
        let historical = uuid::Uuid::new_v4();
        for id in [retained, historical] {
            sqlx::query("INSERT INTO proof_jobs (id, kind, status, input, transaction_hash, submitted_block_number) VALUES ($1, 'settlement', 'confirmed', '{\"recoveredFromEthereum\":true}', $2, 101)")
                .bind(id).bind(B256::repeat_byte(1).to_string()).execute(&pool).await.unwrap();
        }
        sqlx::query("UPDATE gateway_outer_writer SET reservation_id = $1, owner_id = 'sequencer', fencing_token = 1, job_id = $2 WHERE id = TRUE")
            .bind(uuid::Uuid::new_v4()).bind(retained).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO gateway_pending_commands (job_id, public_key, nonce, command_kind, command_base64) VALUES ($1, 'payer', 0, 'zkapp', 'command')")
            .bind(retained).execute(&pool).await.unwrap();
        rollback_after(&pool, 100).await.unwrap();
        let jobs: Vec<uuid::Uuid> = sqlx::query_scalar("SELECT id FROM proof_jobs")
            .fetch_all(&pool)
            .await
            .unwrap();
        assert_eq!(jobs, vec![retained]);
        let row = sqlx::query(
            "SELECT status::text AS status, transaction_hash FROM proof_jobs WHERE id = $1",
        )
        .bind(retained)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(row.get::<String, _>("status"), "reorged");
        assert_eq!(
            row.get::<String, _>("transaction_hash"),
            B256::repeat_byte(1).to_string()
        );
        let pending: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM gateway_pending_commands WHERE job_id = $1")
                .bind(retained)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(pending, 1);
        let owner: Option<uuid::Uuid> =
            sqlx::query_scalar("SELECT job_id FROM gateway_outer_writer WHERE id = TRUE")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(owner, Some(retained));
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn finalized_applied_history_makes_no_receipt_requests(pool: PgPool) {
        let block = BlockRef {
            number: 100,
            hash: B256::repeat_byte(1),
            parent_hash: B256::repeat_byte(2),
        };
        insert_block(&pool, &block).await.unwrap();
        sqlx::query("UPDATE gateway_blocks SET finalized = TRUE")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO proof_jobs (id, kind, status, input, transaction_hash, submitted_block_number, submitted_block_hash, completed_at)
                     SELECT gen_random_uuid(), 'settlement', 'confirmed', '{\"submission\":{}}'::jsonb, $1, 100, $2, '2026-09-01'::timestamptz
                     FROM generate_series(1,1380)")
            .bind(B256::repeat_byte(3).to_string()).bind(block.hash.to_string()).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO gateway_account_history (job_id, public_key, token_id, account_before, ethereum_block_number, ethereum_block_hash)
                     SELECT id, 'outer', '1', '{}'::jsonb, 100, $1 FROM proof_jobs")
            .bind(block.hash.to_string()).execute(&pool).await.unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = calls.clone();
        let server = crate::rpc::test_support::serve(move |request| {
            counter.fetch_add(1, Ordering::SeqCst);
            async move {
                Json(json!({"jsonrpc":"2.0", "id":request["id"], "result":null})).into_response()
            }
        })
        .await;
        let ethereum = test_ethereum(&server.url);
        let config = test_config(FinalityMode::Finalized);
        reconcile_jobs(&pool, &ethereum, &config, 110, Some(&block))
            .await
            .unwrap();
        reconcile_jobs(&pool, &ethereum, &config, 111, Some(&block))
            .await
            .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        // A confirmed row whose state was never applied must remain eligible.
        sqlx::query("DELETE FROM gateway_account_history WHERE job_id = (SELECT id FROM proof_jobs LIMIT 1)").execute(&pool).await.unwrap();
        reconcile_jobs(&pool, &ethereum, &config, 111, Some(&block))
            .await
            .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        // An active submission must be polled even below the finalized head.
        sqlx::query("INSERT INTO proof_jobs (id, kind, status, input, transaction_hash) VALUES (gen_random_uuid(), 'settlement', 'submitted', '{}'::jsonb, $1)")
            .bind(B256::repeat_byte(4).to_string()).execute(&pool).await.unwrap();
        reconcile_jobs(&pool, &ethereum, &config, 111, Some(&block))
            .await
            .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 3);
        let unchanged: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM proof_jobs WHERE completed_at = '2026-09-01'::timestamptz",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(unchanged, 1380);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn confirmation_mode_rechecks_receipts_and_preserves_completion_time(pool: PgPool) {
        let block = BlockRef {
            number: 100,
            hash: B256::repeat_byte(1),
            parent_hash: B256::repeat_byte(2),
        };
        insert_block(&pool, &block).await.unwrap();
        let id = uuid::Uuid::new_v4();
        sqlx::query("INSERT INTO proof_jobs (id, kind, status, input, transaction_hash, submitted_block_number, submitted_block_hash, completed_at)
                     VALUES ($1, 'settlement', 'confirmed', '{}'::jsonb, $2, 100, $3, '2026-09-01'::timestamptz)")
            .bind(id).bind(B256::repeat_byte(3).to_string()).bind(block.hash.to_string()).execute(&pool).await.unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = calls.clone();
        let server = crate::rpc::test_support::serve(move |request| {
            counter.fetch_add(1, Ordering::SeqCst);
            let result = test_receipt(&request, &block, true);
            async move { Json(result).into_response() }
        })
        .await;
        let ethereum = test_ethereum(&server.url);
        let config = test_config(FinalityMode::Confirmations);
        reconcile_jobs(&pool, &ethereum, &config, 112, None)
            .await
            .unwrap();
        let unchanged: bool = sqlx::query_scalar(
            "SELECT completed_at = '2026-09-01'::timestamptz FROM proof_jobs WHERE id = $1",
        )
        .bind(id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(unchanged);
        reconcile_jobs(&pool, &ethereum, &config, 105, None)
            .await
            .unwrap();
        let row = sqlx::query("SELECT status::text AS status, completed_at IS NULL AS unfinished FROM proof_jobs WHERE id = $1").bind(id).fetch_one(&pool).await.unwrap();
        assert_eq!(row.get::<String, _>("status"), "submitted");
        assert!(row.get::<bool, _>("unfinished"));
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn reverted_receipts_only_fail_after_canonical_finality(pool: PgPool) {
        let block = BlockRef {
            number: 100,
            hash: B256::repeat_byte(1),
            parent_hash: B256::repeat_byte(2),
        };
        insert_block(&pool, &block).await.unwrap();
        let id = uuid::Uuid::new_v4();
        sqlx::query("INSERT INTO proof_jobs (id, kind, status, input, transaction_hash) VALUES ($1, 'settlement', 'submitted', '{}'::jsonb, $2)")
            .bind(id).bind(B256::repeat_byte(3).to_string()).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO gateway_pending_commands (job_id, public_key, nonce, command_kind, command_base64) VALUES ($1, 'payer', 0, 'zkapp', 'command')")
            .bind(id).execute(&pool).await.unwrap();
        let served_block = block.clone();
        let server = crate::rpc::test_support::serve(move |request| {
            let result = test_receipt(&request, &served_block, false);
            async move { Json(result).into_response() }
        })
        .await;
        let ethereum = test_ethereum(&server.url);
        let config = test_config(FinalityMode::Finalized);
        let not_finalized = BlockRef {
            number: 99,
            ..block.clone()
        };
        reconcile_jobs(&pool, &ethereum, &config, 120, Some(&not_finalized))
            .await
            .unwrap();
        let status: String =
            sqlx::query_scalar("SELECT status::text FROM proof_jobs WHERE id = $1")
                .bind(id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(status, "submitted");
        sqlx::query("CREATE FUNCTION reject_pending_delete() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'injected pending cleanup failure'; END; $$")
            .execute(&pool).await.unwrap();
        sqlx::query("CREATE TRIGGER reject_pending_delete BEFORE DELETE ON gateway_pending_commands FOR EACH ROW EXECUTE FUNCTION reject_pending_delete()")
            .execute(&pool).await.unwrap();
        let error = reconcile_jobs(&pool, &ethereum, &config, 120, Some(&block))
            .await
            .unwrap_err();
        assert!(error
            .to_string()
            .contains("injected pending cleanup failure"));
        let status: String =
            sqlx::query_scalar("SELECT status::text FROM proof_jobs WHERE id = $1")
                .bind(id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            status, "submitted",
            "failed cleanup must not strand a terminal job"
        );
        let pending: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM gateway_pending_commands WHERE job_id = $1")
                .bind(id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(pending, 1);
        sqlx::query("DROP TRIGGER reject_pending_delete ON gateway_pending_commands")
            .execute(&pool)
            .await
            .unwrap();
        reconcile_jobs(&pool, &ethereum, &config, 120, Some(&block))
            .await
            .unwrap();
        let status: String =
            sqlx::query_scalar("SELECT status::text FROM proof_jobs WHERE id = $1")
                .bind(id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(status, "ethereum_reverted");
        let pending: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM gateway_pending_commands WHERE job_id = $1")
                .bind(id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(pending, 0);
    }

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
    fn settlement_advances_the_virtual_fee_payer_nonce() {
        assert_eq!(fee_payer_nonce_after(2, 1).unwrap(), 3);
        assert!(fee_payer_nonce_after(u64::MAX, 1).is_err());
    }

    #[test]
    fn reconciliation_selects_only_supported_proof_kinds() {
        let source = include_str!("indexer.rs");
        let start = source.find("async fn reconcile_jobs").unwrap();
        let end = source[start..]
            .find("pub(crate) async fn apply_confirmed_settlement")
            .map(|offset| start + offset)
            .unwrap();
        assert!(source[start..end].contains("kind::text IN ('settlement', 'bridge')"));
    }
}
