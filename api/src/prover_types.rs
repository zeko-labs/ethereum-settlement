//! Shared descriptors and receipt decoding; neither backend changes these rules.
use crate::proof_kind::ProofKind;
use anyhow::Result;
use zeko_sp1_lib::{BridgeTransitionPublicValuesV2, SettlementPublicValues};

#[derive(Clone, Debug)]
pub struct NetworkRequestConfig {
    pub timeout: std::time::Duration,
    pub min_auction_period: u64,
    pub gas_limit: Option<u64>,
    pub max_price_per_pgu: Option<u64>,
}

#[derive(Clone, Debug, Default)]
pub struct RequestMetrics {
    pub cycles: Option<u64>,
    pub prover_gas: Option<u64>,
    pub base_fee_prove: Option<String>,
    pub max_price_per_pgu: Option<String>,
    pub actual_cost_prove: Option<String>,
}

#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuctionQuote {
    pub proof_system: String,
    pub base_fee_atto_prove: String,
    pub base_fee_prove: String,
    pub network_max_price_per_pgu: String,
    pub approved_max_pgu: String,
    pub approved_max_price_per_pgu: String,
    pub maximum_cost_atto_prove: String,
    pub maximum_cost_prove: String,
}

pub enum Preflight {
    Settlement {
        values: SettlementPublicValues,
        public_values: Vec<u8>,
        cycles: Option<u64>,
    },
    Bridge {
        values: BridgeTransitionPublicValuesV2,
        public_values: Vec<u8>,
        cycles: Option<u64>,
    },
}

impl Preflight {
    pub fn kind(&self) -> ProofKind {
        match self {
            Self::Settlement { .. } => ProofKind::Settlement,
            Self::Bridge { .. } => ProofKind::Bridge,
        }
    }

    pub fn public_values(&self) -> &[u8] {
        match self {
            Preflight::Settlement { public_values, .. }
            | Preflight::Bridge { public_values, .. } => public_values,
        }
    }

    pub fn cycles(&self) -> Option<u64> {
        match self {
            Preflight::Settlement { cycles, .. } | Preflight::Bridge { cycles, .. } => *cycles,
        }
    }

    pub fn decode(kind: ProofKind, public_values: Vec<u8>, cycles: Option<u64>) -> Result<Self> {
        match kind {
            ProofKind::Settlement => Ok(Self::Settlement {
                values: SettlementPublicValues::decode(&public_values)
                    .map_err(anyhow::Error::msg)?,
                public_values,
                cycles,
            }),
            ProofKind::Bridge => Ok(Self::Bridge {
                values: BridgeTransitionPublicValuesV2::decode(&public_values)
                    .map_err(anyhow::Error::msg)?,
                public_values,
                cycles,
            }),
        }
    }
}
