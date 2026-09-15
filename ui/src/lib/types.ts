// Shared API types mirroring the Rust serde structs (src/state.rs, src/api.rs).

export interface Peer {
  name: string;
  kind: "agent" | "human";
  state: "pending" | "accepted" | "revoked";
  url: string | null;
  healthy: boolean | null;
  channel: boolean;
  registered_at: number;
  last_seen: number | null;
  last_ip: string | null;
  reg_ip: string | null;
  auto_accepted: boolean;
  card: Record<string, unknown> | null;
}

export interface PeerRow {
  peer: Peer;
  reqs_1h: number;
  last_activity: number | null;
  series: number[];
}

export interface PeersPayload {
  pending: Peer[];
  accepted: PeerRow[];
  revoked: Peer[];
}

export interface RouteEntry {
  ts: number;
  src: string;
  dst: string;
  method: string;
  status: number;
  bytes: number;
  latency_ms: number;
  rpc_method?: string | null;
  rpc_id?: string | null;
  preview?: string | null;
  resp_preview?: string | null;
  task_state?: string | null;
  task_id?: string | null;
  context_id?: string | null;
}

export interface Bucket {
  ts: number;
  routed: number;
  errors: number;
  p50: number | null;
  p95: number | null;
}

export interface Summary {
  window_sec: number;
  routed: number;
  rate_per_min: number;
  errors: number;
  err_pct: number;
  p50: number | null;
  p95: number | null;
  pending: number;
  peers_up: number;
  peers_total: number;
  channels: number;
  input_required: number;
  prev_routed: number;
  prev_errors: number;
  buckets: Bucket[];
  recent: RouteEntry[];
  peers: TopoPeer[];
  generated_at: number;
}

export interface TopoPeer {
  name: string;
  state: "pending" | "accepted" | "revoked";
  healthy: boolean | null;
  channel: boolean;
  reqs_1h: number;
}

export interface CardSkill {
  id: string;
  name: string;
  description: string;
  tags: string;
}

export interface PeerDetail {
  peer: Peer;
  channel: boolean;
  reqs_1h: number;
  last_activity: number | null;
  series: number[];
  card: {
    name: string;
    description: string;
    version: string;
    provider: string;
    streaming: boolean | null;
    push: boolean | null;
    sth: boolean | null;
    skills: CardSkill[];
    raw: string;
  } | null;
  traffic: RouteEntry[];
  traffic_total: number;
  ok_count: number;
  err_count: number;
}

export interface LogsPayload {
  entries: RouteEntry[];
  total_matched: number;
  next_before_ts: number | null;
}

export interface TaskHistoryItem {
  ts: number;
  state: string;
  status: number;
}

export interface TaskEntry {
  id: string;
  context_id: string | null;
  src: string;
  dst: string;
  state: string;
  created_ts: number;
  updated_ts: number;
  request_preview: string | null;
  response_preview: string | null;
  history: TaskHistoryItem[];
}

export interface TasksPayload {
  tasks: TaskEntry[];
  total: number;
  active_count: number;
}

export interface Notification {
  kind: "pending" | "unhealthy" | "input_required" | "error_spike";
  severity: "warn" | "bad" | "accent";
  title: string;
  detail: string;
  href: string;
  count: number;
}

export interface NotificationsPayload {
  items: Notification[];
  total: number;
}

export interface ChatMessage {
  id: number;
  ts: number;
  conv: string;
  src: string;
  text: string;
  kind: "chat" | "system";
  status: "ok" | "err";
  error?: string | null;
  pending?: boolean;
}

export interface ChatState {
  humans: string[];
  peers: { name: string; healthy: boolean | null; human: boolean }[];
  rooms: { id: string; name: string; members: string[]; created_by: string; created_at: number }[];
  conversations: {
    id: string;
    kind: "dm" | "room";
    members: string[];
    last?: { id: number; ts: number; src: string; text: string; status: string };
  }[];
  last_id: number;
}

export interface AuthOk {
  ok: boolean;
  password_set: boolean;
  localhost: boolean;
  version: string;
}

export interface SettingsPayload {
  gateway_token: string;
  bootstrap_token: string;
  humans: { name: string; token: string; registered_at: number }[];
  password_set: boolean;
}
