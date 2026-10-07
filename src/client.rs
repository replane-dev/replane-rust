use std::collections::HashMap;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError, RwLock, Weak};
use std::time::Duration;

use futures_util::StreamExt;
use serde::de::DeserializeOwned;
use serde_json::Value;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

use crate::context::Context;
use crate::error::{ReplaneError, Result};
use crate::evaluation::evaluate_overrides;
use crate::sse::{SseEvent, SseParser};
use crate::types::{
    Config, ConfigChange, ReplicationStreamRecord, Snapshot, StartReplicationStreamBody,
};

/// Context key holding an ID generated per client, usable for segmentation.
/// A value provided in the client context takes precedence.
pub const REPLANE_CLIENT_ID_KEY: &str = "replaneClientId";

const MAX_RETRY_DELAY: Duration = Duration::from_secs(10);
const DEFAULT_AGENT: &str = concat!("replane-rust-sdk/", env!("CARGO_PKG_VERSION"));

type Callback = Arc<dyn Fn(&ConfigChange) + Send + Sync>;

/// Replane client.
///
/// Usable immediately with defaults or a snapshot; call [`Replane::connect`] to
/// receive live updates. Cloning is cheap and clones share configs and the
/// connection. The connection is closed when the last clone is dropped.
///
/// ```no_run
/// # async fn run() -> replane::Result<()> {
/// use replane::{ConnectOptions, Context, Replane};
///
/// let replane = Replane::builder()
///     .default_value("new-checkout", false)
///     .connect(ConnectOptions::new("https://replane.example.com", "rp_..."))
///     .await?;
///
/// let ctx = Context::new().with("userId", "u-42").with("plan", "pro");
/// let enabled: bool = replane.get_with("new-checkout", &ctx)?;
/// # Ok(()) }
/// ```
#[derive(Clone)]
pub struct Replane {
    shared: Arc<Shared>,
    context: Arc<Context>,
}

struct Shared {
    configs: RwLock<HashMap<String, Config>>,
    subscribers: Mutex<HashMap<String, Vec<(u64, Callback)>>>,
    next_subscriber_id: AtomicU64,
    connection: Mutex<Option<JoinHandle<()>>>,
}

impl Drop for Shared {
    fn drop(&mut self) {
        if let Some(handle) = lock(&self.connection).take() {
            handle.abort();
        }
    }
}

/// Builder for [`Replane`].
#[derive(Default)]
pub struct ReplaneBuilder {
    context: Context,
    defaults: Vec<(String, Value)>,
    snapshot: Option<Snapshot>,
}

impl ReplaneBuilder {
    /// Context applied to every evaluation; per-call context takes precedence.
    pub fn context(mut self, context: impl Into<Context>) -> Self {
        self.context = context.into();
        self
    }

    /// Value used until the server provides one, and if the server doesn't have the config.
    ///
    /// # Panics
    ///
    /// If `value` can't be serialized to JSON.
    pub fn default_value(mut self, name: impl Into<String>, value: impl serde::Serialize) -> Self {
        let value = serde_json::to_value(value).expect("default value must serialize to JSON");
        self.defaults.push((name.into(), value));
        self
    }

    pub fn defaults<K: Into<String>, V: Into<Value>>(
        mut self,
        defaults: impl IntoIterator<Item = (K, V)>,
    ) -> Self {
        self.defaults
            .extend(defaults.into_iter().map(|(k, v)| (k.into(), v.into())));
        self
    }

    /// Configs from another client's [`Replane::snapshot`]; they take precedence over defaults.
    pub fn snapshot(mut self, snapshot: Snapshot) -> Self {
        self.snapshot = Some(snapshot);
        self
    }

    /// Creates a client that works in-memory until [`Replane::connect`] is called.
    pub fn build(self) -> Replane {
        let mut configs: HashMap<String, Config> = self
            .defaults
            .into_iter()
            .map(|(name, value)| {
                let config = Config {
                    name: name.clone(),
                    value,
                    overrides: Vec::new(),
                };
                (name, config)
            })
            .collect();
        for config in self.snapshot.into_iter().flat_map(|s| s.configs) {
            configs.insert(config.name.clone(), config);
        }

        let context = Context::new()
            .with(REPLANE_CLIENT_ID_KEY, generate_client_id())
            .merged(&self.context);

        Replane {
            shared: Arc::new(Shared {
                configs: RwLock::new(configs),
                subscribers: Mutex::new(HashMap::new()),
                next_subscriber_id: AtomicU64::new(0),
                connection: Mutex::new(None),
            }),
            context: Arc::new(context),
        }
    }

    /// Builds the client and connects it. See [`Replane::connect`].
    pub async fn connect(self, options: ConnectOptions) -> Result<Replane> {
        let replane = self.build();
        replane.connect(options).await?;
        Ok(replane)
    }
}

/// Options for connecting to a Replane server.
#[derive(Debug, Clone)]
pub struct ConnectOptions {
    base_url: String,
    sdk_key: String,
    connect_timeout: Duration,
    request_timeout: Duration,
    inactivity_timeout: Duration,
    retry_delay: Duration,
    agent: String,
    http_client: Option<reqwest::Client>,
}

impl ConnectOptions {
    /// `base_url` is the Replane instance URL, e.g. `https://replane.example.com`.
    pub fn new(base_url: impl Into<String>, sdk_key: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_owned(),
            sdk_key: sdk_key.into(),
            connect_timeout: Duration::from_secs(5),
            request_timeout: Duration::from_secs(2),
            inactivity_timeout: Duration::from_secs(30),
            retry_delay: Duration::from_millis(200),
            agent: DEFAULT_AGENT.to_owned(),
            http_client: None,
        }
    }

    /// How long [`Replane::connect`] waits for the initial configs. Default: 5s.
    pub fn connect_timeout(mut self, timeout: Duration) -> Self {
        self.connect_timeout = timeout;
        self
    }

    /// Timeout for establishing each stream request. Default: 2s.
    pub fn request_timeout(mut self, timeout: Duration) -> Self {
        self.request_timeout = timeout;
        self
    }

    /// Reconnect when nothing (including heartbeats) arrives for this long. Default: 30s.
    pub fn inactivity_timeout(mut self, timeout: Duration) -> Self {
        self.inactivity_timeout = timeout;
        self
    }

    /// Initial delay between reconnect attempts, doubled on each failure up to 10s. Default: 200ms.
    pub fn retry_delay(mut self, delay: Duration) -> Self {
        self.retry_delay = delay;
        self
    }

    /// Value of the `User-Agent` header. Default: `replane-rust-sdk/<version>`.
    pub fn agent(mut self, agent: impl Into<String>) -> Self {
        self.agent = agent.into();
        self
    }

    /// HTTP client to use, e.g. to configure proxies or TLS. It must not have a
    /// total request timeout, since the replication stream is long-lived.
    pub fn http_client(mut self, client: reqwest::Client) -> Self {
        self.http_client = Some(client);
        self
    }
}

impl Replane {
    pub fn builder() -> ReplaneBuilder {
        ReplaneBuilder::default()
    }

    /// Connects to the server and waits until the initial configs arrive.
    ///
    /// After that the client stays connected, reconnecting with backoff when
    /// the connection drops, until [`Replane::disconnect`] is called or the last
    /// clone is dropped. Connecting again replaces the existing connection.
    ///
    /// Must be called within a Tokio runtime.
    pub async fn connect(&self, options: ConnectOptions) -> Result<()> {
        if options.base_url.is_empty() {
            return Err(ReplaneError::InvalidOptions("base_url is required".into()));
        }
        if options.sdk_key.is_empty() {
            return Err(ReplaneError::InvalidOptions("sdk_key is required".into()));
        }
        let http = match options.http_client.clone() {
            Some(client) => client,
            None => reqwest::Client::builder()
                .connect_timeout(options.request_timeout)
                .build()?,
        };

        self.disconnect();

        let (ready_tx, ready_rx) = oneshot::channel();
        let last_error = Arc::new(Mutex::new(None));
        let connect_timeout = options.connect_timeout;
        let stream = Stream {
            shared: Arc::downgrade(&self.shared),
            options,
            http,
            ready: Some(ready_tx),
            last_error: last_error.clone(),
        };
        *lock(&self.shared.connection) = Some(tokio::spawn(stream.run()));

        match tokio::time::timeout(connect_timeout, ready_rx).await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(_)) => Err(ReplaneError::Protocol(
                "connection task stopped unexpectedly".into(),
            )),
            Err(_) => {
                self.disconnect();
                Err(ReplaneError::Timeout {
                    timeout: connect_timeout,
                    last_error: lock(&last_error).take().map(Box::new),
                })
            }
        }
    }

    /// Stops receiving updates. The client keeps serving the configs it has.
    pub fn disconnect(&self) {
        if let Some(handle) = lock(&self.shared.connection).take() {
            handle.abort();
        }
    }

    pub fn is_connected(&self) -> bool {
        lock(&self.shared.connection)
            .as_ref()
            .is_some_and(|h| !h.is_finished())
    }

    /// Returns the config value for the client context, with overrides applied.
    ///
    /// Use `T = serde_json::Value` to get the raw JSON value.
    pub fn get<T: DeserializeOwned>(&self, name: &str) -> Result<T> {
        self.evaluate(name, &self.context)
    }

    /// Like [`Replane::get`], with `context` merged over the client context.
    pub fn get_with<T: DeserializeOwned>(&self, name: &str, context: &Context) -> Result<T> {
        self.evaluate(name, &self.context.merged(context))
    }

    /// Like [`Replane::get`], but returns `default` if the config is missing or
    /// can't be deserialized into `T`.
    pub fn get_or<T: DeserializeOwned>(&self, name: &str, default: T) -> T {
        match self.get(name) {
            Ok(value) => value,
            Err(ReplaneError::NotFound { .. }) => default,
            Err(e) => {
                tracing::warn!("Replane: {e}, using default");
                default
            }
        }
    }

    /// Returns a client sharing this client's configs and connection, with
    /// `context` merged over its context.
    pub fn with_context(&self, context: &Context) -> Replane {
        Replane {
            shared: self.shared.clone(),
            context: Arc::new(self.context.merged(context)),
        }
    }

    /// Calls `callback` whenever the config changes on the server. Unsubscribes on drop.
    ///
    /// The callback receives the base value and runs on the connection task, so it
    /// should return quickly.
    #[must_use = "the subscription is cancelled when dropped"]
    pub fn subscribe<F>(&self, name: &str, callback: F) -> Subscription
    where
        F: Fn(&ConfigChange) + Send + Sync + 'static,
    {
        let id = self
            .shared
            .next_subscriber_id
            .fetch_add(1, Ordering::Relaxed);
        lock(&self.shared.subscribers)
            .entry(name.to_owned())
            .or_default()
            .push((id, Arc::new(callback)));
        Subscription {
            shared: Arc::downgrade(&self.shared),
            name: name.to_owned(),
            id,
        }
    }

    /// Returns a serializable copy of the current configs.
    pub fn snapshot(&self) -> Snapshot {
        let mut configs: Vec<Config> = read(&self.shared.configs).values().cloned().collect();
        configs.sort_by(|a, b| a.name.cmp(&b.name));
        Snapshot { configs }
    }

    fn evaluate<T: DeserializeOwned>(&self, name: &str, context: &Context) -> Result<T> {
        let configs = read(&self.shared.configs);
        let config = configs.get(name).ok_or_else(|| ReplaneError::NotFound {
            name: name.to_owned(),
        })?;
        let value = evaluate_overrides(&config.value, &config.overrides, context);
        T::deserialize(value).map_err(|source| ReplaneError::Deserialize {
            name: name.to_owned(),
            source,
        })
    }
}

impl Default for Replane {
    fn default() -> Self {
        Replane::builder().build()
    }
}

impl std::fmt::Debug for Replane {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Replane")
            .field("configs", &read(&self.shared.configs).len())
            .field("connected", &self.is_connected())
            .finish()
    }
}

/// Handle returned by [`Replane::subscribe`]; dropping it unsubscribes.
#[derive(Debug)]
pub struct Subscription {
    shared: Weak<Shared>,
    name: String,
    id: u64,
}

impl Subscription {
    /// Keeps the subscription active for the lifetime of the client.
    pub fn detach(self) {
        std::mem::forget(self);
    }
}

impl Drop for Subscription {
    fn drop(&mut self) {
        let Some(shared) = self.shared.upgrade() else {
            return;
        };
        let mut subscribers = lock(&shared.subscribers);
        if let Some(callbacks) = subscribers.get_mut(&self.name) {
            callbacks.retain(|(id, _)| *id != self.id);
            if callbacks.is_empty() {
                subscribers.remove(&self.name);
            }
        }
    }
}

/// The background replication stream. Holds only a weak reference to the client
/// state so that dropping the client stops it.
struct Stream {
    shared: Weak<Shared>,
    options: ConnectOptions,
    http: reqwest::Client,
    ready: Option<oneshot::Sender<()>>,
    last_error: Arc<Mutex<Option<ReplaneError>>>,
}

enum StreamEnd {
    /// The server closed the stream.
    Closed,
    /// The client was dropped.
    Stopped,
}

impl Stream {
    async fn run(mut self) {
        let mut failed_attempts = 0u32;
        loop {
            let delay = match self.connect_and_stream(&mut failed_attempts).await {
                Ok(StreamEnd::Stopped) => return,
                Ok(StreamEnd::Closed) => {
                    tracing::info!("Replane: stream closed by server, reconnecting");
                    self.options.retry_delay
                }
                Err(e) => {
                    failed_attempts += 1;
                    let delay = self
                        .options
                        .retry_delay
                        .saturating_mul(2u32.saturating_pow(failed_attempts - 1))
                        .min(MAX_RETRY_DELAY);
                    tracing::error!(
                        "Replane: replication stream failed, retrying in {delay:?}: {e}"
                    );
                    *lock(&self.last_error) = Some(e);
                    delay
                }
            };
            if self.shared.strong_count() == 0 {
                return;
            }
            tokio::time::sleep(with_jitter(delay)).await;
        }
    }

    async fn connect_and_stream(&mut self, failed_attempts: &mut u32) -> Result<StreamEnd> {
        let Some(body) = self.request_body() else {
            return Ok(StreamEnd::Stopped);
        };
        let request = self
            .http
            .post(format!(
                "{}/api/sdk/v1/replication/stream",
                self.options.base_url
            ))
            .bearer_auth(&self.options.sdk_key)
            .header(reqwest::header::ACCEPT, "text/event-stream")
            .header(reqwest::header::USER_AGENT, &self.options.agent)
            .json(&body)
            .send();
        let response = tokio::time::timeout(self.options.request_timeout, request)
            .await
            .map_err(|_| {
                ReplaneError::Protocol(format!(
                    "no response within {:?}",
                    self.options.request_timeout
                ))
            })??;
        let response = ensure_success(response).await?;

        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default();
        if !content_type.contains("text/event-stream") {
            return Err(ReplaneError::Protocol(format!(
                "expected text/event-stream, got {content_type:?}"
            )));
        }
        *failed_attempts = 0;

        let mut body = response.bytes_stream();
        let mut parser = SseParser::default();
        loop {
            let chunk =
                match tokio::time::timeout(self.options.inactivity_timeout, body.next()).await {
                    Err(_) => {
                        return Err(ReplaneError::Protocol(format!(
                            "no events within {:?}",
                            self.options.inactivity_timeout
                        )))
                    }
                    Ok(None) => return Ok(StreamEnd::Closed),
                    Ok(Some(chunk)) => chunk?,
                };
            for event in parser.push(&chunk) {
                let SseEvent::Data(data) = event else {
                    continue;
                };
                let Some(record) = parse_record(&data) else {
                    continue;
                };
                let Some(shared) = self.shared.upgrade() else {
                    return Ok(StreamEnd::Stopped);
                };
                match record {
                    ReplicationStreamRecord::Init { configs } => {
                        tracing::info!("Replane: initialized with {} config(s)", configs.len());
                        apply(&shared, configs);
                    }
                    ReplicationStreamRecord::ConfigChange { config } => {
                        tracing::info!("Replane: config {:?} updated", config.name);
                        apply(&shared, vec![config]);
                    }
                }
                if let Some(ready) = self.ready.take() {
                    let _ = ready.send(());
                }
            }
        }
    }

    /// Sends the configs we already have, so the server can fill in configs it lacks.
    fn request_body(&self) -> Option<Value> {
        let shared = self.shared.upgrade()?;
        let configs: Vec<Config> = read(&shared.configs).values().cloned().collect();
        let body = StartReplicationStreamBody {
            current_configs: &configs,
            required_configs: &[],
        };
        Some(serde_json::to_value(body).expect("configs serialize to JSON"))
    }
}

fn parse_record(data: &str) -> Option<ReplicationStreamRecord> {
    let raw: Value = match serde_json::from_str(data) {
        Ok(raw) => raw,
        Err(e) => {
            tracing::warn!("Replane: ignoring malformed stream event: {e}");
            return None;
        }
    };
    let kind = raw.get("type").and_then(Value::as_str)?.to_owned();
    if kind != "init" && kind != "config_change" {
        tracing::debug!("Replane: ignoring stream event of unknown type {kind:?}");
        return None;
    }
    serde_json::from_value(raw)
        .map_err(|e| tracing::warn!("Replane: ignoring invalid {kind:?} event: {e}"))
        .ok()
}

fn apply(shared: &Shared, configs: Vec<Config>) {
    let mut changes = Vec::new();
    {
        let mut current = write(&shared.configs);
        for config in configs {
            if current.get(&config.name) == Some(&config) {
                continue;
            }
            changes.push(ConfigChange {
                name: config.name.clone(),
                value: config.value.clone(),
            });
            current.insert(config.name.clone(), config);
        }
    }

    for change in changes {
        let callbacks: Vec<Callback> = lock(&shared.subscribers)
            .get(&change.name)
            .map(|cbs| cbs.iter().map(|(_, cb)| cb.clone()).collect())
            .unwrap_or_default();
        for callback in callbacks {
            if catch_unwind(AssertUnwindSafe(|| callback(&change))).is_err() {
                tracing::error!("Replane: subscriber for {:?} panicked", change.name);
            }
        }
    }
}

async fn ensure_success(response: reqwest::Response) -> Result<reqwest::Response> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    let body = response.text().await.unwrap_or_default();
    Err(match status.as_u16() {
        401 => ReplaneError::Auth,
        403 => ReplaneError::Forbidden(body),
        code @ 500.. => ReplaneError::Server { status: code, body },
        code => ReplaneError::Client { status: code, body },
    })
}

/// Spreads reconnects by ±10% so clients don't reconnect in lockstep.
fn with_jitter(delay: Duration) -> Duration {
    delay.mul_f64(0.9 + fastrand::f64() * 0.2)
}

fn generate_client_id() -> String {
    let mut bytes = fastrand::u128(..).to_be_bytes();
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    )
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

fn read<T>(l: &RwLock<T>) -> std::sync::RwLockReadGuard<'_, T> {
    l.read().unwrap_or_else(PoisonError::into_inner)
}

fn write<T>(l: &RwLock<T>) -> std::sync::RwLockWriteGuard<'_, T> {
    l.write().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn client_id_is_uuid_v4() {
        let id = generate_client_id();
        assert_eq!(id.len(), 36);
        assert_eq!(&id[14..15], "4");
        assert!(matches!(&id[19..20], "8" | "9" | "a" | "b"));
    }

    #[test]
    fn snapshot_takes_precedence_over_defaults() {
        let snapshot = Snapshot {
            configs: vec![Config {
                name: "a".into(),
                value: json!("from-snapshot"),
                overrides: vec![],
            }],
        };
        let replane = Replane::builder()
            .default_value("a", "from-default")
            .default_value("b", 2)
            .snapshot(snapshot)
            .build();
        assert_eq!(replane.get::<String>("a").unwrap(), "from-snapshot");
        assert_eq!(replane.get::<i32>("b").unwrap(), 2);
    }
}
