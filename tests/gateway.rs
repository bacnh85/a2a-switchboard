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
    assert_eq!(card["peers"][0]["name"], "alpha");

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
    assert_eq!(card["peers"].as_array().unwrap().len(), 0);
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
    let _ = router
        .clone()
        .oneshot(req("POST", "/register", Some(&boot), Some(&reg)))
        .await
        .unwrap();

    // channel=false in directory before connect
    let r = router
        .clone()
        .oneshot(req("GET", "/.well-known/agent.json", Some(&boot), None))
        .await
        .unwrap();
    let b = axum::body::to_bytes(r.into_body(), 65536).await.unwrap();
    assert!(String::from_utf8_lossy(&b).contains(r#""channel":false"#));

    // Open the channel as the peer (SSE).
    let sse = router
        .clone()
        .oneshot(req("GET", "/channel?name=fw", Some(&boot), None))
        .await
        .unwrap();
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
        .oneshot(req(
            "POST",
            &format!("/channel/response/{id}?name=fw"),
            Some(&boot),
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

    // Routing log records the channel path (anonymous → channel- marker).
    let log = app.recent_log(10).await;
    assert!(log
        .iter()
        .any(|e| e.dst == "fw" && e.src.starts_with("channel-") && e.status == 200));
}

#[tokio::test]
async fn channel_impersonation_rejected() {
    // Two peers share the bootstrap token. Peer B must not answer peer A's
    // pending request even when declaring name=A (secret binding).
    let (router, _app, _gw, boot) = test_app().await;
    for n in ["pa", "pb"] {
        let reg = serde_json::json!({"name": n, "url": "http://127.0.0.1:1/"}).to_string();
        let _ = router
            .clone()
            .oneshot(req("POST", "/register", Some(&boot), Some(&reg)))
            .await
            .unwrap();
    }
    // A opens a channel.
    let sse = router
        .clone()
        .oneshot(req("GET", "/channel?name=pa", Some(&boot), None))
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
    // B responds for A's id with the WRONG secret (B never opened a channel,
    // so it cannot know A's secret — use a bogus one).
    let resp = serde_json::json!({
        "id": id, "status": 200, "headers": {},
        "body_b64": "", "chan_secret": "deadbeef",
    });
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
        StatusCode::NOT_FOUND,
        "B must not resolve A's request"
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
    let _ = router
        .clone()
        .oneshot(req("POST", "/register", Some(&boot), Some(reg)))
        .await
        .unwrap();
    let sse = router
        .clone()
        .oneshot(req("GET", "/channel?name=big", Some(&boot), None))
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
            Some(&boot),
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

    // no password set: UI open
    let r = router
        .clone()
        .oneshot(req("GET", "/", None, None))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);

    // set password (test ClientIp is localhost)
    let r = router
        .clone()
        .oneshot(form_req(
            "POST",
            "/settings/password",
            "new=password123&confirm=password123",
        ))
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
    let (router, app, _, _) = test_app().await;

    // Simulate a podman/docker port-published client: socket source IP is the
    // bridge gateway (10.88.0.35), never 127.0.0.1. First-time set must work.
    let r = router
        .clone()
        .oneshot(form_req_from(
            "POST",
            "/settings/password",
            "new=bridgepass99&confirm=bridgepass99",
            ([10, 88, 0, 35], 50000),
        ))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::SEE_OTHER);
    assert!(app.verify_admin_password("bridgepass99").await);

    // Docker bridge 172.17.0.x also passes; a public IP must still be refused.
    let (router2, app2, _, _) = test_app().await;
    let r = router2
        .clone()
        .oneshot(form_req_from(
            "POST",
            "/settings/password",
            "new=dockerpass7&confirm=dockerpass7",
            ([172, 17, 0, 3], 50000),
        ))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::SEE_OTHER);
    assert!(app2.verify_admin_password("dockerpass7").await);

    let (router3, app3, _, _) = test_app().await;
    let r = router3
        .clone()
        .oneshot(form_req_from(
            "POST",
            "/settings/password",
            "new=publicpass1&confirm=publicpass1",
            ([8, 8, 8, 8], 50000),
        ))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::SEE_OTHER);
    assert!(!app3.admin_set().await);
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
    let _ = router
        .clone()
        .oneshot(req(
            "POST",
            "/register",
            Some(&boot),
            Some(r#"{"name":"fw2","url":"http://127.0.0.1:1/"}"#),
        ))
        .await
        .unwrap();
    // Open a live channel.
    let sse = router
        .clone()
        .oneshot(req("GET", "/channel?name=fw2", Some(&boot), None))
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
                        Some(&boot),
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

    // Repeat heartbeat: token NOT re-disclosed (already minted) — prevents
    // shared-token holders from harvesting another peer's caller token.
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

    let r = router
        .clone()
        .oneshot(req("DELETE", "/register?name=az-peer", Some(&ct), None))
        .await
        .unwrap();
    assert_eq!(
        r.status(),
        StatusCode::UNAUTHORIZED,
        "caller token must not deregister"
    );

    let r = router
        .clone()
        .oneshot(req("GET", "/channel?name=az-peer", Some(&ct), None))
        .await
        .unwrap();
    // Channel binding is fingerprint-based; a caller token's fingerprint does
    // not match the registration token → rejected (403), never opened.
    assert_ne!(
        r.status(),
        StatusCode::OK,
        "caller token must not open channels"
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

    // Re-register (heartbeat) — must report the REAL state (pending), not accepted.
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
    assert_eq!(r.status(), StatusCode::OK);
    let body = axum::body::to_bytes(r.into_body(), 65536).await.unwrap();
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["status"], "updated");
    assert_eq!(
        v["state"], "pending",
        "update response must reflect real admission state"
    );

    // Accept, then re-register again → reports accepted.
    let r = router
        .clone()
        .oneshot(req("POST", "/peers/pend-peer/accept", None, None))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::SEE_OTHER);
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
