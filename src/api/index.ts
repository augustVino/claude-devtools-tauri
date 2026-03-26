/**
 * Unified API adapter.
 *
 * When running inside Tauri, `window.__TAURI_INTERNALS__` is injected.
 * When running in a browser (e.g. via the HTTP server), we fall back to an
 * HTTP+SSE client that implements the same interface.
 *
 * All renderer code should import `api` from this module.
 *
 * The instance is resolved lazily on first property access so that test code
 * can install mocks before the adapter resolves.
 */

import { HttpAPIClient } from "./httpClient";
import { TauriAPIClient } from "./tauriClient";

import type { ElectronAPI } from "@shared/types/api";

/**
 * Resolves the base URL for the HTTP API client.
 *
 * - Desktop "server mode" (browser opened via ?port=XXXX): use explicit port on 127.0.0.1
 * - Standalone/Docker (page served by the same server): use window.location.origin
 *   to avoid cross-origin issues (localhost vs 127.0.0.1)
 */
function getHttpBaseUrl(): string {
  const params = new URLSearchParams(window.location.search);
  const explicitPort = params.get("port");
  if (explicitPort) {
    return `http://127.0.0.1:${parseInt(explicitPort, 10)}`;
  }
  return window.location.origin;
}

let httpClient: HttpAPIClient | null = null;
let tauriClient: TauriAPIClient | null = null;

export const isTauriMode = (): boolean =>
  !!(window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;

function getImpl(): ElectronAPI {
  if (isTauriMode()) {
    if (!tauriClient) {
      tauriClient = new TauriAPIClient();
    }
    return tauriClient;
  }
  // Lazily create the HTTP client only when actually needed (browser mode).
  // Caching avoids creating multiple EventSource connections.
  if (!httpClient) {
    httpClient = new HttpAPIClient(getHttpBaseUrl());
  }
  return httpClient;
}

/**
 * Proxy that lazily resolves the underlying ElectronAPI on first property access.
 * In Tauri: delegates to `TauriAPIClient` (uses @tauri-apps/api invoke).
 * In browser: delegates to `HttpAPIClient` (created on first use).
 * In tests: delegates to whatever mock is installed.
 */
/**
 * Whether the app is running in desktop mode (Tauri) or in a browser via HTTP server (false).
 * Use this to hide desktop-only UI (settings, traffic lights, etc.) in browser mode.
 */
export const isDesktopMode = (): boolean => isTauriMode();

export const api: ElectronAPI = new Proxy({} as ElectronAPI, {
  get(_target, prop, receiver) {
    const impl = getImpl();
    const value = Reflect.get(impl, prop, receiver) as unknown;
    if (typeof value === "function") {
      return (value as (...args: unknown[]) => unknown).bind(impl);
    }
    return value;
  },
});
