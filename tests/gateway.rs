// Integration tests: admission state machine, auth-aware directory, proxy roundtrip.
use a2a_switchboard::state::{App, PeerState};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use std::sync::Arc;
use tower::ServiceExt; // for oneshot

// Build the app router with a temp data dir + fixed tokens for determinism.
static DIR_SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

async fn test_app() -> (axum::Router, Arc<App>, String, String) {
    let seq = DIR_SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("agw-test-{}-{seq}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let app = Arc::new(App::load(dir).await.unwrap());
    {
        let mut inner = app.inner.write().await;
        inner.gateway_token = "gw_test_token".into();
        inner.bootstrap_token = "boot_test_token".into();
    }
    let router = a2a_switchboard::router(app.clone());
    (
        router,
        app,
        "gw_test_token".into(),
        "boot_test_token".into(),
    )
}

fn req(method: &str, uri: &str, token: Option<&str>, body: Option<&str>) -> Request<Body> {
    let mut b = Request::builder().method(method).uri(uri);
    if let Some(t) = token {
        b = b.header("authorization", format!("Bearer {t}"));
    }
    let body = match body {
        Some(s) => {
            b = b.header("content-type", "application/json");
            Body::from(s.to_string())
        }
        None => Body::empty(),
    };
    b.body(body).unwrap()
}

#[tokio::test]
async fn admission_flow() {
    let (router, app, gw, _boot) = test_app().await;

    // 1. No token → 401
    let r = router
        .clone()
        .oneshot(req(
            "POST",
            "/register",
            None,
            Some(r#"{"name":"a","url":"http://127.0.0.1:1/"}"#),
        ))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::UNAUTHORIZED);

    // 2. Gateway token → pending
    let r = router
        .clone()
        .oneshot(req(
            "POST",
            "/register",
            Some(&gw),
            Some(r#"{"name":"alpha","url":"http://127.0.0.1:1/"}"#),
        ))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::CREATED);
    let inner = app.inner.read().await;
    assert_eq!(inner.peers[0].state, PeerState::Pending);
    drop(inner);

    // 3. Pending peer hidden from unauthenticated directory
    let r = router
        .clone()
        .oneshot(req("GET", "/.well-known/agent.json", None, None))
        .await
        .unwrap();
    let body = axum::body::to_bytes(r.into_body(), 65536).await.unwrap();
    let card: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(card["peers"].as_array().unwrap().len(), 0);

    // 4. Pending peer not proxiable → 403
    let r = router
        .clone()
        .oneshot(req("POST", "/peer/alpha", Some(&gw), Some("{}")))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::FORBIDDEN);

    // 5. Accept via admin action
    let r = router
        .clone()
        .oneshot(req("POST", "/peers/alpha/accept", None, None))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::SEE_OTHER);
    let inner = app.inner.read().await;
    assert_eq!(inner.peers[0].state, PeerState::Accepted);
    drop(inner);

    // 6. Now visible in directory
    let r = router
        .clone()
        .oneshot(req("GET", "/.well-known/agent.json", Some(&gw), None))
        .await
        .unwrap();
    let body = axum::body::to_bytes(r.into_body(), 65536).await.unwrap();
    let card: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let names: Vec<&str> = card["peers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"gateway"), "self-entry listed");
    assert!(names.contains(&"alpha"));

    // 7. Revoked → hidden + 403 again
    let _ = router
        .clone()
        .oneshot(req("POST", "/peers/alpha/revoke", None, None))
        .await
        .unwrap();
    let r = router
        .clone()
        .oneshot(req("POST", "/peer/alpha", Some(&gw), Some("{}")))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::FORBIDDEN);
    let r = router
        .clone()
        .oneshot(req("GET", "/.well-known/agent.json", Some(&gw), None))
        .await
        .unwrap();
    let body = axum::body::to_bytes(r.into_body(), 65536).await.unwrap();
    let card: serde_json::Value = serde_json::from_slice(&body).unwrap();
    // Only the gateway self-entry remains (alpha is revoked and hidden).
    let names: Vec<&str> = card["peers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["gateway"]);
}

#[tokio::test]
async fn trailing_slash_peer_routes_to_proxy() {
    // `/peer/name/` (agent-card URL form) must reach the proxy, not fall
    // through to the admin UI redirect. Upstream unreachable → 502 proves it.
    let (router, _app, _gw, boot) = test_app().await;
    let r = router
        .clone()
        .oneshot(req(
            "POST",
            "/register",
            Some(&boot),
            Some(r#"{"name":"slash","url":"http://127.0.0.1:1/","card":{"name":"slash","skills":[]}}"#),
        ))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::CREATED);
    let r = router
        .clone()
        .oneshot(req("POST", "/peer/slash/", Some(&boot), Some("{}")))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::BAD_GATEWAY);
}

#[tokio::test]
async fn bootstrap_auto_accepts() {
    let (router, app, _gw, boot) = test_app().await;
    let r = router
        .clone()
        .oneshot(req(
            "POST",
            "/register",
            Some(&boot),
            Some(
                r#"{"name":"beta","url":"http://127.0.0.1:1/","card":{"name":"beta","skills":[]}}"#,
            ),
        ))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::CREATED);
    let inner = app.inner.read().await;
    assert_eq!(inner.peers[0].state, PeerState::Accepted);
    assert!(inner.peers[0].auto_accepted);
    drop(inner);
    // immediately proxiable? upstream is unreachable → 502, not 403
    let r = router
        .clone()
        .oneshot(req("POST", "/peer/beta", Some(&boot), Some("{}")))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::BAD_GATEWAY);
}

#[tokio::test]
async fn reject_removes_pending() {
    let (router, app, gw, _boot) = test_app().await;
    let _ = router
        .clone()
        .oneshot(req(
            "POST",
            "/register",
            Some(&gw),
            Some(r#"{"name":"gamma","url":"http://127.0.0.1:1/"}"#),
        ))
        .await
        .unwrap();
    let _ = router
        .clone()
        .oneshot(req("POST", "/peers/gamma/reject", None, None))
        .await
        .unwrap();
    let inner = app.inner.read().await;
    assert!(inner.peers.is_empty());
}

#[tokio::test]
async fn name_conflict_rejected() {
    let (router, app, gw, boot) = test_app().await;
    let _ = router
        .clone()
        .oneshot(req(
            "POST",
            "/register",
            Some(&gw),
            Some(r#"{"name":"dup","url":"http://127.0.0.1:1/"}"#),
        ))
        .await
        .unwrap();
    let r = router
        .clone()
        .oneshot(req(
            "POST",
            "/register",
            Some(&boot),
            Some(r#"{"name":"dup","url":"http://127.0.0.1:2/"}"#),
        ))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::CONFLICT);
    let inner = app.inner.read().await;
    assert_eq!(inner.peers.len(), 1);
}

#[tokio::test]
async fn proxy_roundtrip_and_log() {
    // Fake upstream A2A peer on an ephemeral port.
    let fake = axum::Router::new()
        .route(
            "/",
            axum::routing::post(|| async {
                (
                    axum::http::StatusCode::OK,
                    axum::Json(serde_json::json!({"jsonrpc":"2.0","id":1,"result":{"ok":true}})),
                )
            }),
        )
        .route(
            "/.well-known/agent-card.json",
            axum::routing::get(|| async { axum::Json(serde_json::json!({"name":"fake"})) }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, fake).await.unwrap() });

    let (router, app, _gw, boot) = test_app().await;
    let _ = router
        .clone()
        .oneshot(req(
            "POST",
            "/register",
            Some(&boot),
            Some(&format!(r#"{{"name":"fake","url":"http://{addr}/"}}"#)),
        ))
        .await
        .unwrap();

    let r = router
        .clone()
        .oneshot(req(
            "POST",
            "/peer/fake",
            Some(&boot),
            Some(r#"{"jsonrpc":"2.0","method":"message/send","params":{}}"#),
        ))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    let body = axum::body::to_bytes(r.into_body(), 65536).await.unwrap();
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["result"]["ok"], true);

    // Routing log captured the exchange.
    let log = app.recent_log(10).await;
    assert_eq!(log.len(), 1);
    assert_eq!(log[0].dst, "fake");
    assert_eq!(log[0].status, 200);
    assert_eq!(log[0].src, "bootstrap");
}

#[tokio::test]
async fn proxy_subpath_roundtrip() {
    // /peer/{name}/{rest} must reach the upstream's subpath (regression:
    // Path<String> extractor crashed with 2 segments).
    let fake = axum::Router::new()
        .route(
            "/.well-known/agent-card.json",
            axum::routing::get(|| async { axum::Json(serde_json::json!({"name":"fake"})) }),
        )
        .route("/", axum::routing::post(|| async { "ok" }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, fake).await.unwrap() });

    let (router, _app, _gw, boot) = test_app().await;
    let _ = router
        .clone()
        .oneshot(req(
            "POST",
            "/register",
            Some(&boot),
            Some(&format!(r#"{{"name":"fake","url":"http://{addr}/"}}"#)),
        ))
        .await
        .unwrap();

    let r = router
        .clone()
        .oneshot(req(
            "GET",
            "/peer/fake/.well-known/agent-card.json",
            Some(&boot),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    let body = axum::body::to_bytes(r.into_body(), 65536).await.unwrap();
    assert!(body.windows(4).any(|w| w == b"fake"));
}

fn decode(b64: &str) -> String {
    use base64::Engine as _;
    String::from_utf8(
        base64::engine::general_purpose::STANDARD
            .decode(b64)
            .unwrap(),
    )
    .unwrap()
}

#[tokio::test]
async fn channel_roundtrip_full() {
    // Register a peer with an UNROUTABLE url — proves the channel path, not
    // direct HTTP, carries the request.
    let (router, app, _gw, boot) = test_app().await;
    let reg = r#"{"name":"fw","url":"http://127.0.0.1:1/"}"#.to_string();
    let r = router
        .clone()
        .oneshot(req("POST", "/register", Some(&boot), Some(&reg)))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::CREATED);
    let b = axum::body::to_bytes(r.into_body(), 65536).await.unwrap();
    let ct: String = serde_json::from_slice::<serde_json::Value>(&b).unwrap()["caller_token"]
        .as_str()
        .unwrap()
        .to_string();

    // channel=false in directory before connect
    let r = router
        .clone()
        .oneshot(req("GET", "/.well-known/agent.json", Some(&boot), None))
        .await
        .unwrap();
    let b = axum::body::to_bytes(r.into_body(), 65536).await.unwrap();
    assert!(String::from_utf8_lossy(&b).contains(r#""channel":false"#));

    // Open the channel as the peer (SSE) — the peer's own caller token.
    let sse = router
        .clone()
        .oneshot(req("GET", "/channel?name=fw", Some(&ct), None))
        .await
        .unwrap();
    assert_eq!(sse.status(), StatusCode::OK);
    let mut stream = sse.into_body().into_data_stream();

    // Caller posts /peer/fw concurrently.
    let router2 = router.clone();
    let boot2 = boot.clone();
    let call = tokio::spawn(async move {
        router2
            .oneshot(req(
                "POST",
                "/peer/fw",
                Some(&boot2),
                Some(r#"{"jsonrpc":"2.0","id":"ch1","method":"message/send","params":{"x-api-key":"sk-chan","note":"hi"}}"#),
            ))
            .await
            .unwrap()
    });

    // Read SSE chunks until a request envelope appears.
    use base64::Engine as _;
    use futures_util::StreamExt;
    let mut env: Option<serde_json::Value> = None;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while std::time::Instant::now() < deadline && env.is_none() {
        if let Some(Ok(chunk)) = stream.next().await {
            let text = String::from_utf8_lossy(&chunk);
            if text.contains("event: request") {
                let data = text.split("data: ").nth(1).unwrap_or_default().trim();
                env = serde_json::from_str(data).ok();
            }
        }
    }
    let env = env.expect("request envelope not delivered");
    let id = env["id"].as_u64().unwrap();
    let secret = env["chan_secret"].as_str().unwrap().to_string();
    assert_eq!(env["method"], "POST");
    let body_in = decode(env["body_b64"].as_str().unwrap());
    assert!(body_in.contains("message/send"));

    // Peer posts the response — with its own caller token, echoing the
    // per-channel secret.
    let resp_json = serde_json::json!({
        "id": id,
        "status": 200,
        "headers": {"content-type": "application/json"},
        "body_b64": base64::engine::general_purpose::STANDARD.encode("echo:ok"),
        "chan_secret": secret,
    });
    let r = router
        .clone()
        .oneshot(req(
            "POST",
            &format!("/channel/response/{id}?name=fw"),
            Some(&ct),
            Some(&resp_json.to_string()),
        ))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);

    // Caller received the echoed response.
    let out = call.await.unwrap();
    assert_eq!(out.status(), StatusCode::OK);
    let b = axum::body::to_bytes(out.into_body(), 65536).await.unwrap();
    assert_eq!(String::from_utf8_lossy(&b), "echo:ok");

    // Routing log records the channel path (anonymous → channel- marker)
    // AND the audit fields (same capture contract as the direct path).
    let log = app.recent_log(10).await;
    let e = log
        .iter()
        .find(|e| e.dst == "fw" && e.src.starts_with("channel-"))
        .expect("channel route logged");
    assert_eq!(e.status, 200);
    assert_eq!(e.rpc_method.as_deref(), Some("message/send"));
    assert_eq!(e.rpc_id.as_deref(), Some("ch1"));
    let preview = e.preview.as_deref().expect("preview on channel path");
    assert!(
        preview.contains("[redacted]"),
        "x-api-key redacted: {preview}"
    );
    assert!(!preview.contains("sk-chan"));
    assert!(preview.contains("hi"));
}

#[tokio::test]
async fn channel_impersonation_rejected() {
    // Two peers, each with their own caller token. Peer B must not answer
    // peer A's pending request even when declaring name=A (secret binding),
    // and must not open/replace A's channel (per-peer management, issue #3).
    let (router, _app, _gw, boot) = test_app().await;
    let mut cts = std::collections::HashMap::new();
    for n in ["pa", "pb"] {
        let reg = serde_json::json!({"name": n, "url": "http://127.0.0.1:1/"}).to_string();
        let r = router
            .clone()
            .oneshot(req("POST", "/register", Some(&boot), Some(&reg)))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::CREATED);
        let b = axum::body::to_bytes(r.into_body(), 65536).await.unwrap();
        let ct = serde_json::from_slice::<serde_json::Value>(&b).unwrap()["caller_token"]
            .as_str()
            .unwrap()
            .to_string();
        cts.insert(n.to_string(), ct);
    }
    // A opens a channel.
    let sse = router
        .clone()
        .oneshot(req("GET", "/channel?name=pa", Some(&cts["pa"]), None))
        .await
        .unwrap();
    let mut stream = sse.into_body().into_data_stream();
    // deliver a request to A
    let router2 = router.clone();
    let boot2 = boot.clone();
    let call = tokio::spawn(async move {
        router2
            .oneshot(req("POST", "/peer/pa", Some(&boot2), Some("{}")))
            .await
            .unwrap()
    });
    use futures_util::StreamExt;
    let mut env = None;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while std::time::Instant::now() < deadline && env.is_none() {
        if let Some(Ok(chunk)) = stream.next().await {
            let text = String::from_utf8_lossy(&chunk);
            if text.contains("event: request") {
                env = serde_json::from_str(text.split("data: ").nth(1).unwrap_or_default().trim())
                    .ok();
            }
        }
    }
    let env: serde_json::Value = env.expect("envelope");
    let id = env["id"].as_u64().unwrap();
    // B responds for A's id with its own caller token and the WRONG secret
    // (B never opened A's channel, so it cannot know A's secret — use a
    // bogus one). Cross-peer response must be rejected even though B is a
    // legit token holder.
    let resp = serde_json::json!({
        "id": id, "status": 200, "headers": {},
        "body_b64": "", "chan_secret": "deadbeef",
    });
    let r = router
        .clone()
        .oneshot(req(
            "POST",
            &format!("/channel/response/{id}?name=pa"),
            Some(&cts["pb"]),
            Some(&resp.to_string()),
        ))
        .await
        .unwrap();
    assert_eq!(
        r.status(),
        StatusCode::FORBIDDEN,
        "B must not resolve A's request (identity rejected before secret check)"
    );
    // The SHARED bootstrap token must not answer A's request either — the
    // gateway itself never posts channel responses (issue #3).
    let r = router
        .clone()
        .oneshot(req(
            "POST",
            &format!("/channel/response/{id}?name=pa"),
            Some(&boot),
            Some(&resp.to_string()),
        ))
        .await
        .unwrap();
    assert_eq!(
        r.status(),
        StatusCode::FORBIDDEN,
        "shared token must not resolve A's request"
    );
    // A shared token must not REPLACE a live peer's channel (in-flight
    // envelope theft, issue #3).
    let r = router
        .clone()
        .oneshot(req("GET", "/channel?name=pa", Some(&boot), None))
        .await
        .unwrap();
    assert_eq!(
        r.status(),
        StatusCode::FORBIDDEN,
        "shared token must not bind another peer's channel"
    );
    // A's caller still hangs → drop A's channel → immediate 502.
    drop(stream);
    let out = call.await.unwrap();
    assert_eq!(out.status(), StatusCode::BAD_GATEWAY);
}

#[tokio::test]
async fn channel_oversized_response_rejected() {
    let (router, _app, _gw, boot) = test_app().await;
    let reg = r#"{"name":"big","url":"http://127.0.0.1:1/"}"#;
    let r = router
        .clone()
        .oneshot(req("POST", "/register", Some(&boot), Some(reg)))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::CREATED);
    let b = axum::body::to_bytes(r.into_body(), 65536).await.unwrap();
    let ct: String = serde_json::from_slice::<serde_json::Value>(&b).unwrap()["caller_token"]
        .as_str()
        .unwrap()
        .to_string();
    let sse = router
        .clone()
        .oneshot(req("GET", "/channel?name=big", Some(&ct), None))
        .await
        .unwrap();
    let mut stream = sse.into_body().into_data_stream();
    let router2 = router.clone();
    let boot2 = boot.clone();
    let call = tokio::spawn(async move {
        router2
            .oneshot(req("POST", "/peer/big", Some(&boot2), Some("{}")))
            .await
            .unwrap()
    });
    use futures_util::StreamExt;
    let mut env = None;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while std::time::Instant::now() < deadline && env.is_none() {
        if let Some(Ok(chunk)) = stream.next().await {
            let text = String::from_utf8_lossy(&chunk);
            if text.contains("event: request") {
                env = serde_json::from_str(text.split("data: ").nth(1).unwrap_or_default().trim())
                    .ok();
            }
        }
    }
    let env: serde_json::Value = env.expect("envelope");
    let id = env["id"].as_u64().unwrap();
    let secret = env["chan_secret"].as_str().unwrap().to_string();
    // 5MB base64 → rejected BEFORE decode.
    let resp = serde_json::json!({
        "id": id, "status": 200, "headers": {},
        "body_b64": "A".repeat(5_500_000),
        "chan_secret": secret,
    });
    let r = router
        .clone()
        .oneshot(req(
            "POST",
            &format!("/channel/response/{id}?name=big"),
            Some(&ct),
            Some(&resp.to_string()),
        ))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::PAYLOAD_TOO_LARGE);
    drop(stream);
    let _ = call.await;
}

#[tokio::test]
async fn sse_stream_delivers_route_events() {
    let fake = axum::Router::new().route("/", axum::routing::post(|| async { "ok" }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, fake).await.unwrap() });

    let (router, app, _gw, boot) = test_app().await;
    let _ = router
        .clone()
        .oneshot(req(
            "POST",
            "/register",
            Some(&boot),
            Some(&format!(r#"{{"name":"fake","url":"http://{addr}/"}}"#)),
        ))
        .await
        .unwrap();
    // Subscribe before proxying.
    let mut rx = app.log_tx.subscribe();
    let _ = router
        .clone()
        .oneshot(req("POST", "/peer/fake", Some(&boot), Some("{}")))
        .await
        .unwrap();
    let entry = rx.recv().await.unwrap();
    assert_eq!(entry.dst, "fake");
}

#[tokio::test]
async fn bad_urls_rejected() {
    let (router, _app, gw, _boot) = test_app().await;
    for bad in ["ftp://x/y", "http://", "not-a-url", "file:///etc/passwd"] {
        let r = router
            .clone()
            .oneshot(req(
                "POST",
                "/register",
                Some(&gw),
                Some(&format!(r#"{{"name":"bad","url":"{bad}"}}"#)),
            ))
            .await
            .unwrap();
        assert_eq!(
            r.status(),
            StatusCode::UNPROCESSABLE_ENTITY,
            "url accepted: {bad}"
        );
    }
}

#[tokio::test]
async fn rate_limited_registration() {
    let (router, _app, gw, _boot) = test_app().await;
    let mut last = StatusCode::OK;
    for i in 0..25 {
        let r = router
            .clone()
            .oneshot(req(
                "POST",
                "/register",
                Some(&gw),
                Some(&format!(r#"{{"name":"p{i}","url":"http://127.0.0.1:1/"}}"#)),
            ))
            .await
            .unwrap();
        last = r.status();
    }
    assert_eq!(last, StatusCode::TOO_MANY_REQUESTS);
}

// ----- admin auth -----

trait WithHeader {
    fn with_header(self, name: &str, value: &str) -> Request<Body>;
}
impl WithHeader for Request<Body> {
    fn with_header(mut self, name: &str, value: &str) -> Request<Body> {
        self.headers_mut().insert(
            axum::http::HeaderName::from_bytes(name.as_bytes()).unwrap(),
            axum::http::HeaderValue::from_str(value).unwrap(),
        );
        self
    }
}

fn form_req(method: &str, uri: &str, body: &str) -> Request<Body> {
    form_req_from(method, uri, body, ([127, 0, 0, 1], 50000))
}

fn form_req_from(method: &str, uri: &str, body: &str, src: ([u8; 4], u16)) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/x-www-form-urlencoded")
        .extension(std::net::SocketAddr::from(src))
        .body(Body::from(body.to_string()))
        .unwrap()
}

fn cookie_of(resp: &axum::http::Response<axum::body::Body>) -> Option<String> {
    resp.headers()
        .get(axum::http::header::SET_COOKIE)?
        .to_str()
        .ok()?
        .split(';')
        .next()
        .map(str::to_string)
}

#[tokio::test]
async fn admin_password_flow() {
    let (router, app, _, _) = test_app().await;

    // no password set: UI open until startup mints one (issue #4)
    let r = router
        .clone()
        .oneshot(req("GET", "/", None, None))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);

    // startup mints the initial password; the UI locks immediately after
    let initial = app.ensure_admin_password().await.unwrap();
    assert!(app.admin_set().await);

    // login with the minted password → session cookie
    let r = router
        .clone()
        .oneshot(form_req("POST", "/login", &format!("password={initial}")))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::SEE_OTHER);
    let initial_cookie = cookie_of(&r).unwrap();

    // change password over HTTP (session + current password)
    let r = router
        .clone()
        .oneshot(
            form_req(
                "POST",
                "/settings/password",
                &format!("current={initial}&new=password123&confirm=password123"),
            )
            .with_header("cookie", &initial_cookie),
        )
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::SEE_OTHER);
    assert!(app.admin_set().await);

    // now locked
    let r = router
        .clone()
        .oneshot(req("GET", "/", None, None))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::SEE_OTHER);
    assert_eq!(r.headers().get("location").unwrap(), "/login");
    let r = router
        .clone()
        .oneshot(req("GET", "/api/topology", None, None))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::UNAUTHORIZED);

    // wrong password
    let r = router
        .clone()
        .oneshot(form_req("POST", "/login", "password=nope12345"))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK); // error re-render

    // right password -> session cookie
    let r = router
        .clone()
        .oneshot(form_req("POST", "/login", "password=password123"))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::SEE_OTHER);
    let cookie = cookie_of(&r).unwrap();
    assert!(cookie.starts_with("agw_session="));

    // authed access works
    let r = router
        .clone()
        .oneshot(req("GET", "/", None, None).with_header("cookie", &cookie))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);

    // wrong current password on change -> no change
    let r = router
        .clone()
        .oneshot(form_req(
            "POST",
            "/settings/password",
            "current=wrongwrong&new=newpassword9&confirm=newpassword9",
        ))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::SEE_OTHER);
    assert!(app.verify_admin_password("password123").await);
    assert!(!app.verify_admin_password("newpassword9").await);

    // logout clears the session
    let r = router
        .clone()
        .oneshot(form_req("POST", "/logout", "").with_header("cookie", &cookie))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::SEE_OTHER);
    let r = router
        .clone()
        .oneshot(req("GET", "/", None, None).with_header("cookie", &cookie))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::SEE_OTHER);
}

#[tokio::test]
async fn admin_password_first_set_from_container_bridge_ip() {
    // Issue #4: the unauthenticated first-run setup path is GONE. A random
    // password is generated at startup (ensure_admin_password); any client
    // trying to claim the dashboard before that gets redirected, not a form.
    let (router, app, _, _) = test_app().await;

    // Simulate the takeover attempt from an arbitrary private/public IP:
    // POST /settings/password without the current password must NOT set one.
    for src in [
        [10, 88, 0, 35],
        [172, 17, 0, 3],
        [8, 8, 8, 8],
        [192, 168, 1, 50],
    ] {
        let r = router
            .clone()
            .oneshot(form_req_from(
                "POST",
                "/settings/password",
                "new=evilpass99&confirm=evilpass99",
                (src, 50000),
            ))
            .await
            .unwrap();
        // password is set by the app bootstrap below; without `current` the
        // change is rejected either way — and before bootstrap it would be
        // too, because the first-set form no longer exists.
        assert_ne!(r.status(), StatusCode::OK);
    }
    assert!(!app.verify_admin_password("evilpass99").await);

    // The operator path: startup generates a password (issue #4).
    let pw = app.ensure_admin_password().await;
    assert!(pw.is_some(), "startup must mint an admin password");
    assert!(app.admin_set().await);

    // Any subsequent set requires the current password.
    let r = router
        .clone()
        .oneshot(form_req_from(
            "POST",
            "/settings/password",
            "new=stolenpass1&confirm=stolenpass1",
            ([10, 88, 0, 35], 50000),
        ))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::SEE_OTHER);
    assert!(!app.verify_admin_password("stolenpass1").await);
}

#[tokio::test]
async fn login_rate_limited() {
    let (router, app, _, _) = test_app().await;
    app.set_admin_password(None, "password123").await.unwrap();
    for _ in 0..5 {
        let _ = router
            .clone()
            .oneshot(form_req("POST", "/login", "password=wrongwrong"))
            .await
            .unwrap();
    }
    // 6th attempt with the RIGHT password is still rate-limited in the window
    let r = router
        .clone()
        .oneshot(form_req("POST", "/login", "password=password123"))
        .await
        .unwrap();
    let body = axum::body::to_bytes(r.into_body(), 65536).await.unwrap();
    let html = String::from_utf8_lossy(&body);
    assert!(
        html.contains("too many attempts"),
        "expected rate-limit error"
    );
}

#[tokio::test]
async fn topology_lists_peers() {
    let (router, _, gw, _) = test_app().await;
    // register + auto-accept a peer via bootstrap
    let r = router
        .clone()
        .oneshot(req(
            "POST",
            "/register",
            Some(&gw),
            Some(r#"{"name":"topo-peer","url":"http://127.0.0.1:9/v1","card":{}}"#),
        ))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::CREATED);
    let r = router
        .clone()
        .oneshot(req("GET", "/api/topology", None, None))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    let body = axum::body::to_bytes(r.into_body(), 65536).await.unwrap();
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let peers = v["peers"].as_array().unwrap();
    assert_eq!(peers.len(), 1);
    assert_eq!(peers[0]["name"], "topo-peer");
    assert_eq!(peers[0]["state"], "pending");
    // total_routes field present for the live ring counter.
    assert!(v["total_routes"].is_u64());
}

#[tokio::test]
async fn caller_header_attributed_and_stripped() {
    // Fake peer records the headers it receives.
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let seen2 = seen.clone();
    let fake = axum::Router::new().route(
        "/",
        axum::routing::post(move |headers: axum::http::HeaderMap| async move {
            seen2.lock().unwrap().push(
                headers
                    .get("x-gateway-caller")
                    .map(|v| v.to_str().unwrap().to_string())
                    .unwrap_or_default(),
            );
            axum::Json(serde_json::json!({"ok":true}))
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, fake).await.unwrap() });

    let (router, app, _gw, boot) = test_app().await;
    let _ = router
        .clone()
        .oneshot(req(
            "POST",
            "/register",
            Some(&boot),
            Some(&format!(r#"{{"name":"fake","url":"http://{addr}/"}}"#)),
        ))
        .await
        .unwrap();

    // With the caller header → attributed name in the log.
    let mut r = req("POST", "/peer/fake", Some(&boot), Some("{}"));
    r.headers_mut().insert(
        "x-gateway-caller",
        axum::http::HeaderValue::from_static("alice"),
    );
    let resp = router.clone().oneshot(r).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let log = app.recent_log(5).await;
    assert_eq!(log[0].src, "alice");

    // Header must NOT be forwarded to the upstream peer.
    assert_eq!(seen.lock().unwrap()[0], "", "caller header leaked upstream");

    // Without the header → stable label fallback.
    let _ = router
        .clone()
        .oneshot(req("POST", "/peer/fake", Some(&boot), Some("{}")))
        .await
        .unwrap();
    let log = app.recent_log(5).await;
    assert_eq!(log[0].src, "bootstrap");
}

#[tokio::test]
async fn channel_caller_header_drops_prefix() {
    let (router, app, _gw, boot) = test_app().await;
    let r = router
        .clone()
        .oneshot(req(
            "POST",
            "/register",
            Some(&boot),
            Some(r#"{"name":"fw2","url":"http://127.0.0.1:1/"}"#),
        ))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::CREATED);
    let b = axum::body::to_bytes(r.into_body(), 65536).await.unwrap();
    let ct: String = serde_json::from_slice::<serde_json::Value>(&b).unwrap()["caller_token"]
        .as_str()
        .unwrap()
        .to_string();
    // Open a live channel.
    let sse = router
        .clone()
        .oneshot(req("GET", "/channel?name=fw2", Some(&ct), None))
        .await
        .unwrap();
    assert_eq!(sse.status(), StatusCode::OK);
    let mut stream = sse.into_body().into_data_stream();

    // Call with X-Gateway-Caller: alice.
    let router2 = router.clone();
    let boot2 = boot.clone();
    let call = tokio::spawn(async move {
        let mut r = req("POST", "/peer/fw2", Some(&boot2), Some(r#"{"ping":1}"#));
        r.headers_mut().insert(
            "x-gateway-caller",
            axum::http::HeaderValue::from_static("alice"),
        );
        router2.oneshot(r).await.unwrap()
    });

    // Drain the envelope so delivery completes.
    use base64::Engine as _;
    use futures_util::StreamExt;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let mut done = false;
    while std::time::Instant::now() < deadline && !done {
        if let Some(Ok(chunk)) = stream.next().await {
            let text = String::from_utf8_lossy(&chunk);
            if text.contains("event: request") {
                let data = text.split("data: ").nth(1).unwrap_or_default().trim();
                let env: serde_json::Value = serde_json::from_str(data).unwrap();
                let secret = env["chan_secret"].as_str().unwrap().to_string();
                let resp_json = serde_json::json!({
                    "id": env["id"].as_u64().unwrap(),
                    "status": 200,
                    "headers": {"content-type": "application/json"},
                    "body_b64": base64::engine::general_purpose::STANDARD.encode("echo:ok"),
                    "chan_secret": secret,
                });
                let _ = router
                    .clone()
                    .oneshot(req(
                        "POST",
                        &format!("/channel/response/{}?name=fw2", env["id"].as_u64().unwrap()),
                        Some(&ct),
                        Some(&resp_json.to_string()),
                    ))
                    .await
                    .unwrap();
                done = true;
            }
        }
    }
    assert!(done, "envelope not delivered");
    let out = call.await.unwrap();
    assert_eq!(out.status(), StatusCode::OK);

    let log = app.recent_log(10).await;
    // src is the bare caller name — NO channel- prefix (matches a peer node).
    assert!(log
        .iter()
        .any(|e| e.dst == "fw2" && e.src == "alice" && e.status == 200));
}

#[tokio::test]
async fn per_peer_caller_token_attribution() {
    let (router, app, _gw, boot) = test_app().await;
    // Register a peer via bootstrap → gets a caller_token.
    let r = router
        .clone()
        .oneshot(req(
            "POST",
            "/register",
            Some(&boot),
            Some(r#"{"name":"ct-peer","url":"http://127.0.0.1:9/","card":{}}"#),
        ))
        .await
        .unwrap();
    let body = axum::body::to_bytes(r.into_body(), 65536).await.unwrap();
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let ct = v["caller_token"].as_str().unwrap().to_string();
    assert!(ct.starts_with("agw_"), "caller token issued");

    // Upgrade path: a peer registered WITHOUT a caller_token (pre-upgrade)
    // gets one minted on the next heartbeat re-registration.
    {
        let mut inner = app.inner.write().await;
        if let Some(p) = inner.peers.iter_mut().find(|p| p.name == "ct-peer") {
            p.caller_token = None; // simulate pre-upgrade state
        }
    }
    let r = router
        .clone()
        .oneshot(req(
            "POST",
            "/register",
            Some(&boot),
            Some(r#"{"name":"ct-peer","url":"http://127.0.0.1:9/","card":{}}"#),
        ))
        .await
        .unwrap();
    let body = axum::body::to_bytes(r.into_body(), 65536).await.unwrap();
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let ct2 = v["caller_token"].as_str().unwrap().to_string();
    assert!(
        ct2.starts_with("agw_"),
        "token minted on upgrade re-register"
    );

    // Repeat heartbeat by POST with the shared token is now rejected (the
    // peer holds a caller token — issue #3); it must PATCH instead. Neither
    // the 409 nor any other response re-discloses the caller token.
    let r = router
        .clone()
        .oneshot(req(
            "POST",
            "/register",
            Some(&boot),
            Some(r#"{"name":"ct-peer","url":"http://127.0.0.1:9/","card":{}}"#),
        ))
        .await
        .unwrap();
    assert_eq!(
        r.status(),
        StatusCode::CONFLICT,
        "shared-token POST heartbeat must be rejected once a caller token exists"
    );
    // PATCH heartbeat with the per-peer token: succeeds and never includes
    // the caller token in the response.
    let r = router
        .clone()
        .oneshot(req(
            "PATCH",
            "/register",
            Some(&ct2),
            Some(r#"{"name":"ct-peer","url":"http://127.0.0.1:9/","card":{}}"#),
        ))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    let body = axum::body::to_bytes(r.into_body(), 65536).await.unwrap();
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(
        v.get("caller_token").is_none() || v["caller_token"].is_null(),
        "caller_token must not be re-disclosed on repeat heartbeat"
    );

    // Calling with the per-peer token (NOT the shared boot token) authenticates.
    let r = router
        .clone()
        .oneshot(req("POST", "/peer/ct-peer", Some(&ct2), Some("{}")))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::BAD_GATEWAY); // unreachable url, but AUTH passed
    let log = app.recent_log(5).await;
    assert_eq!(log[0].src, "ct-peer", "caller resolved to peer name");
}

#[tokio::test]
async fn peer_caller_token_authz_boundary() {
    let (router, _app, _gw, boot) = test_app().await;
    // Register via bootstrap → accepted, with a caller token.
    let r = router
        .clone()
        .oneshot(req(
            "POST",
            "/register",
            Some(&boot),
            Some(r#"{"name":"az-peer","url":"http://127.0.0.1:9/","card":{}}"#),
        ))
        .await
        .unwrap();
    let body = axum::body::to_bytes(r.into_body(), 65536).await.unwrap();
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let ct = v["caller_token"].as_str().unwrap().to_string();

    // The caller token is a PROXY credential only — it must NOT grant
    // registration, deregistration, or channel rights.
    let r = router
        .clone()
        .oneshot(req(
            "POST",
            "/register",
            Some(&ct),
            Some(r#"{"name":"evil","url":"http://127.0.0.1:9/","card":{}}"#),
        ))
        .await
        .unwrap();
    assert_eq!(
        r.status(),
        StatusCode::UNAUTHORIZED,
        "caller token must not register"
    );

    // Self-deregistration with the peer's OWN caller token is allowed (it is
    // the peer's management credential) — but first prove a DIFFERENT peer's
    // caller token cannot deregister it (404, not yours).
    let _ = router
        .clone()
        .oneshot(req(
            "POST",
            "/register",
            Some(&boot),
            Some(r#"{"name":"az-other","url":"http://127.0.0.1:9/"}"#),
        ))
        .await
        .unwrap();
    let other_ct = {
        let inner = _app.inner.read().await;
        inner
            .peers
            .iter()
            .find(|p| p.name == "az-other")
            .unwrap()
            .caller_token
            .clone()
            .unwrap()
    };
    let r = router
        .clone()
        .oneshot(req(
            "DELETE",
            "/register?name=az-peer",
            Some(&other_ct),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(
        r.status(),
        StatusCode::NOT_FOUND,
        "another peer's caller token must not deregister"
    );
    let r = router
        .clone()
        .oneshot(req("DELETE", "/register?name=az-peer", Some(&ct), None))
        .await
        .unwrap();
    assert_eq!(
        r.status(),
        StatusCode::OK,
        "peer may deregister itself with its own caller token"
    );

    // The shared bootstrap token must NOT be able to deregister a peer that
    // holds a caller token (cross-peer takeover, issue #3).
    let r = router
        .clone()
        .oneshot(req(
            "POST",
            "/register",
            Some(&boot),
            Some(r#"{"name":"az-peer","url":"http://127.0.0.1:9/","card":{}}"#),
        ))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::CREATED);
    let b = axum::body::to_bytes(r.into_body(), 65536).await.unwrap();
    let ct2: String = serde_json::from_slice::<serde_json::Value>(&b).unwrap()["caller_token"]
        .as_str()
        .unwrap()
        .to_string();
    let _ = router
        .clone()
        .oneshot(req("POST", "/peers/az-peer/accept", None, None))
        .await
        .unwrap();
    let r = router
        .clone()
        .oneshot(req("DELETE", "/register?name=az-peer", Some(&boot), None))
        .await
        .unwrap();
    assert_eq!(
        r.status(),
        StatusCode::NOT_FOUND,
        "shared token must not deregister a peer that holds a caller token"
    );

    let r = router
        .clone()
        .oneshot(req("GET", "/channel?name=az-peer", Some(&ct2), None))
        .await
        .unwrap();
    // A peer's own caller token MAY open its channel (per-peer identity);
    // the SSE stream starts. Drop it immediately — this test only checks
    // authorization.
    assert_eq!(
        r.status(),
        StatusCode::OK,
        "peer may open its own channel with its caller token"
    );
}

#[tokio::test]
async fn update_response_reports_real_state() {
    let (router, app, gw, _boot) = test_app().await;
    // Register with the GATEWAY token → pending.
    let r = router
        .clone()
        .oneshot(req(
            "POST",
            "/register",
            Some(&gw),
            Some(r#"{"name":"pend-peer","url":"http://127.0.0.1:9/","card":{}}"#),
        ))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::CREATED);
    let body = axum::body::to_bytes(r.into_body(), 65536).await.unwrap();
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["state"], "pending");
    let ct = v["caller_token"].as_str().unwrap().to_string();

    // Re-registering by POST with the shared token is now a 409: the peer
    // holds a caller token, so the shared token is no longer its management
    // credential (issue #3).
    let r = router
        .clone()
        .oneshot(req(
            "POST",
            "/register",
            Some(&gw),
            Some(r#"{"name":"pend-peer","url":"http://127.0.0.1:9/","card":{}}"#),
        ))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::CONFLICT);

    // Heartbeat (PATCH with the per-peer caller token) — must report the
    // REAL state (pending), not accepted.
    let r = router
        .clone()
        .oneshot(req(
            "PATCH",
            "/register",
            Some(&ct),
            Some(r#"{"name":"pend-peer","url":"http://127.0.0.1:9/","card":{}}"#),
        ))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    let body = axum::body::to_bytes(r.into_body(), 65536).await.unwrap();
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["status"], "updated");
    assert_eq!(
        v["state"], "pending",
        "update response must reflect real admission state"
    );

    // Accept, then heartbeat again (PATCH, caller token) → reports accepted.
    let r = router
        .clone()
        .oneshot(req("POST", "/peers/pend-peer/accept", None, None))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::SEE_OTHER);
    let r = router
        .clone()
        .oneshot(req(
            "PATCH",
            "/register",
            Some(&ct),
            Some(r#"{"name":"pend-peer","url":"http://127.0.0.1:9/","card":{}}"#),
        ))
        .await
        .unwrap();
    let body = axum::body::to_bytes(r.into_body(), 65536).await.unwrap();
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["state"], "accepted");

    // Storage agrees.
    let inner = app.inner.read().await;
    let p = inner.peers.iter().find(|p| p.name == "pend-peer").unwrap();
    assert_eq!(p.state, PeerState::Accepted);
}

#[tokio::test]
async fn sse_stream_terminates_after_logout() {
    let (router, app, _, _) = test_app().await;
    // Set a password, then log in to obtain a session cookie.
    app.set_admin_password(None, "password123").await.unwrap();
    // Login to get a session cookie.
    let r = router
        .clone()
        .oneshot(form_req("POST", "/login", "password=password123"))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::SEE_OTHER);
    let cookie = cookie_of(&r).unwrap();

    // Open the SSE stream with the valid cookie.
    let r = router
        .clone()
        .oneshot(req("GET", "/api/events", None, None).with_header("cookie", &cookie))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    let mut stream = r.into_body().into_data_stream();

    // Log out — the session dies.
    app.drop_session(cookie.split('=').nth(1).unwrap());

    // Push route entries (busy stream — the revalidation must still fire).
    use futures_util::StreamExt;
    use std::time::Duration;
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    let mut terminated = false;
    while std::time::Instant::now() < deadline {
        // Keep the stream busy with events.
        let _ = app
            .log_route(a2a_switchboard::state::RouteEntry {
                ts: a2a_switchboard::state::now(),
                src: "a".into(),
                dst: "b".into(),
                method: "POST".into(),
                status: 200,
                bytes: 0,
                latency_ms: 0,
                rpc_method: None,
                rpc_id: None,
                preview: None,
                resp_preview: None,
                task_state: None,
            })
            .await;
        if let Some(Ok(chunk)) = stream.next().await {
            let _text = String::from_utf8_lossy(&chunk);
            // Any chunk (route event or keepalive) just continues the loop;
            // only stream end passes the test.
            continue;
        } else {
            terminated = true;
            break;
        }
    }
    assert!(
        terminated,
        "SSE stream must terminate after logout (session invalid)"
    );
}

#[tokio::test]
async fn audit_preview_captured_and_redacted() {
    // Proxied JSON-RPC bodies leave an audit trail: rpc method/id captured,
    // params previewed with secret-ish keys redacted.
    let fake = axum::Router::new()
        .route(
            "/",
            axum::routing::post(|| async {
                (
                    axum::http::StatusCode::OK,
                    axum::Json(serde_json::json!({"ok":1})),
                )
            }),
        )
        .route(
            "/.well-known/agent-card.json",
            axum::routing::get(|| async { axum::Json(serde_json::json!({"name":"fake"})) }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, fake).await.unwrap() });

    let (router, app, _gw, boot) = test_app().await;
    let _ = router
        .clone()
        .oneshot(req(
            "POST",
            "/register",
            Some(&boot),
            Some(&format!(r#"{{"name":"fake","url":"http://{addr}/"}}"#)),
        ))
        .await
        .unwrap();

    let r = router
        .clone()
        .oneshot(req(
            "POST",
            "/peer/fake",
            Some(&boot),
            Some(
                r#"{"jsonrpc":"2.0","id":42,"method":"message/send","params":{"message":{"text":"hello"},"api_key":"sk-live-1234567890"}}"#,
            ),
        ))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);

    let log = app.recent_log(10).await;
    assert_eq!(log.len(), 1);
    let e = &log[0];
    assert_eq!(e.rpc_method.as_deref(), Some("message/send"));
    assert_eq!(e.rpc_id.as_deref(), Some("42"));
    let preview = e.preview.as_deref().expect("preview captured");
    assert!(
        preview.contains("[redacted]"),
        "secret key must be redacted in preview: {preview}"
    );
    assert!(
        !preview.contains("sk-live-1234567890"),
        "raw secret must never appear in the audit trail: {preview}"
    );
    assert!(preview.contains("hello"));
}

#[tokio::test]
async fn audit_extract_unit() {
    // non-JSON bodies capture nothing; unit paths over the extractor itself.
    use a2a_switchboard::state::audit_extract;
    let none = audit_extract(b"not json at all");
    assert!(none.rpc_method.is_none());
    assert!(none.preview.is_none());

    let deep = audit_extract(
        br#"{"jsonrpc":"2.0","id":7,"method":"m","params":{"password":"x","list":[1,2]}}"#,
    );
    assert_eq!(deep.rpc_method.as_deref(), Some("m"));
    assert_eq!(deep.rpc_id.as_deref(), Some("7"));
    assert!(deep.preview.as_deref().unwrap().contains("[redacted]"));
}

#[tokio::test]
async fn audit_endpoints_require_admin_session() {
    // With a password set, /logs/full and /logs/export must redirect to
    // /login for unauthenticated GETs (audit trail stays private), and
    // serve normally with a valid session.
    let (router, app, _gw, _boot) = test_app().await;
    app.set_admin_password(None, "password123").await.unwrap();

    for uri in ["/logs/full", "/logs/export"] {
        let r = router
            .clone()
            .oneshot(req("GET", uri, None, None))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::SEE_OTHER, "{uri} must gate");
        assert_eq!(r.headers().get("location").unwrap(), "/login");
    }

    let r = router
        .clone()
        .oneshot(form_req("POST", "/login", "password=password123"))
        .await
        .unwrap();
    let cookie = r.headers().get("set-cookie").unwrap().to_str().unwrap();
    let sid = cookie.split(';').next().unwrap().to_string();
    let r = router
        .clone()
        .oneshot(req("GET", "/logs/export", None, None).with_header("cookie", &sid))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    assert_eq!(
        r.headers().get("content-type").unwrap(),
        "application/x-ndjson"
    );
    assert!(r
        .headers()
        .get("content-disposition")
        .unwrap()
        .to_str()
        .unwrap()
        .contains("attachment"));

    // authed /logs/full renders the page too
    let r = router
        .clone()
        .oneshot(req("GET", "/logs/full", None, None).with_header("cookie", &sid))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    let b = axum::body::to_bytes(r.into_body(), 65536).await.unwrap();
    assert!(String::from_utf8_lossy(&b).contains("Communication log"));
}

#[tokio::test]
async fn old_format_routing_log_still_parses() {
    // Pre-audit-field routing.jsonl lines (no rpc_method/rpc_id/preview)
    // must still render in /logs/full — serde default regression guard.
    let (router, app, _gw, _boot) = test_app().await;
    std::fs::write(
        app.data_dir.join("routing.jsonl"),
        "{\"ts\":1786000000,\"src\":\"legacy-a\",\"dst\":\"legacy-b\",\"method\":\"POST\",\"status\":200,\"bytes\":10,\"latency_ms\":5}\n",
    )
    .unwrap();
    let r = router
        .clone()
        .oneshot(req("GET", "/logs/full", None, None))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    let b = axum::body::to_bytes(r.into_body(), 65536).await.unwrap();
    let html = String::from_utf8_lossy(&b);
    assert!(html.contains("legacy-a"), "old-format entry must render");
    assert!(html.contains("POST"));

    // filter still works over legacy lines
    let r = router
        .clone()
        .oneshot(req("GET", "/logs/full?src=nosuch", None, None))
        .await
        .unwrap();
    let b = axum::body::to_bytes(r.into_body(), 65536).await.unwrap();
    assert!(String::from_utf8_lossy(&b).contains("No matching entries"));
}

#[tokio::test]
async fn audit_extract_bounds() {
    // rpc_id capped at 64 chars; oversized params truncated to ~2KB;
    // hyphenated secret keys redacted (x-api-key, api-key variants).
    use a2a_switchboard::state::audit_extract;

    let long_id = "i".repeat(10_000);
    let body = format!(r#"{{"jsonrpc":"2.0","id":"{long_id}","method":"m","params":{{}}}}"#);
    let a = audit_extract(body.as_bytes());
    assert!(a.rpc_id.as_deref().unwrap().len() <= 64);

    let big = "x".repeat(50_000);
    let body = format!(r#"{{"jsonrpc":"2.0","id":1,"method":"m","params":{{"blob":"{big}"}}}}"#);
    let a = audit_extract(body.as_bytes());
    assert_eq!(a.rpc_id.as_deref(), Some("1"));
    assert!(a.preview.as_deref().unwrap().len() <= 2048 + 10);

    // numeric ids stay numeric text; string ids keep interior quotes intact
    let a = audit_extract(br#"{"id":42}"#);
    assert_eq!(a.rpc_id.as_deref(), Some("42"));
    let a = audit_extract(br#"{"id":"a\"b"}"#);
    assert_eq!(a.rpc_id.as_deref(), Some("a\"b"));

    let body = r#"{"method":"m","params":{"x-api-key":"sk-1","api-key":"sk-2","Authorization":"Bearer z","normal":"v"}}"#;
    let a = audit_extract(body.as_bytes());
    let p = a.preview.as_deref().unwrap();
    assert!(p.contains("[redacted]"));
    assert!(!p.contains("sk-1") && !p.contains("sk-2") && !p.contains("Bearer z"));
    assert!(p.contains("v"));
}

#[tokio::test]
async fn patch_update_self_service() {
    let (router, app, gw, _boot) = test_app().await;
    // register with bootstrap → accepted, caller token issued
    let r = router
        .clone()
        .oneshot(req(
            "POST",
            "/register",
            Some(&gw),
            Some(r#"{"name":"hermes","url":"http://127.0.0.1:1/","card":{"name":"hermes","skills":["web"]}}"#),
        ))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::CREATED);
    let caller_token = {
        let inner = app.inner.read().await;
        inner.peers[0].caller_token.clone().unwrap()
    };
    // accept via admin endpoint
    let _ = router
        .clone()
        .oneshot(req("POST", "/peers/hermes/accept", None, None))
        .await
        .unwrap();
    // PATCH by the peer's own caller token — url + card partial update
    let r = router
        .clone()
        .oneshot(req(
            "PATCH",
            "/register",
            Some(&caller_token),
            Some(r#"{"name":"hermes","url":"http://10.0.0.5:9900/","card":{"name":"hermes","skills":["web","terminal"]}}"#),
        ))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    {
        let inner = app.inner.read().await;
        assert_eq!(inner.peers[0].url, "http://10.0.0.5:9900/");
        assert!(inner.peers[0].card["skills"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s == "terminal"));
        assert_eq!(inner.peers[0].state, PeerState::Accepted);
        assert!(inner.peers[0].last_ip.is_some());
    }
    // PATCH by the shared token that registered the peer is now REJECTED:
    // once a caller token exists, it is the only management credential (issue #3).
    let r = router
        .clone()
        .oneshot(req(
            "PATCH",
            "/register",
            Some(&gw),
            Some(r#"{"name":"hermes","url":"http://10.0.0.6:9900/"}"#),
        ))
        .await
        .unwrap();
    assert_eq!(
        r.status(),
        StatusCode::CONFLICT,
        "shared token must not patch a peer that holds a caller token"
    );
    {
        let inner = app.inner.read().await;
        assert_eq!(
            inner.peers[0].url, "http://10.0.0.5:9900/",
            "rejected PATCH must not change the url"
        );
    }
    // PATCH with the WRONG peer's caller token → 409
    // (register a second peer, try to patch hermes with its token)
    let _ = router
        .clone()
        .oneshot(req(
            "POST",
            "/register",
            Some(&gw),
            Some(r#"{"name":"other","url":"http://127.0.0.1:2/"}"#),
        ))
        .await
        .unwrap();
    let other_ct = {
        let inner = app.inner.read().await;
        inner
            .peers
            .iter()
            .find(|p| p.name == "other")
            .unwrap()
            .caller_token
            .clone()
            .unwrap()
    };
    let r = router
        .clone()
        .oneshot(req(
            "PATCH",
            "/register",
            Some(&other_ct),
            Some(r#"{"name":"hermes","url":"http://evil:1/"}"#),
        ))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::CONFLICT);
    // and hermes url unchanged
    let inner = app.inner.read().await;
    assert_eq!(inner.peers[0].url, "http://10.0.0.5:9900/");
}

#[tokio::test]
async fn patch_unknown_peer_and_revoked() {
    let (router, app, gw, _boot) = test_app().await;
    let r = router
        .clone()
        .oneshot(req(
            "PATCH",
            "/register",
            Some(&gw),
            Some(r#"{"name":"ghost","url":"http://127.0.0.1:1/"}"#),
        ))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::NOT_FOUND);
    // revoked peer may not self-update
    let r = router
        .clone()
        .oneshot(req(
            "POST",
            "/register",
            Some(&gw),
            Some(r#"{"name":"gone","url":"http://127.0.0.1:1/"}"#),
        ))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::CREATED);
    let b = axum::body::to_bytes(r.into_body(), 65536).await.unwrap();
    let gone_ct: String = serde_json::from_slice::<serde_json::Value>(&b).unwrap()["caller_token"]
        .as_str()
        .unwrap()
        .to_string();
    let _ = router
        .clone()
        .oneshot(req("POST", "/peers/gone/revoke", None, None))
        .await
        .unwrap();
    let r = router
        .clone()
        .oneshot(req(
            "PATCH",
            "/register",
            Some(&gone_ct),
            Some(r#"{"name":"gone","url":"http://127.0.0.1:9/"}"#),
        ))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::FORBIDDEN);
    let inner = app.inner.read().await;
    assert_eq!(inner.peers[0].url, "http://127.0.0.1:1/");
}

#[tokio::test]
async fn patch_bad_url_and_oversize_rejected() {
    let (router, _app, gw, _boot) = test_app().await;
    let _ = router
        .clone()
        .oneshot(req(
            "POST",
            "/register",
            Some(&gw),
            Some(r#"{"name":"v","url":"http://127.0.0.1:1/"}"#),
        ))
        .await
        .unwrap();
    // invalid scheme
    let r = router
        .clone()
        .oneshot(req(
            "PATCH",
            "/register",
            Some(&gw),
            Some(r#"{"name":"v","url":"ftp://x"}"#),
        ))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::UNPROCESSABLE_ENTITY);
    // oversized upstream_token
    let r = router
        .clone()
        .oneshot(req(
            "PATCH",
            "/register",
            Some(&gw),
            Some(format!(r#"{{"name":"v","upstream_token":"{}"}}"#, "t".repeat(600)).as_str()),
        ))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::UNPROCESSABLE_ENTITY);
    // no-token → 401
    let r = router
        .clone()
        .oneshot(req(
            "PATCH",
            "/register",
            None,
            Some(r#"{"name":"v","url":"http://127.0.0.1:1/"}"#),
        ))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn patch_shared_token_fingerprint_guard() {
    let (router, app, gw, boot) = test_app().await;
    // register with the GATEWAY token, then PATCH with the BOOTSTRAP token:
    // valid shared credential, different identity → 409, url unchanged.
    let _ = router
        .clone()
        .oneshot(req(
            "POST",
            "/register",
            Some(&gw),
            Some(r#"{"name":"tw","url":"http://127.0.0.1:1/"}"#),
        ))
        .await
        .unwrap();
    let r = router
        .clone()
        .oneshot(req(
            "PATCH",
            "/register",
            Some(&boot),
            Some(r#"{"name":"tw","url":"http://evil:1/"}"#),
        ))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::CONFLICT);
    let inner = app.inner.read().await;
    assert_eq!(inner.peers[0].url, "http://127.0.0.1:1/");
    drop(inner);
    // garbage token must 401 WITHOUT revealing name existence (pending peer
    // 'tw' exists; 'ghost' does not — both look identical to the caller)
    let r = router
        .clone()
        .oneshot(req(
            "PATCH",
            "/register",
            Some("garbage-token"),
            Some(r#"{"name":"tw","url":"http://evil:1/"}"#),
        ))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
    let r = router
        .clone()
        .oneshot(req(
            "PATCH",
            "/register",
            Some("garbage-token"),
            Some(r#"{"name":"ghost","url":"http://evil:1/"}"#),
        ))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
    // valid credential + unknown name still 404
    let r = router
        .clone()
        .oneshot(req(
            "PATCH",
            "/register",
            Some(&gw),
            Some(r#"{"name":"ghost","url":"http://127.0.0.1:1/"}"#),
        ))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::NOT_FOUND);
}

// ---- Issue #5 hardening regression tests ----

#[tokio::test]
async fn unauth_directory_hides_fleet() {
    // Register + accept a peer, then check the directory: no token → no
    // peer names/health/channel; gateway/bootstrap/caller tokens → visible.
    let (router, app, gw, boot) = test_app().await;
    let r = router
        .clone()
        .oneshot(req(
            "POST",
            "/register",
            Some(&boot),
            Some(
                r#"{"name":"d-peer","url":"http://127.0.0.1:9/v1","card":{"capabilities":["x"]}}"#,
            ),
        ))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::CREATED);
    let caller = {
        let inner = app.inner.read().await;
        inner.peers[0].caller_token.clone().unwrap()
    };

    // unauth: gateway card only, zero peers
    let r = router
        .clone()
        .oneshot(req("GET", "/.well-known/agent.json", None, None))
        .await
        .unwrap();
    let body = axum::body::to_bytes(r.into_body(), 65536).await.unwrap();
    let card: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        card["peers"].as_array().unwrap().len(),
        0,
        "unauth must not see the fleet"
    );

    // gateway token: full directory incl. capabilities
    let r = router
        .clone()
        .oneshot(req("GET", "/.well-known/agent.json", Some(&gw), None))
        .await
        .unwrap();
    let body = axum::body::to_bytes(r.into_body(), 65536).await.unwrap();
    let card: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let peers = card["peers"].as_array().unwrap();
    assert_eq!(peers.len(), 2, "gateway self-entry + the accepted peer");
    assert_eq!(peers[0]["name"], "gateway");
    assert_eq!(peers[1]["name"], "d-peer");
    assert!(peers[1].get("capabilities").is_some());

    // peer caller token: directory visible too (authorized_token now accepts
    // caller tokens for the directory, issue #5)
    let r = router
        .clone()
        .oneshot(req("GET", "/.well-known/agent.json", Some(&caller), None))
        .await
        .unwrap();
    let body = axum::body::to_bytes(r.into_body(), 65536).await.unwrap();
    let card: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        card["peers"].as_array().unwrap().len(),
        2,
        "gateway + d-peer"
    );
}

#[tokio::test]
async fn agent_json_rate_limited() {
    let (router, _, _, _) = test_app().await;
    let mut last = StatusCode::OK;
    for _ in 0..125 {
        let r = router
            .clone()
            .oneshot(req("GET", "/.well-known/agent.json", None, None))
            .await
            .unwrap();
        last = r.status();
        if last == StatusCode::TOO_MANY_REQUESTS {
            break;
        }
    }
    assert_eq!(
        last,
        StatusCode::TOO_MANY_REQUESTS,
        "agent.json must be throttled"
    );
}

#[tokio::test]
async fn cross_origin_admin_post_rejected() {
    let (router, app, _, _) = test_app().await;
    let initial = app.ensure_admin_password().await.unwrap();
    let r = router
        .clone()
        .oneshot(form_req("POST", "/login", &format!("password={initial}")))
        .await
        .unwrap();
    let cookie = cookie_of(&r).unwrap();

    // same-origin admin POST works
    let r = router
        .clone()
        .oneshot(
            req("POST", "/peers/x/accept", None, None)
                .with_header("cookie", &cookie)
                .with_header("origin", "http://127.0.0.1:9920")
                .with_header("host", "127.0.0.1:9920"),
        )
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::SEE_OTHER);

    // cross-origin admin POST (CSRF) rejected even with a valid session
    let r = router
        .clone()
        .oneshot(
            req("POST", "/peers/x/delete", None, None)
                .with_header("cookie", &cookie)
                .with_header("origin", "http://evil.example")
                .with_header("host", "127.0.0.1:9920"),
        )
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn legacy_admin_hash_upgraded_to_argon2() {
    // Simulate a pre-0.6.0 state.json: salt + single-iteration sha256.
    let (_, app, _, _) = test_app().await;
    {
        let mut inner = app.inner.write().await;
        let salt = "somesalt".to_string();
        use sha2::Digest;
        let mut h = sha2::Sha256::new();
        h.update(salt.as_bytes());
        h.update(b"legacy-pass");
        let hash = h.finalize();
        let hexhash = hash.iter().map(|b| format!("{b:02x}")).collect::<String>();
        inner.admin = Some(a2a_switchboard::state::AdminCred {
            salt,
            hash: hexhash, // bare hex — exactly what 0.5.x wrote (no marker)
        });
    }
    assert!(app.verify_admin_password("legacy-pass").await);
    // after a successful legacy login the stored hash must be argon2 PHC
    {
        let inner = app.inner.read().await;
        let cred = inner.admin.as_ref().unwrap();
        assert!(
            cred.hash.starts_with("$argon2id$"),
            "expected argon2 PHC after upgrade, got: {}",
            cred.hash.chars().take(12).collect::<String>()
        );
    }
    // and the upgraded credential still verifies
    assert!(app.verify_admin_password("legacy-pass").await);
    assert!(!app.verify_admin_password("wrong-pass").await);
}

#[tokio::test]
async fn cookie_secure_flag_when_enabled() {
    let (router, app, _, _) = test_app().await;
    let initial = app.ensure_admin_password().await.unwrap();
    a2a_switchboard::admin::COOKIE_SECURE.store(true, std::sync::atomic::Ordering::Relaxed);
    let r = router
        .clone()
        .oneshot(form_req("POST", "/login", &format!("password={initial}")))
        .await
        .unwrap();
    let set_cookie = r
        .headers()
        .get(axum::http::header::SET_COOKIE)
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert!(
        set_cookie.contains("Secure"),
        "cookie must carry Secure when TLS fronts the gateway: {set_cookie}"
    );
    a2a_switchboard::admin::COOKIE_SECURE.store(false, std::sync::atomic::Ordering::Relaxed);
}

#[tokio::test]
async fn routing_log_rotation_and_perm() {
    // Issue #5: routing.jsonl is size-capped (rotates to .1) and 0600.
    let (_, app, _, _) = test_app().await;
    let dir = app.data_dir.clone();
    app.persist().await; // materialize state.json so the perms check has a target

    // tiny cap to force rotation quickly
    *a2a_switchboard::state::ROUTING_LOG_MAX_BYTES
        .write()
        .unwrap() = 2000;

    let entry = a2a_switchboard::state::RouteEntry {
        ts: 1,
        src: "a".into(),
        dst: "b".into(),
        method: "POST".into(),
        status: 200,
        bytes: 10,
        latency_ms: 1,
        rpc_method: None,
        rpc_id: None,
        preview: None,
        resp_preview: None,
        task_state: None,
    };
    for _ in 0..60 {
        app.log_route(entry.clone()).await;
    }

    let path = dir.join("routing.jsonl");
    let rotated = dir.join("routing.jsonl.1");
    assert!(
        rotated.exists(),
        "oversized routing.jsonl must rotate to routing.jsonl.1"
    );
    let len_after = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    assert!(len_after <= 2000, "routing.jsonl must stay under the cap");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "routing.jsonl must be 0600");
        let mode = std::fs::metadata(dir.join("state.json"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600, "state.json must be 0600");
    }
}

#[tokio::test]
async fn previews_disabled_by_config() {
    // Issue #5: audit previews can be disabled (secrets in free-text parts).
    let (router, app, _gw, boot) = test_app().await;
    *a2a_switchboard::state::PREVIEW_ENABLED.write().unwrap() = false;
    let r = router
        .clone()
        .oneshot(req(
            "POST",
            "/register",
            Some(&boot),
            Some(r#"{"name":"pv","url":"http://127.0.0.1:1/","card":{"x":"secret-value-abc"}}"#),
        ))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::CREATED);
    // proxy a call so the preview would normally be captured
    let _ = router
        .clone()
        .oneshot(req("POST", "/peer/pv", Some(&boot), Some("{}")))
        .await
        .unwrap();
    let entries = a2a_switchboard::admin::read_routing_log(&app.data_dir);
    let proxied = entries
        .iter()
        .find(|e| e.src == "pv" || e.dst == "pv")
        .expect("proxy call must be logged");
    assert!(
        proxied.preview.is_none(),
        "preview must be dropped when audit_previews=false"
    );
    *a2a_switchboard::state::PREVIEW_ENABLED.write().unwrap() = true;
}

#[tokio::test]
async fn rolling_window_rate_limiter() {
    // Issue #5: rate limiter is a rolling 60s window — the fixed-window 2x
    // boundary burst is gone (verify by exhausting then waiting).
    let rl = a2a_switchboard::state::RateLimiter::default();
    for _ in 0..5 {
        assert!(rl.allow("k", 5), "burst of 5 must pass");
    }
    assert!(!rl.allow("k", 5), "6th within window must be denied");
    // per-key isolation: an unrelated key is unaffected
    assert!(rl.allow("other", 5), "different key has its own window");
}

// ---- /metrics (token-gated Prometheus text format) ----

/// Restores the process-wide LOCALHOST static on drop. cargo test runs
/// test fns on parallel threads sharing this static (the suite is run
/// single-threaded in CI for SSE timing, but must not depend on that), and
/// an assert failing mid-test must not leave it flipped for other tests.
struct LocalhostGuard;
impl Drop for LocalhostGuard {
    fn drop(&mut self) {
        a2a_switchboard::admin::LOCALHOST.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

#[tokio::test]
async fn metrics_requires_token_off_localhost() {
    let (router, _app, gw, boot) = test_app().await;
    // Simulate a non-loopback bind so the token gate engages.
    a2a_switchboard::admin::LOCALHOST.store(false, std::sync::atomic::Ordering::Relaxed);
    let _guard = LocalhostGuard;

    // no token → 401
    let r = router
        .clone()
        .oneshot(req("GET", "/metrics", None, None))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::UNAUTHORIZED);

    // gateway token → 200 with Prometheus content type
    let r = router
        .clone()
        .oneshot(req("GET", "/metrics", Some(&gw), None))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    assert_eq!(
        r.headers().get("content-type").unwrap(),
        "text/plain; version=0.0.4; charset=utf-8"
    );
    let b = axum::body::to_bytes(r.into_body(), 65536).await.unwrap();
    let text = String::from_utf8_lossy(&b);
    assert!(text.contains("a2a_switchboard_build_info"));
    assert!(text.contains("a2a_switchboard_uptime_seconds"));
    assert!(text.contains("a2a_switchboard_peers"));

    // bootstrap token → also accepted
    let r = router
        .clone()
        .oneshot(req("GET", "/metrics", Some(&boot), None))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);

    // unknown token → 403
    let r = router
        .clone()
        .oneshot(req("GET", "/metrics", Some("agw_bogus"), None))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn metrics_counts_proxied_calls_and_accepts_caller_token() {
    let fake = axum::Router::new().route(
        "/",
        axum::routing::post(|| async {
            (
                axum::http::StatusCode::OK,
                axum::Json(serde_json::json!({
                    "jsonrpc":"2.0","id":1,
                    "result":{"id":"task-1","status":{"state":"completed"}}
                })),
            )
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, fake).await.unwrap() });

    let (router, _app, gw, _boot) = test_app().await;
    // register with the GATEWAY token → pending, then mint a caller token
    let _ = router
        .clone()
        .oneshot(req(
            "POST",
            "/register",
            Some(&gw),
            Some(&format!(r#"{{"name":"fake","url":"http://{addr}/"}}"#)),
        ))
        .await
        .unwrap();
    // accept via admin session not needed for /peer on pending → 403; use
    // the direct state mutation used elsewhere in this suite instead.
    {
        // pending → accepted so the proxy path is open
        let mut inner = _app.inner.write().await;
        inner.peers[0].state = PeerState::Accepted;
        let ct = a2a_switchboard::state::gen_token();
        inner.peers[0].caller_token = Some(ct.clone());
        drop(inner);

        // proxied call WITH the peer caller token (advisor: must be accepted)
        a2a_switchboard::admin::LOCALHOST.store(false, std::sync::atomic::Ordering::Relaxed);
        let _guard = LocalhostGuard;
        let r = router
            .clone()
            .oneshot(req("GET", "/metrics", Some(&ct), None))
            .await
            .unwrap();
        assert_eq!(
            r.status(),
            StatusCode::OK,
            "caller token must scrape /metrics"
        );
    }

    // make a proxied call so the counter bumps
    let r = router
        .clone()
        .oneshot(req(
            "POST",
            "/peer/fake/",
            Some(&gw),
            Some(r#"{"jsonrpc":"2.0","method":"message/send","params":{}}"#),
        ))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);

    // counter now reflects the call — src is the fingerprint label of the
    // shared gateway token (caller attribution needs the per-peer caller token)
    let r = router
        .clone()
        .oneshot(req("GET", "/metrics", Some(&gw), None))
        .await
        .unwrap();
    let b = axum::body::to_bytes(r.into_body(), 65536).await.unwrap();
    let text = String::from_utf8_lossy(&b);
    assert!(
        text.contains("dst=\"fake\"") && text.contains("status=\"200\"} 1"),
        "counter missing from /metrics:\n{text}"
    );

    // response audit: task_state + resp_preview captured on the log entry
    let log = _app.recent_log(10).await;
    assert_eq!(log.len(), 1);
    assert_eq!(log[0].task_state.as_deref(), Some("completed"));
    assert!(log[0]
        .resp_preview
        .as_deref()
        .unwrap_or("")
        .contains("task-1"));
}

#[tokio::test]
async fn response_audit_captures_jsonrpc_error_state() {
    let fake = axum::Router::new().route(
        "/",
        axum::routing::post(|| async {
            (
                axum::http::StatusCode::OK,
                axum::Json(serde_json::json!({
                    "jsonrpc":"2.0","id":1,
                    "error":{"code":-32601,"message":"method not found"}
                })),
            )
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, fake).await.unwrap() });

    let (router, app, _gw, boot) = test_app().await;
    let _ = router
        .clone()
        .oneshot(req(
            "POST",
            "/register",
            Some(&boot),
            Some(&format!(r#"{{"name":"fake","url":"http://{addr}/"}}"#)),
        ))
        .await
        .unwrap();
    let r = router
        .clone()
        .oneshot(req(
            "POST",
            "/peer/fake/",
            Some(&boot),
            Some(r#"{"jsonrpc":"2.0","method":"message/send","params":{}}"#),
        ))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);

    let log = app.recent_log(10).await;
    assert_eq!(log.len(), 1);
    assert_eq!(log[0].task_state.as_deref(), Some("error"));
}

#[tokio::test]
async fn peer_detail_direction_filter() {
    let fake = axum::Router::new()
        .route(
            "/",
            axum::routing::post(|| async { (axum::http::StatusCode::OK, "ok") }),
        )
        .route(
            "/.well-known/agent-card.json",
            axum::routing::get(|| async { axum::Json(serde_json::json!({"name":"fake"})) }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, fake).await.unwrap() });

    let (router, app, _gw, boot) = test_app().await;
    let _ = router
        .clone()
        .oneshot(req(
            "POST",
            "/register",
            Some(&boot),
            Some(&format!(r#"{{"name":"fake","url":"http://{addr}/"}}"#)),
        ))
        .await
        .unwrap();

    // one outbound call (bootstrap → fake)
    let _ = router
        .clone()
        .oneshot(req(
            "POST",
            "/peer/fake/",
            Some(&boot),
            Some(r#"{"jsonrpc":"2.0","method":"message/send","params":{}}"#),
        ))
        .await
        .unwrap();
    // one inbound-only log entry: fake as src (fabricate via log_route)
    app.log_route(a2a_switchboard::state::RouteEntry {
        ts: a2a_switchboard::state::now(),
        src: "fake".into(),
        dst: "other-peer".into(),
        method: "POST".into(),
        status: 200,
        bytes: 1,
        latency_ms: 1,
        rpc_method: None,
        rpc_id: None,
        preview: None,
        resp_preview: None,
        task_state: None,
    })
    .await;

    // admin session for the UI pages (no admin password set in test_app →
    // create the session cookie directly)
    let sid = app.create_session();

    let r = router
        .clone()
        .oneshot(req("GET", "/peers/fake", None, None).with_header("cookie", &sid))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    let b = axum::body::to_bytes(r.into_body(), 262144).await.unwrap();
    let html = String::from_utf8_lossy(&b);
    assert!(
        html.contains("other-peer"),
        "all-direction shows both entries"
    );
    assert!(
        html.contains("/logs/full?dst=fake"),
        "default deep link follows dst"
    );

    let r = router
        .clone()
        .oneshot(req("GET", "/peers/fake?dir=out", None, None).with_header("cookie", &sid))
        .await
        .unwrap();
    let b = axum::body::to_bytes(r.into_body(), 262144).await.unwrap();
    let html = String::from_utf8_lossy(&b);
    assert!(
        html.contains("other-peer"),
        "out direction shows fake as src"
    );
    assert!(
        html.contains("/logs/full?src=fake"),
        "out deep link uses src"
    );

    let r = router
        .clone()
        .oneshot(req("GET", "/peers/fake?dir=in", None, None).with_header("cookie", &sid))
        .await
        .unwrap();
    let b = axum::body::to_bytes(r.into_body(), 262144).await.unwrap();
    let html = String::from_utf8_lossy(&b);
    assert!(
        !html.contains("other-peer"),
        "in direction must exclude entries where fake is only the src"
    );
}

#[tokio::test]
async fn old_format_routing_log_still_parses_with_new_fields_absent() {
    // 0.6.x lines without resp_preview/task_state must deserialize (serde
    // defaults) — extends the pre-audit guard above to the new fields.
    let entry: Result<a2a_switchboard::state::RouteEntry, _> = serde_json::from_str(
        "{\"ts\":1786000000,\"src\":\"a\",\"dst\":\"b\",\"method\":\"POST\",\"status\":200,\"bytes\":10,\"latency_ms\":5,\"rpc_method\":\"message/send\",\"rpc_id\":\"1\",\"preview\":\"p\"}",
    );
    let e = entry.unwrap();
    assert!(e.resp_preview.is_none());
    assert!(e.task_state.is_none());
}

// --- live admin UI: peers SSE events + ?fragment=1 partials ---

#[tokio::test]
async fn peers_sse_event_on_register_and_accept() {
    let (router, app, gw, _boot) = test_app().await;
    let mut rx = app.peers_tx.subscribe();
    let _ = router
        .clone()
        .oneshot(req(
            "POST",
            "/register",
            Some(&gw),
            Some(r#"{"name":"live","url":"http://127.0.0.1:1/"}"#),
        ))
        .await
        .unwrap();
    let ev = rx.recv().await.unwrap();
    assert_eq!((ev.kind.as_str(), ev.name.as_str()), ("register", "live"));
    let _ = router
        .clone()
        .oneshot(form_req("POST", "/peers/live/accept", ""))
        .await
        .unwrap();
    let ev = rx.recv().await.unwrap();
    assert_eq!((ev.kind.as_str(), ev.name.as_str()), ("accept", "live"));
}

#[tokio::test]
async fn health_flip_emits_single_event() {
    let fake = axum::Router::new().route("/", axum::routing::post(|| async { "ok" }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, fake).await.unwrap() });

    let (router, app, _gw, boot) = test_app().await;
    let _ = router
        .clone()
        .oneshot(req(
            "POST",
            "/register",
            Some(&boot),
            Some(&format!(r#"{{"name":"fake","url":"http://{addr}/"}}"#)),
        ))
        .await
        .unwrap();
    let mut rx = app.peers_tx.subscribe();

    // First observation: was None (fresh registration) → flip = exactly 1 event.
    a2a_switchboard::health::apply_health(&app, "fake", true, None).await;
    assert_eq!(rx.try_recv().unwrap().kind, "health");
    // Same value again: no event — a stable fleet must not spam the stream.
    a2a_switchboard::health::apply_health(&app, "fake", true, None).await;
    assert!(rx.try_recv().is_err());
    // Flip to unhealthy: exactly one more.
    a2a_switchboard::health::apply_health(&app, "fake", false, Some("probe: HTTP 500".into()))
        .await;
    assert_eq!(rx.try_recv().unwrap().kind, "health");
}

#[tokio::test]
async fn fragments_require_admin_and_render() {
    let fake = axum::Router::new().route("/", axum::routing::post(|| async { "ok" }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, fake).await.unwrap() });

    let (router, app, _gw, boot) = test_app().await;
    let _ = router
        .clone()
        .oneshot(req(
            "POST",
            "/register",
            Some(&boot),
            Some(&format!(r#"{{"name":"fake","url":"http://{addr}/"}}"#)),
        ))
        .await
        .unwrap();

    // Admin unset → UI open: fragments render as bare partials (no layout).
    for path in [
        "/peers?fragment=1",
        "/peers/fake?fragment=1",
        "/logs/full?fragment=1",
    ] {
        let r = router
            .clone()
            .oneshot(req("GET", path, None, None))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK, "{path}");
        let body =
            String::from_utf8_lossy(&axum::body::to_bytes(r.into_body(), 1 << 20).await.unwrap())
                .into_owned();
        assert!(!body.contains("<!DOCTYPE"), "{path} leaked layout");
        assert!(!body.contains("<nav"), "{path} leaked layout");
    }
    let r = router
        .clone()
        .oneshot(req("GET", "/peers?fragment=1", None, None))
        .await
        .unwrap();
    let body =
        String::from_utf8_lossy(&axum::body::to_bytes(r.into_body(), 1 << 20).await.unwrap())
            .into_owned();
    assert!(body.contains("Accepted"));

    // Admin set → locks; fragments follow the same admin gate as full pages.
    let pw = app.ensure_admin_password().await.unwrap();
    let r = router
        .clone()
        .oneshot(req("GET", "/peers?fragment=1", None, None))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::SEE_OTHER);
    assert_eq!(r.headers().get("location").unwrap(), "/login");

    let r = router
        .clone()
        .oneshot(form_req("POST", "/login", &format!("password={pw}")))
        .await
        .unwrap();
    let cookie = cookie_of(&r).unwrap();
    for path in [
        "/peers?fragment=1",
        "/peers/fake?fragment=1",
        "/logs/full?fragment=1",
    ] {
        let r = router
            .clone()
            .oneshot(req("GET", path, None, None).with_header("cookie", &cookie))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK, "{path}");
        let body =
            String::from_utf8_lossy(&axum::body::to_bytes(r.into_body(), 1 << 20).await.unwrap())
                .into_owned();
        assert!(!body.contains("<!DOCTYPE"), "{path} leaked layout");
    }
    // Pages carry the live-update wiring.
    for path in ["/peers", "/logs/full"] {
        let r = router
            .clone()
            .oneshot(req("GET", path, None, None).with_header("cookie", &cookie))
            .await
            .unwrap();
        let body =
            String::from_utf8_lossy(&axum::body::to_bytes(r.into_body(), 1 << 20).await.unwrap())
                .into_owned();
        assert!(body.contains("data-live"), "{path} missing data-live");
        assert!(body.contains("/assets/live.js"), "{path} missing live.js");
    }
    // Caption renders exactly once (page includes the partial; no dupes).
    let r = router
        .clone()
        .oneshot(req("GET", "/logs/full", None, None).with_header("cookie", &cookie))
        .await
        .unwrap();
    let body =
        String::from_utf8_lossy(&axum::body::to_bytes(r.into_body(), 1 << 20).await.unwrap())
            .into_owned();
    assert_eq!(body.matches("entries (newest first)").count(), 1);
    assert_eq!(body.matches("matching entries from").count(), 0);
}

#[tokio::test]
async fn logs_fragment_honors_filters() {
    let (router, app, _gw, _boot) = test_app().await;
    for (src, dst) in [("alice", "w1"), ("bob", "w2")] {
        app.log_route(a2a_switchboard::state::RouteEntry {
            ts: a2a_switchboard::state::now(),
            src: src.into(),
            dst: dst.into(),
            method: "POST".into(),
            status: 200,
            bytes: 0,
            latency_ms: 1,
            rpc_method: None,
            rpc_id: None,
            preview: None,
            resp_preview: None,
            task_state: None,
        })
        .await;
    }
    let r = router
        .clone()
        .oneshot(req("GET", "/logs/full?src=alice&fragment=1", None, None))
        .await
        .unwrap();
    let body =
        String::from_utf8_lossy(&axum::body::to_bytes(r.into_body(), 1 << 20).await.unwrap())
            .into_owned();
    assert!(body.contains("alice"));
    assert!(!body.contains("bob"), "fragment ignored the src filter");
}

// ----- human peers, gateway agent, chat / messenger -----

fn json_req(method: &str, uri: &str, body: &str) -> Request<Body> {
    req(method, uri, None, Some(body))
}

async fn body_json(r: axum::response::Response) -> serde_json::Value {
    let body = axum::body::to_bytes(r.into_body(), 1 << 20).await.unwrap();
    serde_json::from_slice(&body).unwrap()
}

/// Register + auto-accept an upstream peer that records request bodies and
/// replies with an A2A artifact text "roger".
async fn spawn_recording_peer(
    received: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
) -> (String, String, String) {
    let recv = received.clone();
    let fake = axum::Router::new().route(
        "/",
        axum::routing::post(move |body: axum::body::Bytes| {
            let recv = recv.clone();
            async move {
                recv.lock()
                    .unwrap()
                    .push(String::from_utf8_lossy(&body).to_string());
                (
                    axum::http::StatusCode::OK,
                    axum::Json(serde_json::json!({
                        "jsonrpc":"2.0","id":1,
                        "result":{"id":"t1","status":{"state":"completed"},
                                  "artifacts":[{"parts":[{"kind":"text","text":"roger"}]}]}
                    })),
                )
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, fake).await.unwrap() });
    let gw = "gw_test_token".to_string();
    let boot = "boot_test_token".to_string();
    (format!("http://{addr}/"), gw, boot)
}

#[tokio::test]
async fn human_peer_lifecycle_and_attribution() {
    let received = Arc::new(std::sync::Mutex::new(Vec::new()));
    let (url, _gw, boot) = spawn_recording_peer(received).await;
    let (router, app, _gw2, boot2) = test_app().await;
    let _ = router
        .clone()
        .oneshot(req(
            "POST",
            "/register",
            Some(&boot2),
            Some(&format!(r#"{{"name":"fake","url":"{url}/"}}"#)),
        ))
        .await
        .unwrap();

    // Create a human identity via the Settings form.
    let r = router
        .clone()
        .oneshot(form_req("POST", "/settings/humans", "name=alice"))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::SEE_OTHER);
    let alice_token = {
        let inner = app.inner.read().await;
        let p = inner
            .peers
            .iter()
            .find(|p| p.name == "alice")
            .unwrap()
            .clone();
        assert_eq!(p.kind, a2a_switchboard::state::PeerKind::Human);
        assert_eq!(p.state, PeerState::Accepted);
        p.caller_token.unwrap()
    };

    // The human token calls a peer through the gateway; attribution = alice.
    let r = router
        .clone()
        .oneshot(req(
            "POST",
            "/peer/fake",
            Some(&alice_token),
            Some(r#"{"jsonrpc":"2.0","id":1,"method":"message/send","params":{"message":{"role":"user","parts":[{"kind":"text","text":"hello fake"}]}}}"#),
        ))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    let log = app.recent_log(10).await;
    assert_eq!(log[0].src, "alice");
    assert_eq!(log[0].dst, "fake");

    // The messenger store mirrored the exchange as DM bubbles.
    {
        let ring = app.chat_ring.read().await;
        let conv = a2a_switchboard::state::dm_conv("alice", "fake");
        let msgs: Vec<_> = ring.iter().filter(|m| m.conv == conv).collect();
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].src, "alice");
        assert!(msgs[0].text.contains("hello fake"));
        assert_eq!(msgs[1].src, "fake");
        assert_eq!(msgs[1].text, "roger");
    }

    // Humans are excluded from the directory; the gateway agent is listed.
    let r = router
        .clone()
        .oneshot(req("GET", "/.well-known/agent.json", Some(&boot), None))
        .await
        .unwrap();
    let card = body_json(r).await;
    let names: Vec<&str> = card["peers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"gateway"));
    assert!(names.contains(&"fake"));
    assert!(!names.contains(&"alice"));
}

#[tokio::test]
async fn gateway_agent_replies_and_records() {
    let (router, app, _gw, boot) = test_app().await;

    // Plain text → agent ack; command → fleet answer. Both are JSON-RPC
    // message/send results and both land in the messenger store.
    for (text, expect) in [("hello", "/help"), ("/peers", "No peers registered")] {
        let body = serde_json::json!({
            "jsonrpc":"2.0","id":1,"method":"message/send",
            "params":{"message":{"role":"user","parts":[{"kind":"text","text":text}]}}
        });
        let r = router
            .clone()
            .oneshot(req(
                "POST",
                "/peer/gateway/",
                Some(&boot),
                Some(&body.to_string()),
            ))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK);
        let v = body_json(r).await;
        let reply = v["result"]["artifacts"][0]["parts"][0]["text"]
            .as_str()
            .unwrap();
        assert!(reply.contains(expect), "reply to {text} was: {reply}");
        assert_eq!(v["result"]["status"]["state"], "completed");
    }

    {
        let ring = app.chat_ring.read().await;
        let conv = a2a_switchboard::state::dm_conv("bootstrap", "gateway");
        let msgs: Vec<_> = ring.iter().filter(|m| m.conv == conv).collect();
        assert_eq!(msgs.len(), 4, "two exchanges = request + reply bubbles");
        assert_eq!(msgs[0].src, "bootstrap");
        assert_eq!(msgs[1].src, "gateway");
    }
    // Routing log shows the gateway-agent calls.
    let log = app.recent_log(10).await;
    assert_eq!(log[0].dst, "gateway");
    assert_eq!(log[0].src, "bootstrap");
}

#[tokio::test]
async fn gateway_agent_accepts_v1_0_send_message() {
    // A2A v1.0 clients (e.g. pi-a2a) call the method `SendMessage`, not the
    // pre-1.0 alias — the gateway agent must accept both.
    let (router, app, _gw, boot) = test_app().await;
    let body = serde_json::json!({
        "jsonrpc":"2.0","id":9,"method":"SendMessage",
        "params":{"message":{"role":"user","parts":[{"kind":"text","text":"/whoami"}]}}
    });
    let r = router
        .clone()
        .oneshot(req(
            "POST",
            "/peer/gateway/",
            Some(&boot),
            Some(&body.to_string()),
        ))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    let v = body_json(r).await;
    assert_eq!(v["result"]["kind"], "task");
    let reply = v["result"]["artifacts"][0]["parts"][0]["text"]
        .as_str()
        .unwrap();
    assert!(reply.contains("bootstrap"), "whoami reply was: {reply}");
    // v1.0 proxied traffic mirrors into the messenger too.
    let ring = app.chat_ring.read().await;
    let conv = a2a_switchboard::state::dm_conv("bootstrap", "gateway");
    assert!(ring.iter().any(|m| m.conv == conv && m.src == "gateway"));
}

#[tokio::test]
async fn proxied_send_message_v1_0_mirrors_to_chat() {
    // Proxied SendMessage (v1.0) exchanges must mirror request + reply DM
    // bubbles exactly like the pre-1.0 alias (record_chat_roundtrip gate).
    let fake = axum::Router::new()
        .route(
            "/",
            axum::routing::post(|| async {
                (
                    axum::http::StatusCode::OK,
                    axum::Json(serde_json::json!({
                        "jsonrpc":"2.0","id":1,
                        "result":{"kind":"task","status":{"state":"completed"},
                                  "artifacts":[{"parts":[{"kind":"text","text":"fake reply"}]}]}
                    })),
                )
            }),
        )
        .route(
            "/.well-known/agent-card.json",
            axum::routing::get(|| async { axum::Json(serde_json::json!({"name":"fake"})) }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, fake).await.unwrap() });

    let (router, app, _gw, boot) = test_app().await;
    let _ = router
        .clone()
        .oneshot(req(
            "POST",
            "/register",
            Some(&boot),
            Some(&format!(r#"{{"name":"fake","url":"http://{addr}/"}}"#)),
        ))
        .await
        .unwrap();

    let body = serde_json::json!({
        "jsonrpc":"2.0","id":2,"method":"SendMessage",
        "params":{"message":{"role":"user","kind":"message","messageId":"m1",
                             "parts":[{"kind":"text","text":"v1 hello"}]}}
    });
    let r = router
        .clone()
        .oneshot(req(
            "POST",
            "/peer/fake",
            Some(&boot),
            Some(&body.to_string()),
        ))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);

    let ring = app.chat_ring.read().await;
    let conv = a2a_switchboard::state::dm_conv("bootstrap", "fake");
    let msgs: Vec<_> = ring.iter().filter(|m| m.conv == conv).collect();
    assert_eq!(msgs.len(), 2, "request + reply bubbles: {msgs:?}");
    assert!(msgs[0].text.contains("v1 hello"), "was: {}", msgs[0].text);
    assert!(msgs[1].text.contains("fake reply"), "was: {}", msgs[1].text);
}

#[tokio::test]
async fn chat_endpoints_require_admin_session() {
    // With an admin password set, chat APIs must not be reachable without a
    // session (regression guard: all other chat tests run password-less).
    let (router, app, _gw, boot) = test_app().await;
    let _ = app.set_admin_password(None, "test-pw-123").await;
    let session = app.create_session();

    // Cookie-less chat API POST → 401.
    let r = router
        .clone()
        .oneshot(req(
            "POST",
            "/api/chat/send",
            Some(&boot),
            Some(r#"{"conv":"dm:a|b","text":"hi"}"#),
        ))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::UNAUTHORIZED);

    // Non-API form POST (human creation) → redirect to login.
    let r = router
        .clone()
        .oneshot(req(
            "POST",
            "/settings/humans",
            Some(&boot),
            Some("name=tester"),
        ))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::SEE_OTHER);

    // A valid admin session passes.
    let r = router
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/chat/state")
                .header("cookie", format!("agw_session={session}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
}

#[tokio::test]
async fn room_create_rate_limited() {
    // Room creation rewrites state per call — bound it like the other chat
    // mutations (30/min per client IP).
    let (router, _app, _gw, _boot) = test_app().await;
    let mut last = StatusCode::OK;
    for i in 0..35 {
        let r = router
            .clone()
            .oneshot(req(
                "POST",
                "/api/chat/rooms",
                None,
                Some(&format!(r#"{{"name":"rate-room-{i}"}}"#)),
            ))
            .await
            .unwrap();
        last = r.status();
        if i < 30 {
            assert_ne!(last, StatusCode::TOO_MANY_REQUESTS, "request {i} limited");
        }
    }
    assert_eq!(last, StatusCode::TOO_MANY_REQUESTS);
}

#[test]
fn chat_text_response_unwraps_v1_0_task() {
    use a2a_switchboard::chat::chat_text_response;
    // v1.0 wrapper: {"task": {artifacts, status}} — what pi-a2a returns.
    let wrapped = r#"{"jsonrpc":"2.0","id":1,"result":{"task":{"status":{"state":"completed"},
        "artifacts":[{"parts":[{"kind":"text","text":"wrapped reply"}]}]}}}"#;
    assert_eq!(
        chat_text_response(wrapped.as_bytes()).as_deref(),
        Some("wrapped reply")
    );
    // Bare message result: parts directly on the result object.
    let msg = r#"{"result":{"kind":"message","parts":[{"kind":"text","text":"hi"}]}}"#;
    assert_eq!(chat_text_response(msg.as_bytes()).as_deref(), Some("hi"));
    // Pre-1.0 flat shape still extracts.
    let flat = r#"{"result":{"artifacts":[{"parts":[{"kind":"text","text":"flat"}]}]}}"#;
    assert_eq!(chat_text_response(flat.as_bytes()).as_deref(), Some("flat"));
}

#[tokio::test]
async fn room_send_requires_sender_membership() {
    // `as` cannot attribute a room message to a human who isn't a member
    // (mirrors the DM conv-participant contract).
    let received = Arc::new(std::sync::Mutex::new(Vec::new()));
    let (url, _gw, _boot) = spawn_recording_peer(received).await;
    let (router, _app, _gw2, boot2) = test_app().await;
    let _ = router
        .clone()
        .oneshot(req(
            "POST",
            "/register",
            Some(&boot2),
            Some(&format!(r#"{{"name":"bob","url":"{url}/"}}"#)),
        ))
        .await
        .unwrap();
    for h in ["alice", "mallory"] {
        let _ = router
            .clone()
            .oneshot(form_req("POST", "/settings/humans", &format!("name={h}")))
            .await
            .unwrap();
    }
    let r = router
        .clone()
        .oneshot(json_req(
            "POST",
            "/api/chat/rooms",
            r#"{"name":"ops","members":["bob"],"as":"alice"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    let room_id = body_json(r).await["room"]["id"]
        .as_str()
        .unwrap()
        .to_string();

    // Creator is auto-membered — alice speaks fine…
    let r = router
        .clone()
        .oneshot(json_req(
            "POST",
            "/api/chat/send",
            &serde_json::json!({"conv": format!("room:{room_id}"), "as": "alice", "text": "hi"})
                .to_string(),
        ))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);

    // …but a non-member human is rejected before any fanout.
    let r = router
        .clone()
        .oneshot(json_req(
            "POST",
            "/api/chat/send",
            &serde_json::json!({"conv": format!("room:{room_id}"), "as": "mallory", "text": "hi"})
                .to_string(),
        ))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn chat_text_cap_counts_characters_not_bytes() {
    // The composer caps characters (maxlength=8000) — the server must count
    // the same way, or an 8000-char CJK/emoji message is rejected 413.
    let (router, _app, _gw, _boot) = test_app().await;
    let _ = router
        .clone()
        .oneshot(form_req("POST", "/settings/humans", "name=alice"))
        .await
        .unwrap();
    let cjk = |n: usize| "あ".repeat(n);
    let r = router
        .clone()
        .oneshot(json_req(
            "POST",
            "/api/chat/send",
            &serde_json::json!({"conv": "dm:alice|gateway", "as": "alice", "text": cjk(8000)})
                .to_string(),
        ))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    let r = router
        .clone()
        .oneshot(json_req(
            "POST",
            "/api/chat/send",
            &serde_json::json!({"conv": "dm:alice|gateway", "as": "alice", "text": cjk(8001)})
                .to_string(),
        ))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::PAYLOAD_TOO_LARGE);
}

#[tokio::test]
async fn admin_created_room_without_identity_send_flow() {
    // resolve_human never yields "admin": with no accepted human it rejects
    // earlier ("no such human identity"); once one exists, as-less sends
    // attribute to the first human and the membership check applies.
    let (router, _app, _gw, _boot) = test_app().await;
    let r = router
        .clone()
        .oneshot(json_req("POST", "/api/chat/rooms", r#"{"name":"ops"}"#))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    let room_id = body_json(r).await["room"]["id"]
        .as_str()
        .unwrap()
        .to_string();

    // No humans yet → clean rejection before the membership check.
    let r = router
        .clone()
        .oneshot(json_req(
            "POST",
            "/api/chat/send",
            &serde_json::json!({"conv": format!("room:{room_id}"), "text": "hi"}).to_string(),
        ))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let v = body_json(r).await;
    assert!(v["error"]
        .as_str()
        .unwrap()
        .contains("no such human identity"));

    // A human appears but isn't a member → membership check rejects.
    let _ = router
        .clone()
        .oneshot(form_req("POST", "/settings/humans", "name=alice"))
        .await
        .unwrap();
    let r = router
        .clone()
        .oneshot(json_req(
            "POST",
            "/api/chat/send",
            &serde_json::json!({"conv": format!("room:{room_id}"), "text": "hi"}).to_string(),
        ))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let v = body_json(r).await;
    assert!(v["error"].as_str().unwrap().contains("not a member"));

    // Once alice joins, the as-less send attributes to her and passes.
    let r = router
        .clone()
        .oneshot(json_req(
            "POST",
            &format!("/api/chat/rooms/{room_id}/members"),
            r#"{"add":["alice"]}"#,
        ))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    let r = router
        .clone()
        .oneshot(json_req(
            "POST",
            "/api/chat/send",
            &serde_json::json!({"conv": format!("room:{room_id}"), "text": "hi"}).to_string(),
        ))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
}

#[tokio::test]
async fn reserved_gateway_name_rejected() {
    let (router, _app, gw, _boot) = test_app().await;
    let r = router
        .clone()
        .oneshot(req(
            "POST",
            "/register",
            Some(&gw),
            Some(r#"{"name":"gateway","url":"http://127.0.0.1:1/"}"#),
        ))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn room_create_notifies_and_fans_out() {
    let received = Arc::new(std::sync::Mutex::new(Vec::new()));
    let (url, _gw, boot) = spawn_recording_peer(received.clone()).await;
    let (router, app, _gw2, boot2) = test_app().await;
    let _ = router
        .clone()
        .oneshot(req(
            "POST",
            "/register",
            Some(&boot2),
            Some(&format!(r#"{{"name":"bob","url":"{url}/"}}"#)),
        ))
        .await
        .unwrap();

    // Human identity + room with bob as member.
    let _ = router
        .clone()
        .oneshot(form_req("POST", "/settings/humans", "name=alice"))
        .await
        .unwrap();
    let alice_token = {
        let inner = app.inner.read().await;
        inner
            .peers
            .iter()
            .find(|p| p.name == "alice")
            .unwrap()
            .caller_token
            .clone()
            .unwrap()
    };

    let r = router
        .clone()
        .oneshot(json_req(
            "POST",
            "/api/chat/rooms",
            &serde_json::json!({"name":"ops","members":["bob"],"as":"alice"}).to_string(),
        ))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    let v = body_json(r).await;
    let room_id = v["room"]["id"].as_str().unwrap().to_string();
    assert!(!room_id.is_empty());

    // Roster notification reached the agent member.
    std::thread::sleep(std::time::Duration::from_millis(150));
    let got = received.lock().unwrap().clone();
    assert!(
        got.iter()
            .any(|b| b.contains("You were added to room 'ops'")),
        "roster notify missing; got: {got:?}"
    );

    // Room send fans out to bob with room context and records his reply.
    let r = router
        .clone()
        .oneshot(json_req(
            "POST",
            "/api/chat/send",
            &serde_json::json!({
                "conv": format!("room:{room_id}"),
                "as": "alice",
                "text": "team sync 🚀"
            })
            .to_string(),
        ))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    let v = body_json(r).await;
    let msgs = v["messages"].as_array().unwrap();
    assert_eq!(msgs[0]["src"], "alice");
    assert_eq!(msgs[0]["text"], "team sync 🚀");

    std::thread::sleep(std::time::Duration::from_millis(150));
    let got = received.lock().unwrap().clone();
    assert!(
        got.iter().any(|b| b.contains("[ops] alice: team sync")),
        "fanout text missing; got: {got:?}"
    );

    {
        let ring = app.chat_ring.read().await;
        let conv = format!("room:{room_id}");
        let room_msgs: Vec<_> = ring.iter().filter(|m| m.conv == conv).collect();
        assert!(room_msgs
            .iter()
            .any(|m| m.kind == "system" && m.text.contains("Room 'ops' created")));
        assert!(room_msgs
            .iter()
            .any(|m| m.src == "bob" && m.text == "roger"));
    }

    // The chat API serves the thread back (emoji intact).
    let r = router
        .clone()
        .oneshot(req(
            "GET",
            &format!("/api/chat/messages?conv=room%3A{room_id}"),
            None,
            None,
        ))
        .await
        .unwrap();
    let v = body_json(r).await;
    let texts: Vec<String> = v["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["text"].as_str().unwrap().to_string())
        .collect();
    assert!(texts.iter().any(|t| t.contains("team sync 🚀")));

    // The human token also lets external A2A clients reach the gateway agent.
    let r = router
        .clone()
        .oneshot(req(
            "POST",
            "/peer/gateway/",
            Some(&alice_token),
            Some(r#"{"jsonrpc":"2.0","id":1,"method":"message/send","params":{"message":{"role":"user","parts":[{"kind":"text","text":"/whoami"}]}}}"#),
        ))
        .await
        .unwrap();
    let v = body_json(r).await;
    let reply = v["result"]["artifacts"][0]["parts"][0]["text"]
        .as_str()
        .unwrap();
    assert!(reply.contains("alice"), "whoami reply was: {reply}");

    let _ = boot; // silence unused in some cfgs
}

#[tokio::test]
async fn human_name_gateway_reserved() {
    let (router, app, _gw, _boot) = test_app().await;
    let r = router
        .clone()
        .oneshot(form_req("POST", "/settings/humans", "name=gateway"))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::SEE_OTHER);
    let inner = app.inner.read().await;
    assert!(inner.peers.iter().all(|p| p.name != "gateway"));
}

#[tokio::test]
async fn dm_send_failure_still_stored() {
    let (router, app, _gw, _boot) = test_app().await;
    let _ = router
        .clone()
        .oneshot(form_req("POST", "/settings/humans", "name=alice"))
        .await
        .unwrap();

    // Unknown target: the human's message must be stored (✗) — never lost.
    let r = router
        .clone()
        .oneshot(json_req(
            "POST",
            "/api/chat/send",
            r#"{"conv":"dm:alice|ghost","as":"alice","text":"anyone there?"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    let v = body_json(r).await;
    let msgs = v["messages"].as_array().unwrap();
    assert_eq!(msgs.len(), 1);
    assert_eq!(msgs[0]["src"], "alice");
    assert_eq!(msgs[0]["status"], "err");
    assert!(msgs[0]["error"].as_str().unwrap().contains("404"));

    let ring = app.chat_ring.read().await;
    let conv = a2a_switchboard::state::dm_conv("alice", "ghost");
    let stored: Vec<_> = ring.iter().filter(|m| m.conv == conv).collect();
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].text, "anyone there?");
}

#[tokio::test]
async fn dm_send_stored_with_previews_disabled() {
    let received = Arc::new(std::sync::Mutex::new(Vec::new()));
    let (url, _gw, _boot) = spawn_recording_peer(received).await;
    let (router, _app, _gw2, boot2) = test_app().await;
    // Panic-safe restore: the global static must not leak false to other
    // preview-asserting tests even if an assertion below fails.
    struct RestorePreview;
    impl Drop for RestorePreview {
        fn drop(&mut self) {
            *a2a_switchboard::state::PREVIEW_ENABLED.write().unwrap() = true;
        }
    }
    let _guard = RestorePreview;
    let _ = router
        .clone()
        .oneshot(req(
            "POST",
            "/register",
            Some(&boot2),
            Some(&format!(r#"{{"name":"fake","url":"{url}/"}}"#)),
        ))
        .await
        .unwrap();
    let _ = router
        .clone()
        .oneshot(form_req("POST", "/settings/humans", "name=alice"))
        .await
        .unwrap();

    *a2a_switchboard::state::PREVIEW_ENABLED.write().unwrap() = false;
    let r = router
        .clone()
        .oneshot(json_req(
            "POST",
            "/api/chat/send",
            r#"{"conv":"dm:alice|fake","as":"alice","text":"hello with previews off"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    let v = body_json(r).await;
    let msgs = v["messages"].as_array().unwrap();
    // Human conversation is the feature — stored even with previews off.
    assert!(msgs
        .iter()
        .any(|m| m["src"] == "alice" && m["status"] == "ok"));
    assert!(msgs
        .iter()
        .any(|m| m["src"] == "fake" && m["text"] == "roger"));
}

#[tokio::test]
async fn room_members_add_remove_and_notify() {
    let received = Arc::new(std::sync::Mutex::new(Vec::new()));
    let (url, _gw, _boot) = spawn_recording_peer(received.clone()).await;
    let (router, app, _gw2, boot2) = test_app().await;
    for n in ["bob", "carol"] {
        let _ = router
            .clone()
            .oneshot(req(
                "POST",
                "/register",
                Some(&boot2),
                Some(&format!(r#"{{"name":"{n}","url":"{url}/"}}"#)),
            ))
            .await
            .unwrap();
    }
    let _ = router
        .clone()
        .oneshot(form_req("POST", "/settings/humans", "name=alice"))
        .await
        .unwrap();

    let r = router
        .clone()
        .oneshot(json_req(
            "POST",
            "/api/chat/rooms",
            r#"{"name":"ops","members":["bob"],"as":"alice"}"#,
        ))
        .await
        .unwrap();
    let room_id = body_json(r).await["room"]["id"]
        .as_str()
        .unwrap()
        .to_string();

    // Add carol: roster notify to carol + system bubble + roster updated.
    let r = router
        .clone()
        .oneshot(json_req(
            "POST",
            &format!("/api/chat/rooms/{room_id}/members"),
            r#"{"add":["carol"],"as":"alice"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    std::thread::sleep(std::time::Duration::from_millis(150));
    let got = received.lock().unwrap().clone();
    assert!(got
        .iter()
        .any(|b| b.contains("You were added to room 'ops'")));
    {
        let ring = app.chat_ring.read().await;
        let conv = format!("room:{room_id}");
        assert!(ring
            .iter()
            .any(|m| m.conv == conv && m.kind == "system" && m.text.contains("alice added carol")));
    }

    // Remove bob: system bubble + roster updated (persisted).
    let r = router
        .clone()
        .oneshot(json_req(
            "POST",
            &format!("/api/chat/rooms/{room_id}/members"),
            r#"{"remove":["bob"],"as":"alice"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    let inner = app.inner.read().await;
    let room = inner.rooms.iter().find(|r| r.id == room_id).unwrap();
    // bob removed; alice stays (creator is always a member).
    assert_eq!(room.members, vec!["alice".to_string(), "carol".to_string()]);
}

#[tokio::test]
async fn gateway_agent_hides_fleet_from_pending() {
    // Names/health are admission state (issue #5): a pending peer's
    // caller_token must not enumerate the fleet via the gateway agent.
    let (router, _app, gw, boot) = test_app().await;
    let _ = router
        .clone()
        .oneshot(req(
            "POST",
            "/register",
            Some(&boot),
            Some(r#"{"name":"alpha","url":"http://127.0.0.1:1/"}"#),
        ))
        .await
        .unwrap();
    let r = router
        .clone()
        .oneshot(req(
            "POST",
            "/register",
            Some(&gw),
            Some(r#"{"name":"pending1","url":"http://127.0.0.1:1/"}"#),
        ))
        .await
        .unwrap();
    let ptok = body_json(r).await["caller_token"]
        .as_str()
        .unwrap()
        .to_string();

    for cmd in ["/peers", "/rooms"] {
        let body = serde_json::json!({
            "jsonrpc":"2.0","id":1,"method":"message/send",
            "params":{"message":{"role":"user","parts":[{"kind":"text","text":cmd}]}}
        });
        let r = router
            .clone()
            .oneshot(req(
                "POST",
                "/peer/gateway/",
                Some(&ptok),
                Some(&body.to_string()),
            ))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK);
        let v = body_json(r).await;
        let reply = v["result"]["artifacts"][0]["parts"][0]["text"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(
            !reply.contains("alpha"),
            "{cmd} leaked the fleet to a pending peer: {reply}"
        );
        assert!(reply.contains("accepted peers only"), "reply was: {reply}");
    }

    // Roster side: an accepted caller's /peers lists accepted agents only —
    // pending names stay hidden (same disclosure as agent_card).
    let body = serde_json::json!({
        "jsonrpc":"2.0","id":1,"method":"message/send",
        "params":{"message":{"role":"user","parts":[{"kind":"text","text":"/peers"}]}}
    });
    let r = router
        .clone()
        .oneshot(req(
            "POST",
            "/peer/gateway/",
            Some(&boot),
            Some(&body.to_string()),
        ))
        .await
        .unwrap();
    let v = body_json(r).await;
    let reply = v["result"]["artifacts"][0]["parts"][0]["text"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(reply.contains("alpha"));
    assert!(
        !reply.contains("pending1"),
        "roster leaked a pending peer: {reply}"
    );
}

#[tokio::test]
async fn chat_log_is_newline_delimited() {
    // Regression: log_chat once wrote records without '\n', producing one
    // concatenated line that read_chat_records couldn't parse — history (and
    // chat_seq) was lost on every restart, resetting ids to 0 (which then
    // broke the client's data-mid dedupe → duplicate bubbles).
    let (router, app, gw, _boot) = test_app().await;
    for text in ["first", "second"] {
        let body = serde_json::json!({
            "jsonrpc":"2.0","id":1,"method":"message/send",
            "params":{"message":{"role":"user","parts":[{"kind":"text","text":text}]}}
        });
        let r = router
            .clone()
            .oneshot(req(
                "POST",
                "/peer/gateway/",
                Some(&gw),
                Some(&body.to_string()),
            ))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK);
    }
    let raw = std::fs::read_to_string(app.data_dir.join("chat.jsonl")).unwrap();
    let lines: Vec<_> = raw.lines().collect();
    assert_eq!(lines.len(), 4, "one record per line");
    for l in &lines {
        let parsed: a2a_switchboard::state::ChatMessage = serde_json::from_str(l).unwrap();
        assert!(parsed.id < 4);
    }
    // Restart round-trip: seq continues from the file's max id.
    let app2 = a2a_switchboard::state::App::load(app.data_dir.clone())
        .await
        .unwrap();
    assert_eq!(app2.chat_seq.load(std::sync::atomic::Ordering::Relaxed), 4);
}

// ----- UI data-meaning improvements (0.7.2): formatting, seeding, states -----

#[test]
fn unit_formatting_helpers() {
    use a2a_switchboard::state::{fmt_bytes, fmt_ms, percentile};
    assert_eq!(fmt_ms(812), "812 ms");
    assert_eq!(fmt_ms(5_400), "5.4 s");
    assert_eq!(fmt_ms(38_000), "38 s");
    assert_eq!(fmt_ms(185_000), "3m 05s");
    assert_eq!(fmt_ms(3_720_000), "1h 02m");
    assert_eq!(fmt_bytes(271), "271 B");
    assert_eq!(fmt_bytes(1_200), "1.2 kB");
    assert_eq!(fmt_bytes(3_400_000), "3.2 MB");
    let sorted = vec![1u64, 2, 3, 4, 5, 6, 7, 8, 9, 10];
    assert_eq!(percentile(&sorted, 95), 10);
    assert_eq!(percentile(&sorted, 50), 6); // upper median at even counts
    assert_eq!(percentile(&[], 95), 0);
}

#[test]
fn task_state_display_mapping() {
    use a2a_switchboard::state::RouteEntry;
    let e = |t: Option<&str>| RouteEntry {
        ts: 0,
        src: "a".into(),
        dst: "b".into(),
        method: "POST".into(),
        status: 200,
        bytes: 1,
        latency_ms: 1,
        rpc_method: None,
        rpc_id: None,
        preview: None,
        resp_preview: None,
        task_state: t.map(String::from),
    };
    assert_eq!(
        e(Some("TASK_STATE_COMPLETED")).task_state_display(),
        "completed"
    );
    assert_eq!(
        e(Some("TASK_STATE_INPUT_REQUIRED")).task_state_class(),
        "warn"
    );
    assert_eq!(e(Some("TASK_STATE_FAILED")).task_state_class(), "bad");
    assert_eq!(e(Some("TASK_STATE_WORKING")).task_state_class(), "");
    assert_eq!(e(None).task_state_display(), "");
}

#[tokio::test]
async fn ring_seeds_from_routing_log_on_boot() {
    // Pre-existing routing.jsonl history must populate the ring at boot so
    // dashboard KPIs and the flow log survive restarts.
    let seq = DIR_SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("agw-test-{}-{seq}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let entry = |ts: i64| {
        format!(
            r#"{{"ts":{ts},"src":"a","dst":"b","method":"POST","status":200,"bytes":10,"latency_ms":5}}"#
        )
    };
    std::fs::write(
        dir.join("routing.jsonl"),
        format!("{}\n{}\n{}\n", entry(1), entry(2), entry(3)),
    )
    .unwrap();
    let app = App::load(dir).await.unwrap();
    let ring = app.log_ring.read().await;
    assert_eq!(ring.len(), 3, "ring must be seeded from routing.jsonl tail");
    assert_eq!(ring.back().unwrap().ts, 3, "newest entry last");
}

#[tokio::test]
async fn peers_ui_three_state_health_and_activity() {
    let (router, app, _gw, boot) = test_app().await;
    let _ = router
        .clone()
        .oneshot(req(
            "POST",
            "/register",
            Some(&boot),
            Some(r#"{"name":"alpha","url":"http://127.0.0.1:1/"}"#),
        ))
        .await
        .unwrap();
    // healthy == None → "unknown", never a red "unreachable".
    let r = router
        .clone()
        .oneshot(req("GET", "/peers?fragment=1", None, None))
        .await
        .unwrap();
    let body = axum::body::to_bytes(r.into_body(), usize::MAX)
        .await
        .unwrap();
    let b = String::from_utf8_lossy(&body);
    assert!(b.contains(">unknown<"), "unknown health state: {b}");
    assert!(
        !b.contains("unreachable"),
        "None must not render as unreachable"
    );
    assert!(b.contains("Admitted"), "source column renamed: {b}");
    assert!(b.contains("Activity"), "activity column present: {b}");

    // Ring traffic → activity count renders.
    app.log_route(a2a_switchboard::state::RouteEntry {
        ts: a2a_switchboard::state::now(),
        src: "alpha".into(),
        dst: "alpha".into(),
        method: "POST".into(),
        status: 200,
        bytes: 10,
        latency_ms: 5,
        rpc_method: None,
        rpc_id: None,
        preview: None,
        resp_preview: None,
        task_state: None,
    })
    .await;
    let r = router
        .clone()
        .oneshot(req("GET", "/peers?fragment=1", None, None))
        .await
        .unwrap();
    let body = axum::body::to_bytes(r.into_body(), usize::MAX)
        .await
        .unwrap();
    let b = String::from_utf8_lossy(&body);
    assert!(b.contains("1/h"), "activity count: {b}");

    // Peer detail: reverse channel "direct" (neutral), no red "no".
    let r = router
        .clone()
        .oneshot(req("GET", "/peers/alpha?fragment=1", None, None))
        .await
        .unwrap();
    let body = axum::body::to_bytes(r.into_body(), usize::MAX)
        .await
        .unwrap();
    let b = String::from_utf8_lossy(&body);
    assert!(b.contains(">direct<"), "reverse channel neutral state: {b}");
    assert!(b.contains("Gateway identity"), "caller token section: {b}");
}

#[tokio::test]
async fn logs_table_renders_task_and_human_units() {
    let (router, app, _gw, _boot) = test_app().await;
    app.log_route(a2a_switchboard::state::RouteEntry {
        ts: a2a_switchboard::state::now(),
        src: "a".into(),
        dst: "b".into(),
        method: "POST".into(),
        status: 200,
        bytes: 271,
        latency_ms: 5_400,
        rpc_method: Some("message/send".into()),
        rpc_id: None,
        preview: None,
        resp_preview: None,
        task_state: Some("TASK_STATE_COMPLETED".into()),
    })
    .await;
    let r = router
        .clone()
        .oneshot(req("GET", "/logs/full?fragment=1", None, None))
        .await
        .unwrap();
    let body = axum::body::to_bytes(r.into_body(), usize::MAX)
        .await
        .unwrap();
    let b = String::from_utf8_lossy(&body);
    assert!(b.contains("completed"), "task state column: {b}");
    assert!(b.contains("5.4 s"), "human latency: {b}");
    assert!(b.contains("271 B"), "human bytes: {b}");
    assert!(
        !b.contains("data/routing.jsonl"),
        "caption must not leak server paths: {b}"
    );
}

#[tokio::test]
async fn dashboard_red_kpis_render() {
    let (router, _app, _gw, _boot) = test_app().await;
    let r = router
        .clone()
        .oneshot(req("GET", "/", None, None))
        .await
        .unwrap();
    let body = axum::body::to_bytes(r.into_body(), usize::MAX)
        .await
        .unwrap();
    let b = String::from_utf8_lossy(&body);
    assert!(b.contains("routed · last hour"), "windowed KPI: {b}");
    assert!(b.contains("p95 latency"), "p95 KPI: {b}");
    assert!(b.contains("peers healthy"), "fleet KPI: {b}");
}

#[tokio::test]
async fn fleet_kpi_excludes_revoked_peers() {
    let (router, app, _gw, boot) = test_app().await;
    let _ = router
        .clone()
        .oneshot(req(
            "POST",
            "/register",
            Some(&boot),
            Some(r#"{"name":"alpha","url":"http://127.0.0.1:1/"}"#),
        ))
        .await
        .unwrap();
    // Healthy, then revoked: peers_up must not count it (set_state leaves
    // healthy=Some(true) intact).
    a2a_switchboard::health::apply_health(&app, "alpha", true, None).await;
    let _ = router
        .clone()
        .oneshot(req("POST", "/peers/alpha/revoke", None, None))
        .await
        .unwrap();
    let r = router
        .clone()
        .oneshot(req("GET", "/", None, None))
        .await
        .unwrap();
    let body = axum::body::to_bytes(r.into_body(), usize::MAX)
        .await
        .unwrap();
    let b = String::from_utf8_lossy(&body);
    assert!(
        b.contains(">0/0<"),
        "revoked healthy peer must not count as up: {b}"
    );
}
