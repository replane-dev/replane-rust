use std::sync::{Arc, Mutex};
use std::time::Duration;

use replane::{ConnectOptions, Context, Replane, ReplaneError};
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;

/// A request received by the mock server, with a handle to stream the response.
struct Request {
    headers: String,
    body: Value,
    socket: TcpStream,
}

impl Request {
    async fn respond_sse(&mut self) {
        let head = "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncache-control: no-cache\r\nconnection: close\r\n\r\n";
        self.socket.write_all(head.as_bytes()).await.unwrap();
        self.send_raw(": connected\n\n").await;
    }

    async fn respond_status(mut self, status: &str, body: &str) {
        let resp = format!(
            "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        );
        self.socket.write_all(resp.as_bytes()).await.unwrap();
    }

    async fn send(&mut self, record: Value) {
        self.send_raw(&format!("data: {record}\n\n")).await;
    }

    async fn send_raw(&mut self, raw: &str) {
        self.socket.write_all(raw.as_bytes()).await.unwrap();
        self.socket.flush().await.unwrap();
    }
}

async fn mock_server() -> (String, mpsc::UnboundedReceiver<Request>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let (tx, rx) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        loop {
            let (socket, _) = listener.accept().await.unwrap();
            let tx = tx.clone();
            tokio::spawn(async move {
                if let Some(request) = read_request(socket).await {
                    let _ = tx.send(request);
                }
            });
        }
    });
    (url, rx)
}

async fn read_request(mut socket: TcpStream) -> Option<Request> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let header_end = loop {
        let n = socket.read(&mut chunk).await.ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break pos + 4;
        }
    };
    let headers = String::from_utf8_lossy(&buf[..header_end]).to_lowercase();
    let content_length: usize = headers
        .lines()
        .find_map(|l| l.strip_prefix("content-length:"))
        .map_or(0, |v| v.trim().parse().unwrap());
    while buf.len() < header_end + content_length {
        let n = socket.read(&mut chunk).await.ok()?;
        buf.extend_from_slice(&chunk[..n]);
    }
    let body = serde_json::from_slice(&buf[header_end..header_end + content_length])
        .unwrap_or(Value::Null);
    Some(Request {
        headers,
        body,
        socket,
    })
}

fn init(configs: Value) -> Value {
    json!({"type": "init", "configs": configs})
}

fn options(url: &str) -> ConnectOptions {
    ConnectOptions::new(url, "rp_test_key")
        .connect_timeout(Duration::from_secs(2))
        .retry_delay(Duration::from_millis(20))
}

async fn next_request(rx: &mut mpsc::UnboundedReceiver<Request>) -> Request {
    tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("timed out waiting for a request")
        .expect("server stopped")
}

#[tokio::test]
async fn receives_configs_and_evaluates_overrides() {
    let (url, mut rx) = mock_server().await;
    let replane = Replane::builder()
        .default_value("only-default", "d")
        .build();

    let server = tokio::spawn(async move {
        let mut req = next_request(&mut rx).await;
        assert!(req
            .headers
            .starts_with("post /api/sdk/v1/replication/stream "));
        assert!(req.headers.contains("authorization: bearer rp_test_key"));
        assert!(req.headers.contains("user-agent: replane-rust-sdk/"));
        assert_eq!(req.body["requiredConfigs"], json!([]));
        assert_eq!(req.body["currentConfigs"][0]["name"], "only-default");
        req.respond_sse().await;
        req.send(init(json!([{
            "name": "rate-limit",
            "value": 100,
            "overrides": [{"name": "pro", "value": 1000, "conditions": [
                {"operator": "equals", "property": "plan", "value": "pro"}
            ]}]
        }])))
        .await;
        req
    });

    replane.connect(options(&url)).await.unwrap();
    assert!(replane.is_connected());
    assert_eq!(replane.get::<u32>("rate-limit").unwrap(), 100);
    assert_eq!(
        replane
            .get_with::<u32>("rate-limit", &Context::from([("plan", "pro")]))
            .unwrap(),
        1000
    );
    assert_eq!(
        replane
            .with_context(&Context::from([("plan", "pro")]))
            .get::<u32>("rate-limit")
            .unwrap(),
        1000
    );
    assert_eq!(replane.get::<String>("only-default").unwrap(), "d");
    assert!(matches!(
        replane.get::<u32>("missing"),
        Err(ReplaneError::NotFound { .. })
    ));
    assert!(matches!(
        replane.get::<String>("rate-limit"),
        Err(ReplaneError::Deserialize { .. })
    ));
    assert_eq!(replane.get_or("missing", 7), 7);
    drop(server.await.unwrap());
}

#[tokio::test]
async fn notifies_subscribers_on_change() {
    let (url, mut rx) = mock_server().await;
    let replane = Replane::default();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let seen_clone = seen.clone();
    let subscription = replane.subscribe("flag", move |change| {
        seen_clone.lock().unwrap().push(change.value.clone());
    });
    let _other = replane.subscribe("other", |_| panic!("subscriber panics are contained"));

    let (send_change, mut changes) = mpsc::unbounded_channel::<Value>();
    tokio::spawn(async move {
        let mut req = next_request(&mut rx).await;
        req.respond_sse().await;
        req.send(init(json!([
            {"name": "flag", "value": false, "overrides": []},
            {"name": "other", "value": 1, "overrides": []}
        ])))
        .await;
        while let Some(record) = changes.recv().await {
            req.send(record).await;
        }
    });

    replane.connect(options(&url)).await.unwrap();
    assert!(!replane.get::<bool>("flag").unwrap());

    send_change.send(json!({"type": "config_change", "config": {"name": "flag", "value": true, "overrides": []}})).unwrap();
    // Same value again: no notification.
    send_change.send(json!({"type": "config_change", "config": {"name": "flag", "value": true, "overrides": []}})).unwrap();
    send_change
        .send(json!({"type": "future_record", "x": 1}))
        .unwrap();
    send_change.send(json!({"type": "config_change", "config": {"name": "other", "value": 2, "overrides": []}})).unwrap();
    wait_until(|| replane.get::<i32>("other").ok() == Some(2)).await;

    assert!(replane.get::<bool>("flag").unwrap());
    assert_eq!(*seen.lock().unwrap(), vec![json!(false), json!(true)]);

    drop(subscription);
    send_change.send(json!({"type": "config_change", "config": {"name": "flag", "value": false, "overrides": []}})).unwrap();
    wait_until(|| replane.get::<bool>("flag").ok() == Some(false)).await;
    assert_eq!(seen.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn reconnects_with_current_configs_after_disconnect() {
    let (url, mut rx) = mock_server().await;
    let replane = Replane::default();

    let (reconnected_tx, reconnected_rx) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        let mut first = next_request(&mut rx).await;
        first.respond_sse().await;
        first
            .send(init(json!([{"name": "a", "value": 1, "overrides": []}])))
            .await;
        drop(first);

        let failing = next_request(&mut rx).await;
        failing
            .respond_status("503 Service Unavailable", "{}")
            .await;

        let mut second = next_request(&mut rx).await;
        let _ = reconnected_tx.send(second.body.clone());
        second.respond_sse().await;
        second
            .send(init(json!([{"name": "a", "value": 2, "overrides": []}])))
            .await;
        std::future::pending::<()>().await;
    });

    replane.connect(options(&url)).await.unwrap();
    let body = tokio::time::timeout(Duration::from_secs(3), reconnected_rx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        body["currentConfigs"],
        json!([{"name": "a", "value": 1, "overrides": []}])
    );
    wait_until(|| replane.get::<i32>("a").ok() == Some(2)).await;
}

#[tokio::test]
async fn reconnects_after_inactivity() {
    let (url, mut rx) = mock_server().await;
    let replane = Replane::default();

    tokio::spawn(async move {
        let mut first = next_request(&mut rx).await;
        first.respond_sse().await;
        first
            .send(init(json!([{"name": "a", "value": 1, "overrides": []}])))
            .await;
        // Keep the socket open but silent.
        let mut second = next_request(&mut rx).await;
        drop(first);
        second.respond_sse().await;
        second
            .send(init(json!([{"name": "a", "value": 2, "overrides": []}])))
            .await;
        std::future::pending::<()>().await;
    });

    replane
        .connect(options(&url).inactivity_timeout(Duration::from_millis(200)))
        .await
        .unwrap();
    wait_until(|| replane.get::<i32>("a").ok() == Some(2)).await;
}

#[tokio::test]
async fn connect_times_out_with_last_error() {
    let (url, mut rx) = mock_server().await;
    tokio::spawn(async move {
        while let Some(req) = rx.recv().await {
            req.respond_status("401 Unauthorized", r#"{"msg":"Invalid SDK key"}"#)
                .await;
        }
    });

    let replane = Replane::builder().default_value("a", 1).build();
    let err = replane
        .connect(options(&url).connect_timeout(Duration::from_millis(300)))
        .await
        .unwrap_err();
    match &err {
        ReplaneError::Timeout {
            last_error: Some(last),
            ..
        } => assert!(matches!(**last, ReplaneError::Auth)),
        other => panic!("unexpected error: {other:?}"),
    }
    assert_eq!(err.code(), "timeout");
    assert!(!replane.is_connected());
    assert_eq!(replane.get::<i32>("a").unwrap(), 1);
}

#[tokio::test]
async fn dropping_client_closes_connection() {
    let (url, mut rx) = mock_server().await;
    let replane = Replane::default();

    let (closed_tx, closed_rx) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        let mut req = next_request(&mut rx).await;
        req.respond_sse().await;
        req.send(init(json!([]))).await;
        let mut buf = [0u8; 1];
        let n = req.socket.read(&mut buf).await.unwrap_or(0);
        let _ = closed_tx.send(n);
    });

    replane.connect(options(&url)).await.unwrap();
    drop(replane);
    let n = tokio::time::timeout(Duration::from_secs(2), closed_rx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(n, 0, "client should close the socket");
}

#[tokio::test]
async fn rejects_missing_options() {
    let replane = Replane::default();
    let err = replane
        .connect(ConnectOptions::new("", "key"))
        .await
        .unwrap_err();
    assert!(matches!(err, ReplaneError::InvalidOptions(_)));
}

async fn wait_until(mut condition: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(3), async {
        while !condition() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("condition not reached in time");
}
