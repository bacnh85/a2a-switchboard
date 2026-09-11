//! Messenger: built-in switchboard agent, chat store queries, rooms, and the
//! JSON API behind the admin session. Conversation ids: `dm:<a>|<b>` (sorted
//! name pair) or `room:<id>`. Storage lives in state.rs (ring + chat.jsonl +
//! SSE `chat` broadcast); delivery reuses peers::deliver for exact
//! attribution and logging.

use crate::auth::{extract_token, ClientIp, TokenKind};
use crate::peers::{caller_label, deliver, resolve_caller, Caller};
use crate::state::{
    dm_conv, fingerprint, gen_token, now, AppState, ChatMessage, Peer, PeerKind, PeerState, Room,
};
use askama::Template;
use axum::body::Bytes;
use axum::extract::{Form, Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::Json;
use serde::Deserialize;
use std::collections::HashMap;

/// Hard cap on composer text (bytes). Captured bubble text is capped smaller
/// (EXTRACT_CAP) like the audit previews.
const CHAT_TEXT_CAP: usize = 8000;
const EXTRACT_CAP: usize = 2000;
/// Per-member fanout timeout — much shorter than the 600s proxy timeout so
/// one dead peer can't stall a room send.
const FANOUT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);
/// Max members per room.
const ROOM_MEMBER_CAP: usize = 32;

fn cap_text(s: String) -> String {
    if s.chars().count() <= EXTRACT_CAP {
        return s;
    }
    let mut out: String = s.chars().take(EXTRACT_CAP).collect();
    out.push('…');
    out
}

fn join_text_parts(msg: &serde_json::Value) -> String {
    // A2A parts: [{kind: "text", text: "..."}]; legacy flat message.text.
    let mut out: Vec<String> = Vec::new();
    if let Some(parts) = msg.get("parts").and_then(|p| p.as_array()) {
        for p in parts {
            let kind = p
                .get("kind")
                .or_else(|| p.get("type"))
                .and_then(|k| k.as_str());
            if kind.is_none() || kind == Some("text") {
                if let Some(t) = p.get("text").and_then(|t| t.as_str()) {
                    out.push(t.to_string());
                }
            }
        }
    }
    if out.is_empty() {
        if let Some(t) = msg.get("text").and_then(|t| t.as_str()) {
            out.push(t.to_string());
        }
    }
    out.join("\n")
}

/// A2A message-send under both the pre-1.0 alias and the v1.0 method name.
pub(crate) fn is_message_send(method: Option<&str>) -> bool {
    matches!(method, Some("message/send") | Some("SendMessage"))
}

/// Chat text from an A2A message/send request: joined text parts. None for
/// other methods or non-JSON bodies.
pub fn chat_text_request(body: &[u8]) -> Option<String> {
    let v: serde_json::Value = serde_json::from_slice(body).ok()?;
    if !is_message_send(v.get("method").and_then(|m| m.as_str())) {
        return None;
    }
    let msg = v.pointer("/params/message")?;
    let text = join_text_parts(msg);
    if text.is_empty() {
        None
    } else {
        Some(cap_text(text))
    }
}

/// Chat text from a message/send response: result.artifacts[].parts[].text
/// with a result.status.message fallback. None on JSON-RPC errors.
pub fn chat_text_response(body: &[u8]) -> Option<String> {
    let v: serde_json::Value = serde_json::from_slice(body).ok()?;
    if v.get("error").is_some() {
        return None;
    }
    let result = v.get("result")?;
    // v1.0 SendMessageResponse wraps the payload as {"task": …} or
    // {"message": …}; pre-1.0 servers return the flat result object.
    let inner = result
        .get("task")
        .filter(|t| t.is_object())
        .or_else(|| result.get("message").filter(|m| m.is_object()))
        .unwrap_or(result);
    let mut out: Vec<String> = Vec::new();
    if let Some(arts) = inner.get("artifacts").and_then(|a| a.as_array()) {
        for a in arts {
            push_text_parts(a, &mut out);
        }
    }
    if out.is_empty() {
        // A bare message result carries its text parts directly.
        push_text_parts(inner, &mut out);
    }
    if out.is_empty() {
        if let Some(sm) = inner.pointer("/status/message") {
            push_text_parts(sm, &mut out);
        }
    }
    if out.is_empty() {
        None
    } else {
        Some(cap_text(out.join("\n")))
    }
}

fn push_text_parts(x: &serde_json::Value, out: &mut Vec<String>) {
    if let Some(parts) = x.get("parts").and_then(|p| p.as_array()) {
        for p in parts {
            if let Some(t) = p.get("text").and_then(|t| t.as_str()) {
                out.push(t.to_string());
            }
        }
    }
}

// ----- built-in switchboard agent (POST /peer/gateway/) -----

fn json_rpc_err(status: StatusCode, id: serde_json::Value, code: i64, msg: &str) -> Response {
    (
        status,
        Json(serde_json::json!({
            "jsonrpc": "2.0", "id": id,
            "error": {"code": code, "message": msg}
        })),
    )
        .into_response()
}

fn agent_envelope(id: serde_json::Value, text: &str) -> serde_json::Value {
    let task = format!("gw-{}", &gen_token()[4..12]);
    serde_json::json!({
        "jsonrpc": "2.0", "id": id,
        "result": {
            "kind": "task",
            "id": task,
            "status": {"state": "completed", "timestamp": now()},
            "artifacts": [{"artifactId": task, "name": "reply",
                           "parts": [{"kind": "text", "text": text}]}],
        }
    })
}

/// Fleet-aware replies for the switchboard agent. Zero-dep: everything comes
/// from in-memory state. `fleet_ok` gates roster disclosure (/peers, /rooms,
/// fleet counts) — names/health are admission state (issue #5 boundary), so
/// pending/unaccepted identities get a generic reply.
async fn command_reply(app: &AppState, caller: &str, fleet_ok: bool, text: &str) -> String {
    let t = text.trim();
    if let Some(cmd) = t.strip_prefix('/') {
        let mut parts = cmd.splitn(2, ' ');
        let cmd = parts.next().unwrap_or("");
        match cmd {
            "help" => "Switchboard commands:\n/peers — fleet status\n/rooms — chat rooms\n/whoami — your identity\nAnything else is echoed back.".into(),
            "peers" => {
                if !fleet_ok {
                    return "Fleet details are available to accepted peers only.".into();
                }
                let inner = app.inner.read().await;
                let agents: Vec<_> = inner
                    .peers
                    .iter()
                    // Same disclosure as agent_card: accepted agents only —
                    // pending/revoked names are admission state (issue #5).
                    .filter(|p| p.kind == PeerKind::Agent && p.state == PeerState::Accepted)
                    .collect();
                if agents.is_empty() {
                    return "No peers registered yet.".into();
                }
                agents
                    .iter()
                    .map(|p| {
                        let health = match (p.state_str(), p.healthy) {
                            ("accepted", Some(true)) => "healthy",
                            ("accepted", Some(false)) => "unreachable",
                            ("accepted", None) => "unprobed",
                            (s, _) => s,
                        };
                        format!("{} — {} ({})", p.name, p.card_summary(), health)
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            }
            "rooms" => {
                if !fleet_ok {
                    return "Fleet details are available to accepted peers only.".into();
                }
                let inner = app.inner.read().await;
                if inner.rooms.is_empty() {
                    return "No rooms yet. Create one in the web UI under Chat.".into();
                }
                inner
                    .rooms
                    .iter()
                    .map(|r| format!("{} — members: {}", r.name, r.members.join(", ")))
                    .collect::<Vec<_>>()
                    .join("\n")
            }
            "whoami" => {
                format!("You are '{caller}'. Your token is shared with this switchboard — it identifies you on every routed call.")
            }
            _ => format!("Unknown command /{cmd}. Type /help."),
        }
    } else {
        if !fleet_ok {
            return "Switchboard here. Type /help for commands.".into();
        }
        let inner = app.inner.read().await;
        let agents = inner
            .peers
            .iter()
            .filter(|p| p.state == crate::state::PeerState::Accepted && p.kind == PeerKind::Agent)
            .count();
        let rooms = inner.rooms.len();
        format!("Switchboard here — {agents} peer(s) accepted, {rooms} room(s). Type /help for commands.")
    }
}

/// POST /peer/gateway/ — the reserved name is answered here (routed from
/// peers::proxy after auth). Replies with an A2A message/send result and
/// mirrors the exchange into the messenger store + routing log.
pub async fn gateway_agent(
    State(app): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if body.len() > CHAT_TEXT_CAP * 16 {
        return json_rpc_err(
            StatusCode::PAYLOAD_TOO_LARGE,
            serde_json::Value::Null,
            -32600,
            "body too large",
        );
    }
    let started = std::time::Instant::now();
    let v: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(_) => {
            return json_rpc_err(
                StatusCode::UNPROCESSABLE_ENTITY,
                serde_json::Value::Null,
                -32700,
                "invalid JSON",
            )
        }
    };
    let id = v.get("id").cloned().unwrap_or(serde_json::Value::Null);
    if !is_message_send(v.get("method").and_then(|m| m.as_str())) {
        return json_rpc_err(
            StatusCode::NOT_FOUND,
            id,
            -32601,
            "method not supported by gateway agent",
        );
    }
    let Some(text) = chat_text_request(&body) else {
        return json_rpc_err(
            StatusCode::UNPROCESSABLE_ENTITY,
            id,
            -32602,
            "no text part in message",
        );
    };

    // Caller identity: per-peer/human token → name; shared token → class label.
    let (gateway, bootstrap) = {
        let inner = app.inner.read().await;
        (inner.gateway_token.clone(), inner.bootstrap_token.clone())
    };
    let token = extract_token(&headers);
    // Names/health are admission state (issue #5): roster commands are only
    // answered for accepted identities — a pending peer's caller_token must
    // not enumerate the fleet.
    let caller_name = match token.as_deref() {
        Some(t) => match resolve_caller(&app, t).await {
            Some(Caller::Peer(n)) => {
                let ok = {
                    let inner = app.inner.read().await;
                    inner
                        .peers
                        .iter()
                        .any(|p| p.name == n && p.state == PeerState::Accepted)
                };
                (n, ok)
            }
            Some(Caller::Shared(TokenKind::Bootstrap)) => ("bootstrap".to_string(), true),
            Some(Caller::Shared(_)) => (caller_label(t, &gateway, &bootstrap), true),
            None => ("unknown".to_string(), false),
        },
        None => ("unknown".to_string(), false),
    };

    let reply = command_reply(&app, &caller_name.0, caller_name.1, &text).await;
    let envelope = agent_envelope(id, &reply);
    let envelope_bytes = serde_json::to_vec(&envelope).unwrap_or_default();
    let (task_state, resp_preview) = crate::state::audit_extract_response(&envelope_bytes);
    let audit = crate::state::audit_extract(&body);

    // Mirror into the messenger store (human-typed text is always stored).
    let (caller_name, _) = caller_name;
    let conv = dm_conv(&caller_name, "gateway");
    app.log_chat(ChatMessage {
        id: 0,
        ts: 0,
        conv: conv.clone(),
        src: caller_name.clone(),
        text: text.trim().to_string(),
        kind: "chat".into(),
        status: "ok".into(),
        error: None,
    })
    .await;
    app.log_chat(ChatMessage {
        id: 0,
        ts: 0,
        conv,
        src: "gateway".into(),
        text: reply,
        kind: "chat".into(),
        status: "ok".into(),
        error: None,
    })
    .await;

    app.log_route(crate::state::RouteEntry {
        ts: now(),
        src: caller_name,
        dst: "gateway".into(),
        method: "POST".into(),
        status: 200,
        bytes: body.len() as u64,
        latency_ms: started.elapsed().as_millis() as u64,
        rpc_method: audit.rpc_method,
        rpc_id: audit.rpc_id,
        preview: audit.preview,
        resp_preview,
        task_state,
    })
    .await;

    (StatusCode::OK, Json(envelope)).into_response()
}

// ----- JSON API (admin session) -----

// ----- human operator identities (Settings forms) -----

#[derive(Deserialize)]
pub struct HumanForm {
    name: String,
}

/// POST /settings/humans (form) — create a human operator identity:
/// auto-accepted, no upstream endpoint, mints the token shared between that
/// human and the gateway (its caller_token).
pub async fn create_human(State(app): State<AppState>, Form(f): Form<HumanForm>) -> Response {
    let name = f.name.trim().to_string();
    if name == "gateway" {
        // Reserved: would shadow the built-in switchboard agent (DM routing
        // and token attribution both key on the name).
        return Redirect::to("/settings?human=name").into_response();
    }
    if name.is_empty()
        || name.len() > 64
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    {
        return Redirect::to("/settings?human=name").into_response();
    }
    {
        let mut inner = app.inner.write().await;
        if inner.peers.iter().any(|p| p.name == name) {
            return Redirect::to("/settings?human=duplicate").into_response();
        }
        let token = gen_token();
        inner.peers.push(Peer {
            name: name.clone(),
            kind: PeerKind::Human,
            // Sentinel: humans are never proxied anywhere.
            url: "local://human".into(),
            card: serde_json::json!({
                "name": name,
                "description": "Human operator — talks through the switchboard",
            }),
            state: PeerState::Accepted,
            fingerprint: fingerprint(&token),
            upstream_token: None,
            caller_token: Some(token),
            registered_at: now(),
            last_seen: None,
            last_ip: None,
            reg_ip: None,
            healthy: Some(true),
            last_error: None,
            auto_accepted: true,
            last_probe_ts: None,
            last_ok_ts: None,
        });
    }
    app.persist().await;
    app.emit_peers("register", &name);
    Redirect::to("/settings").into_response()
}

/// POST /settings/humans/{name}/delete (form) — remove a human identity.
pub async fn delete_human(State(app): State<AppState>, Path(name): Path<String>) -> Response {
    {
        let mut inner = app.inner.write().await;
        inner
            .peers
            .retain(|p| !(p.name == name && p.kind == PeerKind::Human));
        // Stale roster entries would surface in /rooms and member lists.
        for r in &mut inner.rooms {
            r.members.retain(|m| m != &name);
        }
    }
    app.persist().await;
    app.emit_peers("delete", &name);
    Redirect::to("/settings").into_response()
}

#[derive(Template)]
#[template(path = "chat.html")]
pub struct ChatTmpl {
    pub title: &'static str,
    pub active_nav: &'static str,
    pub localhost: bool,
    pub authed: bool,
}

pub async fn chat_page(State(app): State<AppState>) -> Response {
    let t = ChatTmpl {
        title: "Chat",
        active_nav: "chat",
        localhost: crate::admin::is_localhost(),
        authed: app.admin_set().await,
    };
    Html(t.render().unwrap_or_default()).into_response()
}

fn err_json(status: StatusCode, msg: &str) -> Response {
    (status, Json(serde_json::json!({"error": msg}))).into_response()
}

/// Identity of the sender for internal sends: the chosen human's caller
/// token (exact attribution) — or the human's name only.
async fn resolve_human(
    app: &AppState,
    name: Option<&str>,
) -> Result<(String, Option<String>), Response> {
    let inner = app.inner.read().await;
    let pick = match name {
        Some(n) => inner
            .peers
            .iter()
            .find(|p| p.name == n && p.kind == PeerKind::Human),
        None => inner.peers.iter().find(|p| p.kind == PeerKind::Human),
    };
    match pick {
        Some(p) if p.state == crate::state::PeerState::Accepted => {
            Ok((p.name.clone(), p.caller_token.clone()))
        }
        _ => Err(err_json(
            StatusCode::UNPROCESSABLE_ENTITY,
            "no such human identity — create one in Settings first",
        )),
    }
}

/// GET /api/chat/state — sidebar data: humans, peers, rooms, known
/// conversations (from the ring) with last-message previews.
pub async fn api_state(State(app): State<AppState>) -> Response {
    let (humans, peers) = {
        let inner = app.inner.read().await;
        let humans: Vec<String> = inner
            .peers
            .iter()
            .filter(|p| p.kind == PeerKind::Human && p.state == crate::state::PeerState::Accepted)
            .map(|p| p.name.clone())
            .collect();
        let peers: Vec<serde_json::Value> = inner
            .peers
            .iter()
            .filter(|p| p.kind == PeerKind::Agent && p.state == crate::state::PeerState::Accepted)
            .map(|p| {
                serde_json::json!({
                    "name": p.name,
                    "healthy": p.healthy,
                    "human": false,
                })
            })
            .collect();
        (humans, peers)
    };
    let rooms: Vec<serde_json::Value> = {
        let inner = app.inner.read().await;
        inner
            .rooms
            .iter()
            .map(|r| {
                serde_json::json!({
                    "id": r.id, "name": r.name, "members": r.members,
                    "created_by": r.created_by, "created_at": r.created_at,
                })
            })
            .collect()
    };
    let ring = app.chat_ring.read().await;
    let mut last: HashMap<String, &ChatMessage> = HashMap::new();
    for m in ring.iter() {
        last.insert(m.conv.clone(), m); // ring is oldest-first → last wins
    }
    let mut conversations: Vec<serde_json::Value> = last
        .iter()
        .map(|(conv, m)| {
            let members: Vec<String> = conv
                .strip_prefix("dm:")
                .map(|p| p.split('|').map(str::to_string).collect())
                .unwrap_or_default();
            serde_json::json!({
                "id": conv,
                "kind": if conv.starts_with("room:") { "room" } else { "dm" },
                "members": members,
                "last": {"id": m.id, "ts": m.ts, "src": m.src, "text": m.text, "status": m.status},
            })
        })
        .collect();
    conversations.sort_by_key(|c| {
        std::cmp::Reverse(c.pointer("/last/id").and_then(|v| v.as_u64()).unwrap_or(0))
    });
    let last_id = ring.back().map(|m| m.id).unwrap_or(0);
    Json(serde_json::json!({
        "humans": humans,
        "gateway": "gateway",
        "peers": peers,
        "rooms": rooms,
        "conversations": conversations,
        "last_id": last_id,
    }))
    .into_response()
}

#[derive(Deserialize)]
pub struct MsgQuery {
    conv: String,
    #[serde(default)]
    since_id: u64,
}

/// GET /api/chat/messages?conv=&since_id= — thread fetch for the messenger
/// (last 200 matching bubbles, oldest first; with since_id, only newer).
pub async fn api_messages(State(app): State<AppState>, Query(q): Query<MsgQuery>) -> Response {
    let ring = app.chat_ring.read().await;
    let mut msgs: Vec<&ChatMessage> = ring
        .iter()
        .filter(|m| m.conv == q.conv && m.id > q.since_id)
        .collect();
    if msgs.len() > 200 {
        msgs = msgs.split_off(msgs.len() - 200);
    }
    Json(serde_json::json!({ "messages": msgs })).into_response()
}

fn bubble(
    conv: &str,
    src: &str,
    text: String,
    kind: &str,
    status: &str,
    error: Option<String>,
) -> ChatMessage {
    ChatMessage {
        id: 0,
        ts: 0,
        conv: conv.to_string(),
        src: src.to_string(),
        text,
        kind: kind.to_string(),
        status: status.to_string(),
        error,
    }
}

/// Build an A2A message/send envelope carrying one text part.
fn message_envelope(id: &str, text: &str) -> Bytes {
    Bytes::from(
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "message/send",
            "params": {
                "message": {
                    "role": "user",
                    "messageId": id,
                    "kind": "message",
                    "parts": [{"kind": "text", "text": text}],
                }
            }
        })
        .to_string(),
    )
}

/// Internal message/send to a peer through the normal dual-mode delivery
/// (pinned URL or reverse channel) — same logging and admission as proxy
/// traffic. Returns the peer's reply text when the exchange succeeded.
pub(crate) async fn internal_send(
    app: &AppState,
    target: &str,
    from_token: Option<&str>,
    text: &str,
    client_ip: &str,
    record_chat: bool,
) -> Result<Option<String>, String> {
    let id = format!(
        "chat-{}-{}",
        app.chat_seq.load(std::sync::atomic::Ordering::Relaxed),
        &gen_token()[4..10]
    );
    let d = deliver(
        app,
        target,
        from_token.unwrap_or(""),
        client_ip,
        "/",
        None,
        "POST",
        &HeaderMap::new(),
        message_envelope(&id, text),
        record_chat,
    )
    .await;
    let status = d.resp.status().as_u16();
    if status >= 400 {
        return Err(format!("HTTP {status}"));
    }
    Ok(d.body.as_deref().and_then(chat_text_response))
}

/// Send a text into a room: concurrent fanout to agent members (60s per
/// member), then the sender's bubble (✓✓ unless every delivery failed) and
/// member replies / delivery-failure system bubbles.
async fn send_room(
    app: &AppState,
    room: &Room,
    as_name: &str,
    as_token: Option<&str>,
    text: &str,
    client_ip: &str,
) -> Vec<ChatMessage> {
    let conv = format!("room:{}", room.id);
    let wrapped = format!("[{}] {}: {}", room.name, as_name, text);
    let agents: Vec<String> = {
        let inner = app.inner.read().await;
        room.members
            .iter()
            .filter(|m| {
                inner.peers.iter().any(|p| {
                    p.name == m.as_str()
                        && p.kind == PeerKind::Agent
                        && p.state == PeerState::Accepted
                })
            })
            .cloned()
            .collect()
    };
    let futs = agents.iter().map(|m| {
        let app = app.clone();
        let m = m.clone();
        let wrapped = wrapped.clone();
        let as_token = as_token.map(str::to_string);
        let client_ip = client_ip.to_string();
        async move {
            let r = tokio::time::timeout(
                FANOUT_TIMEOUT,
                internal_send(&app, &m, as_token.as_deref(), &wrapped, &client_ip, false),
            )
            .await;
            (m, r)
        }
    });
    let results = futures_util::future::join_all(futs).await;
    // Sender's tick reflects fanout outcome: ✓✓ unless every agent delivery
    // failed (then ✗ — matching the client's delivered-state semantics).
    let any_ok = results.iter().any(|(_, r)| matches!(r, Ok(Ok(_))));
    let all_failed = !agents.is_empty() && !any_ok;
    let mut recorded = vec![
        app.log_chat(bubble(
            &conv,
            as_name,
            text.to_string(),
            "chat",
            if all_failed { "err" } else { "ok" },
            all_failed.then(|| "all member deliveries failed".to_string()),
        ))
        .await,
    ];
    for (member, r) in results {
        match r {
            Ok(Ok(reply)) => {
                if let Some(t) = reply {
                    recorded.push(
                        app.log_chat(bubble(&conv, &member, t, "chat", "ok", None))
                            .await,
                    );
                }
            }
            Ok(Err(e)) => recorded.push(
                app.log_chat(bubble(
                    &conv,
                    "system",
                    format!("delivery to {member} failed: {e}"),
                    "system",
                    "err",
                    None,
                ))
                .await,
            ),
            Err(_) => recorded.push(
                app.log_chat(bubble(
                    &conv,
                    "system",
                    format!("delivery to {member} timed out"),
                    "system",
                    "err",
                    None,
                ))
                .await,
            ),
        }
    }
    recorded
}

#[derive(Deserialize)]
pub struct SendBody {
    conv: String,
    text: String,
    #[serde(rename = "as")]
    as_human: Option<String>,
}

/// POST /api/chat/send {conv, as, text} — the human speaks: DM to a peer /
/// the gateway / another human, or a room fanout.
pub async fn api_send(
    State(app): State<AppState>,
    ClientIp(client_ip): ClientIp,
    Json(b): Json<SendBody>,
) -> Response {
    if !app.limiter.allow(&format!("chat-{client_ip}"), 60) {
        return err_json(StatusCode::TOO_MANY_REQUESTS, "rate limit exceeded");
    }
    let text = b.text.trim().to_string();
    if text.is_empty() {
        return err_json(StatusCode::UNPROCESSABLE_ENTITY, "empty message");
    }
    if text.chars().count() > CHAT_TEXT_CAP {
        return err_json(StatusCode::PAYLOAD_TOO_LARGE, "message too long");
    }
    let (as_name, as_token) = match resolve_human(&app, b.as_human.as_deref()).await {
        Ok(v) => v,
        Err(r) => return r,
    };

    let recorded: Vec<ChatMessage> = if b.conv.starts_with("room:") {
        let room = {
            let inner = app.inner.read().await;
            let rid = b.conv.strip_prefix("room:").unwrap_or_default();
            inner.rooms.iter().find(|r| r.id == rid).cloned()
        };
        let Some(room) = room else {
            return err_json(StatusCode::NOT_FOUND, "no such room");
        };
        // Same contract as the DM branch: speak only in rooms you're in —
        // `as` cannot attribute a message to a non-member human.
        if !room.members.contains(&as_name) {
            return err_json(
                StatusCode::UNPROCESSABLE_ENTITY,
                "you are not a member of this room",
            );
        }
        send_room(
            &app,
            &room,
            &as_name,
            as_token.as_deref(),
            &text,
            &client_ip,
        )
        .await
    } else if let Some(pair) = b.conv.strip_prefix("dm:") {
        let names: Vec<&str> = pair.split('|').collect();
        if names.len() != 2 || !names.contains(&as_name.as_str()) {
            return err_json(
                StatusCode::UNPROCESSABLE_ENTITY,
                "conversation does not include the sender",
            );
        }
        let target = if names[0] == as_name {
            names[1]
        } else {
            names[0]
        };
        let conv = dm_conv(&as_name, target);
        if target == "gateway" {
            // Talk to the switchboard itself: same command agent as /peer/gateway.
            let reply = command_reply(&app, &as_name, true, &text).await;
            let m1 = app
                .log_chat(bubble(&conv, &as_name, text, "chat", "ok", None))
                .await;
            let m2 = app
                .log_chat(bubble(&conv, "gateway", reply, "chat", "ok", None))
                .await;
            vec![m1, m2]
        } else if target_is_human(&app, target).await {
            // Human-to-human: UI-only conversation, stored locally.
            vec![
                app.log_chat(bubble(&conv, &as_name, text, "chat", "ok", None))
                    .await,
            ]
        } else {
            // Human-typed DMs are ALWAYS stored — the request bubble lands
            // even on delivery failure or with AGW_AUDIT_PREVIEWS=false
            // (deliver's auto-recording only covers proxied traffic, so we
            // self-record here with record_chat=false).
            let res =
                internal_send(&app, target, as_token.as_deref(), &text, &client_ip, false).await;
            let (ok, reply, err_s) = match res {
                Ok(r) => (true, r, None),
                Err(e) => (false, None, Some(e)),
            };
            let mut recorded = vec![
                app.log_chat(bubble(
                    &conv,
                    &as_name,
                    text,
                    "chat",
                    if ok { "ok" } else { "err" },
                    err_s,
                ))
                .await,
            ];
            if let Some(t) = reply {
                recorded.push(
                    app.log_chat(bubble(&conv, target, t, "chat", "ok", None))
                        .await,
                );
            }
            recorded
        }
    } else {
        return err_json(StatusCode::UNPROCESSABLE_ENTITY, "bad conversation id");
    };
    Json(serde_json::json!({ "messages": recorded })).into_response()
}

async fn target_is_human(app: &AppState, name: &str) -> bool {
    let inner = app.inner.read().await;
    inner
        .peers
        .iter()
        .any(|p| p.name == name && p.kind == PeerKind::Human)
}

/// Validate room member names against the accepted registry; dedup, cap.
async fn validated_members(app: &AppState, members: &[String]) -> Result<Vec<String>, Response> {
    let inner = app.inner.read().await;
    let mut out: Vec<String> = Vec::new();
    for m in members {
        let m = m.trim();
        if m.is_empty() {
            continue;
        }
        if !inner
            .peers
            .iter()
            .any(|p| p.name == m && p.state == crate::state::PeerState::Accepted)
        {
            return Err(err_json(
                StatusCode::UNPROCESSABLE_ENTITY,
                &format!("unknown or unaccepted member: {m}"),
            ));
        }
        if !out.iter().any(|x| x == m) {
            out.push(m.to_string());
        }
    }
    if out.len() > ROOM_MEMBER_CAP {
        return Err(err_json(
            StatusCode::UNPROCESSABLE_ENTITY,
            "too many members",
        ));
    }
    Ok(out)
}

/// Notify agent members about a room roster change; failures become system
/// bubbles in the room thread.
async fn notify_members(app: &AppState, room: &Room, added: &[String], note: &str) {
    let conv = format!("room:{}", room.id);
    let mut agents = Vec::new();
    for m in added {
        let is_agent = {
            let inner = app.inner.read().await;
            inner
                .peers
                .iter()
                .any(|p| p.name == *m && p.kind == PeerKind::Agent)
        };
        if is_agent {
            agents.push(m.clone());
        }
    }
    // Concurrent fanout (same as send_room): total wait is one timeout, not
    // N×timeout, so stalling agents can't pin the admin's request.
    let futs = agents.iter().map(|m| {
        let app = app.clone();
        let text = format!(
            "You were added to room '{}' by {}. Members: {}",
            room.name,
            note,
            room.members.join(", ")
        );
        async move {
            let r = tokio::time::timeout(
                FANOUT_TIMEOUT,
                internal_send(&app, m, None, &text, "internal", false),
            )
            .await;
            (m.clone(), r)
        }
    });
    for (m, r) in futures_util::future::join_all(futs).await {
        let ok = matches!(r, Ok(Ok(_)));
        if !ok {
            app.log_chat(bubble(
                &conv,
                "system",
                format!("roster notification to {m} failed"),
                "system",
                "err",
                None,
            ))
            .await;
        }
    }
}

#[derive(Deserialize)]
pub struct RoomBody {
    name: String,
    #[serde(default)]
    members: Vec<String>,
    #[serde(rename = "as")]
    as_human: Option<String>,
}

/// POST /api/chat/rooms {name, members, as} — create a room and notify the
/// agent members about the roster.
pub async fn api_rooms_create(
    State(app): State<AppState>,
    ClientIp(client_ip): ClientIp,
    Json(b): Json<RoomBody>,
) -> Response {
    if !app.limiter.allow(&format!("chat-rooms-{client_ip}"), 30) {
        return err_json(StatusCode::TOO_MANY_REQUESTS, "rate limit exceeded");
    }
    let name = b.name.trim().to_string();
    if name.is_empty() || name.len() > 64 {
        return err_json(
            StatusCode::UNPROCESSABLE_ENTITY,
            "room name must be 1-64 chars",
        );
    }
    // Creator is always a member of their own room (Telegram-style) so the
    // send-side membership check can't lock them out.
    let creator = match resolve_human(&app, b.as_human.as_deref()).await {
        Ok((n, _)) => n,
        Err(_) => "admin".to_string(),
    };
    let mut requested = b.members.clone();
    if creator != "admin" && !requested.contains(&creator) {
        requested.push(creator.clone());
    }
    let members = match validated_members(&app, &requested).await {
        Ok(m) => m,
        Err(r) => return r,
    };
    let room = Room {
        id: gen_token()[4..10].to_string(),
        name: name.clone(),
        members: members.clone(),
        created_by: creator.clone(),
        created_at: now(),
    };
    {
        let mut inner = app.inner.write().await;
        inner.rooms.push(room.clone());
    }
    app.persist().await;
    app.emit_peers("room", &room.id);

    let conv = format!("room:{}", room.id);
    let sys = app
        .log_chat(bubble(
            &conv,
            "system",
            format!(
                "Room '{}' created by {}. Members: {}",
                room.name,
                creator,
                room.members.join(", ")
            ),
            "system",
            "ok",
            None,
        ))
        .await;
    notify_members(&app, &room, &members, &creator).await;
    Json(serde_json::json!({
        "ok": true,
        "room": {"id": room.id, "name": room.name, "members": room.members,
                 "created_by": room.created_by, "created_at": room.created_at},
        "messages": [sys],
    }))
    .into_response()
}

#[derive(Deserialize)]
pub struct MembersBody {
    #[serde(default)]
    add: Vec<String>,
    #[serde(default)]
    remove: Vec<String>,
    #[serde(rename = "as")]
    as_human: Option<String>,
}

/// POST /api/chat/rooms/{id}/members {add[], remove[], as} — roster change
/// with system bubble + notifications.
pub async fn api_rooms_members(
    State(app): State<AppState>,
    ClientIp(client_ip): ClientIp,
    Path(id): Path<String>,
    Json(b): Json<MembersBody>,
) -> Response {
    if !app.limiter.allow(&format!("chat-members-{client_ip}"), 30) {
        return err_json(StatusCode::TOO_MANY_REQUESTS, "rate limit exceeded");
    }
    let actor = match resolve_human(&app, b.as_human.as_deref()).await {
        Ok((n, _)) => n,
        Err(_) => "admin".to_string(),
    };
    let add = match validated_members(&app, &b.add).await {
        Ok(m) => m,
        Err(r) => return r,
    };
    let room = {
        let mut inner = app.inner.write().await;
        let Some(room) = inner.rooms.iter_mut().find(|r| r.id == id) else {
            return err_json(StatusCode::NOT_FOUND, "no such room");
        };
        // Total-size cap: reject before mutating the roster.
        let would_add = add.iter().filter(|m| !room.members.contains(m)).count();
        if room.members.len() + would_add > ROOM_MEMBER_CAP {
            return err_json(StatusCode::UNPROCESSABLE_ENTITY, "too many members");
        }
        let mut added: Vec<String> = Vec::new();
        for m in &add {
            if !room.members.contains(m) {
                room.members.push(m.clone());
                added.push(m.clone());
            }
        }
        let removed: Vec<String> = room
            .members
            .iter()
            .filter(|m| b.remove.contains(m))
            .cloned()
            .collect();
        room.members.retain(|m| !b.remove.contains(m));
        Some((added, removed))
    };
    let Some((added, removed)) = room else {
        return err_json(StatusCode::NOT_FOUND, "no such room");
    };
    app.persist().await;
    app.emit_peers("room", &id);

    let conv = format!("room:{id}");
    let mut recorded: Vec<ChatMessage> = Vec::new();
    if !added.is_empty() {
        recorded.push(
            app.log_chat(bubble(
                &conv,
                "system",
                format!("{} added {} to the room", actor, added.join(", ")),
                "system",
                "ok",
                None,
            ))
            .await,
        );
    }
    if !removed.is_empty() {
        recorded.push(
            app.log_chat(bubble(
                &conv,
                "system",
                format!("{} removed {} from the room", actor, removed.join(", ")),
                "system",
                "ok",
                None,
            ))
            .await,
        );
    }
    let (room_name, members_now) = {
        let inner = app.inner.read().await;
        match inner.rooms.iter().find(|r| r.id == id) {
            Some(r) => (r.name.clone(), r.members.clone()),
            None => (id.clone(), Vec::new()),
        }
    };
    let probe_room = Room {
        id: id.clone(),
        name: room_name,
        members: members_now,
        created_by: String::new(),
        created_at: 0,
    };
    notify_members(&app, &probe_room, &added, &actor).await;
    Json(serde_json::json!({"ok": true, "messages": recorded})).into_response()
}
