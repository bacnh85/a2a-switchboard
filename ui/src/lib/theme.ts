import { signal } from "@preact/signals";

export type Theme = "light" | "dark";

function initial(): Theme {
  try {
    const stored = localStorage.getItem("agw-theme");
    if (stored === "light" || stored === "dark") return stored;
  } catch {
    /* storage unavailable */
  }
  return matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light";
}

export const theme = signal<Theme>(initial());

export function setTheme(t: Theme) {
  theme.value = t;
  document.documentElement.dataset.theme = t;
  try {
    localStorage.setItem("agw-theme", t);
  } catch {
    /* storage unavailable */
  }
}

export function toggleTheme() {
  setTheme(theme.value === "dark" ? "light" : "dark");
}
