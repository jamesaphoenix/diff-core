/**
 * Transport selection: Tauri IPC → web API (diffcore-web server) → demo mocks.
 *
 * `initBackend()` must resolve before first render so mode flags are stable.
 */

export const IS_TAURI = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

/** True when a real backend (Tauri IPC or diffcore-web) is reachable. */
export let HAS_BACKEND = IS_TAURI;
export let DEFAULT_REPO: string | null = null;

export async function initBackend(): Promise<void> {
  if (IS_TAURI) return;
  try {
    // Bounded so a stalling proxy can't blank-screen the app forever.
    const res = await fetch("/api/health", { signal: AbortSignal.timeout(2000) });
    if (res.ok) {
      const health = await res.json();
      if (health?.ok === true) {
        HAS_BACKEND = true;
        DEFAULT_REPO = health.default_repo ?? null;
      }
    }
  } catch {
    // No server — demo mode with mock data.
  }
}

/** Invoke a backend command; mirrors Tauri's `invoke` contract in web mode. */
export async function invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  if (IS_TAURI) {
    const { invoke: tauri } = await import("@tauri-apps/api/core");
    return tauri<T>(cmd, args);
  }
  const res = await fetch(`/api/invoke/${cmd}`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(args ?? {}),
  });
  if (!res.ok) {
    throw new Error(await res.text());
  }
  return (await res.json()) as T;
}
