import { useEffect, useState } from "preact/hooks";
import { apiGet, apiPost } from "../lib/api";
import type { AuthOk } from "../lib/types";
import { BrandMark, Btn } from "../components/ui";
import { theme, setTheme } from "../lib/theme";
import { Icon } from "../components/ui";

export function Login() {
  const [password, setPassword] = useState("");
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const [info, setInfo] = useState<AuthOk | null>(null);

  // Already authed (or no password set)? Skip the form. Reaching the success
  // path at all proves a valid session — the API layer would have 401'd.
  useEffect(() => {
    apiGet<AuthOk>("/api/auth/ok").then((a) => {
      setInfo(a);
      if (a.ok && a.password_set) location.href = "/";
    });
  }, []);

  const submit = async (e: Event) => {
    e.preventDefault();
    if (!password || busy) return;
    setBusy(true);
    setError("");
    try {
      await apiPost("/api/login", { password });
      location.href = "/";
    } catch (err) {
      setError(err instanceof Error ? err.message : "sign-in failed");
      setBusy(false);
    }
  };

  return (
    <div class="login-wrap">
      <div class="login-card">
        <div class="brand">
          <BrandMark size={26} />
          <span>
            a2a-<b style="color:var(--accent)">switchboard</b>
          </span>
          <span style="flex:1" />
          <button class="btn-icon" aria-label="Toggle theme" onClick={() => setTheme(theme.value === "dark" ? "light" : "dark")}>
            <Icon name={theme.value === "dark" ? "sun" : "moon"} />
          </button>
        </div>
        {info && !info.password_set ? (
          <>
            <p style="color:var(--muted);font-size:0.9rem;margin:0 0 18px">
              No admin password is set on this gateway yet. The console is open — set a password in Settings once you're in.
            </p>
            <Btn variant="primary" onClick={() => (location.href = "/")} style="width:100%">
              Open console
            </Btn>
          </>
        ) : (
          <form onSubmit={submit}>
            {error && <div class="login-error" role="alert">{error}</div>}
            <div class="field">
              <label for="pw">Admin password</label>
              <input
                id="pw"
                type="password"
                class="input"
                placeholder="••••••••"
                autocomplete="current-password"
                autofocus
                value={password}
                onInput={(e) => setPassword((e.target as HTMLInputElement).value)}
              />
            </div>
            <Btn variant="primary" type="submit" disabled={!password || busy} style="width:100%;margin-top:4px">
              {busy ? "Signing in…" : "Sign in"}
            </Btn>
          </form>
        )}
        {info && (
          <p style="color:var(--faint);font-size:0.75rem;margin:18px 0 0;text-align:center" class="mono">
            gateway v{info.version} · sessions last 12h
          </p>
        )}
      </div>
    </div>
  );
}
