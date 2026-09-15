import { useEffect, useState } from "preact/hooks";
import { apiGet, apiPost } from "../lib/api";
import { identityIdx } from "../lib/format";
import type { SettingsPayload } from "../lib/types";
import { Badge, Btn, EmptyState, Skeleton, TokenRow, toast } from "../components/ui";

export function Settings() {
  const [data, setData] = useState<SettingsPayload | null>(null);
  const [error, setError] = useState("");

  const load = () =>
    apiGet<SettingsPayload>("/api/settings")
      .then(setData)
      .catch((e) => setError(e instanceof Error ? e.message : String(e)));

  useEffect(() => {
    load();
  }, []);

  if (error && !data) return <EmptyState title="Could not load settings" hint={error} />;
  if (!data) return <Skeleton h={260} />;

  return (
    <div class="settings-grid">
      <PasswordSection passwordSet={data.password_set} onChanged={load} />
      <HumansSection humans={data.humans} onChanged={load} />
      <TokensSection data={data} onChanged={load} />
    </div>
  );
}

function PasswordSection(props: { passwordSet: boolean; onChanged: () => void }) {
  const [current, setCurrent] = useState("");
  const [next, setNext] = useState("");
  const [confirm, setConfirm] = useState("");
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState("");

  const submit = async (e: Event) => {
    e.preventDefault();
    setErr("");
    if (next.length < 8) return setErr("New password must be at least 8 characters.");
    if (next !== confirm) return setErr("New passwords don't match.");
    setBusy(true);
    try {
      await apiPost("/api/settings/password", { current: current || undefined, new: next, confirm });
      toast("Password changed", "ok");
      setCurrent("");
      setNext("");
      setConfirm("");
      props.onChanged();
    } catch (e2) {
      setErr(e2 instanceof Error ? e2.message : "change failed");
    } finally {
      setBusy(false);
    }
  };

  return (
    <div class="section">
      <div class="section-head">
        <h2>Admin password</h2>
        {props.passwordSet ? <Badge tone="ok">set</Badge> : <Badge tone="warn">not set — console is open</Badge>}
      </div>
      <div class="panel">
        <form onSubmit={submit}>
          <div class="field">
            <label>Current password</label>
            <input type="password" class="input" autocomplete="current-password" value={current} onInput={(e) => setCurrent((e.target as HTMLInputElement).value)} />
          </div>
          <div class="field-row">
            <div class="field">
              <label>New password (min 8 chars)</label>
              <input type="password" class="input" autocomplete="new-password" value={next} onInput={(e) => setNext((e.target as HTMLInputElement).value)} />
            </div>
            <div class="field">
              <label>Confirm new password</label>
              <input type="password" class="input" autocomplete="new-password" value={confirm} onInput={(e) => setConfirm((e.target as HTMLInputElement).value)} />
            </div>
          </div>
          {err && <div class="field">
            <span class="err">{err}</span>
          </div>}
          <Btn variant="primary" type="submit" disabled={busy || !next}>
            {busy ? "Changing…" : "Change password"}
          </Btn>
        </form>
      </div>
    </div>
  );
}

function HumansSection(props: { humans: SettingsPayload["humans"]; onChanged: () => void }) {
  const [name, setName] = useState("");
  const [busy, setBusy] = useState(false);
  const [newToken, setNewToken] = useState<string | null>(null);

  const create = async (e: Event) => {
    e.preventDefault();
    if (!name.trim()) return;
    setBusy(true);
    try {
      const res = await apiPost<{ token: string }>("/api/settings/humans", { name: name.trim() });
      setNewToken(res.token);
      setName("");
      toast(`Operator “${name.trim()}” created — copy the token now, it's shown once`, "ok");
      props.onChanged();
    } catch (e2) {
      toast(e2 instanceof Error ? e2.message : "create failed", "bad");
    } finally {
      setBusy(false);
    }
  };

  const remove = async (n: string) => {
    if (!confirm(`Delete operator “${n}”? Their token stops working immediately.`)) return;
    try {
      await apiPost(`/api/settings/humans/${encodeURIComponent(n)}/delete`);
      toast(`Deleted ${n}`);
      props.onChanged();
    } catch (e) {
      toast(e instanceof Error ? e.message : "delete failed", "bad");
    }
  };

  return (
    <div class="section">
      <div class="section-head">
        <h2>Human operators</h2>
        <span class="hint">identities for the messenger — they can chat as themselves</span>
      </div>
      <div class="panel">
        <form onSubmit={create} class="inline-form" style="margin-bottom:14px">
          <div class="field" style="flex:1">
            <label>Operator name</label>
            <input class="input" placeholder="e.g. HB" value={name} onInput={(e) => setName((e.target as HTMLInputElement).value)} />
          </div>
          <Btn variant="primary" type="submit" disabled={busy || !name.trim()}>
            Create
          </Btn>
        </form>
        {newToken && (
          <div style="margin-bottom:14px" class="panel">
            <div style="font-size:0.82rem;font-weight:600;margin-bottom:6px">
              Token for the new operator — shown once:
            </div>
            <TokenRow token={newToken} reveal />
          </div>
        )}
        {props.humans.length === 0 ? (
          <EmptyState title="No operators yet" hint="Create one per human who wants to use the Chat console — messages get attributed to them." />
        ) : (
          <div class="table-wrap">
            <table class="data">
              <thead>
                <tr>
                  <th>Name</th>
                  <th>Token</th>
                  <th style="text-align:right">Actions</th>
                </tr>
              </thead>
              <tbody>
                {props.humans.map((h) => (
                  <tr key={h.name}>
                    <td>
                      <span style="display:inline-flex;align-items:center;gap:8px">
                        <span class="avatar" style={`background:var(--c${identityIdx(h.name)}-bg);color:var(--c${identityIdx(h.name)});width:24px;height:24px;font-size:0.7rem`}>
                          {h.name.slice(0, 2).toUpperCase()}
                        </span>
                        <span class="mono">{h.name}</span>
                      </span>
                    </td>
                    <td>
                      <TokenRow token={h.token} />
                    </td>
                    <td style="text-align:right">
                      <Btn variant="danger" size="sm" onClick={() => remove(h.name)}>
                        Delete
                      </Btn>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </div>
    </div>
  );
}

function TokensSection(props: { data: SettingsPayload; onChanged: () => void }) {
  const [confirmBootstrap, setConfirmBootstrap] = useState(false);
  const regen = async () => {
    try {
      await apiPost("/api/settings/bootstrap/regenerate");
      toast("Bootstrap token regenerated", "ok");
      setConfirmBootstrap(false);
      props.onChanged();
    } catch (e) {
      toast(e instanceof Error ? e.message : "regenerate failed", "bad");
    }
  };

  return (
    <div class="section">
      <div class="section-head">
        <h2>Gateway tokens</h2>
        <span class="hint">agents authenticate with these — treat them like secrets</span>
      </div>
      <div class="panel">
        <div class="field">
          <label>Gateway API token — agents pending your approval</label>
          <TokenRow token={props.data.gateway_token} />
        </div>
        <div class="field" style="margin-bottom:10px">
          <label>Bootstrap token — auto-accepts registration (careful)</label>
          <TokenRow token={props.data.bootstrap_token} />
        </div>
        {confirmBootstrap ? (
          <div style="display:flex;gap:8px;align-items:center">
            <span style="font-size:0.82rem;color:var(--warn)">Invalidate the old bootstrap token?</span>
            <Btn variant="danger" size="sm" onClick={regen}>
              Confirm
            </Btn>
            <Btn variant="ghost" size="sm" onClick={() => setConfirmBootstrap(false)}>
              Cancel
            </Btn>
          </div>
        ) : (
          <Btn variant="ghost" size="sm" onClick={() => setConfirmBootstrap(true)}>
            Regenerate bootstrap token
          </Btn>
        )}
      </div>
    </div>
  );
}
