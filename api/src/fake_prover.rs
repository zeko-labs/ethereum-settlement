//! Explicit fixtures for the non-deployable unit-test build. No proof is accepted
//! by default, and this module never imports a prover SDK or sends network traffic.
#![cfg(all(test, feature = "fake-prover-tests"))]

use anyhow::{Context, Result};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::VecDeque,
    future::Future,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
};

use crate::proof_kind::ProofKind;
pub use crate::prover_types::*;

#[derive(Debug)]
pub struct ProofOutput {
    pub proof: FixtureProof,
    pub public_values: Vec<u8>,
}

#[derive(Debug)]
pub struct FixtureProof(Vec<u8>);
impl FixtureProof {
    pub fn bytes(&self) -> Vec<u8> {
        self.0.clone()
    }
}

tokio::task_local! { static ACTIVE: Arc<FixtureState>; }

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Operation {
    Preflight,
    RequestProof,
    WaitProof,
    ProgramVkey,
    RequestMetrics,
    AuctionQuote,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Counts {
    pub preflight: u64,
    pub request_proof: u64,
    pub wait_proof: u64,
    pub program_vkey: u64,
    pub request_metrics: u64,
    pub auction_quote: u64,
}

#[derive(Default)]
struct Counters {
    preflight: AtomicU64,
    request_proof: AtomicU64,
    wait_proof: AtomicU64,
    program_vkey: AtomicU64,
    request_metrics: AtomicU64,
    auction_quote: AtomicU64,
}

struct FixtureState {
    kind: ProofKind,
    input_digest: String,
    request_id: String,
    public_values: Vec<u8>,
    proof_bytes: Vec<u8>,
    program_vkey: String,
    counts: Counters,
    failures: Mutex<VecDeque<(Operation, String)>>,
}

impl FixtureState {
    fn called(&self, operation: Operation) -> Result<()> {
        let counter = match operation {
            Operation::Preflight => &self.counts.preflight,
            Operation::RequestProof => &self.counts.request_proof,
            Operation::WaitProof => &self.counts.wait_proof,
            Operation::ProgramVkey => &self.counts.program_vkey,
            Operation::RequestMetrics => &self.counts.request_metrics,
            Operation::AuctionQuote => &self.counts.auction_quote,
        };
        counter.fetch_add(1, Ordering::Relaxed);
        let mut failures = self
            .failures
            .lock()
            .expect("fixture failure queue poisoned");
        if let Some(index) = failures.iter().position(|(op, _)| *op == operation) {
            let (_, message) = failures.remove(index).expect("failure index exists");
            anyhow::bail!("{message}");
        }
        Ok(())
    }

    fn check_kind(&self, kind: ProofKind) -> Result<()> {
        anyhow::ensure!(
            self.kind == kind,
            "no explicit fake-prover fixture for {kind}"
        );
        Ok(())
    }

    fn check_input(&self, kind: ProofKind, input: &Value) -> Result<()> {
        self.check_kind(kind)?;
        anyhow::ensure!(
            self.input_digest == input_digest(input),
            "input differs from explicit fake-prover fixture"
        );
        Ok(())
    }
}

fn active() -> Result<Arc<FixtureState>> {
    ACTIVE
        .try_with(Arc::clone)
        .context("no explicit fake-prover fixture installed for this task")
}

fn input_digest(input: &Value) -> String {
    let mut input = input.clone();
    // The real worker hydrates this Ethereum-domain context after claiming.
    // Actual validate_preflight still checks its values against the fake RPC.
    if let Some(proof) = input.get_mut("proof").and_then(Value::as_object_mut) {
        proof.remove("context");
    }
    format!("0x{}", hex::encode(Sha256::digest(input.to_string())))
}

pub async fn preflight(
    kind: ProofKind,
    input: &Value,
    _execute_settlement: bool,
) -> Result<Preflight> {
    let fixture = active()?;
    fixture.check_input(kind, input)?;
    fixture.called(Operation::Preflight)?;
    Preflight::decode(kind, fixture.public_values.clone(), Some(100))
}

pub async fn request_proof(
    kind: ProofKind,
    input: &Value,
    _system: &str,
    _config: &NetworkRequestConfig,
) -> Result<String> {
    let fixture = active()?;
    fixture.check_input(kind, input)?;
    fixture.called(Operation::RequestProof)?;
    Ok(fixture.request_id.clone())
}

pub async fn wait_proof(kind: ProofKind, request_id: &str) -> Result<ProofOutput> {
    let fixture = active()?;
    fixture.check_kind(kind)?;
    anyhow::ensure!(
        request_id == fixture.request_id,
        "unknown fake proof request"
    );
    fixture.called(Operation::WaitProof)?;
    Ok(ProofOutput {
        proof: FixtureProof(fixture.proof_bytes.clone()),
        public_values: fixture.public_values.clone(),
    })
}

pub async fn program_vkey(kind: ProofKind) -> Result<String> {
    let fixture = active()?;
    fixture.check_kind(kind)?;
    fixture.called(Operation::ProgramVkey)?;
    Ok(fixture.program_vkey.clone())
}

pub async fn request_metrics(request_id: &str) -> Result<RequestMetrics> {
    let fixture = active()?;
    anyhow::ensure!(
        request_id == fixture.request_id,
        "unknown fake proof request"
    );
    fixture.called(Operation::RequestMetrics)?;
    Ok(RequestMetrics {
        cycles: Some(100),
        prover_gas: Some(100),
        ..Default::default()
    })
}

pub async fn auction_quote(
    system: &str,
    max_pgu: u64,
    approved_max_price_per_pgu: Option<u64>,
) -> Result<AuctionQuote> {
    let fixture = active()?;
    fixture.called(Operation::AuctionQuote)?;
    let price = approved_max_price_per_pgu.unwrap_or(1);
    Ok(AuctionQuote {
        proof_system: system.to_owned(),
        base_fee_atto_prove: "0".into(),
        base_fee_prove: "0".into(),
        network_max_price_per_pgu: price.to_string(),
        approved_max_pgu: max_pgu.to_string(),
        approved_max_price_per_pgu: price.to_string(),
        maximum_cost_atto_prove: (u128::from(max_pgu) * u128::from(price)).to_string(),
        maximum_cost_prove: "0".into(),
    })
}

pub mod testing {
    use super::*;
    pub use super::{Counts, Operation};

    /// Scope the fixture around production handlers/state-machine calls. Tokio
    /// task-local storage isolates parallel tests; spawned tasks require their
    /// own explicit `run` scope rather than inheriting a permissive global mock.
    #[derive(Clone)]
    pub struct Fixture {
        state: Arc<FixtureState>,
    }

    impl Fixture {
        pub fn new(
            kind: ProofKind,
            input: Value,
            public_values: Vec<u8>,
            program_vkey: String,
        ) -> Result<Self> {
            // Tests cannot bypass receipt layout decoding with arbitrary bytes.
            Preflight::decode(kind, public_values.clone(), Some(100))?;
            let digest = input_digest(&input);
            Ok(Self {
                state: Arc::new(FixtureState {
                    kind,
                    request_id: digest.clone(),
                    input_digest: digest,
                    public_values,
                    proof_bytes: b"explicit-test-proof".to_vec(),
                    program_vkey,
                    counts: Counters::default(),
                    failures: Mutex::new(VecDeque::new()),
                }),
            })
        }

        pub async fn run<F: Future>(&self, future: F) -> F::Output {
            ACTIVE.scope(self.state.clone(), future).await
        }

        pub fn request_id(&self) -> &str {
            &self.state.request_id
        }

        pub fn fail_next(&self, operation: Operation, message: impl Into<String>) {
            self.state
                .failures
                .lock()
                .expect("fixture failure queue poisoned")
                .push_back((operation, message.into()));
        }

        pub fn counts(&self) -> Counts {
            let c = &self.state.counts;
            Counts {
                preflight: c.preflight.load(Ordering::Relaxed),
                request_proof: c.request_proof.load(Ordering::Relaxed),
                wait_proof: c.wait_proof.load(Ordering::Relaxed),
                program_vkey: c.program_vkey.load(Ordering::Relaxed),
                request_metrics: c.request_metrics.load(Ordering::Relaxed),
                auction_quote: c.auction_quote.load(Ordering::Relaxed),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn no_fixture_can_never_execute_or_request_a_proof() {
        assert!(preflight(ProofKind::Settlement, &json!({}), true)
            .await
            .is_err());
        assert!(program_vkey(ProofKind::Settlement).await.is_err());
        let config = NetworkRequestConfig {
            timeout: std::time::Duration::from_secs(1),
            min_auction_period: 1,
            gas_limit: None,
            max_price_per_pgu: None,
        };
        assert!(
            request_proof(ProofKind::Settlement, &json!({}), "groth16", &config)
                .await
                .is_err()
        );
        assert!(wait_proof(ProofKind::Settlement, "0x00").await.is_err());
    }

    #[test]
    fn fixture_installation_uses_production_receipt_decoding() {
        assert!(testing::Fixture::new(
            ProofKind::Settlement,
            json!({}),
            vec![1, 2, 3],
            "0x00".into()
        )
        .is_err());
    }

    #[tokio::test]
    async fn fixtures_are_scoped_and_count_attempts_including_injected_failures() {
        use zeko_sp1_lib::{BridgeOuterActionV2, BridgeTransitionPublicValuesV2};
        let values = BridgeTransitionPublicValuesV2 {
            ethereum_state_before: [1; 32],
            ethereum_state_after: [2; 32],
            ethereum_nonce_before: 0,
            ethereum_nonce_after: 1,
            zeko_action_state_before: [3; 32],
            zeko_action_state_after: [4; 32],
            zeko_action_state_length_before: 0,
            zeko_action_state_length_after: 1,
            actions: vec![BridgeOuterActionV2 {
                fields: [[5; 32]; 5],
                state_after: [4; 32],
            }],
        }
        .encode();
        let input = json!({"test": "first"});
        let fixture = testing::Fixture::new(
            ProofKind::Bridge,
            input.clone(),
            values.clone(),
            "first".into(),
        )
        .unwrap();
        let other = testing::Fixture::new(
            ProofKind::Bridge,
            json!({"test": "second"}),
            values,
            "second".into(),
        )
        .unwrap();
        fixture.fail_next(Operation::WaitProof, "injected transport error");
        fixture
            .run(async {
                assert_eq!(program_vkey(ProofKind::Bridge).await.unwrap(), "first");
                other
                    .run(async {
                        assert_eq!(program_vkey(ProofKind::Bridge).await.unwrap(), "second");
                        assert!(preflight(ProofKind::Bridge, &input, false).await.is_err());
                    })
                    .await;
                assert_eq!(program_vkey(ProofKind::Bridge).await.unwrap(), "first");
                // A spawned worker cannot accidentally inherit another test's fixture.
                assert!(
                    tokio::spawn(async { program_vkey(ProofKind::Bridge).await })
                        .await
                        .unwrap()
                        .is_err()
                );
                let receipt = preflight(ProofKind::Bridge, &input, false).await.unwrap();
                assert!(matches!(receipt, Preflight::Bridge { .. }));
                assert!(wait_proof(ProofKind::Bridge, fixture.request_id())
                    .await
                    .unwrap_err()
                    .to_string()
                    .contains("injected transport error"));
                assert_eq!(
                    wait_proof(ProofKind::Bridge, fixture.request_id())
                        .await
                        .unwrap()
                        .proof
                        .bytes(),
                    b"explicit-test-proof"
                );
            })
            .await;
        assert!(program_vkey(ProofKind::Bridge).await.is_err());
        assert_eq!(fixture.counts().preflight, 1);
        assert_eq!(fixture.counts().wait_proof, 2);
        assert_eq!(fixture.counts().program_vkey, 2);
        assert_eq!(other.counts().preflight, 0);
        assert_eq!(other.counts().program_vkey, 1);
    }
}
