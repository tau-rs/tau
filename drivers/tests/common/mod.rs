//! A stub Messages API: one `TcpListener`, canned answers, and a record of
//! what the driver actually sent. Real HTTP over a real socket, so the
//! driver's transport path — including abort and timeout — is what is under
//! test, and nothing in the crate's public surface exists only for tests.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, dead_code)]

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, Notify};

/// One request the stub received.
#[derive(Debug, Clone)]
pub(crate) struct Captured {
    /// The request line and headers, verbatim.
    pub(crate) head: String,
    /// The body bytes.
    pub(crate) body: Vec<u8>,
}

impl Captured {
    /// The request path from the request line (`POST <path> HTTP/1.1`).
    pub(crate) fn path(&self) -> &str {
        self.head
            .lines()
            .next()
            .and_then(|line| line.split(' ').nth(1))
            .unwrap_or("")
    }

    /// The value of header `name` (case-insensitive), if present.
    pub(crate) fn header(&self, name: &str) -> Option<&str> {
        self.head.lines().find_map(|line| {
            let (k, v) = line.split_once(':')?;
            k.eq_ignore_ascii_case(name).then(|| v.trim())
        })
    }

    /// The body, parsed as JSON.
    pub(crate) fn json(&self) -> serde_json::Value {
        serde_json::from_slice(&self.body).expect("captured body is JSON")
    }
}

/// How the stub answers.
#[derive(Debug, Clone)]
pub(crate) enum Answer {
    /// An HTTP status and a body, `content-type: application/json`.
    Json(u16, String),
    /// Read the request, then hold the connection open until the client
    /// closes it, and signal `closed` when that happens.
    Hang(Arc<Notify>),
    /// One `(status, body)` per connection, in order; a connection past the
    /// end of the script gets a 500 so the test fails loudly.
    Script(Arc<Mutex<VecDeque<(u16, String)>>>),
}

/// A [`Answer::Script`] over `answers`, in order.
pub(crate) fn script(answers: impl IntoIterator<Item = (u16, String)>) -> Answer {
    Answer::Script(Arc::new(Mutex::new(answers.into_iter().collect())))
}

/// A running stub.
pub(crate) struct Stub {
    /// The base URL to point a driver at.
    pub(crate) base_url: String,
    /// Every request the stub has read, in order.
    pub(crate) captured: mpsc::UnboundedReceiver<Captured>,
}

/// Which answer a request path gets. `None` as the path is the catch-all.
type Routes = Arc<Vec<(Option<&'static str>, Answer)>>;

/// Starts a stub that answers every request, whatever its path, with
/// `answer`.
pub(crate) async fn start(answer: Answer) -> Stub {
    serve(Arc::new(vec![(None, answer)])).await
}

/// Starts a stub that answers by exact request path. A path not listed
/// gets a 404 with a JSON body, so a driver that hits the wrong endpoint
/// fails loudly instead of being served the answer meant for another.
pub(crate) async fn start_routed(routes: Vec<(&'static str, Answer)>) -> Stub {
    let routes = routes
        .into_iter()
        .map(|(path, answer)| (Some(path), answer))
        .collect();
    serve(Arc::new(routes)).await
}

async fn serve(routes: Routes) -> Stub {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (tx, captured) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                return;
            };
            let tx = tx.clone();
            let routes = Arc::clone(&routes);
            tokio::spawn(read_request(stream, routes, tx));
        }
    });
    Stub {
        base_url: format!("http://{addr}"),
        captured,
    }
}

/// A base URL nothing listens on: the port is bound, then released.
pub(crate) async fn refused_base_url() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    format!("http://{addr}")
}

async fn read_request(
    mut stream: TcpStream,
    routes: Routes,
    tx: mpsc::UnboundedSender<Captured>,
) -> Option<()> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let head_end = loop {
        if let Some(pos) = find(&buf, b"\r\n\r\n") {
            break pos + 4;
        }
        let n = stream.read(&mut chunk).await.ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(chunk.get(..n)?);
    };
    let head = String::from_utf8_lossy(buf.get(..head_end)?).into_owned();
    let length: usize = head
        .lines()
        .find_map(|l| {
            let (k, v) = l.split_once(':')?;
            k.eq_ignore_ascii_case("content-length")
                .then(|| v.trim().parse().ok())
                .flatten()
        })
        .unwrap_or(0);
    let mut body = buf.get(head_end..)?.to_vec();
    while body.len() < length {
        let n = stream.read(&mut chunk).await.ok()?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(chunk.get(..n)?);
    }
    let captured = Captured { head, body };
    let answer = routes
        .iter()
        .find(|(path, _)| path.is_none_or(|p| p == captured.path()))
        .map(|(_, answer)| answer.clone())
        .unwrap_or_else(|| {
            Answer::Json(
                404,
                format!(
                    r#"{{"type":"error","error":{{"type":"not_found_error","message":"no route for {}"}}}}"#,
                    captured.path()
                ),
            )
        });
    // Reported before answering: a `Hang` answer only ends when the client
    // goes away, and the test needs to know the request arrived before that.
    let _ = tx.send(captured);
    let answer = match answer {
        Answer::Script(script) => {
            let next = script.lock().unwrap().pop_front();
            let (status, body) = next.unwrap_or((
                500,
                r#"{"type":"error","error":{"type":"stub","message":"script exhausted"}}"#.into(),
            ));
            Answer::Json(status, body)
        }
        other => other,
    };
    match answer {
        Answer::Json(status, body) => {
            let reason = match status {
                200 => "OK",
                400 => "Bad Request",
                404 => "Not Found",
                429 => "Too Many Requests",
                500 => "Internal Server Error",
                529 => "Overloaded",
                _ => "Whatever",
            };
            let response = format!(
                "HTTP/1.1 {status} {reason}\r\ncontent-type: application/json\r\n\
                 content-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes()).await;
            let _ = stream.shutdown().await;
        }
        Answer::Hang(closed) => {
            // Anything the client does next — including going away — ends
            // this read; EOF or an error both mean the connection closed.
            let mut scratch = [0u8; 64];
            let _ = stream.read(&mut scratch).await;
            closed.notify_one();
        }
        Answer::Script(_) => unreachable!("resolved above"),
    }
    Some(())
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}
