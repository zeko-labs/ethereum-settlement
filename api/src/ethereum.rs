use alloy::{
    consensus::Transaction,
    eips::BlockNumberOrTag,
    network::EthereumWallet,
    primitives::{Address, Bytes, TxHash, B256, U256},
    providers::{Provider, ProviderBuilder},
    rpc::types::Filter,
    signers::local::PrivateKeySigner,
    sol,
    sol_types::{SolCall, SolEvent},
};
use anyhow::{Context, Result};
use std::{
    collections::{HashMap, HashSet},
    str::FromStr,
};
use zeko_sp1_lib::ERC20_ACTION_ENCODING_V2;

use crate::proof_kind::ProofKind;

pub const HISTORICAL_ERC20_ACTION_ENCODING_V1: u32 = 1;

sol! {
    #[sol(rpc)]
    interface IZekoSettlement {
        function verifier() external view returns (address);
        function programVKey() external view returns (bytes32);
        function vkHash() external view returns (bytes32);
        function actionState() external view returns (bytes32);
        function currentRoot() external view returns (bytes32);
        function outerState() external view returns (bytes32[8]);
        function outerActionStateLength() external view returns (uint32);
        function batchSequence() external view returns (uint64);
        function currentVirtualSlot() external view returns (uint64);
        function verifyAndUpdateRoot(bytes publicValues, bytes proofBytes) external;
        event SettlementAccepted(
            uint64 indexed batchSequence,
            bytes32 indexed minaTransactionHash,
            bytes32 indexed ledgerHash,
            bytes32 outerActionState,
            uint32 outerActionStateLength,
            bytes32 innerActionState,
            uint32 innerActionStateLength,
            uint32 slotLower,
            uint32 slotUpper
        );
        event InnerActionBatchAccepted(
            uint64 indexed batchSequence,
            bytes32 indexed stateAfter,
            bytes32 indexed root,
            uint32 startIndex,
            uint32 count,
            uint32 claimableSlot
        );
    }

    #[sol(rpc)]
    interface IEthereumZekoBridge {
        function bridgeVerifier() external view returns (address);
        function bridgeProgramVKey() external view returns (bytes32);
        function depositNonce() external view returns (uint64);
        function currentDepositState() external view returns (bytes32);
        function bridgedDepositNonce() external view returns (uint64);
        function withdrawalDelaySlots() external view returns (uint32);
        function nextWithdrawalIndex(address recipient) external view returns (uint32);
        function nextTokenWithdrawalIndex(address token, address recipient) external view returns (uint32);
        function canonicalTokenRegistered(address token) external view returns (bool);
        function assetIdByToken(address token) external view returns (bytes32);
        function registryIndexByToken(address token) external view returns (uint32);
        function recordCommitmentByToken(address token) external view returns (bytes32);
        function assetTokenByRegistryIndex(uint32 registryIndex) external view returns (address);
        function processedActionState(bytes32 actionState) external view returns (bool);
        function paused() external view returns (bool);
        function depositStateByNonce(uint64 nonce) external view returns (bytes32);
        function submitBridgeTransition(bytes publicValues, bytes proofBytes) external;
        event BridgeDeposit(
            uint64 indexed nonce,
            bytes32 indexed depositLeaf,
            bytes32 indexed newDepositState,
            bytes32 oldDepositState,
            address token,
            address sender,
            uint256 zekoRecipient,
            uint256 amount,
            uint256 zekoAmount,
            uint64 timeout
        );
        event ERC20DepositSubmitted(
            uint64 indexed nonce,
            bytes32 indexed assetId,
            bytes32 indexed depositLeaf,
            bytes32 newDepositState,
            address token,
            address sender,
            uint256 zekoRecipient,
            uint64 amount,
            uint64 timeout
        );
        event ERC20DepositSubmittedV2(
            uint64 indexed nonce,
            bytes32 indexed assetId,
            bytes32 indexed depositLeaf,
            bytes32 newDepositState,
            address token,
            address sender,
            uint256 zekoRecipient,
            uint64 amount,
            uint64 timeout,
            uint32 encodingVersion,
            uint32 registryIndex,
            bytes32 recordCommitment
        );
        event NativeWithdrawalClaimed(
            uint64 indexed settlementSequence,
            uint32 indexed globalActionIndex,
            address indexed recipient,
            uint64 zekoAmount,
            uint256 ethereumAmount,
            bytes32 actionFieldsHash
        );
        event TokenRegistered(
            address indexed token,
            bytes32 indexed assetId,
            bytes32 zekoTokenOwner,
            bytes32 indexed zekoTokenId,
            uint8 zekoDecimals,
            uint64 depositCap
        );
        event BridgeTransitionAccepted(
            bytes32 indexed oldActionState,
            bytes32 indexed newActionState,
            bytes32 indexed newDepositState,
            uint64 newDepositNonce
        );
    }

    #[sol(rpc)]
    interface ILocalSP1Verifier {
        function isLocalSP1Verifier() external view returns (bool);
    }
}

mod legacy_bridge_events {
    use alloy::sol;

    sol! {
        event BridgeTransitionAccepted(
            bytes32 indexed oldActionState,
            bytes32 indexed newActionState,
            bytes32 indexed newDepositState,
            bytes32 newWithdrawState,
            uint64 newDepositNonce
        );
    }
}

#[derive(Clone)]
pub struct Ethereum {
    rpc_url: String,
    settlement_address: Address,
    bridge_address: Address,
    settlement_key: String,
    bridge_key: String,
}

pub struct SettlementState {
    pub program_vkey: B256,
    pub vk_hash: B256,
    pub action_state: B256,
    pub current_root: B256,
    pub outer_state: [B256; 8],
    pub outer_action_state_length: u32,
    pub batch_sequence: u64,
}

pub struct BridgeState {
    pub program_vkey: B256,
    pub deposit_nonce: u64,
    pub current_deposit_state: B256,
    pub bridged_deposit_nonce: u64,
    pub action_state_processed: Option<bool>,
    pub paused: bool,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct TokenWithdrawalIdentity {
    pub encoding_version: u32,
    pub registry_index: u32,
    pub record_commitment: B256,
    pub asset_id: B256,
}

#[derive(Clone, Debug)]
pub struct BridgeDepositLog {
    pub nonce: u64,
    pub deposit_leaf: B256,
    pub new_deposit_state: B256,
    pub old_deposit_state: B256,
    pub token: Address,
    pub sender: Address,
    pub zeko_recipient: B256,
    pub amount: U256,
    pub zeko_amount: U256,
    pub timeout: u64,
    pub asset_id: Option<B256>,
    pub action_encoding_version: u32,
    pub registry_index: Option<u32>,
    pub record_commitment: Option<B256>,
    pub block_number: u64,
    pub block_hash: B256,
    pub transaction_hash: TxHash,
    pub log_index: u64,
}

#[derive(Clone, Debug)]
struct ERC20DepositMetadata {
    asset_id: B256,
    deposit_leaf: B256,
    new_deposit_state: B256,
    token: Address,
    sender: Address,
    zeko_recipient: B256,
    amount: u64,
    timeout: u64,
    registry_index: Option<u32>,
    record_commitment: Option<B256>,
}

fn classify_erc20_deposit_identity(
    registry_index: Option<u32>,
    record_commitment: Option<B256>,
) -> Result<(u32, Option<u32>, Option<B256>)> {
    match (registry_index, record_commitment) {
        (None, None) => Ok((HISTORICAL_ERC20_ACTION_ENCODING_V1, None, None)),
        (Some(registry_index), Some(record_commitment)) => Ok((
            ERC20_ACTION_ENCODING_V2,
            Some(registry_index),
            Some(record_commitment),
        )),
        _ => anyhow::bail!("ERC20 deposit has incomplete registry identity"),
    }
}

fn bridge_deposit_filter(bridge_address: Address, from_block: u64, to_block: u64) -> Filter {
    Filter::new()
        .address(bridge_address)
        .event_signature(vec![
            IEthereumZekoBridge::BridgeDeposit::SIGNATURE_HASH,
            IEthereumZekoBridge::ERC20DepositSubmitted::SIGNATURE_HASH,
            IEthereumZekoBridge::ERC20DepositSubmittedV2::SIGNATURE_HASH,
        ])
        .from_block(from_block)
        .to_block(to_block)
}

fn bridge_transition_filter(bridge_address: Address, from_block: u64, to_block: u64) -> Filter {
    Filter::new()
        .address(bridge_address)
        .event_signature(vec![
            IEthereumZekoBridge::BridgeTransitionAccepted::SIGNATURE_HASH,
            legacy_bridge_events::BridgeTransitionAccepted::SIGNATURE_HASH,
        ])
        .from_block(from_block)
        .to_block(to_block)
}

#[derive(Clone, Debug)]
pub struct SettlementAcceptedLog {
    pub batch_sequence: u64,
    pub mina_transaction_hash: B256,
    pub ledger_hash: B256,
    pub outer_action_state: B256,
    pub outer_action_state_length: u32,
    pub inner_action_state: B256,
    pub inner_action_state_length: u32,
    pub slot_lower: u32,
    pub slot_upper: u32,
    pub block_number: u64,
    pub block_hash: B256,
    pub transaction_hash: TxHash,
    pub log_index: u64,
}

#[derive(Clone, Debug)]
pub struct InnerActionBatchAcceptedLog {
    pub batch_sequence: u64,
    pub state_after: B256,
    pub root: B256,
    pub start_index: u32,
    pub count: u32,
    pub claimable_slot: u32,
    pub transaction_hash: TxHash,
}

#[derive(Clone, Debug)]
pub struct NativeWithdrawalClaimedLog {
    pub settlement_sequence: u64,
    pub global_action_index: u32,
    pub recipient: Address,
    pub zeko_amount: u64,
    pub ethereum_amount: U256,
    pub action_fields_hash: B256,
    pub block_number: u64,
    pub block_hash: B256,
    pub transaction_hash: TxHash,
    pub log_index: u64,
}

#[derive(Clone, Debug)]
pub struct BridgeTransitionAcceptedLog {
    pub old_action_state: B256,
    pub new_action_state: B256,
    pub new_deposit_state: B256,
    pub new_deposit_nonce: u64,
    pub block_number: u64,
    pub block_hash: B256,
    pub transaction_hash: TxHash,
    pub log_index: u64,
}

#[derive(Clone, Debug)]
pub struct BlockRef {
    pub number: u64,
    pub hash: B256,
    pub parent_hash: B256,
}

#[derive(Clone, Debug)]
pub struct TransactionReceiptRef {
    pub block_number: u64,
    pub block_hash: B256,
    pub gas_used: u64,
    pub succeeded: bool,
}

impl Ethereum {
    pub fn new(
        rpc_url: String,
        settlement_address: String,
        bridge_address: String,
        settlement_key: String,
        bridge_key: String,
    ) -> Result<Self> {
        anyhow::ensure!(
            !settlement_key.is_empty(),
            "SETTLEMENT_PRIVATE_KEY is required"
        );
        anyhow::ensure!(!bridge_key.is_empty(), "BRIDGE_PRIVATE_KEY is required");
        Ok(Self {
            rpc_url,
            settlement_address: settlement_address
                .parse()
                .context("invalid settlement address")?,
            bridge_address: bridge_address.parse().context("invalid bridge address")?,
            settlement_key,
            bridge_key,
        })
    }

    pub async fn chain_id(&self) -> Result<u64> {
        Ok(ProviderBuilder::new()
            .connect_http(self.rpc_url.parse()?)
            .get_chain_id()
            .await?)
    }

    pub async fn ensure_local_mock_verifiers(
        &self,
        unsafe_allow_mock_on_sepolia: bool,
    ) -> Result<()> {
        let chain_id = self.chain_id().await?;
        anyhow::ensure!(
            local_mock_chain_allowed(chain_id, unsafe_allow_mock_on_sepolia),
            "API_LOCAL_MOCK_SUBMIT is restricted to chain ID 31337 unless \
             API_UNSAFE_ALLOW_MOCK_ON_SEPOLIA=true is explicitly set on Sepolia"
        );
        let provider = ProviderBuilder::new().connect_http(self.rpc_url.parse()?);
        let settlement_verifier = IZekoSettlement::new(self.settlement_address, &provider)
            .verifier()
            .call()
            .await?;
        let bridge = IEthereumZekoBridge::new(self.bridge_address, &provider);
        let bridge_verifier = bridge.bridgeVerifier().call().await?;
        anyhow::ensure!(
            settlement_verifier == bridge_verifier,
            "local settlement and bridge verifiers must be identical"
        );
        let is_local = ILocalSP1Verifier::new(settlement_verifier, provider)
            .isLocalSP1Verifier()
            .call()
            .await
            .context("configured verifier is not LocalSP1Verifier")?;
        anyhow::ensure!(is_local, "configured verifier is not LocalSP1Verifier");
        Ok(())
    }

    pub async fn configured_program_vkeys(&self) -> Result<[B256; 2]> {
        let provider = ProviderBuilder::new().connect_http(self.rpc_url.parse()?);
        let settlement = IZekoSettlement::new(self.settlement_address, &provider)
            .programVKey()
            .call()
            .await?;
        let bridge = IEthereumZekoBridge::new(self.bridge_address, provider);
        Ok([settlement, bridge.bridgeProgramVKey().call().await?])
    }

    pub fn settlement_address(&self) -> Address {
        self.settlement_address
    }

    pub async fn block_number(&self) -> Result<u64> {
        Ok(ProviderBuilder::new()
            .connect_http(self.rpc_url.parse()?)
            .get_block_number()
            .await?)
    }

    pub async fn block(&self, number: u64) -> Result<BlockRef> {
        let provider = ProviderBuilder::new().connect_http(self.rpc_url.parse()?);
        let block = provider
            .get_block_by_number(BlockNumberOrTag::Number(number))
            .await?
            .with_context(|| format!("Ethereum block {number} is unavailable"))?;
        Ok(BlockRef {
            number: block.header.number,
            hash: block.header.hash,
            parent_hash: block.header.parent_hash,
        })
    }

    pub async fn finalized_block(&self) -> Result<BlockRef> {
        let provider = ProviderBuilder::new().connect_http(self.rpc_url.parse()?);
        let block = provider
            .get_block_by_number(BlockNumberOrTag::Finalized)
            .await?
            .context("Ethereum RPC did not return a consensus-finalized block")?;
        Ok(BlockRef {
            number: block.header.number,
            hash: block.header.hash,
            parent_hash: block.header.parent_hash,
        })
    }

    pub async fn transaction_receipt(
        &self,
        transaction_hash: &str,
    ) -> Result<Option<TransactionReceiptRef>> {
        let hash: TxHash = transaction_hash
            .parse()
            .context("invalid Ethereum transaction hash")?;
        let provider = ProviderBuilder::new().connect_http(self.rpc_url.parse()?);
        let Some(receipt) = provider.get_transaction_receipt(hash).await? else {
            return Ok(None);
        };
        let block_number = receipt
            .block_number
            .context("Ethereum receipt is not included in a block")?;
        let block_hash = receipt
            .block_hash
            .context("Ethereum receipt has no block hash")?;
        Ok(Some(TransactionReceiptRef {
            block_number,
            block_hash,
            gas_used: receipt.gas_used,
            succeeded: receipt.status(),
        }))
    }

    pub async fn settlement_state(&self) -> Result<SettlementState> {
        let provider = ProviderBuilder::new().connect_http(self.rpc_url.parse()?);
        let contract = IZekoSettlement::new(self.settlement_address, provider);
        Ok(SettlementState {
            program_vkey: contract.programVKey().call().await?,
            vk_hash: contract.vkHash().call().await?,
            action_state: contract.actionState().call().await?,
            current_root: contract.currentRoot().call().await?,
            outer_state: contract.outerState().call().await?,
            outer_action_state_length: contract.outerActionStateLength().call().await?,
            batch_sequence: contract.batchSequence().call().await?,
        })
    }

    pub async fn bridge_state(
        &self,
        nonce: Option<u64>,
        action_state_after: Option<B256>,
    ) -> Result<(BridgeState, Option<B256>)> {
        let provider = ProviderBuilder::new().connect_http(self.rpc_url.parse()?);
        let contract = IEthereumZekoBridge::new(self.bridge_address, provider);
        let program_vkey = contract.bridgeProgramVKey().call().await?;
        let historical = match nonce {
            Some(nonce) => Some(contract.depositStateByNonce(nonce).call().await?),
            None => None,
        };
        Ok((
            BridgeState {
                program_vkey,
                deposit_nonce: contract.depositNonce().call().await?,
                current_deposit_state: contract.currentDepositState().call().await?,
                bridged_deposit_nonce: contract.bridgedDepositNonce().call().await?,
                action_state_processed: match action_state_after {
                    Some(action_state) => {
                        Some(contract.processedActionState(action_state).call().await?)
                    }
                    None => None,
                },
                paused: contract.paused().call().await?,
            },
            historical,
        ))
    }

    pub async fn bridge_deposit_logs(
        &self,
        from_block: u64,
        to_block: u64,
    ) -> Result<Vec<BridgeDepositLog>> {
        let provider = ProviderBuilder::new().connect_http(self.rpc_url.parse()?);
        let filter = bridge_deposit_filter(self.bridge_address, from_block, to_block);
        let mut bridge_logs = Vec::new();
        let mut erc20_logs = Vec::new();
        let mut erc20_v2_logs = Vec::new();
        for log in provider.get_logs(&filter).await? {
            match log.topic0().copied() {
                Some(signature)
                    if signature == IEthereumZekoBridge::BridgeDeposit::SIGNATURE_HASH =>
                {
                    bridge_logs.push(log)
                }
                Some(signature)
                    if signature == IEthereumZekoBridge::ERC20DepositSubmitted::SIGNATURE_HASH =>
                {
                    erc20_logs.push(log)
                }
                Some(signature)
                    if signature
                        == IEthereumZekoBridge::ERC20DepositSubmittedV2::SIGNATURE_HASH =>
                {
                    erc20_v2_logs.push(log)
                }
                Some(signature) => anyhow::bail!("unexpected bridge deposit event {signature}"),
                None => anyhow::bail!("bridge deposit event is missing topic zero"),
            }
        }

        let mut metadata = HashMap::new();
        for log in erc20_logs {
            let decoded = log
                .log_decode_validate::<IEthereumZekoBridge::ERC20DepositSubmitted>()
                .context("decode ERC20DepositSubmitted log")?;
            let transaction_hash = decoded
                .transaction_hash
                .context("ERC20DepositSubmitted log missing transaction hash")?;
            let data = decoded.data();
            anyhow::ensure!(
                !data.assetId.is_zero(),
                "ERC20 deposit asset identity is zero"
            );
            let previous = metadata.insert(
                (transaction_hash, data.nonce),
                ERC20DepositMetadata {
                    asset_id: data.assetId,
                    deposit_leaf: data.depositLeaf,
                    new_deposit_state: data.newDepositState,
                    token: data.token,
                    sender: data.sender,
                    zeko_recipient: B256::from(data.zekoRecipient.to_be_bytes()),
                    amount: data.amount,
                    timeout: data.timeout,
                    registry_index: None,
                    record_commitment: None,
                },
            );
            anyhow::ensure!(previous.is_none(), "duplicate ERC20 deposit identity event");
        }

        for log in erc20_v2_logs {
            let decoded = log
                .log_decode_validate::<IEthereumZekoBridge::ERC20DepositSubmittedV2>()
                .context("decode ERC20DepositSubmittedV2 log")?;
            let transaction_hash = decoded
                .transaction_hash
                .context("ERC20DepositSubmittedV2 log missing transaction hash")?;
            let data = decoded.data();
            anyhow::ensure!(
                data.encodingVersion == ERC20_ACTION_ENCODING_V2,
                "ERC20 deposit has an unsupported action encoding"
            );
            anyhow::ensure!(
                !data.recordCommitment.is_zero(),
                "ERC20 deposit record commitment is zero"
            );
            let identity = metadata
                .get_mut(&(transaction_hash, data.nonce))
                .context("registry ERC20 deposit is missing its base identity event")?;
            anyhow::ensure!(
                identity.asset_id == data.assetId
                    && identity.deposit_leaf == data.depositLeaf
                    && identity.new_deposit_state == data.newDepositState
                    && identity.token == data.token
                    && identity.sender == data.sender
                    && identity.zeko_recipient == B256::from(data.zekoRecipient.to_be_bytes())
                    && identity.amount == data.amount
                    && identity.timeout == data.timeout,
                "registry ERC20 deposit identity events disagree"
            );
            anyhow::ensure!(
                identity.record_commitment.is_none(),
                "duplicate registry ERC20 deposit identity event"
            );
            identity.registry_index = Some(data.registryIndex);
            identity.record_commitment = Some(data.recordCommitment);
        }

        let mut deposits = Vec::new();
        for log in bridge_logs {
            let decoded = log
                .log_decode_validate::<IEthereumZekoBridge::BridgeDeposit>()
                .context("decode BridgeDeposit log")?;
            let transaction_hash = decoded
                .transaction_hash
                .context("BridgeDeposit log missing transaction hash")?;
            let data = decoded.data();
            let identity = metadata.remove(&(transaction_hash, data.nonce));
            let (asset_id, action_encoding_version, registry_index, record_commitment) =
                if data.token.is_zero() {
                    anyhow::ensure!(
                        identity.is_none(),
                        "native deposit has an ERC20 identity event"
                    );
                    (None, 0, None, None)
                } else {
                    let identity = identity
                        .context("ERC20 deposit is missing its immutable identity event")?;
                    anyhow::ensure!(
                        identity.deposit_leaf == data.depositLeaf
                            && identity.new_deposit_state == data.newDepositState
                            && identity.token == data.token
                            && identity.sender == data.sender
                            && identity.zeko_recipient
                                == B256::from(data.zekoRecipient.to_be_bytes())
                            && U256::from(identity.amount) == data.amount
                            && data.amount == data.zekoAmount
                            && identity.timeout == data.timeout,
                        "BridgeDeposit and ERC20 identity events disagree"
                    );
                    let (encoding_version, registry_index, record_commitment) =
                        classify_erc20_deposit_identity(
                            identity.registry_index,
                            identity.record_commitment,
                        )?;
                    (
                        Some(identity.asset_id),
                        encoding_version,
                        registry_index,
                        record_commitment,
                    )
                };
            deposits.push(BridgeDepositLog {
                nonce: data.nonce,
                deposit_leaf: data.depositLeaf,
                new_deposit_state: data.newDepositState,
                old_deposit_state: data.oldDepositState,
                token: data.token,
                sender: data.sender,
                zeko_recipient: B256::from(data.zekoRecipient.to_be_bytes()),
                amount: data.amount,
                zeko_amount: data.zekoAmount,
                timeout: data.timeout,
                asset_id,
                action_encoding_version,
                registry_index,
                record_commitment,
                block_number: decoded
                    .block_number
                    .context("BridgeDeposit log missing block number")?,
                block_hash: decoded
                    .block_hash
                    .context("BridgeDeposit log missing block hash")?,
                transaction_hash,
                log_index: decoded
                    .log_index
                    .context("BridgeDeposit log missing log index")?,
            });
        }
        anyhow::ensure!(
            metadata.is_empty(),
            "ERC20 identity event is missing its BridgeDeposit event"
        );
        Ok(deposits)
    }

    pub async fn settlement_accepted_logs(
        &self,
        from_block: u64,
        to_block: u64,
    ) -> Result<Vec<SettlementAcceptedLog>> {
        let provider = ProviderBuilder::new().connect_http(self.rpc_url.parse()?);
        let filter = Filter::new()
            .address(self.settlement_address)
            .event_signature(IZekoSettlement::SettlementAccepted::SIGNATURE_HASH)
            .from_block(from_block)
            .to_block(to_block);
        provider
            .get_logs(&filter)
            .await?
            .into_iter()
            .map(|log| {
                let decoded = log
                    .log_decode_validate::<IZekoSettlement::SettlementAccepted>()
                    .context("decode SettlementAccepted log")?;
                let data = decoded.data();
                Ok(SettlementAcceptedLog {
                    batch_sequence: data.batchSequence,
                    mina_transaction_hash: data.minaTransactionHash,
                    ledger_hash: data.ledgerHash,
                    outer_action_state: data.outerActionState,
                    outer_action_state_length: data.outerActionStateLength,
                    inner_action_state: data.innerActionState,
                    inner_action_state_length: data.innerActionStateLength,
                    slot_lower: data.slotLower,
                    slot_upper: data.slotUpper,
                    block_number: decoded
                        .block_number
                        .context("SettlementAccepted log missing block number")?,
                    block_hash: decoded
                        .block_hash
                        .context("SettlementAccepted log missing block hash")?,
                    transaction_hash: decoded
                        .transaction_hash
                        .context("SettlementAccepted log missing transaction hash")?,
                    log_index: decoded
                        .log_index
                        .context("SettlementAccepted log missing log index")?,
                })
            })
            .collect()
    }

    pub async fn inner_action_batch_logs(
        &self,
        from_block: u64,
        to_block: u64,
    ) -> Result<Vec<InnerActionBatchAcceptedLog>> {
        let provider = ProviderBuilder::new().connect_http(self.rpc_url.parse()?);
        let filter = Filter::new()
            .address(self.settlement_address)
            .event_signature(IZekoSettlement::InnerActionBatchAccepted::SIGNATURE_HASH)
            .from_block(from_block)
            .to_block(to_block);
        provider
            .get_logs(&filter)
            .await?
            .into_iter()
            .map(|log| {
                let decoded = log
                    .log_decode_validate::<IZekoSettlement::InnerActionBatchAccepted>()
                    .context("decode InnerActionBatchAccepted log")?;
                let data = decoded.data();
                Ok(InnerActionBatchAcceptedLog {
                    batch_sequence: data.batchSequence,
                    state_after: data.stateAfter,
                    root: data.root,
                    start_index: data.startIndex,
                    count: data.count,
                    claimable_slot: data.claimableSlot,
                    transaction_hash: decoded
                        .transaction_hash
                        .context("InnerActionBatchAccepted log missing transaction hash")?,
                })
            })
            .collect()
    }

    pub async fn native_withdrawal_claimed_logs(
        &self,
        from_block: u64,
        to_block: u64,
    ) -> Result<Vec<NativeWithdrawalClaimedLog>> {
        let provider = ProviderBuilder::new().connect_http(self.rpc_url.parse()?);
        let filter = Filter::new()
            .address(self.bridge_address)
            .event_signature(IEthereumZekoBridge::NativeWithdrawalClaimed::SIGNATURE_HASH)
            .from_block(from_block)
            .to_block(to_block);
        provider
            .get_logs(&filter)
            .await?
            .into_iter()
            .map(|log| {
                let decoded = log
                    .log_decode_validate::<IEthereumZekoBridge::NativeWithdrawalClaimed>()
                    .context("decode NativeWithdrawalClaimed log")?;
                let data = decoded.data();
                Ok(NativeWithdrawalClaimedLog {
                    settlement_sequence: data.settlementSequence,
                    global_action_index: data.globalActionIndex,
                    recipient: data.recipient,
                    zeko_amount: data.zekoAmount,
                    ethereum_amount: data.ethereumAmount,
                    action_fields_hash: data.actionFieldsHash,
                    block_number: decoded
                        .block_number
                        .context("NativeWithdrawalClaimed log missing block number")?,
                    block_hash: decoded
                        .block_hash
                        .context("NativeWithdrawalClaimed log missing block hash")?,
                    transaction_hash: decoded
                        .transaction_hash
                        .context("NativeWithdrawalClaimed log missing transaction hash")?,
                    log_index: decoded
                        .log_index
                        .context("NativeWithdrawalClaimed log missing log index")?,
                })
            })
            .collect()
    }

    pub async fn bridge_transition_accepted_logs(
        &self,
        from_block: u64,
        to_block: u64,
    ) -> Result<Vec<BridgeTransitionAcceptedLog>> {
        let provider = ProviderBuilder::new().connect_http(self.rpc_url.parse()?);
        let filter = bridge_transition_filter(self.bridge_address, from_block, to_block);
        provider
            .get_logs(&filter)
            .await?
            .into_iter()
            .map(|log| match log.topic0().copied() {
                Some(signature)
                    if signature
                        == IEthereumZekoBridge::BridgeTransitionAccepted::SIGNATURE_HASH =>
                {
                    let decoded = log
                        .log_decode_validate::<IEthereumZekoBridge::BridgeTransitionAccepted>()
                        .context("decode BridgeTransitionAccepted log")?;
                    let data = decoded.data();
                    Ok(BridgeTransitionAcceptedLog {
                        old_action_state: data.oldActionState,
                        new_action_state: data.newActionState,
                        new_deposit_state: data.newDepositState,
                        new_deposit_nonce: data.newDepositNonce,
                        block_number: decoded
                            .block_number
                            .context("BridgeTransitionAccepted log missing block number")?,
                        block_hash: decoded
                            .block_hash
                            .context("BridgeTransitionAccepted log missing block hash")?,
                        transaction_hash: decoded
                            .transaction_hash
                            .context("BridgeTransitionAccepted log missing transaction hash")?,
                        log_index: decoded
                            .log_index
                            .context("BridgeTransitionAccepted log missing log index")?,
                    })
                }
                Some(signature)
                    if signature
                        == legacy_bridge_events::BridgeTransitionAccepted::SIGNATURE_HASH =>
                {
                    let decoded = log
                        .log_decode_validate::<legacy_bridge_events::BridgeTransitionAccepted>()
                        .context("decode legacy BridgeTransitionAccepted log")?;
                    let data = decoded.data();
                    Ok(BridgeTransitionAcceptedLog {
                        old_action_state: data.oldActionState,
                        new_action_state: data.newActionState,
                        new_deposit_state: data.newDepositState,
                        new_deposit_nonce: data.newDepositNonce,
                        block_number: decoded
                            .block_number
                            .context("legacy BridgeTransitionAccepted log missing block number")?,
                        block_hash: decoded
                            .block_hash
                            .context("legacy BridgeTransitionAccepted log missing block hash")?,
                        transaction_hash: decoded.transaction_hash.context(
                            "legacy BridgeTransitionAccepted log missing transaction hash",
                        )?,
                        log_index: decoded
                            .log_index
                            .context("legacy BridgeTransitionAccepted log missing log index")?,
                    })
                }
                Some(signature) => {
                    anyhow::bail!("unexpected bridge transition event {signature}")
                }
                None => anyhow::bail!("bridge transition event is missing topic zero"),
            })
            .collect()
    }

    pub async fn accepted_public_values(
        &self,
        kind: ProofKind,
        transaction_hash: &str,
    ) -> Result<Vec<u8>> {
        let hash: TxHash = transaction_hash
            .parse()
            .context("invalid Ethereum transaction hash")?;
        let provider = ProviderBuilder::new().connect_http(self.rpc_url.parse()?);
        let transaction = provider
            .get_transaction_by_hash(hash)
            .await?
            .context("accepted Ethereum transaction is unavailable")?;
        let input = transaction.input();
        let public_values = match kind {
            ProofKind::Settlement => {
                IZekoSettlement::verifyAndUpdateRootCall::abi_decode_validate(input)
                    .context("decode accepted settlement transaction calldata")?
                    .publicValues
            }
            ProofKind::Bridge => {
                IEthereumZekoBridge::submitBridgeTransitionCall::abi_decode_validate(input)
                    .context("decode accepted bridge transaction calldata")?
                    .publicValues
            }
        };
        Ok(public_values.to_vec())
    }

    pub async fn withdrawal_delay_slots(&self) -> Result<u32> {
        let provider = ProviderBuilder::new().connect_http(self.rpc_url.parse()?);
        Ok(IEthereumZekoBridge::new(self.bridge_address, provider)
            .withdrawalDelaySlots()
            .call()
            .await?)
    }

    pub async fn current_virtual_slot(&self) -> Result<u64> {
        let provider = ProviderBuilder::new().connect_http(self.rpc_url.parse()?);
        Ok(IZekoSettlement::new(self.settlement_address, provider)
            .currentVirtualSlot()
            .call()
            .await?)
    }

    pub async fn next_withdrawal_index(&self, recipient: Address) -> Result<u32> {
        let provider = ProviderBuilder::new().connect_http(self.rpc_url.parse()?);
        Ok(IEthereumZekoBridge::new(self.bridge_address, provider)
            .nextWithdrawalIndex(recipient)
            .call()
            .await?)
    }

    pub async fn next_token_withdrawal_index(
        &self,
        token: Address,
        recipient: Address,
    ) -> Result<u32> {
        let provider = ProviderBuilder::new().connect_http(self.rpc_url.parse()?);
        Ok(IEthereumZekoBridge::new(self.bridge_address, provider)
            .nextTokenWithdrawalIndex(token, recipient)
            .call()
            .await?)
    }

    pub async fn resolve_token_withdrawal_identities(
        &self,
        identities: &[TokenWithdrawalIdentity],
    ) -> Result<HashMap<TokenWithdrawalIdentity, Address>> {
        let identities = identities.iter().copied().collect::<HashSet<_>>();
        let provider = ProviderBuilder::new().connect_http(self.rpc_url.parse()?);
        let contract = IEthereumZekoBridge::new(self.bridge_address, provider);
        let mut resolved = HashMap::with_capacity(identities.len());
        for identity in identities {
            anyhow::ensure!(
                identity.encoding_version == ERC20_ACTION_ENCODING_V2,
                "unsupported ERC20 withdrawal encoding version {}",
                identity.encoding_version
            );
            anyhow::ensure!(
                !identity.record_commitment.is_zero(),
                "archived ERC20 registry identity is zero"
            );
            let token = contract
                .assetTokenByRegistryIndex(identity.registry_index)
                .call()
                .await?;
            anyhow::ensure!(!token.is_zero(), "archived ERC20 token is zero");
            anyhow::ensure!(
                contract.canonicalTokenRegistered(token).call().await?,
                "archived ERC20 token is not canonically registered"
            );
            anyhow::ensure!(
                contract.assetIdByToken(token).call().await? == identity.asset_id,
                "archived ERC20 asset id does not match Ethereum"
            );
            anyhow::ensure!(
                contract.registryIndexByToken(token).call().await? == identity.registry_index
                    && contract.recordCommitmentByToken(token).call().await?
                        == identity.record_commitment,
                "archived ERC20 registry identity does not match Ethereum"
            );
            resolved.insert(identity, token);
        }
        Ok(resolved)
    }

    pub fn bridge_address(&self) -> Address {
        self.bridge_address
    }

    pub async fn submit(
        &self,
        kind: ProofKind,
        public_values: Vec<u8>,
        proof: Vec<u8>,
    ) -> Result<TxHash> {
        let key = match kind {
            ProofKind::Settlement => &self.settlement_key,
            ProofKind::Bridge => &self.bridge_key,
        };
        let signer = PrivateKeySigner::from_str(key).context("invalid Ethereum private key")?;
        let wallet = EthereumWallet::from(signer);
        let provider = ProviderBuilder::new()
            .wallet(wallet)
            .connect_http(self.rpc_url.parse()?);
        let public_values = Bytes::from(public_values);
        let proof = Bytes::from(proof);

        let transaction_hash = match kind {
            ProofKind::Settlement => {
                let contract = IZekoSettlement::new(self.settlement_address, provider.clone());
                contract
                    .verifyAndUpdateRoot(public_values.clone(), proof.clone())
                    .call()
                    .await
                    .context("simulate settlement submission")?;
                let pending = contract
                    .verifyAndUpdateRoot(public_values, proof)
                    .send()
                    .await?;
                *pending.tx_hash()
            }
            ProofKind::Bridge => {
                let contract = IEthereumZekoBridge::new(self.bridge_address, provider.clone());
                contract
                    .submitBridgeTransition(public_values.clone(), proof.clone())
                    .call()
                    .await
                    .context("simulate bridge submission")?;
                let pending = contract
                    .submitBridgeTransition(public_values, proof)
                    .send()
                    .await?;
                *pending.tx_hash()
            }
        };
        Ok(transaction_hash)
    }
}

fn local_mock_chain_allowed(chain_id: u64, unsafe_allow_mock_on_sepolia: bool) -> bool {
    chain_id == 31_337 || (chain_id == 11_155_111 && unsafe_allow_mock_on_sepolia)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_mock_chain_policy_requires_an_explicit_sepolia_opt_in() {
        assert!(local_mock_chain_allowed(31_337, false));
        assert!(local_mock_chain_allowed(31_337, true));
        assert!(!local_mock_chain_allowed(11_155_111, false));
        assert!(local_mock_chain_allowed(11_155_111, true));
        assert!(!local_mock_chain_allowed(1, true));
    }

    #[test]
    fn bridge_deposit_signature_uses_the_solidity_value_type_abi() {
        assert_eq!(
            IEthereumZekoBridge::BridgeDeposit::SIGNATURE_HASH,
            alloy::primitives::keccak256(
                "BridgeDeposit(uint64,bytes32,bytes32,bytes32,address,address,uint256,uint256,uint256,uint64)"
            )
        );
    }

    #[test]
    fn erc20_deposit_identity_signatures_match_solidity() {
        assert_eq!(
            IEthereumZekoBridge::ERC20DepositSubmitted::SIGNATURE_HASH,
            alloy::primitives::keccak256(
                "ERC20DepositSubmitted(uint64,bytes32,bytes32,bytes32,address,address,uint256,uint64,uint64)"
            )
        );
        assert_eq!(
            IEthereumZekoBridge::ERC20DepositSubmittedV2::SIGNATURE_HASH,
            alloy::primitives::keccak256(
                "ERC20DepositSubmittedV2(uint64,bytes32,bytes32,bytes32,address,address,uint256,uint64,uint64,uint32,uint32,bytes32)"
            )
        );
    }

    #[test]
    fn bridge_deposit_filter_uses_one_topic_zero_or_set() {
        let filter = bridge_deposit_filter(Address::ZERO, 7, 9);
        assert_eq!(filter.topics[0].len(), 3);
        assert!(filter.topics[0].contains(&IEthereumZekoBridge::BridgeDeposit::SIGNATURE_HASH));
        assert!(
            filter.topics[0].contains(&IEthereumZekoBridge::ERC20DepositSubmitted::SIGNATURE_HASH)
        );
        assert!(filter.topics[0]
            .contains(&IEthereumZekoBridge::ERC20DepositSubmittedV2::SIGNATURE_HASH));
    }

    #[test]
    fn bridge_transition_signature_matches_the_recovery_event() {
        assert_eq!(
            IEthereumZekoBridge::BridgeTransitionAccepted::SIGNATURE_HASH,
            alloy::primitives::keccak256(
                "BridgeTransitionAccepted(bytes32,bytes32,bytes32,uint64)"
            )
        );
        assert_eq!(
            legacy_bridge_events::BridgeTransitionAccepted::SIGNATURE_HASH,
            alloy::primitives::keccak256(
                "BridgeTransitionAccepted(bytes32,bytes32,bytes32,bytes32,uint64)"
            )
        );
        let filter = bridge_transition_filter(Address::ZERO, 7, 9);
        assert_eq!(filter.topics[0].len(), 2);
        assert!(filter.topics[0]
            .contains(&IEthereumZekoBridge::BridgeTransitionAccepted::SIGNATURE_HASH));
        assert!(filter.topics[0]
            .contains(&legacy_bridge_events::BridgeTransitionAccepted::SIGNATURE_HASH));
    }

    #[test]
    fn historical_erc20_deposits_retain_their_action_encoding() {
        assert_eq!(
            classify_erc20_deposit_identity(None, None).unwrap(),
            (HISTORICAL_ERC20_ACTION_ENCODING_V1, None, None)
        );
        let commitment = B256::repeat_byte(0x11);
        assert_eq!(
            classify_erc20_deposit_identity(Some(7), Some(commitment)).unwrap(),
            (ERC20_ACTION_ENCODING_V2, Some(7), Some(commitment))
        );
        assert!(classify_erc20_deposit_identity(Some(7), None).is_err());
        assert!(classify_erc20_deposit_identity(None, Some(commitment)).is_err());
    }

    #[test]
    fn token_identity_resolution_accepts_only_registry_bound_actions() {
        let source = include_str!("ethereum.rs");
        let start = source
            .find("pub async fn resolve_token_withdrawal_identities")
            .unwrap();
        let end = source[start..]
            .find("pub fn bridge_address")
            .map(|offset| start + offset)
            .unwrap();
        let resolver = &source[start..end];

        assert!(resolver.contains("identity.encoding_version == ERC20_ACTION_ENCODING_V2"));
        assert!(!resolver.contains("ERC20_ACTION_ENCODING_V1"));
    }
}
