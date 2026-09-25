//! One HTTP connection pool and request budget for every Ethereum provider,
//! including wallet fillers. Only read operations are retried here. Signed
//! transaction retries belong to the durable submission state machine.
use alloy::{
    primitives::B256,
    rpc::{
        client::RpcClient,
        json_rpc::{RequestPacket, ResponsePacket},
    },
    transports::{TransportError, TransportErrorKind, TransportFut, TransportResult},
};
use anyhow::{Context, Result};
use reqwest::{header::RETRY_AFTER, Client, Url};
use std::{
    future::Future,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    task::{Context as TaskContext, Poll},
    time::Duration,
};
use tokio::{
    sync::{Mutex, Semaphore},
    time::{sleep_until, Instant},
};
use tower_service::Service;

#[derive(Clone, Debug)]
pub struct RpcConfig {
    pub requests_per_second: u32,
    pub max_concurrent: usize,
    pub max_retries: u32,
    pub initial_backoff: Duration,
    pub max_backoff: Duration,
    pub request_timeout: Duration,
    pub operation_timeout: Duration,
}

impl RpcConfig {
    pub fn from_env() -> Result<Self> {
        fn number(name: &str, default: u64) -> Result<u64> {
            std::env::var(name).map_or(Ok(default), |value| {
                value.parse().with_context(|| format!("invalid {name}"))
            })
        }
        let config = Self {
            requests_per_second: number("ETHEREUM_RPC_REQUESTS_PER_SECOND", 8)?.try_into()?,
            max_concurrent: number("ETHEREUM_RPC_MAX_CONCURRENT", 4)?.try_into()?,
            max_retries: number("ETHEREUM_RPC_MAX_RETRIES", 5)?.try_into()?,
            initial_backoff: Duration::from_millis(number("ETHEREUM_RPC_INITIAL_BACKOFF_MS", 500)?),
            max_backoff: Duration::from_millis(number("ETHEREUM_RPC_MAX_BACKOFF_MS", 10_000)?),
            request_timeout: Duration::from_millis(number(
                "ETHEREUM_RPC_REQUEST_TIMEOUT_MS",
                30_000,
            )?),
            operation_timeout: Duration::from_millis(number(
                "ETHEREUM_RPC_OPERATION_TIMEOUT_MS",
                90_000,
            )?),
        };
        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> Result<()> {
        anyhow::ensure!(
            self.requests_per_second > 0 && self.max_concurrent > 0,
            "Ethereum RPC rate and concurrency limits must be positive"
        );
        anyhow::ensure!(
            !self.initial_backoff.is_zero() && self.initial_backoff <= self.max_backoff,
            "invalid Ethereum RPC backoff bounds"
        );
        anyhow::ensure!(
            !self.request_timeout.is_zero() && self.request_timeout <= self.operation_timeout,
            "invalid Ethereum RPC timeout bounds"
        );
        Ok(())
    }
}

#[derive(Debug)]
struct Budget {
    next_request: Mutex<Instant>,
    concurrent: Semaphore,
    jitter: AtomicU64,
}

/// Exhausted transient reads are distinguishable from contract reverts.
#[derive(Debug)]
pub struct RpcUnavailable {
    cause: Option<TransportError>,
}
impl std::fmt::Display for RpcUnavailable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Ethereum RPC unavailable after bounded retries; retry the durable stage")?;
        if let Some(cause) = &self.cause {
            write!(f, ": {cause}")?;
        }
        Ok(())
    }
}
impl std::error::Error for RpcUnavailable {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.cause
            .as_ref()
            .map(|cause| cause as &dyn std::error::Error)
    }
}

/// The node can no longer serve the canonical hash selected for a snapshot.
/// Retry the complete durable stage with a new header, not the same eth_call.
#[derive(Debug)]
pub struct SnapshotUnavailable {
    block_hash: B256,
    cause: anyhow::Error,
}

impl std::fmt::Display for SnapshotUnavailable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Ethereum snapshot {} is unavailable; retry the stage with a fresh canonical block: {}",
            self.block_hash, self.cause
        )
    }
}

impl std::error::Error for SnapshotUnavailable {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.cause.as_ref())
    }
}

/// Apply only at a hash-pinned read boundary. The same RPC codes also describe
/// contract failures, so neither the code alone nor arbitrary error text is
/// enough to turn a failure into a retryable snapshot error.
pub async fn read_pinned_snapshot<T>(
    block_hash: B256,
    read: impl Future<Output = Result<T>>,
) -> Result<T> {
    match read.await {
        Err(error)
            if error.chain().filter_map(rpc_error).any(|error| {
                error.as_error_resp().is_some_and(|response| {
                    matches!(response.code, -32000 | -32001 | -32603)
                        && matches!(
                            response.message.trim().to_ascii_lowercase().as_str(),
                            "hash is not currently canonical"
                                | "block is not canonical"
                                | "block not canonical"
                                | "block hash is not canonical"
                                | "header for hash not found"
                                | "header not found"
                                | "block not found"
                                | "unknown block"
                                | "resource not found"
                        )
                })
            }) =>
        {
            Err(SnapshotUnavailable {
                block_hash,
                cause: error,
            }
            .into())
        }
        result => result,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FailureKind {
    SnapshotUnavailable,
    Transient,
    InsufficientFunds,
    Other,
}

fn rpc_error<'a>(cause: &'a (dyn std::error::Error + 'static)) -> Option<&'a TransportError> {
    cause.downcast_ref::<TransportError>().or_else(|| {
        // Transparent contract wrappers can omit the RPC enum from source().
        match cause.downcast_ref::<alloy::contract::Error>() {
            Some(alloy::contract::Error::TransportError(error)) => Some(error),
            _ => None,
        }
    })
}

pub fn is_rate_limited(error: &anyhow::Error) -> bool {
    error.chain().filter_map(rpc_error).any(|error| {
        error
            .as_transport_err()
            .and_then(TransportErrorKind::as_http_error)
            .is_some_and(|error| error.status == 429)
            || error.as_error_resp().is_some_and(|error| {
                matches!(error.code, 429 | -32005 | -32007)
                    || error.message.to_ascii_lowercase().contains("rate limit")
                    || error
                        .message
                        .to_ascii_lowercase()
                        .contains("too many requests")
            })
    })
}

pub fn classify(error: &anyhow::Error) -> FailureKind {
    for cause in error.chain() {
        if cause.is::<SnapshotUnavailable>() {
            return FailureKind::SnapshotUnavailable;
        }
        if cause.is::<RpcUnavailable>() {
            return FailureKind::Transient;
        }
        if let Some(error) = cause.downcast_ref::<reqwest::Error>() {
            if error.is_timeout() || error.is_connect() || error.is_request() || error.is_body() {
                return FailureKind::Transient;
            }
        }
        if let Some(error) = rpc_error(cause) {
            if let Some(response) = error.as_error_resp() {
                if response
                    .message
                    .to_ascii_lowercase()
                    .contains("insufficient funds")
                {
                    return FailureKind::InsufficientFunds;
                }
                if transient_rpc_error(response.code, &response.message) {
                    return FailureKind::Transient;
                }
            }
            if error
                .as_transport_err()
                .and_then(TransportErrorKind::as_http_error)
                .is_some_and(|error| transient_http_status(error.status))
            {
                return FailureKind::Transient;
            }
        }
    }
    FailureKind::Other
}

#[derive(Clone, Debug)]
pub struct RpcTransport {
    client: Client,
    url: Url,
    config: RpcConfig,
    budget: Arc<Budget>,
}

impl RpcTransport {
    pub fn new(url: &str, config: RpcConfig) -> Result<Self> {
        config.validate()?;
        let url: Url = url.parse().context("invalid Ethereum RPC URL")?;
        anyhow::ensure!(
            matches!(url.scheme(), "https" | "http"),
            "Ethereum RPC URL must use HTTP(S)"
        );
        // reqwest must not replay a POST underneath our explicit read/send policy.
        let client = Client::builder()
            .retry(reqwest::retry::never())
            .timeout(config.request_timeout)
            .build()?;
        Ok(Self {
            client,
            url,
            budget: Arc::new(Budget {
                next_request: Mutex::new(Instant::now()),
                concurrent: Semaphore::new(config.max_concurrent),
                jitter: AtomicU64::new(u64::from_le_bytes(
                    uuid::Uuid::new_v4().as_bytes()[..8].try_into().unwrap(),
                )),
            }),
            config,
        })
    }

    pub fn into_client(self) -> RpcClient {
        let is_local = matches!(self.url.host_str(), Some("localhost" | "127.0.0.1" | "::1"));
        RpcClient::builder().transport(self, is_local)
    }

    async fn pace(&self, cost: u32) {
        loop {
            let mut next = self.budget.next_request.lock().await;
            let now = Instant::now();
            if *next <= now {
                *next = now
                    + Duration::from_secs_f64(
                        f64::from(cost) / f64::from(self.config.requests_per_second),
                    );
                return;
            }
            let wake_at = *next;
            drop(next);
            sleep_until(wake_at).await;
        }
    }

    async fn cooldown(&self, delay: Duration) {
        let mut next = self.budget.next_request.lock().await;
        *next = (*next).max(Instant::now() + delay);
    }

    fn backoff(&self, retry: u32) -> Duration {
        let ceiling = self
            .config
            .initial_backoff
            .saturating_mul(1u32.checked_shl(retry).unwrap_or(u32::MAX))
            .min(self.config.max_backoff);
        // Equal jitter, independent across processes and shared by this client's clones.
        let seed = self
            .budget
            .jitter
            .fetch_add(0x9e3779b97f4a7c15, Ordering::Relaxed);
        let mixed = (seed ^ (seed >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
        ceiling / 2 + ceiling.mul_f64((mixed as f64 / u64::MAX as f64) / 2.0)
    }

    async fn execute(&self, request: RequestPacket) -> TransportResult<ResponsePacket> {
        let cost = u32::try_from(request.method_names().count())
            .unwrap_or(u32::MAX)
            .max(1);
        let read_only = request.method_names().all(is_read_method);
        for attempt in 0..=self.config.max_retries {
            let permit = self
                .budget
                .concurrent
                .acquire()
                .await
                .map_err(TransportErrorKind::custom)?;
            self.pace(cost).await;
            let result = self
                .client
                .post(self.url.clone())
                .headers(request.headers())
                .json(&request)
                .send()
                .await;
            let (result, transient, hint) = match result {
                Err(error) => {
                    let transient = error.is_timeout()
                        || error.is_connect()
                        || error.is_request()
                        || error.is_body();
                    (
                        Err(TransportErrorKind::custom(error.without_url())),
                        transient,
                        None,
                    )
                }
                Ok(response) => {
                    let status = response.status();
                    let hint = response
                        .headers()
                        .get(RETRY_AFTER)
                        .and_then(|value| value.to_str().ok())
                        .and_then(retry_after);
                    match response.bytes().await {
                        Err(error) => (
                            Err(TransportErrorKind::custom(error.without_url())),
                            true,
                            hint,
                        ),
                        Ok(body) if !status.is_success() => (
                            Err(TransportErrorKind::http_error(
                                status.as_u16(),
                                String::from_utf8_lossy(&body).into_owned(),
                            )),
                            transient_http_status(status.as_u16()),
                            hint,
                        ),
                        Ok(body) => {
                            let parsed =
                                serde_json::from_slice::<ResponsePacket>(&body).map_err(|error| {
                                    TransportError::deser_err(error, String::from_utf8_lossy(&body))
                                });
                            let transient = parsed.as_ref().is_ok_and(|packet| {
                                packet
                                    .iter_errors()
                                    .any(|error| transient_rpc_error(error.code, &error.message))
                            });
                            let rpc_hint = parsed.as_ref().ok().and_then(|packet| {
                                packet
                                    .iter_errors()
                                    .filter_map(|error| {
                                        let data =
                                            error.try_data_as::<serde_json::Value>()?.ok()?;
                                        data.pointer("/rate/backoff_seconds")?
                                            .as_f64()
                                            .filter(|seconds| {
                                                seconds.is_finite()
                                                    && *seconds >= 0.0
                                                    && *seconds <= 86_400.0
                                            })
                                            .map(Duration::from_secs_f64)
                                    })
                                    .max()
                            });
                            (parsed, transient, hint.or(rpc_hint))
                        }
                    }
                }
            };
            drop(permit);
            if transient {
                // Share cooldown even when the send must be reconciled by its caller.
                self.cooldown(hint.unwrap_or_else(|| self.backoff(attempt)))
                    .await;
            }
            if !transient || !read_only {
                return result;
            }
            if attempt == self.config.max_retries {
                let cause = match result {
                    Err(error) => Some(error),
                    Ok(packet) => packet
                        .iter_errors()
                        .next()
                        .cloned()
                        .map(TransportError::err_resp),
                };
                return Err(TransportErrorKind::custom(RpcUnavailable { cause }));
            }
            tracing::warn!(
                attempt = attempt + 1,
                "retrying transient Ethereum RPC read failure"
            );
        }
        unreachable!()
    }
}

fn is_read_method(method: &str) -> bool {
    matches!(
        method,
        "eth_call"
            | "eth_estimateGas"
            | "eth_chainId"
            | "eth_blockNumber"
            | "eth_gasPrice"
            | "eth_maxPriorityFeePerGas"
            | "eth_feeHistory"
            | "net_version"
            | "eth_getBlockByNumber"
            | "eth_getBlockByHash"
            | "eth_getTransactionByHash"
            | "eth_getTransactionReceipt"
            | "eth_getTransactionCount"
            | "eth_getLogs"
            | "eth_getBalance"
            | "eth_getCode"
            | "eth_getStorageAt"
            | "eth_getProof"
            | "eth_getBlockReceipts"
    )
}

fn transient_rpc_error(code: i64, message: &str) -> bool {
    let message = message.to_ascii_lowercase();
    matches!(code, 429 | -32005 | -32007)
        || message.contains("rate limit")
        || message.contains("too many requests")
        || message.contains("temporarily unavailable")
}

fn transient_http_status(status: u16) -> bool {
    matches!(status, 408 | 429 | 500 | 502 | 503 | 504)
}

fn retry_after(value: &str) -> Option<Duration> {
    if let Ok(seconds) = value.parse::<u64>() {
        return Some(Duration::from_secs(seconds.min(86_400)));
    }
    let timestamp = chrono::DateTime::parse_from_rfc2822(value).ok()?;
    (timestamp.with_timezone(&chrono::Utc) - chrono::Utc::now())
        .to_std()
        .ok()
        .map(|delay| delay.min(Duration::from_secs(86_400)))
}

impl Service<RequestPacket> for RpcTransport {
    type Response = ResponsePacket;
    type Error = TransportError;
    type Future = TransportFut<'static>;

    fn poll_ready(&mut self, _: &mut TaskContext<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, request: RequestPacket) -> Self::Future {
        let this = self.clone();
        Box::pin(async move {
            tokio::time::timeout(this.config.operation_timeout, this.execute(request))
                .await
                .map_err(|_| TransportErrorKind::custom(RpcUnavailable { cause: None }))?
        })
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use axum::{response::Response, routing::post, Json, Router};
    use serde_json::Value;
    use std::future::Future;

    pub struct TestServer {
        pub url: String,
        task: tokio::task::JoinHandle<()>,
    }

    impl Drop for TestServer {
        fn drop(&mut self) {
            self.task.abort();
        }
    }

    pub async fn serve<F, Fut>(handler: F) -> TestServer
    where
        F: Fn(Value) -> Fut + Clone + Send + Sync + 'static,
        Fut: Future<Output = Response> + Send + 'static,
    {
        let app = Router::new().route(
            "/",
            post(move |Json(request): Json<Value>| handler(request)),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        TestServer { url, task }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::rpc::json_rpc::{Id, Request};
    use axum::{http::StatusCode, response::IntoResponse, Json};
    use serde_json::{json, Value};
    use std::sync::atomic::AtomicUsize;

    fn config() -> RpcConfig {
        RpcConfig {
            requests_per_second: 100_000,
            max_concurrent: 2,
            max_retries: 2,
            initial_backoff: Duration::from_millis(2),
            max_backoff: Duration::from_millis(10),
            request_timeout: Duration::from_millis(200),
            operation_timeout: Duration::from_secs(3),
        }
    }

    fn request(method: &'static str) -> RequestPacket {
        Request::new(method, Id::Number(1), Vec::<Value>::new())
            .serialize()
            .unwrap()
            .into()
    }

    #[tokio::test]
    async fn retries_http_and_json_rate_limits_then_succeeds() {
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = calls.clone();
        let server = test_support::serve(move |request| {
            let attempt = counter.fetch_add(1, Ordering::SeqCst);
            async move {
                match attempt {
                    0 => (StatusCode::TOO_MANY_REQUESTS, [("retry-after", "0")], "limit").into_response(),
                    1 => Json(json!({"jsonrpc":"2.0", "id":request["id"], "error":{"code":-32007,"message":"rate limit exceeded"}})).into_response(),
                    _ => Json(json!({"jsonrpc":"2.0", "id":request["id"], "result":"0x1"})).into_response(),
                }
            }
        }).await;
        let mut transport = RpcTransport::new(&server.url, config()).unwrap();
        assert!(transport
            .call(request("eth_chainId"))
            .await
            .unwrap()
            .is_success());
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn transient_reads_are_bounded_and_typed() {
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = calls.clone();
        let server = test_support::serve(move |_| {
            counter.fetch_add(1, Ordering::SeqCst);
            async { StatusCode::SERVICE_UNAVAILABLE.into_response() }
        })
        .await;
        let mut transport = RpcTransport::new(&server.url, config()).unwrap();
        let error = transport
            .call(request("eth_blockNumber"))
            .await
            .unwrap_err();
        assert_eq!(classify(&error.into()), FailureKind::Transient);
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn exhausted_rate_limits_retain_their_typed_cause() {
        let server =
            test_support::serve(|_| async { StatusCode::TOO_MANY_REQUESTS.into_response() }).await;
        let mut transport = RpcTransport::new(&server.url, config()).unwrap();
        let error = anyhow::Error::new(transport.call(request("eth_chainId")).await.unwrap_err());
        assert_eq!(classify(&error), FailureKind::Transient);
        assert!(is_rate_limited(&error));
    }

    #[tokio::test]
    async fn read_timeouts_retry_but_send_timeouts_do_not() {
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = calls.clone();
        let server = test_support::serve(move |request| {
            counter.fetch_add(1, Ordering::SeqCst);
            async move {
                tokio::time::sleep(Duration::from_millis(100)).await;
                Json(json!({"jsonrpc":"2.0", "id":request["id"], "result":"0x1"})).into_response()
            }
        })
        .await;
        let mut config = config();
        config.request_timeout = Duration::from_millis(25);
        let mut transport = RpcTransport::new(&server.url, config).unwrap();
        let error = transport.call(request("eth_chainId")).await.unwrap_err();
        assert_eq!(classify(&error.into()), FailureKind::Transient);
        assert_eq!(calls.load(Ordering::SeqCst), 3);
        transport
            .call(request("eth_sendRawTransaction"))
            .await
            .unwrap_err();
        assert_eq!(calls.load(Ordering::SeqCst), 4);
    }

    #[tokio::test]
    async fn raw_sends_and_contract_reverts_are_not_replayed() {
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = calls.clone();
        let server = test_support::serve(move |request| {
            counter.fetch_add(1, Ordering::SeqCst);
            async move {
                if request["method"] == "eth_sendRawTransaction" {
                    StatusCode::BAD_GATEWAY.into_response()
                } else {
                    Json(json!({"jsonrpc":"2.0", "id":request["id"], "error":{"code":-32000,"message":"execution reverted: action state mismatch"}})).into_response()
                }
            }
        }).await;
        let mut transport = RpcTransport::new(&server.url, config()).unwrap();
        assert!(transport
            .call(request("eth_sendRawTransaction"))
            .await
            .is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(transport
            .call(request("eth_call"))
            .await
            .unwrap()
            .is_error());
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn clones_share_concurrency_limit() {
        let active = Arc::new(AtomicUsize::new(0));
        let maximum = Arc::new(AtomicUsize::new(0));
        let in_flight = active.clone();
        let max = maximum.clone();
        let server = test_support::serve(move |request| {
            let active = in_flight.clone();
            let maximum = max.clone();
            async move {
                let current = active.fetch_add(1, Ordering::SeqCst) + 1;
                maximum.fetch_max(current, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(30)).await;
                active.fetch_sub(1, Ordering::SeqCst);
                Json(json!({"jsonrpc":"2.0", "id":request["id"], "result":"0x1"})).into_response()
            }
        })
        .await;
        let transport = RpcTransport::new(&server.url, config()).unwrap();
        let mut tasks = tokio::task::JoinSet::new();
        for _ in 0..8 {
            let mut clone = transport.clone();
            tasks.spawn(async move {
                clone.call(request("eth_chainId")).await.unwrap();
            });
        }
        while let Some(result) = tasks.join_next().await {
            result.unwrap();
        }
        assert_eq!(maximum.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn batches_pay_for_every_rpc_call_and_clones_share_pacing() {
        let server = test_support::serve(|request| async move {
            let response =
                |request: &Value| json!({"jsonrpc":"2.0", "id":request["id"], "result":"0x1"});
            Json(match request.as_array() {
                Some(requests) => Value::Array(requests.iter().map(response).collect()),
                None => response(&request),
            })
            .into_response()
        })
        .await;
        let mut config = config();
        config.requests_per_second = 20;
        let mut transport = RpcTransport::new(&server.url, config).unwrap();
        let mut batch = request("eth_chainId");
        batch.push(
            Request::new("eth_blockNumber", Id::Number(2), Vec::<Value>::new())
                .serialize()
                .unwrap(),
        );
        let started = Instant::now();
        transport.call(batch).await.unwrap();
        transport
            .clone()
            .call(request("eth_chainId"))
            .await
            .unwrap();
        assert!(started.elapsed() >= Duration::from_millis(100));
    }

    #[tokio::test]
    async fn retry_after_cools_down_other_clones() {
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = calls.clone();
        let server = test_support::serve(move |request| {
            let attempt = counter.fetch_add(1, Ordering::SeqCst);
            async move {
                if attempt == 0 {
                    (
                        StatusCode::TOO_MANY_REQUESTS,
                        [("retry-after", "1")],
                        "limit",
                    )
                        .into_response()
                } else {
                    Json(json!({"jsonrpc":"2.0", "id":request["id"], "result":"0x1"}))
                        .into_response()
                }
            }
        })
        .await;
        let mut transport = RpcTransport::new(&server.url, config()).unwrap();
        // Sends do not retry, but their rate-limit hint must throttle readers too.
        transport
            .call(request("eth_sendRawTransaction"))
            .await
            .unwrap_err();
        let started = Instant::now();
        transport
            .clone()
            .call(request("eth_chainId"))
            .await
            .unwrap();
        assert!(started.elapsed() >= Duration::from_millis(990));
    }

    #[test]
    fn insufficient_funds_has_a_distinct_typed_classification() {
        let error: TransportError = TransportError::err_resp(alloy::rpc::json_rpc::ErrorPayload {
            code: -32000,
            message: "insufficient funds for gas * price + value".into(),
            data: None,
        });
        assert_eq!(
            classify(&anyhow::Error::new(error)),
            FailureKind::InsufficientFunds
        );
        let wrapped = alloy::contract::Error::TransportError(TransportError::err_resp(
            alloy::rpc::json_rpc::ErrorPayload {
                code: -32000,
                message: "insufficient funds for gas * price + value".into(),
                data: None,
            },
        ));
        assert_eq!(
            classify(&anyhow::Error::new(wrapped)),
            FailureKind::InsufficientFunds
        );
    }

    #[tokio::test]
    async fn snapshot_errors_require_a_pinned_read_and_a_specific_rpc_response() {
        for (code, message, expected) in [
            (
                -32000,
                "hash is not currently canonical",
                FailureKind::SnapshotUnavailable,
            ),
            (
                -32001,
                "header for hash not found",
                FailureKind::SnapshotUnavailable,
            ),
            (-32001, "block not found", FailureKind::SnapshotUnavailable),
            (
                -32001,
                "Resource not found",
                FailureKind::SnapshotUnavailable,
            ),
            (
                -32603,
                "block not canonical",
                FailureKind::SnapshotUnavailable,
            ),
            (-32000, "execution reverted", FailureKind::Other),
            (
                -32000,
                "execution reverted: header not found",
                FailureKind::Other,
            ),
            (
                -32602,
                "invalid argument 1: expected block number",
                FailureKind::Other,
            ),
            (3, "hash is not currently canonical", FailureKind::Other),
        ] {
            let error = anyhow::Error::new(alloy::contract::Error::TransportError(
                TransportError::err_resp(alloy::rpc::json_rpc::ErrorPayload {
                    code,
                    message: message.into(),
                    data: None,
                }),
            ));
            assert_eq!(
                classify(&error),
                FailureKind::Other,
                "unpinned error: {message}"
            );
            let error = read_pinned_snapshot::<()>(B256::repeat_byte(1), async { Err(error) })
                .await
                .unwrap_err();
            assert_eq!(classify(&error), expected, "{message}");
            assert_eq!(
                error.is::<SnapshotUnavailable>(),
                expected == FailureKind::SnapshotUnavailable
            );
        }
        let error = read_pinned_snapshot::<()>(B256::repeat_byte(1), async {
            anyhow::bail!("header for hash not found")
        })
        .await
        .unwrap_err();
        assert_eq!(
            classify(&error),
            FailureKind::Other,
            "arbitrary text is not an RPC response"
        );
    }
}
