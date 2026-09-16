//! What every HTTP model driver does the same way: register a call so
//! `abandon` can reach it, POST a body with a timeout on each half of the
//! exchange, race that against the abandon signal, and encode the reply.
//!
//! The provider decides the endpoint, the headers and the timeout; the
//! wire format and the mapping of a 200 stay with the driver. Nothing here
//! is public: the drivers' public surface is theirs.
//!
//! # Abandon before handle
//!
//! The kernel computes phase two of `cancel` from the sender's open corrs,
//! which include a delivery still queued for this driver, so `abandon` can
//! precede `handle`. A corr is never reused within a run, so an early
//! abandon is remembered and the call it names answers `transport` without
//! going out. Registration is synchronous in `handle`, before the future is
//! first polled, so an abandon in between finds the entry.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::Duration;

use tau_kernel::abi::Corr;
use tau_kernel::bridge::{ErrorKind, ModelError, ModelReply, StopReason, Usage, VERSION};
use tokio::sync::Notify;

use super::ConfigError;

/// One HTTP client, the headers it always sends, the timeout it enforces,
/// and the table of calls `abandon` can reach.
pub(crate) struct Transport {
    client: reqwest::Client,
    /// Sent on every request, after `content-type`. The API key is one of
    /// them, which is why `Debug` does not print this.
    headers: Vec<(&'static str, String)>,
    timeout: Duration,
    flights: Arc<Mutex<Flights>>,
}

/// The calls `abandon` can reach. Ordered maps, not hashed: the workspace
/// forbids the latter, and these are never large.
#[derive(Debug, Default)]
struct Flights {
    /// corr → the signal `abandon` pulls. An entry lives exactly as long as
    /// the call: [`Flight`] removes it on drop.
    open: BTreeMap<Corr, Arc<Notify>>,
    /// Abandoned before the driver saw the request; see the module docs.
    abandoned_early: BTreeSet<Corr>,
}

/// One call's registration, removed when the call ends however it ends —
/// including a driver task torn down at shutdown.
pub(crate) struct Flight {
    flights: Arc<Mutex<Flights>>,
    corr: Corr,
    notify: Arc<Notify>,
}

impl Flight {
    /// The signal `abandon` pulls for this call; race the exchange against
    /// it.
    pub(crate) fn notify(&self) -> &Notify {
        &self.notify
    }
}

impl Drop for Flight {
    fn drop(&mut self) {
        lock(&self.flights).open.remove(&self.corr);
    }
}

/// What one HTTP exchange produced, before mapping.
pub(crate) enum Exchange {
    /// The provider answered; here is the status and the body.
    Answered {
        /// The HTTP status.
        status: u16,
        /// The body, whole.
        body: Vec<u8>,
    },
    /// It did not: connection, timeout, or abandon.
    Lost(String),
}

fn lock(flights: &Mutex<Flights>) -> MutexGuard<'_, Flights> {
    flights.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The one `reqwest::Client` of the process, cloned into every transport.
///
/// Building a client loads the platform root store — about 120 ms on macOS,
/// and serialized inside the system framework, so a harness that builds many
/// drivers pays it many times over. A `Client` is an `Arc` inside, and
/// reqwest's own advice is to build one and reuse it: cloning costs nothing
/// and the connection pool is per host either way, so every driver sees the
/// same behaviour it saw with a client of its own. The build is attempted
/// once; its error, if any, is remembered and returned to every caller.
fn shared_client() -> Result<reqwest::Client, ConfigError> {
    static CLIENT: OnceLock<Result<reqwest::Client, String>> = OnceLock::new();
    CLIENT
        .get_or_init(|| {
            reqwest::Client::builder()
                .build()
                .map_err(|e| e.to_string())
        })
        .clone()
        .map_err(ConfigError::Client)
}

impl Transport {
    /// Takes a clone of the process's client. No client-side timeout:
    /// [`exchange`](Self::exchange) enforces `timeout` with `tokio::time`,
    /// and that is what turns into `error.transport`.
    ///
    /// # Errors
    ///
    /// [`ConfigError::Client`] if the client cannot be built (a TLS backend
    /// problem).
    pub(crate) fn new(
        headers: Vec<(&'static str, String)>,
        timeout: Duration,
    ) -> Result<Self, ConfigError> {
        Ok(Self {
            client: shared_client()?,
            headers,
            timeout,
            flights: Arc::new(Mutex::new(Flights::default())),
        })
    }

    /// `base_url`, trailing slashes trimmed, with `path` appended.
    ///
    /// # Errors
    ///
    /// [`ConfigError::BadBaseUrl`] if the result does not parse.
    pub(crate) fn endpoint(base_url: &str, path: &str) -> Result<reqwest::Url, ConfigError> {
        let raw = format!("{}{path}", base_url.trim_end_matches('/'));
        reqwest::Url::parse(&raw).map_err(|e| ConfigError::BadBaseUrl {
            url: base_url.to_owned(),
            reason: e.to_string(),
        })
    }

    /// Registers a call. Synchronous, so the caller can do it in `handle`
    /// before returning the future. An abandon that arrived earlier is
    /// honoured: the flight's signal is already pulled.
    pub(crate) fn enter(&self, corr: Corr) -> Flight {
        let notify = Arc::new(Notify::new());
        {
            let mut flights = lock(&self.flights);
            if flights.abandoned_early.remove(&corr) {
                notify.notify_one();
            }
            flights.open.insert(corr, Arc::clone(&notify));
        }
        Flight {
            flights: Arc::clone(&self.flights),
            corr,
            notify,
        }
    }

    /// Pulls the signal of an open call, or remembers the corr if the call
    /// has not been entered yet.
    pub(crate) fn abandon(&self, corr: Corr) {
        let mut flights = lock(&self.flights);
        match flights.open.get(&corr) {
            // A permit is stored if nobody is waiting yet, so an abandon that
            // lands before the `select!` is reached still takes effect.
            Some(notify) => notify.notify_one(),
            None => {
                flights.abandoned_early.insert(corr);
            }
        }
    }

    /// How many calls are in flight right now.
    pub(crate) fn in_flight(&self) -> usize {
        lock(&self.flights).open.len()
    }

    /// POSTs `bytes` as JSON to `endpoint`, raced against `notify`. Each
    /// half of the exchange — the answer, then the body — gets the full
    /// timeout.
    pub(crate) async fn exchange(
        &self,
        endpoint: &reqwest::Url,
        bytes: Vec<u8>,
        notify: &Notify,
    ) -> Exchange {
        tokio::select! {
            exchange = self.post(endpoint, bytes) => exchange,
            () = notify.notified() => Exchange::Lost("abandoned by cancel".to_owned()),
        }
    }

    async fn post(&self, endpoint: &reqwest::Url, bytes: Vec<u8>) -> Exchange {
        let mut send = self
            .client
            .post(endpoint.clone())
            .header("content-type", "application/json");
        for (name, value) in &self.headers {
            send = send.header(*name, value);
        }
        let send = send.body(bytes).send();
        let response = match tokio::time::timeout(self.timeout, send).await {
            Ok(Ok(response)) => response,
            Ok(Err(e)) => return Exchange::Lost(format!("request failed: {}", without_url(e))),
            Err(_elapsed) => return Exchange::Lost(format!("no answer within {:?}", self.timeout)),
        };
        let status = response.status().as_u16();
        match tokio::time::timeout(self.timeout, response.bytes()).await {
            Ok(Ok(body)) => Exchange::Answered {
                status,
                body: body.to_vec(),
            },
            Ok(Err(e)) => Exchange::Lost(format!("body failed: {}", without_url(e))),
            Err(_elapsed) => Exchange::Lost(format!("no body within {:?}", self.timeout)),
        }
    }
}

impl fmt::Debug for Transport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Transport")
            .field("timeout", &self.timeout)
            .field("in_flight", &self.in_flight())
            .finish_non_exhaustive()
    }
}

/// A reply whose stop is an error, carrying whatever `usage` was billed.
pub(crate) fn error_reply(
    kind: ErrorKind,
    message: String,
    model: Option<String>,
    usage: Usage,
) -> ModelReply {
    ModelReply {
        v: VERSION,
        model,
        content: Vec::new(),
        stop: StopReason::Error(ModelError { kind, message }),
        usage,
    }
}

/// reqwest's `Display` includes the URL, which may carry a query string the
/// operator considers sensitive; the base URL is config, not something to
/// echo into every agent's mailbox.
fn without_url(e: reqwest::Error) -> String {
    e.without_url().to_string()
}

/// A reply, as bytes. Serializing these types cannot fail in practice
/// (`Value` inputs came from JSON); if it ever does, the reply says so
/// instead of being empty.
pub(crate) fn encode(reply: &ModelReply) -> Vec<u8> {
    serde_json::to_vec(reply).unwrap_or_else(|_| {
        format!(
            r#"{{"v":{VERSION},"content":[],"stop":{{"error":{{"kind":"provider","message":"reply could not be serialized"}}}},"usage":{{"input_tokens":0,"output_tokens":0}}}}"#
        )
        .into_bytes()
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;

    fn transport() -> Transport {
        Transport::new(
            vec![("x-api-key", "sk-secret".to_owned())],
            Duration::from_secs(1),
        )
        .unwrap()
    }

    /// Whether the flight's signal has already been pulled.
    async fn fired(flight: &Flight) -> bool {
        tokio::time::timeout(Duration::from_millis(50), flight.notify().notified())
            .await
            .is_ok()
    }

    #[tokio::test]
    async fn an_abandon_after_enter_pulls_the_signal_and_drop_removes_the_entry() {
        let t = transport();
        let flight = t.enter(Corr::new(1));
        assert_eq!(t.in_flight(), 1);
        assert!(!fired(&flight).await, "nothing pulled it yet");

        t.abandon(Corr::new(1));
        assert!(fired(&flight).await);

        drop(flight);
        assert_eq!(t.in_flight(), 0);
    }

    #[tokio::test]
    async fn an_abandon_before_enter_is_remembered_once() {
        let t = transport();
        t.abandon(Corr::new(2));
        assert_eq!(t.in_flight(), 0);

        let flight = t.enter(Corr::new(2));
        assert!(fired(&flight).await, "the early abandon was honoured");
        drop(flight);

        // Once consumed, it does not fire a second entry with the same corr
        // (which never happens within a run) nor any other corr.
        let again = t.enter(Corr::new(2));
        assert!(!fired(&again).await);
        let other = t.enter(Corr::new(3));
        assert!(!fired(&other).await);
        assert_eq!(t.in_flight(), 2);
    }

    #[tokio::test]
    async fn abandoning_a_finished_call_does_not_reach_a_later_one() {
        let t = transport();
        let flight = t.enter(Corr::new(4));
        drop(flight);
        t.abandon(Corr::new(4));
        let other = t.enter(Corr::new(5));
        assert!(!fired(&other).await);
    }

    #[tokio::test]
    async fn a_pulled_signal_ends_the_exchange_as_abandoned() {
        let t = transport();
        let flight = t.enter(Corr::new(6));
        t.abandon(Corr::new(6));
        // A port nothing listens on: had the abandon not won, this would be
        // `request failed`.
        let endpoint = Transport::endpoint("http://127.0.0.1:9/", "/v1/x").unwrap();
        match t.exchange(&endpoint, b"{}".to_vec(), flight.notify()).await {
            Exchange::Lost(message) => assert_eq!(message, "abandoned by cancel"),
            Exchange::Answered { .. } => panic!("nothing answers on port 9"),
        }
    }

    #[test]
    fn endpoints_trim_the_slash_and_reject_garbage() {
        assert_eq!(
            Transport::endpoint("http://localhost:8000/", "/v1/chat/completions")
                .unwrap()
                .as_str(),
            "http://localhost:8000/v1/chat/completions"
        );
        assert!(matches!(
            Transport::endpoint("not a url", "/v1/messages"),
            Err(ConfigError::BadBaseUrl { url, .. }) if url == "not a url"
        ));
    }

    #[test]
    fn debug_output_never_carries_a_header_value() {
        let shown = format!("{:?}", transport());
        assert!(!shown.contains("sk-secret"), "{shown}");
        assert!(shown.contains("in_flight: 0"), "{shown}");
    }

    #[test]
    fn encode_never_returns_nothing() {
        let reply = ModelReply {
            v: VERSION,
            model: None,
            content: vec![],
            stop: StopReason::EndTurn,
            usage: Usage::default(),
        };
        let parsed: ModelReply = serde_json::from_slice(&encode(&reply)).unwrap();
        assert_eq!(parsed, reply);
    }
}
