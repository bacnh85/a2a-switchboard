import { signal } from "@preact/signals";
import { apiGet } from "./api";
import type { AuthOk, NotificationsPayload } from "./types";

/** Session/instance info from GET /api/auth/ok (null = not loaded yet). */
export const auth = signal<AuthOk | null>(null);
export const notifications = signal<NotificationsPayload>({ items: [], total: 0 });

export async function fetchAuth() {
  try {
    auth.value = await apiGet<AuthOk>("/api/auth/ok");
  } catch {
    auth.value = null;
  }
}

/** Bell + nav badges refresh (called on SSE activity and on an interval). */
export async function refreshNotifications() {
  try {
    notifications.value = await apiGet<NotificationsPayload>("/api/notifications");
  } catch {
    /* transient — keep the last snapshot */
  }
}
