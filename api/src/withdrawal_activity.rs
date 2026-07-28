use alloy::primitives::{Address as EthereumAddress, U256};
use anyhow::{Context, Result};
use serde::Serialize;
use serde_json::{json, Value};
use sqlx::{PgPool, Row};
use std::time::Duration;
use tokio::time::sleep;
use zeko_sp1_lib::{
    inner_action_commitment, Address, Bytes32, NativeWithdrawalV2, TokenWithdrawalV3,
    ERC20_ACTION_ENCODING_V1, ERC20_ACTION_ENCODING_V2,
};

use crate::AppState;

#[derive(Clone, Debug)]
pub(crate) struct ArchiveInnerAction {
    pub global_action_index: u32,
    pub transaction_hash: String,
    pub block_height: u64,
    pub timestamp: String,
    pub fields: Vec<Bytes32>,
    pub withdrawal: Option<ArchiveWithdrawal>,
}

#[derive(Clone, Debug)]
pub(crate) enum ArchiveWithdrawal {
    Native(ArchiveNativeWithdrawal),
    Token(ArchiveTokenWithdrawal),
}

#[derive(Clone, Debug)]
pub(crate) struct ArchiveNativeWithdrawal {
    pub recipient: Address,
    pub amount: u64,
}

#[derive(Clone, Debug)]
pub(crate) struct ArchiveTokenWithdrawal {
    pub encoding_version: u32,
    pub registry_index: u32,
    pub record_commitment: Bytes32,
    pub asset_id: Bytes32,
    pub recipient: Address,
    pub amount: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PendingWithdrawal {
    pub global_action_index: u32,
    pub transaction_hash: String,
    pub block_height: u64,
    pub timestamp: String,
    pub recipient: String,
    pub amount: String,
    pub status: &'static str,
    pub next_action: &'static str,
}

const ARCHIVE_INNER_ACTIONS_SQL: &str = r#"
WITH action_rows AS (
    SELECT b.height, b.id AS block_id, b.timestamp, command.hash,
           block_command.sequence_no,
           (updates.ordinality - 1)::bigint AS update_ordinal,
           (events.ordinality - 1)::bigint AS action_ordinal,
           public_key.value AS public_key, body.balance_change,
           body.increment_nonce, body.call_depth,
           array_agg(field.field ORDER BY fields.ordinality) AS fields
    FROM blocks b
    JOIN blocks_zkapp_commands block_command ON block_command.block_id = b.id
    JOIN zkapp_commands command ON command.id = block_command.zkapp_command_id
    CROSS JOIN LATERAL unnest(command.zkapp_account_updates_ids)
        WITH ORDINALITY AS updates(body_id, ordinality)
    JOIN zkapp_account_update_body body ON body.id = updates.body_id
    JOIN account_identifiers identifier ON identifier.id = body.account_identifier_id
    JOIN public_keys public_key ON public_key.id = identifier.public_key_id
    JOIN zkapp_events actions ON actions.id = body.actions_id
    CROSS JOIN LATERAL unnest(actions.element_ids)
        WITH ORDINALITY AS events(field_array_id, ordinality)
    JOIN zkapp_field_array field_array ON field_array.id = events.field_array_id
    CROSS JOIN LATERAL unnest(field_array.element_ids)
        WITH ORDINALITY AS fields(field_id, ordinality)
    JOIN zkapp_field field ON field.id = fields.field_id
    WHERE b.chain_status = 'canonical' AND block_command.status = 'applied'
    GROUP BY b.height, b.id, b.timestamp, command.hash,
             block_command.sequence_no, updates.ordinality, events.ordinality,
             public_key.value, body.balance_change, body.increment_nonce,
             body.call_depth
),
numbered_inner_actions AS (
    SELECT action_rows.*,
           (row_number() OVER (
               ORDER BY height, block_id, sequence_no, update_ordinal,
                        action_ordinal
           ) - 1)::bigint AS global_action_index
    FROM action_rows
    WHERE public_key = $1
)
SELECT inner_action.global_action_index, inner_action.hash,
       inner_action.height, inner_action.timestamp, inner_action.fields,
       withdrawal.fields AS withdrawal_fields
FROM numbered_inner_actions inner_action
LEFT JOIN LATERAL (
    SELECT candidate.fields
    FROM action_rows candidate
    WHERE candidate.hash = inner_action.hash
      AND candidate.update_ordinal < inner_action.update_ordinal
      AND candidate.balance_change::numeric < 0
      AND (
        (candidate.increment_nonce AND candidate.call_depth = 0
         AND cardinality(candidate.fields) = 3)
        OR cardinality(candidate.fields) IN (11, 14)
      )
    ORDER BY (cardinality(candidate.fields) IN (11, 14)) DESC,
             candidate.update_ordinal DESC, candidate.action_ordinal DESC
    LIMIT 1
) withdrawal ON TRUE
ORDER BY inner_action.global_action_index
"#;

const ARCHIVE_ACTION_BLOCKS_SQL: &str = r#"
WITH account_states AS (
    SELECT b.id AS block_id, b.height,
           current_state.field AS action_state_after,
           COALESCE(
             lag(current_state.field) OVER (ORDER BY b.height, b.id),
             previous_state.field
           ) AS action_state_before
    FROM accounts_accessed accessed
    JOIN blocks b ON b.id = accessed.block_id
    JOIN account_identifiers identifier
      ON identifier.id = accessed.account_identifier_id
    JOIN public_keys public_key ON public_key.id = identifier.public_key_id
    JOIN zkapp_accounts account ON account.id = accessed.zkapp_id
    JOIN zkapp_action_states states ON states.id = account.action_state_id
    JOIN zkapp_field current_state ON current_state.id = states.element0
    JOIN zkapp_field previous_state ON previous_state.id = states.element1
    WHERE public_key.value = $1
      AND b.chain_status IN ('canonical', 'pending')
),
action_items AS (
    SELECT b.id AS block_id, b.height, b.timestamp,
           b.chain_status::text AS chain_status,
           block_command.sequence_no,
           (updates.ordinality - 1)::bigint AS update_ordinal,
           (events.ordinality - 1)::bigint AS action_ordinal,
           command.hash,
           array_agg(field.field ORDER BY fields.ordinality) AS fields
    FROM blocks b
    JOIN blocks_zkapp_commands block_command ON block_command.block_id = b.id
    JOIN zkapp_commands command ON command.id = block_command.zkapp_command_id
    CROSS JOIN LATERAL unnest(command.zkapp_account_updates_ids)
      WITH ORDINALITY AS updates(body_id, ordinality)
    JOIN zkapp_account_update_body body ON body.id = updates.body_id
    JOIN account_identifiers identifier ON identifier.id = body.account_identifier_id
    JOIN public_keys public_key ON public_key.id = identifier.public_key_id
    JOIN zkapp_events actions ON actions.id = body.actions_id
    CROSS JOIN LATERAL unnest(actions.element_ids)
      WITH ORDINALITY AS events(field_array_id, ordinality)
    JOIN zkapp_field_array field_array ON field_array.id = events.field_array_id
    CROSS JOIN LATERAL unnest(field_array.element_ids)
      WITH ORDINALITY AS fields(field_id, ordinality)
    JOIN zkapp_field field ON field.id = fields.field_id
    WHERE public_key.value = $1
      AND b.chain_status IN ('canonical', 'pending')
      AND b.height >= $2 AND b.height < $3
    GROUP BY b.id, b.height, b.timestamp, b.chain_status,
             block_command.sequence_no, updates.ordinality,
             events.ordinality, command.hash
),
action_blocks AS (
    SELECT block_id, height, timestamp, chain_status,
           jsonb_agg(
             jsonb_build_object(
               'accountUpdateId', update_ordinal::text,
               'data', to_jsonb(fields),
               'transactionInfo', jsonb_build_object('hash', hash)
             ) ORDER BY sequence_no, update_ordinal, action_ordinal
           ) AS action_data
    FROM action_items
    GROUP BY block_id, height, timestamp, chain_status
)
SELECT block.block_id, block.height, block.timestamp, block.chain_status,
       states.action_state_after, states.action_state_before,
       block.action_data
FROM action_blocks block
JOIN account_states states ON states.block_id = block.block_id
ORDER BY block.height, block.block_id
"#;

pub(crate) async fn archive_network_state(state: &AppState) -> Result<Value> {
    let archive = state
        .archive_pool
        .as_ref()
        .context("Zeko archive is not configured")?;
    let row = sqlx::query(
        "SELECT
           COALESCE(MAX(height) FILTER (WHERE chain_status = 'canonical'), 0)::bigint
             AS canonical_height,
           COALESCE(MAX(height) FILTER (
             WHERE chain_status IN ('canonical', 'pending')
           ), 0)::bigint AS pending_height
         FROM blocks",
    )
    .fetch_one(archive)
    .await?;
    Ok(json!({
        "networkState": {
            "maxBlockHeight": {
                "canonicalMaxBlockHeight": row.try_get::<i64, _>("canonical_height")?,
                "pendingMaxBlockHeight": row.try_get::<i64, _>("pending_height")?
            }
        }
    }))
}

pub(crate) async fn archive_actions(
    state: &AppState,
    public_key: &str,
    from: i64,
    to: i64,
) -> Result<Value> {
    let archive = state
        .archive_pool
        .as_ref()
        .context("Zeko archive is not configured")?;
    let pending_height = sqlx::query_scalar::<_, Option<i64>>(
        "SELECT MAX(height) FROM blocks
         WHERE chain_status IN ('canonical', 'pending')",
    )
    .fetch_one(archive)
    .await?
    .unwrap_or(0);
    let rows = sqlx::query(ARCHIVE_ACTION_BLOCKS_SQL)
        .bind(public_key)
        .bind(from)
        .bind(to)
        .fetch_all(archive)
        .await?;
    let actions = rows
        .into_iter()
        .map(|row| -> Result<Value> {
            let height: i64 = row.try_get("height")?;
            Ok(json!({
                "blockInfo": {
                    "timestamp": row.try_get::<String, _>("timestamp")?,
                    "height": height,
                    "distanceFromMaxBlockHeight": pending_height.saturating_sub(height),
                    "chainStatus": row.try_get::<String, _>("chain_status")?
                },
                "actionState": {
                    "actionStateOne": row.try_get::<String, _>("action_state_after")?,
                    "actionStateTwo": row.try_get::<String, _>("action_state_before")?
                },
                "actionData": row.try_get::<Value, _>("action_data")?
            }))
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(json!({"actions": actions}))
}

pub(crate) async fn load_archive_inner_actions(
    archive: &PgPool,
    inner_public_key: &str,
) -> Result<Vec<ArchiveInnerAction>> {
    let rows = sqlx::query(ARCHIVE_INNER_ACTIONS_SQL)
        .bind(inner_public_key)
        .fetch_all(archive)
        .await
        .context("read canonical inner actions from the Zeko archive")?;

    rows.into_iter()
        .map(|row| {
            let fields = parse_fields(row.try_get::<Vec<String>, _>("fields")?)?;
            let withdrawal = row
                .try_get::<Option<Vec<String>>, _>("withdrawal_fields")?
                .map(parse_withdrawal)
                .transpose()?;
            Ok(ArchiveInnerAction {
                global_action_index: u32::try_from(row.try_get::<i64, _>("global_action_index")?)?,
                transaction_hash: row.try_get("hash")?,
                block_height: u64::try_from(row.try_get::<i64, _>("height")?)?,
                timestamp: row.try_get("timestamp")?,
                fields,
                withdrawal,
            })
        })
        .collect()
}

pub(crate) async fn pending_withdrawals(state: &AppState) -> Result<Vec<PendingWithdrawal>> {
    let archive = state
        .archive_pool
        .as_ref()
        .context("Zeko archive is not configured")?;
    let inner_public_key = state
        .inner_public_key
        .as_deref()
        .context("VIRTUAL_MINA_INNER_PUBLIC_KEY is not configured")?;
    let actions = load_archive_inner_actions(archive, inner_public_key).await?;
    let settled = sqlx::query_scalar::<_, i64>(
        "SELECT global_action_index FROM gateway_inner_action_leaves WHERE NOT removed",
    )
    .fetch_all(&state.pool)
    .await?
    .into_iter()
    .collect::<std::collections::HashSet<_>>();

    Ok(actions
        .into_iter()
        .filter(|action| !settled.contains(&i64::from(action.global_action_index)))
        .filter_map(|action| match action.withdrawal {
            Some(ArchiveWithdrawal::Native(withdrawal)) => Some(PendingWithdrawal {
                global_action_index: action.global_action_index,
                transaction_hash: action.transaction_hash,
                block_height: action.block_height,
                timestamp: action.timestamp,
                recipient: EthereumAddress::from(withdrawal.recipient).to_string(),
                amount: withdrawal.amount.to_string(),
                status: "pendingSettlement",
                next_action: "waitForSettlement",
            }),
            Some(ArchiveWithdrawal::Token(_)) | None => None,
        })
        .collect())
}

pub(crate) async fn recovery_loop(state: AppState, interval: Duration) {
    loop {
        if let Err(error) = recover_settled_inner_actions(&state).await {
            tracing::warn!(%error, "could not recover settled inner actions from archive");
        }
        sleep(interval).await;
    }
}

async fn recover_settled_inner_actions(state: &AppState) -> Result<()> {
    let Some(archive) = state.archive_pool.as_ref() else {
        return Ok(());
    };
    let Some(inner_public_key) = state.inner_public_key.as_deref() else {
        return Ok(());
    };
    let actions = load_archive_inner_actions(archive, inner_public_key).await?;
    let settlements = sqlx::query(
        "SELECT batch_sequence, inner_action_root, inner_action_start_index,
                inner_action_count, slot_upper, ethereum_block_number,
                ethereum_block_hash, ethereum_tx_hash
         FROM gateway_explorer_settlements settlements
         WHERE NOT removed AND inner_action_count > 0
           AND NOT EXISTS (
             SELECT 1 FROM gateway_inner_action_leaves leaves
             WHERE leaves.settlement_sequence = settlements.batch_sequence
               AND NOT leaves.removed
           )
         ORDER BY batch_sequence",
    )
    .fetch_all(&state.pool)
    .await?;
    let chain_id = state.ethereum.chain_id().await?;
    let bridge = state.ethereum.bridge_address().into_array();

    for settlement in settlements {
        let sequence = u64::try_from(settlement.try_get::<i64, _>("batch_sequence")?)?;
        let start = u32::try_from(settlement.try_get::<i64, _>("inner_action_start_index")?)?;
        let count = u32::try_from(settlement.try_get::<i64, _>("inner_action_count")?)?;
        let end = start
            .checked_add(count)
            .context("inner action range overflow")?;
        let batch = actions
            .iter()
            .filter(|action| (start..end).contains(&action.global_action_index))
            .collect::<Vec<_>>();
        if batch.len() != usize::try_from(count)? {
            tracing::debug!(
                sequence,
                start,
                count,
                available = batch.len(),
                "archive has not caught up to an accepted inner-action batch"
            );
            continue;
        }
        recover_batch(
            state,
            sequence,
            start,
            &batch,
            chain_id,
            bridge,
            &settlement,
        )
        .await
        .with_context(|| format!("recover settlement {sequence} inner-action batch"))?;
    }
    Ok(())
}

async fn recover_batch(
    state: &AppState,
    sequence: u64,
    start: u32,
    actions: &[&ArchiveInnerAction],
    chain_id: u64,
    bridge: Address,
    settlement: &sqlx::postgres::PgRow,
) -> Result<()> {
    let expected_root = parse_hex_bytes32(settlement.try_get("inner_action_root")?)?;
    let mut rows = Vec::with_capacity(actions.len());
    for (offset, action) in actions.iter().enumerate() {
        let expected_index = start
            .checked_add(u32::try_from(offset)?)
            .context("inner action index overflow")?;
        anyhow::ensure!(
            action.global_action_index == expected_index,
            "archive inner actions are not contiguous"
        );
        let action_fields_hash = inner_action_commitment::action_fields_hash(&action.fields);
        let token = match &action.withdrawal {
            Some(ArchiveWithdrawal::Token(withdrawal)) => Some(
                state
                    .ethereum
                    .resolve_token_withdrawal_identity(
                        withdrawal.encoding_version,
                        withdrawal.registry_index,
                        withdrawal.record_commitment.into(),
                        withdrawal.asset_id.into(),
                    )
                    .await?
                    .into_array(),
            ),
            Some(ArchiveWithdrawal::Native(_)) | None => None,
        };
        let leaf = archive_action_leaf(
            chain_id,
            bridge,
            action.global_action_index,
            action_fields_hash,
            action.withdrawal.as_ref(),
            token,
        )?;
        rows.push((offset, action, action_fields_hash, leaf, token));
    }
    let leaves = rows.iter().map(|row| row.3).collect::<Vec<_>>();
    anyhow::ensure!(
        inner_action_commitment::root(&leaves) == expected_root,
        "archive actions do not reproduce the Ethereum-accepted inner-action root"
    );

    let block_number = settlement.try_get::<i64, _>("ethereum_block_number")?;
    let block_hash = settlement.try_get::<String, _>("ethereum_block_hash")?;
    let transaction_hash = settlement.try_get::<String, _>("ethereum_tx_hash")?;
    let slot_upper = settlement.try_get::<i64, _>("slot_upper")?;
    let mut tx = state.pool.begin().await?;
    for (offset, action, action_fields_hash, leaf, token) in rows {
        let (
            token,
            asset_id,
            action_encoding_version,
            registry_index,
            record_commitment,
            recipient,
            amount,
        ) = match (&action.withdrawal, token) {
            (Some(ArchiveWithdrawal::Native(withdrawal)), None) => (
                None,
                None,
                None,
                None,
                None,
                Some(EthereumAddress::from(withdrawal.recipient).to_string()),
                Some(withdrawal.amount.to_string()),
            ),
            (Some(ArchiveWithdrawal::Token(withdrawal)), Some(token)) => (
                Some(EthereumAddress::from(token).to_string()),
                Some(format!("0x{}", hex::encode(withdrawal.asset_id))),
                Some(i32::try_from(withdrawal.encoding_version)?),
                (withdrawal.encoding_version == ERC20_ACTION_ENCODING_V2)
                    .then_some(i64::from(withdrawal.registry_index)),
                (withdrawal.encoding_version == ERC20_ACTION_ENCODING_V2)
                    .then(|| format!("0x{}", hex::encode(withdrawal.record_commitment))),
                Some(EthereumAddress::from(withdrawal.recipient).to_string()),
                Some(withdrawal.amount.to_string()),
            ),
            (None, None) => (None, None, None, None, None, None, None),
            _ => anyhow::bail!("archive withdrawal token classification is inconsistent"),
        };
        let field_json = action
            .fields
            .iter()
            .map(|field| format!("0x{}", hex::encode(field)))
            .collect::<Vec<_>>();
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
        .bind(i64::try_from(sequence)?)
        .bind(i32::try_from(offset)?)
        .bind(i64::from(action.global_action_index))
        .bind(serde_json::to_value(field_json)?)
        .bind(format!("0x{}", hex::encode(action_fields_hash)))
        .bind(format!("0x{}", hex::encode(leaf)))
        .bind(token)
        .bind(asset_id)
        .bind(action_encoding_version)
        .bind(registry_index)
        .bind(record_commitment)
        .bind(recipient)
        .bind(amount)
        .bind(format!("0x{}", hex::encode(expected_root)))
        .bind(slot_upper)
        .bind(block_number)
        .bind(&block_hash)
        .bind(&transaction_hash)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    tracing::info!(
        sequence,
        count = actions.len(),
        "recovered settlement inner-action leaves from archive and Ethereum"
    );
    Ok(())
}

fn archive_action_leaf(
    chain_id: u64,
    bridge: Address,
    global_action_index: u32,
    action_fields_hash: Bytes32,
    withdrawal: Option<&ArchiveWithdrawal>,
    token: Option<Address>,
) -> Result<Bytes32> {
    match (withdrawal, token) {
        (Some(ArchiveWithdrawal::Native(withdrawal)), None) => {
            Ok(inner_action_commitment::native_withdrawal_leaf(
                chain_id,
                bridge,
                global_action_index,
                &NativeWithdrawalV2 {
                    recipient: withdrawal.recipient,
                    amount: withdrawal.amount,
                },
                action_fields_hash,
            ))
        }
        (Some(ArchiveWithdrawal::Token(withdrawal)), Some(token)) => {
            Ok(inner_action_commitment::erc20_withdrawal_leaf(
                chain_id,
                bridge,
                global_action_index,
                &TokenWithdrawalV3 {
                    encoding_version: withdrawal.encoding_version,
                    registry_index: withdrawal.registry_index,
                    record_commitment: withdrawal.record_commitment,
                    token,
                    asset_id: withdrawal.asset_id,
                    recipient: withdrawal.recipient,
                    amount: withdrawal.amount,
                    params_fields: Vec::new(),
                },
                action_fields_hash,
            ))
        }
        (None, None) => Ok(inner_action_commitment::raw_inner_action_leaf(
            chain_id,
            bridge,
            global_action_index,
            action_fields_hash,
        )),
        _ => anyhow::bail!("archive withdrawal token classification is inconsistent"),
    }
}

fn parse_fields(fields: Vec<String>) -> Result<Vec<Bytes32>> {
    fields
        .into_iter()
        .map(|field| {
            let value = U256::from_str_radix(&field, 10)
                .with_context(|| format!("invalid archive action field {field}"))?;
            Ok(value.to_be_bytes())
        })
        .collect()
}

fn parse_withdrawal(fields: Vec<String>) -> Result<ArchiveWithdrawal> {
    if fields.len() == 3 {
        return parse_native_withdrawal(fields).map(ArchiveWithdrawal::Native);
    }
    parse_token_withdrawal(fields).map(ArchiveWithdrawal::Token)
}

fn parse_native_withdrawal(fields: Vec<String>) -> Result<ArchiveNativeWithdrawal> {
    anyhow::ensure!(
        fields.len() == 3,
        "withdrawal action must have three fields"
    );
    let recipient = U256::from_str_radix(&fields[0], 10)?;
    let parity = U256::from_str_radix(&fields[1], 10)?;
    let amount = U256::from_str_radix(&fields[2], 10)?;
    anyhow::ensure!(
        parity.is_zero(),
        "Ethereum withdrawal recipient must use even parity"
    );
    // Ethereum addresses are 160 bits. Checking the high 96 bits avoids a lossy cast.
    let recipient_bytes: Bytes32 = recipient.to_be_bytes();
    anyhow::ensure!(
        recipient_bytes[..12].iter().all(|byte| *byte == 0),
        "withdrawal recipient does not fit 160 bits"
    );
    anyhow::ensure!(
        amount > U256::ZERO && amount <= U256::from(u64::MAX),
        "withdrawal amount is outside UInt64"
    );
    let mut address = [0u8; 20];
    address.copy_from_slice(&recipient_bytes[12..]);
    Ok(ArchiveNativeWithdrawal {
        recipient: address,
        amount: amount.to::<u64>(),
    })
}

fn parse_token_withdrawal(fields: Vec<String>) -> Result<ArchiveTokenWithdrawal> {
    anyhow::ensure!(
        matches!(fields.len(), 11 | 14),
        "invalid ERC20 withdrawal parameter width"
    );
    let fields = fields
        .into_iter()
        .map(|field| U256::from_str_radix(&field, 10))
        .collect::<Result<Vec<_>, _>>()?;
    let (encoding_version, registry_index, record_commitment, asset_offset) = if fields.len() == 14
    {
        anyhow::ensure!(
            fields[0] == U256::from(ERC20_ACTION_ENCODING_V2),
            "unsupported ERC20 withdrawal encoding version"
        );
        let registry_index =
            u32::try_from(fields[1]).context("ERC20 registry index is outside UInt32")?;
        let record_commitment = fields[2].to_be_bytes();
        anyhow::ensure!(
            record_commitment != [0u8; 32],
            "ERC20 record commitment is zero"
        );
        (
            ERC20_ACTION_ENCODING_V2,
            registry_index,
            record_commitment,
            3,
        )
    } else {
        (ERC20_ACTION_ENCODING_V1, 0, [0u8; 32], 0)
    };
    let asset_high: Bytes32 = fields[asset_offset].to_be_bytes();
    let asset_low: Bytes32 = fields[asset_offset + 1].to_be_bytes();
    anyhow::ensure!(
        asset_high[..16].iter().all(|byte| *byte == 0)
            && asset_low[..16].iter().all(|byte| *byte == 0),
        "ERC20 asset limbs exceed 128 bits"
    );
    let mut asset_id = [0u8; 32];
    asset_id[..16].copy_from_slice(&asset_high[16..]);
    asset_id[16..].copy_from_slice(&asset_low[16..]);
    anyhow::ensure!(asset_id != [0u8; 32], "ERC20 asset id is zero");

    let amount = fields[fields.len() - 3];
    let recipient = fields[fields.len() - 2];
    let parity = fields[fields.len() - 1];
    anyhow::ensure!(
        parity.is_zero(),
        "Ethereum withdrawal recipient must use even parity"
    );
    let recipient_bytes: Bytes32 = recipient.to_be_bytes();
    anyhow::ensure!(
        recipient_bytes[..12].iter().all(|byte| *byte == 0),
        "withdrawal recipient does not fit 160 bits"
    );
    anyhow::ensure!(
        recipient_bytes[12..].iter().any(|byte| *byte != 0),
        "withdrawal recipient is zero"
    );
    anyhow::ensure!(
        amount > U256::ZERO && amount <= U256::from(u64::MAX),
        "withdrawal amount is outside UInt64"
    );
    let mut recipient = [0u8; 20];
    recipient.copy_from_slice(&recipient_bytes[12..]);
    Ok(ArchiveTokenWithdrawal {
        encoding_version,
        registry_index,
        record_commitment,
        asset_id,
        recipient,
        amount: amount.to::<u64>(),
    })
}

fn parse_hex_bytes32(value: String) -> Result<Bytes32> {
    let bytes = hex::decode(value.strip_prefix("0x").unwrap_or(&value))?;
    bytes
        .try_into()
        .map_err(|_| anyhow::anyhow!("expected 32-byte hex value"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_native_withdrawal_sender_action() {
        let withdrawal = parse_withdrawal(vec![
            "1390849295786071768276380950238675083608645509734".into(),
            "0".into(),
            "5000000000".into(),
        ])
        .unwrap();
        let ArchiveWithdrawal::Native(withdrawal) = withdrawal else {
            panic!("expected native withdrawal");
        };
        assert_eq!(withdrawal.amount, 5_000_000_000);
        assert_eq!(
            EthereumAddress::from(withdrawal.recipient).to_string(),
            "0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266"
        );
    }

    #[test]
    fn rejects_non_ethereum_recipient_parity() {
        assert!(parse_withdrawal(vec!["1".into(), "1".into(), "2".into()]).is_err());
    }

    #[test]
    fn parses_registry_erc20_withdrawal_parameters() {
        let fields = [
            "2", "7", "991", "1", "2", "0", "1", "1", "123", "0", "0", "2000000", "16909060", "0",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect();

        let withdrawal = parse_withdrawal(fields).unwrap();
        let ArchiveWithdrawal::Token(withdrawal) = withdrawal else {
            panic!("expected token withdrawal");
        };
        let mut asset_id = [0u8; 32];
        asset_id[15] = 1;
        asset_id[31] = 2;
        let mut recipient = [0u8; 20];
        recipient[16..].copy_from_slice(&[1, 2, 3, 4]);
        assert_eq!(withdrawal.encoding_version, ERC20_ACTION_ENCODING_V2);
        assert_eq!(withdrawal.registry_index, 7);
        assert_eq!(withdrawal.record_commitment, U256::from(991).to_be_bytes());
        assert_eq!(withdrawal.asset_id, asset_id);
        assert_eq!(withdrawal.recipient, recipient);
        assert_eq!(withdrawal.amount, 2_000_000);

        let token = [0x33; 20];
        let action_fields_hash = [0x44; 32];
        let actual = archive_action_leaf(
            31_337,
            [0x55; 20],
            9,
            action_fields_hash,
            Some(&ArchiveWithdrawal::Token(withdrawal.clone())),
            Some(token),
        )
        .unwrap();
        let expected = inner_action_commitment::erc20_withdrawal_leaf(
            31_337,
            [0x55; 20],
            9,
            &TokenWithdrawalV3 {
                encoding_version: withdrawal.encoding_version,
                registry_index: withdrawal.registry_index,
                record_commitment: withdrawal.record_commitment,
                token,
                asset_id: withdrawal.asset_id,
                recipient: withdrawal.recipient,
                amount: withdrawal.amount,
                params_fields: Vec::new(),
            },
            action_fields_hash,
        );
        assert_eq!(actual, expected);
    }

    #[test]
    fn parses_legacy_erc20_withdrawal_parameters() {
        let fields = [
            "2", "3", "0", "1", "1", "123", "0", "0", "2000000", "16909060", "0",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect();
        let ArchiveWithdrawal::Token(withdrawal) = parse_withdrawal(fields).unwrap() else {
            panic!("expected token withdrawal");
        };
        assert_eq!(withdrawal.encoding_version, ERC20_ACTION_ENCODING_V1);
        assert_eq!(withdrawal.registry_index, 0);
        assert_eq!(withdrawal.record_commitment, [0u8; 32]);
    }
}
