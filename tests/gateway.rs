// Integration tests: admission state machine, auth-aware directory, proxy roundtrip.
use agent_gateway::state::{App, PeerState};
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
    let router = agent_gateway::router(app.clone());
    (router, app, "gw_test_token".into(), "boot_test_token".into())
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
        .oneshot(req("POST", "/register", None, Some(r#"{"name":"a","url":"http://127.0.0.1:1/"}"#)))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::UNAUTHORIZED);

    // 2. Gateway token → pending
    let r = router
        .clone()
        .oneshot(req("POST", "/register", Some(&gw), Some(r#"{"name":"alpha","url":"http://127.0.0.1:1/"}"#)))
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
        .oneshot(req("POST", "/peer/alpha", Some(&gw), Some("{}"))
        ).await
        .unwrap();
    assert_eq!(r.status(), StatusCode::FORBIDDEN);

    // 5. Accept via admin action
    let r = router.clone().oneshot(req("POST", "/peers/alpha/accept", None, None)).await.unwrap();
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
    assert_eq!(card["peers"][0]["name"], "alpha");

    // 7. Revoked → hidden + 403 again
    let _ = router.clone().oneshot(req("POST", "/peers/alpha/revoke", None, None)).await.unwrap();
    let r = router.clone().oneshot(req("POST", "/peer/alpha", Some(&gw), Some("{}"))).await.unwrap();
    assert_eq!(r.status(), StatusCode::FORBIDDEN);
    let r = router
        .clone()
        .oneshot(req("GET", "/.well-known/agent.json", Some(&gw), None))
        .await
        .unwrap();
    let body = axum::body::to_bytes(r.into_body(), 65536).await.unwrap();
    let card: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(card["peers"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn bootstrap_auto_accepts() {
    let (router, app, _gw, boot) = test_app().await;
    let r = router
        .clone()
        .oneshot(req("POST", "/register", Some(&boot), Some(r#"{"name":"beta","url":"http://127.0.0.1:1/","card":{"name":"beta","skills":[]}}"#)))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::CREATED);
    let inner = app.inner.read().await;
    assert_eq!(inner.peers[0].state, PeerState::Accepted);
    assert!(inner.peers[0].auto_accepted);
    drop(inner);
    // immediately proxiable? upstream is unreachable → 502, not 403
    let r = router.clone().oneshot(req("POST", "/peer/beta", Some(&boot), Some("{}"))).await.unwrap();
    assert_eq!(r.status(), StatusCode::BAD_GATEWAY);
}

#[tokio::test]
async fn reject_removes_pending() {
    let (router, app, gw, _boot) = test_app().await;
    let _ = router
        .clone()
        .oneshot(req("POST", "/register", Some(&gw), Some(r#"{"name":"gamma","url":"http://127.0.0.1:1/"}"#)))
        .await
        .unwrap();
    let _ = router.clone().oneshot(req("POST", "/peers/gamma/reject", None, None)).await.unwrap();
    let inner = app.inner.read().await;
    assert!(inner.peers.is_empty());
}

#[tokio::test]
async fn name_conflict_rejected() {
    let (router, app, gw, boot) = test_app().await;
    let _ = router
        .clone()
        .oneshot(req("POST", "/register", Some(&gw), Some(r#"{"name":"dup","url":"http://127.0.0.1:1/"}"#)))
        .await
        .unwrap();
    let r = router
        .clone()
        .oneshot(req("POST", "/register", Some(&boot), Some(r#"{"name":"dup","url":"http://127.0.0.1:2/"}"#)))
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
        .route("/", axum::routing::post(|| async {
            (
                axum::http::StatusCode::OK,
                axum::Json(serde_json::json!({"jsonrpc":"2.0","id":1,"result":{"ok":true}})),
            )
        }))
        .route("/.well-known/agent-card.json", axum::routing::get(|| async {
            axum::Json(serde_json::json!({"name":"fake"}))
        }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, fake).await.unwrap() });

    let (router, app, _gw, boot) = test_app().await;
    let _ = router
        .clone()
        .oneshot(req("POST", "/register", Some(&boot), Some(&format!(r#"{{"name":"fake","url":"http://{addr}/"}}"#))))
        .await
        .unwrap();

    let r = router
        .clone()
        .oneshot(req("POST", "/peer/fake", Some(&boot), Some(r#"{"jsonrpc":"2.0","method":"message/send","params":{}}"#)))
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
        .route("/.well-known/agent-card.json", axum::routing::get(|| async {
            axum::Json(serde_json::json!({"name":"fake"}))
        }))
        .route("/", axum::routing::post(|| async { "ok" }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, fake).await.unwrap() });

    let (router, _app, _gw, boot) = test_app().await;
    let _ = router
        .clone()
        .oneshot(req("POST", "/register", Some(&boot), Some(&format!(r#"{{"name":"fake","url":"http://{addr}/"}}"#))))
        .await
        .unwrap();

    let r = router
        .clone()
        .oneshot(req("GET", "/peer/fake/.well-known/agent-card.json", Some(&boot), None))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    let body = axum::body::to_bytes(r.into_body(), 65536).await.unwrap();
    assert!(body.windows(4).any(|w| w == b"fake"));
}

fn decode(b64: &str) -> String {
    use base64::Engine as _;
    String::from_utf8(base64::engine::general_purpose::STANDARD.decode(b64).unwrap()).unwrap()
}

#[tokio::test]
async fn channel_roundtrip_full() {
    // Register a peer with an UNROUTABLE url — proves the channel path, not
    // direct HTTP, carries the request.
    let (router, app, _gw, boot) = test_app().await;
    let reg = format!(r#"{{"name":"fw","url":"http://127.0.0.1:1/"}}"#);
    let _ = router
        .clone()
        .oneshot(req("POST", "/register", Some(&boot), Some(&reg)))
        .await
        .unwrap();

    // channel=false in directory before connect
    let r = router.clone().oneshot(req("GET", "/.well-known/agent.json", Some(&boot), None)).await.unwrap();
    let b = axum::body::to_bytes(r.into_body(), 65536).await.unwrap();
    assert!(String::from_utf8_lossy(&b).contains(r#""channel":false"#));

    // Open the channel as the peer (SSE).
    let sse = router.clone().oneshot(req("GET", "/channel?name=fw", Some(&boot), None)).await.unwrap();
    assert_eq!(sse.status(), StatusCode::OK);
    let mut stream = sse.into_body().into_data_stream();

    // Caller posts /peer/fw concurrently.
    let router2 = router.clone();
    let boot2 = boot.clone();
    let call = tokio::spawn(async move {
        router2
            .oneshot(req("POST", "/peer/fw", Some(&boot2), Some(r#"{"ping":1}"#)))
            .await
            .unwrap()
    });

    // Read SSE chunks until a request envelope appears.
    use futures_util::StreamExt;
    use base64::Engine as _;
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
    assert_eq!(body_in, r#"{"ping":1}"#);

    // Peer posts the response — echoing the per-channel secret.
    let resp_json = serde_json::json!({
        "id": id,
        "status": 200,
        "headers": {"content-type": "application/json"},
        "body_b64": base64::engine::general_purpose::STANDARD.encode("echo:ok"),
        "chan_secret": secret,
    });
    let r = router
        .clone()
        .oneshot(req("POST", &format!("/channel/response/{id}?name=fw"), Some(&boot), Some(&resp_json.to_string())))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);

    // Caller received the echoed response.
    let out = call.await.unwrap();
    assert_eq!(out.status(), StatusCode::OK);
    let b = axum::body::to_bytes(out.into_body(), 65536).await.unwrap();
    assert_eq!(String::from_utf8_lossy(&b), "echo:ok");

    // Routing log records the channel path.
    let log = app.recent_log(10).await;
    assert!(log.iter().any(|e| e.dst == "fw" && e.src.starts_with("channel-") && e.status == 200));
}


#[tokio::test]
async fn channel_impersonation_rejected() {
    // Two peers share the bootstrap token. Peer B must not answer peer A's
    // pending request even when declaring name=A (secret binding).
    let (router, _app, _gw, boot) = test_app().await;
    for n in ["pa", "pb"] {
        let reg = serde_json::json!({"name": n, "url": "http://127.0.0.1:1/"}).to_string();
        let _ = router.clone().oneshot(req("POST", "/register", Some(&boot), Some(&reg))).await.unwrap();
    }
    // A opens a channel.
    let sse = router.clone().oneshot(req("GET", "/channel?name=pa", Some(&boot), None)).await.unwrap();
    let mut stream = sse.into_body().into_data_stream();
    // deliver a request to A
    let router2 = router.clone();
    let boot2 = boot.clone();
    let call = tokio::spawn(async move {
        router2.oneshot(req("POST", "/peer/pa", Some(&boot2), Some("{}"))).await.unwrap()
    });
    use futures_util::StreamExt;
    let mut env = None;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while std::time::Instant::now() < deadline && env.is_none() {
        if let Some(Ok(chunk)) = stream.next().await {
            let text = String::from_utf8_lossy(&chunk);
            if text.contains("event: request") {
                env = serde_json::from_str(text.split("data: ").nth(1).unwrap_or_default().trim()).ok();
            }
        }
    }
    let env: serde_json::Value = env.expect("envelope");
    let id = env["id"].as_u64().unwrap();
    // B responds for A's id with the WRONG secret (B never opened a channel,
    // so it cannot know A's secret — use a bogus one).
    let resp = serde_json::json!({
        "id": id, "status": 200, "headers": {},
        "body_b64": "", "chan_secret": "deadbeef",
    });
    let r = router.clone().oneshot(req("POST", &format!("/channel/response/{id}?name=pa"), Some(&boot), Some(&resp.to_string()))).await.unwrap();
    assert_eq!(r.status(), StatusCode::NOT_FOUND, "B must not resolve A's request");
    // A's caller still hangs → drop A's channel → immediate 502.
    drop(stream);
    let out = call.await.unwrap();
    assert_eq!(out.status(), StatusCode::BAD_GATEWAY);
}

#[tokio::test]
async fn channel_oversized_response_rejected() {
    let (router, _app, _gw, boot) = test_app().await;
    let reg = r#"{"name":"big","url":"http://127.0.0.1:1/"}"#;
    let _ = router.clone().oneshot(req("POST", "/register", Some(&boot), Some(reg))).await.unwrap();
    let sse = router.clone().oneshot(req("GET", "/channel?name=big", Some(&boot), None)).await.unwrap();
    let mut stream = sse.into_body().into_data_stream();
    let router2 = router.clone();
    let boot2 = boot.clone();
    let call = tokio::spawn(async move {
        router2.oneshot(req("POST", "/peer/big", Some(&boot2), Some("{}"))).await.unwrap()
    });
    use futures_util::StreamExt;
    let mut env = None;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while std::time::Instant::now() < deadline && env.is_none() {
        if let Some(Ok(chunk)) = stream.next().await {
            let text = String::from_utf8_lossy(&chunk);
            if text.contains("event: request") {
                env = serde_json::from_str(text.split("data: ").nth(1).unwrap_or_default().trim()).ok();
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
    let r = router.clone().oneshot(req("POST", &format!("/channel/response/{id}?name=big"), Some(&boot), Some(&resp.to_string()))).await.unwrap();
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
        .oneshot(req("POST", "/register", Some(&boot), Some(&format!(r#"{{"name":"fake","url":"http://{addr}/"}}"#))))
        .await
        .unwrap();
    // Subscribe before proxying.
    let mut rx = app.log_tx.subscribe();
    let _ = router.clone().oneshot(req("POST", "/peer/fake", Some(&boot), Some("{}"))).await.unwrap();
    let entry = rx.recv().await.unwrap();
    assert_eq!(entry.dst, "fake");
}

#[tokio::test]
async fn bad_urls_rejected() {
    let (router, _app, gw, _boot) = test_app().await;
    for bad in ["ftp://x/y", "http://", "not-a-url", "file:///etc/passwd"] {
        let r = router
            .clone()
            .oneshot(req("POST", "/register", Some(&gw), Some(&format!(r#"{{"name":"bad","url":"{bad}"}}"#))))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::UNPROCESSABLE_ENTITY, "url accepted: {bad}");
    }
}

#[tokio::test]
async fn rate_limited_registration() {
    let (router, _app, gw, _boot) = test_app().await;
    let mut last = StatusCode::OK;
    for i in 0..25 {
        let r = router
            .clone()
            .oneshot(req("POST", "/register", Some(&gw), Some(&format!(r#"{{"name":"p{i}","url":"http://127.0.0.1:1/"}}"#))))
            .await
            .unwrap();
        last = r.status();
    }
    assert_eq!(last, StatusCode::TOO_MANY_REQUESTS);
}
