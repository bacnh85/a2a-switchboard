//! A2A task inbox: task lifecycle derived from routed message/send traffic.
//! Every proxied `message/send` whose response carries task structure
//! (`result.status.state` / `result.id`) updates an in-memory task entry —
//! keyed by the A2A task id when present, else the JSON-RPC request id.
//! State is re-derived from routing.jsonl at boot, so the inbox survives
//! restarts exactly like the audit log. Operators answer `input-required`
//! tasks and cancel active ones; both interventions travel the normal
//! dual-mode delivery path so they are audited and chat-mirrored.

use crate::chat::{err_json, internal_rpc, is_message_send};
use crate::state::{now, AppState, RouteEntry};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::RwLock as StdRwLock;

/// Kept tasks (bounded: oldest closed entries evicted first).
const TASK_CAP: usize = 500;

#[derive(Debug, Clone, serde::Serialize)]
pub struct TaskHistory {
    pub ts: i64,
    pub state: String,
    pub status: u16,
}

/// One tracked A2A task. `state` is stored display-normalized (lowercase,
/// no `TASK_STATE_` prefix); `a2a` marks ids that came from a real task
/// object (safe to address with `tasks/cancel`), vs. rpc-id fallbacks.
#[derive(Debug, Clone, serde::Serialize)]
pub struct TaskEntry {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_id: Option<String>,
    pub src: String,
    pub dst: String,
    pub state: String,
    pub a2a: bool,
    pub created_ts: i64,
    pub updated_ts: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_preview: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_preview: Option<String>,
    pub history: Vec<TaskHistory>,
}

#[derive(Default)]
pub struct Tasks {
    map: StdRwLock<HashMap<String, TaskEntry>>,
}

fn normalize(state: &str) -> String {
    state
        .trim_start_matches("TASK_STATE_")
        .trim_start_matches("task_state_")
        .to_ascii_lowercase()
        .replace('_', "-")
}

fn is_active(state: &str) -> bool {
    matches!(state, "submitted" | "working" | "queued" | "input-required")
}

impl Tasks {
    /// Boot derivation: replay routing entries (oldest first) into the map.
    pub fn derive(entries: &[RouteEntry]) -> Self {
        let tasks = Self::default();
        for e in entries {
            let map = tasks.map.write().unwrap();
            observe_locked(map, e);
        }
        tasks
    }

    /// Live update from log_route (single choke point for routed traffic).
    pub async fn observe(&self, e: &RouteEntry) {
        let map = self.map.write().unwrap();
        observe_locked(map, e);
    }

    pub async fn get(&self, id: &str) -> Option<TaskEntry> {
        self.map.read().unwrap().get(id).cloned()
    }

    /// Newest-activity-first listing. `state`: "" | active | closed |
    /// input-required; `q`: substring over id/context/src/dst.
    pub async fn list(&self, state_filter: &str, q: &str, n: usize) -> (Vec<TaskEntry>, usize) {
        let map = self.map.read().unwrap();
        let ql = q.to_lowercase();
        let mut v: Vec<TaskEntry> = map
            .values()
            .filter(|t| match state_filter {
                "active" => is_active(&t.state),
                "closed" => !is_active(&t.state),
                "input-required" => t.state == "input-required",
                _ => true,
            })
            .filter(|t| {
                ql.is_empty()
                    || t.id.to_lowercase().contains(&ql)
                    || t.src.to_lowercase().contains(&ql)
                    || t.dst.to_lowercase().contains(&ql)
                    || t.context_id
                        .as_deref()
                        .is_some_and(|c| c.to_lowercase().contains(&ql))
            })
            .cloned()
            .collect();
        let total = v.len();
        v.sort_by(|a, b| b.updated_ts.cmp(&a.updated_ts).then(a.id.cmp(&b.id)));
        v.truncate(n);
        (v, total)
    }

    pub async fn active_count(&self) -> usize {
        self.map
            .read()
            .unwrap()
            .values()
            .filter(|t| is_active(&t.state))
            .count()
    }

    pub async fn input_required_count(&self) -> usize {
        self.map
            .read()
            .unwrap()
            .values()
            .filter(|t| t.state == "input-required")
            .count()
    }

    fn insert(&self, entry: TaskEntry) {
        self.map.write().unwrap().insert(entry.id.clone(), entry);
    }
}

fn observe_locked(
    mut map: std::sync::RwLockWriteGuard<'_, HashMap<String, TaskEntry>>,
    e: &RouteEntry,
) {
    if !is_message_send(e.rpc_method.as_deref()) {
        return;
    }
    let state = e
        .task_state
        .as_deref()
        .map(normalize)
        .or_else(|| e.task_id.as_ref().map(|_| "submitted".to_string()));
    let Some(state) = state else { return };
    let Some(key) = e.task_id.clone().or_else(|| e.rpc_id.clone()) else {
        return;
    };
    if let Some(t) = map.get_mut(&key) {
        if e.context_id.is_some() {
            t.context_id = e.context_id.clone();
        }
        if e.task_id.is_some() {
            t.a2a = true;
        }
        t.src = e.src.clone();
        t.dst = e.dst.clone();
        t.updated_ts = e.ts;
        if e.preview.is_some() {
            t.request_preview = e.preview.clone();
        }
        t.response_preview = e.resp_preview.clone();
        if t.history.last().map(|h| h.state.as_str()) != Some(state.as_str()) {
            t.history.push(TaskHistory {
                ts: e.ts,
                state: state.clone(),
                status: e.status,
            });
            t.state = state;
        }
        return;
    }
    map.insert(
        key.clone(),
        TaskEntry {
            id: key,
            context_id: e.context_id.clone(),
            src: e.src.clone(),
            dst: e.dst.clone(),
            state: state.clone(),
            a2a: e.task_id.is_some(),
            created_ts: e.ts,
            updated_ts: e.ts,
            request_preview: e.preview.clone(),
            response_preview: e.resp_preview.clone(),
            history: vec![TaskHistory {
                ts: e.ts,
                state,
                status: e.status,
            }],
        },
    );
    if map.len() > TASK_CAP {
        // Evict the oldest closed task; fall back to the oldest overall.
        let victim = map
            .values()
            .filter(|t| !is_active(&t.state))
            .min_by_key(|t| (t.updated_ts, t.created_ts))
            .map(|t| t.id.clone())
            .or_else(|| {
                map.values()
                    .min_by_key(|t| (t.updated_ts, t.created_ts))
                    .map(|t| t.id.clone())
            });
        if let Some(v) = victim {
            map.remove(&v);
        }
    }
}

// ----- endpoints -----

#[derive(Deserialize, Default)]
pub struct TaskQuery {
    pub state: Option<String>,
    pub q: Option<String>,
    pub n: Option<usize>,
}

/// GET /api/tasks?state=&q=&n= — inbox listing, newest activity first.
pub async fn list(State(app): State<AppState>, Query(q): Query<TaskQuery>) -> Response {
    let (tasks, total) = app
        .tasks
        .list(
            q.state.as_deref().unwrap_or_default(),
            q.q.as_deref().unwrap_or_default(),
            q.n.unwrap_or(200).clamp(1, 1000),
        )
        .await;
    let active_count = app.tasks.active_count().await;
    Json(serde_json::json!({
        "tasks": tasks,
        "total": total,
        "active_count": active_count,
    }))
    .into_response()
}

/// GET /api/tasks/{id} — one task with its lifecycle history.
pub async fn detail(State(app): State<AppState>, Path(id): Path<String>) -> Response {
    match app.tasks.get(&id).await {
        Some(t) => Json(serde_json::json!({ "task": t })).into_response(),
        None => err_json(StatusCode::NOT_FOUND, "no such task"),
    }
}

#[derive(Deserialize)]
pub struct ReplyBody {
    #[serde(rename = "as")]
    as_human: Option<String>,
    text: String,
}

/// POST /api/tasks/{id}/reply {as, text} — operator answers an active task:
/// a follow-up message/send on the task's context, delivered through the
/// normal path (audited + mirrored into the human↔agent DM).
pub async fn reply(
    State(app): State<AppState>,
    crate::auth::ClientIp(client_ip): crate::auth::ClientIp,
    Path(id): Path<String>,
    Json(b): Json<ReplyBody>,
) -> Response {
    let text = b.text.trim().to_string();
    if text.is_empty() {
        return err_json(StatusCode::UNPROCESSABLE_ENTITY, "empty reply");
    }
    let Some(task) = app.tasks.get(&id).await else {
        return err_json(StatusCode::NOT_FOUND, "no such task");
    };
    if !is_active(&task.state) {
        return err_json(
            StatusCode::CONFLICT,
            "task is closed — no further intervention",
        );
    }
    let Some(context_id) = task.context_id.clone() else {
        return err_json(
            StatusCode::UNPROCESSABLE_ENTITY,
            "task carries no A2A context id — reply through Chat instead",
        );
    };
    let (_, as_token) = match crate::chat::resolve_human(&app, b.as_human.as_deref()).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    let res = internal_rpc(
        &app,
        &task.dst,
        as_token.as_deref(),
        &client_ip,
        "message/send",
        serde_json::json!({
            "message": {
                "role": "user",
                "kind": "message",
                "messageId": format!("op-{}", &crate::state::gen_token()[4..12]),
                "contextId": context_id,
                "taskId": task.id,
                "parts": [{"kind": "text", "text": text}],
            }
        }),
        true,
    )
    .await;
    match res {
        Ok((status, _body)) => {
            // Deliver → log_route already observed any task state change.
            match app.tasks.get(&id).await {
                Some(t) => {
                    Json(serde_json::json!({ "ok": true, "status": status.as_u16(), "task": t }))
                        .into_response()
                }
                None => Json(serde_json::json!({ "ok": true, "status": status.as_u16() }))
                    .into_response(),
            }
        }
        Err(e) => err_json(StatusCode::BAD_GATEWAY, &e),
    }
}

/// POST /api/tasks/{id}/cancel — operator cancels an active task via the
/// A2A `tasks/cancel` method (only possible for real task ids).
pub async fn cancel(
    State(app): State<AppState>,
    crate::auth::ClientIp(client_ip): crate::auth::ClientIp,
    Path(id): Path<String>,
) -> Response {
    let Some(task) = app.tasks.get(&id).await else {
        return err_json(StatusCode::NOT_FOUND, "no such task");
    };
    if !is_active(&task.state) {
        return err_json(StatusCode::CONFLICT, "task is already closed");
    }
    if !task.a2a {
        return err_json(
            StatusCode::UNPROCESSABLE_ENTITY,
            "task id is an rpc-id fallback, not an A2A task id — cannot cancel",
        );
    }
    let res = internal_rpc(
        &app,
        &task.dst,
        None,
        &client_ip,
        "tasks/cancel",
        serde_json::json!({ "id": task.id }),
        false,
    )
    .await;
    match res {
        Ok((status, body)) => {
            // Cancel responses carry the task object with its new state; the
            // cancel method isn't observed (not message/send), so apply it here.
            if let Some(b) = body.as_deref() {
                let (state, _, _) = crate::state::audit_extract_response(b.as_bytes());
                if let Some(s) = state {
                    if let Some(mut t) = app.tasks.get(&id).await {
                        t.state = normalize(&s);
                        t.updated_ts = now();
                        t.history.push(TaskHistory {
                            ts: t.updated_ts,
                            state: t.state.clone(),
                            status: status.as_u16(),
                        });
                        app.tasks.insert(t);
                    }
                }
            }
            match app.tasks.get(&id).await {
                Some(t) => Json(serde_json::json!({ "ok": true, "task": t })).into_response(),
                None => Json(serde_json::json!({ "ok": true })).into_response(),
            }
        }
        Err(e) => err_json(StatusCode::BAD_GATEWAY, &e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn observe_creates_task_from_route_entry() {
        let e = crate::state::RouteEntry {
            ts: now(),
            src: "bootstrap".into(),
            dst: "agentx".into(),
            method: "POST".into(),
            status: 200,
            bytes: 1,
            latency_ms: 1,
            rpc_method: Some("message/send".into()),
            rpc_id: Some("1".into()),
            preview: None,
            resp_preview: None,
            task_state: Some("TASK_STATE_INPUT_REQUIRED".into()),
            task_id: Some("task-xyz".into()),
            context_id: Some("ctx-1".into()),
        };
        let tasks = Tasks::default();
        futures_executor_block_on(tasks.observe(&e));
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let (v, _total) = rt.block_on(tasks.list("input-required", "", 100));
        assert_eq!(v.len(), 1, "{v:?}");
        assert_eq!(v[0].state, "input-required");
    }

    fn futures_executor_block_on(f: impl std::future::Future<Output = ()>) {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(f);
    }
}
