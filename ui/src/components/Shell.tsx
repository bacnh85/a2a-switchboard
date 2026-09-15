import type { ComponentChildren } from "preact";
import { useEffect, useRef, useState } from "preact/hooks";
import { useLocation } from "wouter-preact";
import { theme, toggleTheme } from "../lib/theme";
import { auth, notifications, refreshNotifications } from "../lib/store";
import { apiPost } from "../lib/api";
import { sseStart } from "../lib/sse";
import { BrandMark, Conn, Icon, Toasts } from "./ui";

const NAV = [
  { href: "/", name: "Dashboard", icon: "dashboard" },
  { href: "/peers", name: "Peers", icon: "peers" },
  { href: "/tasks", name: "Tasks", icon: "tasks" },
  { href: "/logs", name: "Logs", icon: "logs" },
  { href: "/chat", name: "Chat", icon: "chat" },
  { href: "/settings", name: "Settings", icon: "settings" },
];

const TITLES: Record<string, string> = {
  "/": "Dashboard",
  "/peers": "Peers",
  "/tasks": "Tasks",
  "/logs": "Communication log",
  "/chat": "Chat",
  "/settings": "Settings",
  "/login": "Sign in",
};

function isActivePath(path: string, href: string): boolean {
  if (href === "/") return path === "/";
  return path === href || path.startsWith(href + "/");
}

function Bell() {
  const [open, setOpen] = useState(false);
  const wrap = useRef<HTMLDivElement>(null);
  const items = notifications.value.items;

  useEffect(() => {
    const onDoc = (e: MouseEvent) => {
      if (wrap.current && !wrap.current.contains(e.target as Node)) setOpen(false);
    };
    document.addEventListener("mousedown", onDoc);
    return () => document.removeEventListener("mousedown", onDoc);
  }, []);

  return (
    <div class="bell-wrap" ref={wrap}>
      <button
        class="btn-icon"
        aria-label={`Notifications${notifications.value.total ? ` (${notifications.value.total})` : ""}`}
        onClick={() => {
          setOpen(!open);
          if (!open) refreshNotifications();
        }}
      >
        <Icon name="bell" />
      </button>
      {notifications.value.total > 0 && <span class="bell-count">{notifications.value.total > 99 ? "99+" : notifications.value.total}</span>}
      {open && (
        <div class="bell-pop">
          <div class="bell-head">
            Notifications
            <span style="flex:1" />
            <span class="hint" style="font-size:0.75rem;color:var(--faint)">
              live
            </span>
          </div>
          <div class="bell-list">
            {items.length === 0 ? (
              <div style="padding:22px;text-align:center;color:var(--muted);font-size:0.84rem">All clear — nothing needs attention.</div>
            ) : (
              items.map((n, i) => (
                <a
                  key={i}
                  class="bell-item unread"
                  href={n.href}
                  onClick={() => setOpen(false)}
                >
                  <span class={`dot ${n.severity === "bad" ? "bad" : n.severity === "warn" ? "warn" : ""}`} style="margin-top:5px" />
                  <span style="min-width:0">
                    <div class="b-title">
                      {n.title}
                      {n.count > 1 && <span class="chip" style="margin-left:7px">×{n.count}</span>}
                    </div>
                    <div class="b-sub">{n.detail}</div>
                  </span>
                </a>
              ))
            )}
          </div>
        </div>
      )}
    </div>
  );
}

export function Shell(props: { title?: string; children: ComponentChildren }) {
  const [loc] = useLocation();
  const [menuOpen, setMenuOpen] = useState(false);

  // one SSE connection + notification polling for the whole app
  useEffect(() => {
    sseStart();
    refreshNotifications();
    const iv = setInterval(refreshNotifications, 30_000);
    return () => clearInterval(iv);
  }, []);

  const title = props.title ?? TITLES[loc] ?? "a2a-switchboard";
  const signOut = async () => {
    try {
      await apiPost("/api/logout");
    } catch {
      /* session may already be gone */
    }
    location.href = "/login";
  };

  return (
    <div class="shell">
      <nav class={`sidebar${menuOpen ? " open" : ""}`}>
        <div class="brand">
          <BrandMark />
          <span class="brand-name">
            a2a-<b>switchboard</b>
          </span>
        </div>
        {NAV.map((n) => (
          <a key={n.href} href={n.href} class={`nav-item${isActivePath(loc, n.href) ? " active" : ""}`} onClick={() => setMenuOpen(false)}>
            <span class="nav-ico">
              <Icon name={n.icon} />
            </span>
            {n.name}
            {n.href === "/peers" && notifications.value.items.some((i) => i.kind === "pending") && (
              <span class="nav-badge">!</span>
            )}
          </a>
        ))}
        <div class="sidebar-foot">
          <span>v{auth.value?.version ?? "–"}</span>
          <a
            href="#"
            onClick={(e) => {
              e.preventDefault();
              signOut();
            }}
          >
            Sign out
          </a>
        </div>
      </nav>
      <div class="main">
        {auth.value && !auth.value.localhost && (
          <div class="sec-banner">
            ⚠ This console is reachable beyond localhost. Make sure it sits behind a TLS terminator and uses a strong admin password.
          </div>
        )}
        <header class="topbar">
          <button class="btn-icon menu-btn" aria-label="Menu" onClick={() => setMenuOpen(!menuOpen)}>
            <Icon name="menu" />
          </button>
          <h1>{title}</h1>
          <span class="topbar-spacer" />
          <div class="topbar-ctl">
            <Conn />
            <Bell />
            <button class="btn-icon" aria-label="Toggle theme" title={theme.value === "dark" ? "Switch to light" : "Switch to dark"} onClick={toggleTheme}>
              <Icon name={theme.value === "dark" ? "sun" : "moon"} />
            </button>
          </div>
        </header>
        <main class="content">{props.children}</main>
      </div>
      <Toasts />
    </div>
  );
}
