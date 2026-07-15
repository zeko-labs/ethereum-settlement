use alloy::{
    eips::BlockNumberOrTag,
    network::EthereumWallet,
    primitives::{Address, Bytes, TxHash, B256, U256},
    providers::{Provider, ProviderBuilder},
    rpc::types::Filter,
    signers::local::PrivateKeySigner,
    sol,
    sol_types::SolEvent,
};
use anyhow::{Context, Result};
use std::str::FromStr;

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
        function l2ActionStateInfo(bytes32 actionState) external view returns (uint64 index, bool valid);
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
        function withdrawVerifier() external view returns (address);
        function bridgeProgramVKey() external view returns (bytes32);
        function withdrawProgramVKey() external view returns (bytes32);
        function depositNonce() external view returns (uint64);
        function currentDepositState() external view returns (bytes32);
        function currentWithdrawState() external view returns (bytes32);
        function currentWithdrawActionStateIndex() external view returns (uint64);
        function bridgedDepositNonce() external view returns (uint64);
        function withdrawalDelaySlots() external view returns (uint32);
        function nextWithdrawalIndex(address recipient) external view returns (uint32);
        function processedActionState(bytes32 actionState) external view returns (bool);
        function paused() external view returns (bool);
        function depositStateByNonce(uint64 nonce) external view returns (bytes32);
        function submitBridgeTransition(bytes publicValues, bytes proofBytes) external;
        function submitWithdrawTransition(bytes publicValues, bytes proofBytes) external;
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
        event NativeWithdrawalClaimed(
            uint64 indexed settlementSequence,
            uint32 indexed globalActionIndex,
            address indexed recipient,
            uint64 zekoAmount,
            uint256 ethereumAmount,
            bytes32 actionFieldsHash
        );
    }

    #[sol(rpc)]
    interface ILocalSP1Verifier {
        function isLocalSP1Verifier() external view returns (bool);
    }
}

#[derive(Clone)]
pub struct Ethereum {
    rpc_url: String,
    settlement_address: Address,
    bridge_address: Address,
    settlement_key: String,
    bridge_key: String,
    withdraw_key: String,
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
    pub current_withdraw_state: B256,
    pub current_withdraw_action_state_index: u64,
    pub bridged_deposit_nonce: u64,
    pub action_state_processed: Option<bool>,
    pub paused: bool,
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
    pub block_number: u64,
    pub block_hash: B256,
    pub transaction_hash: TxHash,
    pub log_index: u64,
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
        withdraw_key: String,
    ) -> Result<Self> {
        anyhow::ensure!(
            !settlement_key.is_empty(),
            "SETTLEMENT_PRIVATE_KEY is required"
        );
        anyhow::ensure!(!bridge_key.is_empty(), "BRIDGE_PRIVATE_KEY is required");
        anyhow::ensure!(!withdraw_key.is_empty(), "WITHDRAW_PRIVATE_KEY is required");
        Ok(Self {
            rpc_url,
            settlement_address: settlement_address
                .parse()
                .context("invalid settlement address")?,
            bridge_address: bridge_address.parse().context("invalid bridge address")?,
            settlement_key,
            bridge_key,
            withdraw_key,
        })
    }

    pub async fn chain_id(&self) -> Result<u64> {
        Ok(ProviderBuilder::new()
            .connect_http(self.rpc_url.parse()?)
            .get_chain_id()
            .await?)
    }

    pub async fn ensure_local_mock_verifiers(&self) -> Result<()> {
        let chain_id = self.chain_id().await?;
        anyhow::ensure!(
            chain_id == 31_337,
            "API_LOCAL_MOCK_SUBMIT is restricted to chain ID 31337"
        );
        let provider = ProviderBuilder::new().connect_http(self.rpc_url.parse()?);
        let settlement_verifier = IZekoSettlement::new(self.settlement_address, &provider)
            .verifier()
            .call()
            .await?;
        let bridge = IEthereumZekoBridge::new(self.bridge_address, &provider);
        let bridge_verifier = bridge.bridgeVerifier().call().await?;
        let withdraw_verifier = bridge.withdrawVerifier().call().await?;
        anyhow::ensure!(
            settlement_verifier == bridge_verifier && settlement_verifier == withdraw_verifier,
            "local settlement, bridge, and withdrawal verifiers must be identical"
        );
        let is_local = ILocalSP1Verifier::new(settlement_verifier, provider)
            .isLocalSP1Verifier()
            .call()
            .await
            .context("configured verifier is not LocalSP1Verifier")?;
        anyhow::ensure!(is_local, "configured verifier is not LocalSP1Verifier");
        Ok(())
    }

    pub async fn configured_program_vkeys(&self) -> Result<[B256; 3]> {
        let provider = ProviderBuilder::new().connect_http(self.rpc_url.parse()?);
        let settlement = IZekoSettlement::new(self.settlement_address, &provider)
            .programVKey()
            .call()
            .await?;
        let bridge = IEthereumZekoBridge::new(self.bridge_address, provider);
        Ok([
            settlement,
            bridge.bridgeProgramVKey().call().await?,
            bridge.withdrawProgramVKey().call().await?,
        ])
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
        kind: &str,
        nonce: Option<u64>,
        action_state_after: Option<B256>,
    ) -> Result<(BridgeState, Option<B256>)> {
        let provider = ProviderBuilder::new().connect_http(self.rpc_url.parse()?);
        let contract = IEthereumZekoBridge::new(self.bridge_address, provider);
        let program_vkey = match kind {
            "bridge" => contract.bridgeProgramVKey().call().await?,
            "withdraw" => contract.withdrawProgramVKey().call().await?,
            _ => anyhow::bail!("unsupported bridge proof kind: {kind}"),
        };
        let historical = match nonce {
            Some(nonce) => Some(contract.depositStateByNonce(nonce).call().await?),
            None => None,
        };
        Ok((
            BridgeState {
                program_vkey,
                deposit_nonce: contract.depositNonce().call().await?,
                current_deposit_state: contract.currentDepositState().call().await?,
                current_withdraw_state: contract.currentWithdrawState().call().await?,
                current_withdraw_action_state_index: contract
                    .currentWithdrawActionStateIndex()
                    .call()
                    .await?,
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
        let filter = Filter::new()
            .address(self.bridge_address)
            .event_signature(IEthereumZekoBridge::BridgeDeposit::SIGNATURE_HASH)
            .from_block(from_block)
            .to_block(to_block);
        provider
            .get_logs(&filter)
            .await?
            .into_iter()
            .map(|log| {
                let decoded = log
                    .log_decode_validate::<IEthereumZekoBridge::BridgeDeposit>()
                    .context("decode BridgeDeposit log")?;
                let data = decoded.data();
                Ok(BridgeDepositLog {
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
                    block_number: decoded
                        .block_number
                        .context("BridgeDeposit log missing block number")?,
                    block_hash: decoded
                        .block_hash
                        .context("BridgeDeposit log missing block hash")?,
                    transaction_hash: decoded
                        .transaction_hash
                        .context("BridgeDeposit log missing transaction hash")?,
                    log_index: decoded
                        .log_index
                        .context("BridgeDeposit log missing log index")?,
                })
            })
            .collect()
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

    pub async fn l2_action_state_info(&self, action_state: B256) -> Result<(u64, bool)> {
        let provider = ProviderBuilder::new().connect_http(self.rpc_url.parse()?);
        let info = IZekoSettlement::new(self.settlement_address, provider)
            .l2ActionStateInfo(action_state)
            .call()
            .await?;
        Ok((info.index, info.valid))
    }

    pub fn bridge_address(&self) -> Address {
        self.bridge_address
    }

    pub async fn submit(
        &self,
        kind: &str,
        public_values: Vec<u8>,
        proof: Vec<u8>,
    ) -> Result<TxHash> {
        let key = match kind {
            "settlement" => &self.settlement_key,
            "bridge" => &self.bridge_key,
            "withdraw" => &self.withdraw_key,
            _ => anyhow::bail!("unsupported proof kind: {kind}"),
        };
        let signer = PrivateKeySigner::from_str(key).context("invalid Ethereum private key")?;
        let wallet = EthereumWallet::from(signer);
        let provider = ProviderBuilder::new()
            .wallet(wallet)
            .connect_http(self.rpc_url.parse()?);
        let public_values = Bytes::from(public_values);
        let proof = Bytes::from(proof);

        let transaction_hash = match kind {
            "settlement" => {
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
            "bridge" => {
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
            "withdraw" => {
                let contract = IEthereumZekoBridge::new(self.bridge_address, provider);
                contract
                    .submitWithdrawTransition(public_values.clone(), proof.clone())
                    .call()
                    .await
                    .context("simulate withdraw submission")?;
                let pending = contract
                    .submitWithdrawTransition(public_values, proof)
                    .send()
                    .await?;
                *pending.tx_hash()
            }
            _ => unreachable!(),
        };
        Ok(transaction_hash)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bridge_deposit_signature_uses_the_solidity_value_type_abi() {
        assert_eq!(
            IEthereumZekoBridge::BridgeDeposit::SIGNATURE_HASH,
            alloy::primitives::keccak256(
                "BridgeDeposit(uint64,bytes32,bytes32,bytes32,address,address,uint256,uint256,uint256,uint64)"
            )
        );
    }
}
