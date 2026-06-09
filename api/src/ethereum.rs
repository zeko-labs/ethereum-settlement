use alloy::{
    network::EthereumWallet,
    primitives::{Address, Bytes, B256},
    providers::{Provider, ProviderBuilder},
    signers::local::PrivateKeySigner,
    sol,
};
use anyhow::{Context, Result};
use std::str::FromStr;

sol! {
    #[sol(rpc)]
    interface IZekoSettlement {
        function programVKey() external view returns (bytes32);
        function vkHash() external view returns (bytes32);
        function actionState() external view returns (bytes32);
        function currentRoot() external view returns (bytes32);
        function l2ActionStateInfo(bytes32 actionState) external view returns (uint64 index, bool valid);
        function verifyAndUpdateRoot(bytes publicValues, bytes proofBytes) external;
    }

    #[sol(rpc)]
    interface IEthereumZekoBridge {
        function bridgeProgramVKey() external view returns (bytes32);
        function withdrawProgramVKey() external view returns (bytes32);
        function depositNonce() external view returns (uint64);
        function currentDepositState() external view returns (bytes32);
        function currentWithdrawState() external view returns (bytes32);
        function currentWithdrawActionStateIndex() external view returns (uint64);
        function processedActionState(bytes32 actionState) external view returns (bool);
        function paused() external view returns (bool);
        function depositStateByNonce(uint64 nonce) external view returns (bytes32);
        function submitBridgeTransition(bytes publicValues, bytes proofBytes) external;
        function submitWithdrawTransition(bytes publicValues, bytes proofBytes) external;
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
}

pub struct BridgeState {
    pub program_vkey: B256,
    pub deposit_nonce: u64,
    pub current_deposit_state: B256,
    pub current_withdraw_state: B256,
    pub current_withdraw_action_state_index: u64,
    pub action_state_processed: Option<bool>,
    pub paused: bool,
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

    pub async fn settlement_state(&self) -> Result<SettlementState> {
        let provider = ProviderBuilder::new().connect_http(self.rpc_url.parse()?);
        let contract = IZekoSettlement::new(self.settlement_address, provider);
        Ok(SettlementState {
            program_vkey: contract.programVKey().call().await?,
            vk_hash: contract.vkHash().call().await?,
            action_state: contract.actionState().call().await?,
            current_root: contract.currentRoot().call().await?,
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
    ) -> Result<String> {
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

        let receipt = match kind {
            "settlement" => {
                let contract = IZekoSettlement::new(self.settlement_address, provider.clone());
                contract
                    .verifyAndUpdateRoot(public_values.clone(), proof.clone())
                    .call()
                    .await
                    .context("simulate settlement submission")?;
                contract
                    .verifyAndUpdateRoot(public_values, proof)
                    .send()
                    .await?
                    .get_receipt()
                    .await?
            }
            "bridge" => {
                let contract = IEthereumZekoBridge::new(self.bridge_address, provider.clone());
                contract
                    .submitBridgeTransition(public_values.clone(), proof.clone())
                    .call()
                    .await
                    .context("simulate bridge submission")?;
                contract
                    .submitBridgeTransition(public_values, proof)
                    .send()
                    .await?
                    .get_receipt()
                    .await?
            }
            "withdraw" => {
                let contract = IEthereumZekoBridge::new(self.bridge_address, provider);
                contract
                    .submitWithdrawTransition(public_values.clone(), proof.clone())
                    .call()
                    .await
                    .context("simulate withdraw submission")?;
                contract
                    .submitWithdrawTransition(public_values, proof)
                    .send()
                    .await?
                    .get_receipt()
                    .await?
            }
            _ => unreachable!(),
        };
        Ok(receipt.transaction_hash.to_string())
    }
}
