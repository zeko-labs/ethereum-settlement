//! Completed receipts need a submission window, not a second proof-purchase
//! window. Exercise the production worker, SQL recovery and signed submission;
//! only the prover and remote Ethereum responses are explicit test fixtures.
use crate::{prover::testing::Fixture, AppState, ProofKind};
use alloy::primitives::{keccak256, B256};
use axum::{response::IntoResponse, Json};
use serde_json::{json, Value};
use sqlx::{PgPool, Row};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use uuid::Uuid;
use zeko_sp1_lib::{OuterStateV1, SettlementDaMode, SettlementPublicValuesV1};

const SLOT_UPPER: u32 = 2_500;

fn values() -> Vec<u8> {
    SettlementPublicValuesV1 {
        da_mode: SettlementDaMode::Multisig,
        chain_id: 31_337,
        settlement_contract: [0x11; 20],
        batch_sequence: 1,
        vk_hash: [0; 32],
        app_statement: [0; 32],
        mina_transaction_hash: [1; 32],
        state_before: OuterStateV1::default(),
        state_after: OuterStateV1::default(),
        outer_action_state_before: [0; 32],
        outer_action_state_after: [0; 32],
        outer_action_state_length_before: 0,
        outer_action_state_length_after: 1,
        synchronized_outer_action_state: [0; 32],
        synchronized_outer_action_state_length: 0,
        slot_lower: 0,
        slot_upper: SLOT_UPPER,
    }
    .encode()
    .to_vec()
}

fn selector(signature: &str) -> String {
    format!("0x{}", hex::encode(&keccak256(signature.as_bytes())[..4]))
}

struct Harness {
    state: AppState,
    fixture: Fixture,
    slots: Arc<Mutex<Vec<u64>>>,
    broadcasts: Arc<Mutex<Vec<String>>>,
    // Keep the server alive until the production worker and assertions finish.
    _server: crate::rpc::test_support::TestServer,
}

impl Harness {
    async fn new(pool: PgPool, initial_slot: u64, completed_slot: u64, margin: u64) -> Self {
        let fixture = Fixture::new(
            ProofKind::Settlement,
            json!({"fixture": "completed-proof-lifetime"}),
            values(),
            B256::ZERO.to_string(),
        )
        .unwrap();
        let slots = Arc::new(Mutex::new(Vec::new()));
        let broadcasts = Arc::new(Mutex::new(Vec::new()));
        let observed_slots = slots.clone();
        let observed_broadcasts = broadcasts.clone();
        let observed_fixture = fixture.clone();
        let rpc_pool = pool.clone();
        let server = crate::rpc::test_support::serve(move |request| {
            let slots = observed_slots.clone();
            let broadcasts = observed_broadcasts.clone();
            let fixture = observed_fixture.clone();
            let pool = rpc_pool.clone();
            async move {
                let result = match request["method"].as_str().unwrap() {
                    "eth_call" => {
                        let data = request["params"][0]["input"]
                            .as_str()
                            .or_else(|| request["params"][0]["data"].as_str())
                            .unwrap();
                        if data == selector("currentVirtualSlot()") {
                            // Advance the fake clock only once the explicit fake
                            // proof has completed, rather than counting unrelated RPCs.
                            let slot = if fixture.counts().wait_proof == 0 {
                                initial_slot
                            } else {
                                completed_slot
                            };
                            slots.lock().unwrap().push(slot);
                            json!(format!("0x{slot:064x}"))
                        } else if data == selector("outerState()") {
                            json!(format!("0x{}", "00".repeat(256)))
                        } else if [
                            "programVKey()", "vkHash()", "actionState()", "currentRoot()",
                            "outerActionStateLength()", "batchSequence()",
                        ].iter().any(|name| data == selector(name)) {
                            json!(B256::ZERO.to_string())
                        } else {
                            assert!(data.starts_with(&selector("verifyAndUpdateRoot(bytes,bytes)")),
                                "unexpected eth_call selector: {data}");
                            json!("0x")
                        }
                    }
                    "eth_getBlockByNumber" => {
                        let empty: alloy::rpc::types::Block = Default::default();
                        let mut block = serde_json::to_value(empty).unwrap();
                        block["hash"] = json!(B256::repeat_byte(7).to_string());
                        block["number"] = json!("0x64");
                        block
                    }
                    "eth_chainId" => json!("0x7a69"),
                    "eth_estimateGas" => json!("0x5208"),
                    "eth_feeHistory" => json!({"oldestBlock":"0x0", "baseFeePerGas":["0x1","0x1"], "gasUsedRatio":[0.5], "reward":[["0x1"]]}),
                    "eth_getTransactionCount" => json!("0x2"),
                    "eth_getTransactionByHash" | "eth_getTransactionReceipt" => Value::Null,
                    "eth_sendRawTransaction" => {
                        let raw = request["params"][0].as_str().unwrap().to_owned();
                        let persisted: bool = sqlx::query_scalar(
                            "SELECT EXISTS(SELECT 1 FROM proof_jobs WHERE prepared_transaction->>'raw_transaction' = $1 AND proof_bytes IS NOT NULL)")
                            .bind(&raw).fetch_one(&pool).await.unwrap();
                        assert!(persisted, "proof and signed bytes must be durable before broadcast");
                        let hash = keccak256(hex::decode(raw.strip_prefix("0x").unwrap()).unwrap());
                        broadcasts.lock().unwrap().push(raw);
                        json!(hash.to_string())
                    }
                    method => panic!("unexpected RPC {method}"),
                };
                Json(json!({"jsonrpc":"2.0", "id":request["id"], "result":result})).into_response()
            }
        }).await;
        let state = AppState {
            pool,
            archive_pool: None,
            api_key: "test".into(),
            ethereum: crate::ethereum::Ethereum::new(
                server.url.clone(),
                format!("0x{}", "11".repeat(20)),
                format!("0x{}", "22".repeat(20)),
                "01".repeat(32),
                "02".repeat(32),
            )
            .unwrap(),
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
            require_proof_approval: true,
            min_remaining_slots: 1_900,
            submission_min_remaining_slots: margin,
            ethereum_finality_mode: crate::indexer::FinalityMode::Finalized,
            ethereum_confirmations: 12,
            sequencer_graphql_url: None,
            inner_public_key: None,
            fee_payer_public_key: None,
            http_client: reqwest::Client::new(),
        };
        Self {
            state,
            fixture,
            slots,
            broadcasts,
            _server: server,
        }
    }

    async fn job(&self, cached: bool) -> Uuid {
        let pool = &self.state.pool;
        sqlx::query("INSERT INTO gateway_config (genesis_timestamp, fork_slot, account_creation_fee, state_hash, recovery_ready) VALUES ('0', 0, '1', 'test', TRUE)")
            .execute(pool).await.unwrap();
        let id = Uuid::new_v4();
        sqlx::query("INSERT INTO proof_jobs (id, kind, status, input, public_values, cycle_count, approved_at, approval_max_pgu, approval_max_price_per_pgu, approval_input_digest) VALUES ($1, 'settlement', 'approved', $2, $3, 100, NOW(), 100, 1, $4)")
            .bind(id).bind(json!({"fixture": "completed-proof-lifetime"}))
            .bind(format!("0x{}", hex::encode(values())))
            .bind(crate::proof_input_digest(&json!({"fixture": "completed-proof-lifetime"})))
            .execute(pool).await.unwrap();
        sqlx::query("INSERT INTO gateway_pending_commands (job_id, public_key, nonce, command_kind, command_base64) VALUES ($1, 'outer', 0, 'zkapp', 'command')")
            .bind(id).execute(pool).await.unwrap();
        if cached {
            // Crash after persisting the completed proof but before signing.
            sqlx::query("UPDATE proof_jobs SET status = 'proving', proof_bytes = $2, proof_request_id = $3 WHERE id = $1")
                .bind(id).bind(format!("0x{}", hex::encode(b"explicit-test-proof")))
                .bind(self.fixture.request_id()).execute(pool).await.unwrap();
            crate::submission::recover_interrupted(pool).await.unwrap();
        }
        id
    }

    async fn run(&self, id: Uuid) {
        let claimed = crate::claim_job(&self.state.pool).await.unwrap().unwrap();
        assert_eq!(claimed.id, id);
        assert!(!claimed.prepared);
        self.fixture
            .run(crate::process_job(&self.state, claimed))
            .await
            .unwrap();
    }

    async fn assert_submitted(&self, id: Uuid) {
        let row = sqlx::query("SELECT status::text AS status, error, proof_bytes, transaction_hash, prepared_transaction, proof_request_id FROM proof_jobs WHERE id = $1")
            .bind(id).fetch_one(&self.state.pool).await.unwrap();
        assert_eq!(
            row.get::<String, _>("status"),
            "submitted",
            "{:?}",
            row.get::<Option<String>, _>("error")
        );
        assert!(row
            .get::<Option<Value>, _>("prepared_transaction")
            .is_some());
        assert!(row.get::<Option<String>, _>("transaction_hash").is_some());
        assert_eq!(
            row.get::<String, _>("proof_bytes"),
            format!("0x{}", hex::encode(b"explicit-test-proof"))
        );
        assert_eq!(
            row.get::<String, _>("proof_request_id"),
            self.fixture.request_id()
        );
        assert_eq!(self.broadcasts.lock().unwrap().len(), 1);
        let pending: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM gateway_pending_commands WHERE job_id = $1")
                .bind(id)
                .fetch_one(&self.state.pool)
                .await
                .unwrap();
        assert_eq!(pending, 1, "submission is not finality");
    }

    async fn assert_expired_without_signing(&self, id: Uuid) {
        let row = sqlx::query("SELECT status::text AS status, error_class, prepared_transaction, transaction_hash FROM proof_jobs WHERE id = $1")
            .bind(id).fetch_one(&self.state.pool).await.unwrap();
        assert_eq!(row.get::<String, _>("status"), "failed");
        assert_eq!(row.get::<String, _>("error_class"), "expired");
        assert!(row
            .get::<Option<Value>, _>("prepared_transaction")
            .is_none());
        assert!(row.get::<Option<String>, _>("transaction_hash").is_none());
        assert!(self.broadcasts.lock().unwrap().is_empty());
    }
}

#[sqlx::test(migrations = "./migrations")]
async fn completed_proof_uses_submission_margin_after_proving(pool: PgPool) {
    let h = Harness::new(pool, 100, 700, 10).await;
    let id = h.job(false).await;
    h.run(id).await;
    h.assert_submitted(id).await;
    assert_eq!(*h.slots.lock().unwrap(), vec![100, 700]);
    assert_eq!(h.fixture.counts().request_proof, 1);
    assert_eq!(h.fixture.counts().wait_proof, 1);
    assert_eq!(h.fixture.counts().preflight, 0);
}

#[sqlx::test(migrations = "./migrations")]
async fn restarted_cached_unsigned_proof_submits_without_buying_or_waiting_again(pool: PgPool) {
    let h = Harness::new(pool, 700, 700, 10).await;
    let id = h.job(true).await;
    h.run(id).await;
    h.assert_submitted(id).await;
    assert_eq!(*h.slots.lock().unwrap(), vec![700]);
    assert_eq!(h.fixture.counts().request_proof, 0);
    assert_eq!(h.fixture.counts().wait_proof, 0);
    assert_eq!(h.fixture.counts().preflight, 0);
}

#[sqlx::test(migrations = "./migrations")]
async fn restarted_approved_request_finishes_without_reapplying_purchase_margin(pool: PgPool) {
    let h = Harness::new(pool, 700, 700, 10).await;
    let id = h.job(false).await;
    sqlx::query("UPDATE proof_jobs SET status = 'proof_requested', proof_request_intent = $2, proof_request_id = $3 WHERE id = $1")
        .bind(id).bind(Uuid::new_v4()).bind(h.fixture.request_id())
        .execute(&h.state.pool).await.unwrap();
    crate::submission::recover_interrupted(&h.state.pool)
        .await
        .unwrap();
    let status: String = sqlx::query_scalar("SELECT status::text FROM proof_jobs WHERE id = $1")
        .bind(id)
        .fetch_one(&h.state.pool)
        .await
        .unwrap();
    assert_eq!(
        status, "approved",
        "the recovered job keeps its original approval"
    );
    h.run(id).await;
    h.assert_submitted(id).await;
    assert_eq!(*h.slots.lock().unwrap(), vec![700]);
    assert_eq!(h.fixture.counts().request_proof, 0);
    assert_eq!(h.fixture.counts().wait_proof, 1);
    assert_eq!(h.fixture.counts().preflight, 0);
}

#[sqlx::test(migrations = "./migrations")]
async fn completed_proof_below_submission_margin_is_rejected_before_signing(pool: PgPool) {
    let h = Harness::new(pool, 100, u64::from(SLOT_UPPER) - 9, 10).await;
    let id = h.job(false).await;
    h.run(id).await;
    h.assert_expired_without_signing(id).await;
    assert_eq!(h.fixture.counts().wait_proof, 1);
}

#[sqlx::test(migrations = "./migrations")]
async fn completed_proof_at_submission_margin_is_accepted(pool: PgPool) {
    let h = Harness::new(pool, 100, u64::from(SLOT_UPPER) - 10, 10).await;
    let id = h.job(false).await;
    h.run(id).await;
    h.assert_submitted(id).await;
}

#[sqlx::test(migrations = "./migrations")]
async fn expired_cached_proof_is_rejected_even_with_zero_submission_margin(pool: PgPool) {
    let h = Harness::new(
        pool,
        u64::from(SLOT_UPPER) + 1,
        u64::from(SLOT_UPPER) + 1,
        0,
    )
    .await;
    let id = h.job(true).await;
    h.run(id).await;
    h.assert_expired_without_signing(id).await;
    assert_eq!(h.fixture.counts().request_proof, 0);
    assert_eq!(h.fixture.counts().wait_proof, 0);
}

#[sqlx::test(migrations = "./migrations")]
async fn proof_expiring_during_proving_is_rejected_even_with_zero_submission_margin(pool: PgPool) {
    let h = Harness::new(pool, 100, u64::from(SLOT_UPPER) + 1, 0).await;
    let id = h.job(false).await;
    h.run(id).await;
    h.assert_expired_without_signing(id).await;
    assert_eq!(h.fixture.counts().request_proof, 1);
    assert_eq!(h.fixture.counts().wait_proof, 1);
}

#[sqlx::test(migrations = "./migrations")]
async fn cached_proof_at_inclusive_slot_upper_is_accepted_with_zero_margin(pool: PgPool) {
    let h = Harness::new(pool, u64::from(SLOT_UPPER), u64::from(SLOT_UPPER), 0).await;
    let id = h.job(true).await;
    h.run(id).await;
    h.assert_submitted(id).await;
}

#[sqlx::test(migrations = "./migrations")]
async fn proof_purchase_still_requires_the_larger_preproof_window(pool: PgPool) {
    let h = Harness::new(pool, 700, 700, 10).await;
    let id = h.job(false).await;
    h.run(id).await;
    h.assert_expired_without_signing(id).await;
    assert_eq!(h.fixture.counts().request_proof, 0);
    assert_eq!(h.fixture.counts().wait_proof, 0);
}
