/**
 * Store index - combines all slices and exports the unified store.
 */

import { api, isDesktopMode } from "@renderer/api";
import { create } from "zustand";

import { createConfigSlice } from "./slices/configSlice";
import { createConnectionSlice } from "./slices/connectionSlice";
import { createContextSlice } from "./slices/contextSlice";
import { createConversationSlice } from "./slices/conversationSlice";
import { createNotificationSlice } from "./slices/notificationSlice";
import { createPaneSlice } from "./slices/paneSlice";
import { createProjectSlice } from "./slices/projectSlice";
import { createRepositorySlice } from "./slices/repositorySlice";
import { createSessionDetailSlice } from "./slices/sessionDetailSlice";
import { createSessionSlice } from "./slices/sessionSlice";
import { createSubagentSlice } from "./slices/subagentSlice";
import { createTabSlice } from "./slices/tabSlice";
import { createTabUISlice } from "./slices/tabUISlice";
import { createUISlice } from "./slices/uiSlice";
import { createUpdateSlice } from "./slices/updateSlice";

import type { DetectedError } from "../types/data";
import type { AppState } from "./types";


// =============================================================================
// Store Creation
// =============================================================================

export const useStore = create<AppState>()((...args) => ({
  ...createProjectSlice(...args),
  ...createRepositorySlice(...args),
  ...createSessionSlice(...args),
  ...createSessionDetailSlice(...args),
  ...createSubagentSlice(...args),
  ...createConversationSlice(...args),
  ...createTabSlice(...args),
  ...createTabUISlice(...args),
  ...createPaneSlice(...args),
  ...createUISlice(...args),
  ...createNotificationSlice(...args),
  ...createConfigSlice(...args),
  ...createConnectionSlice(...args),
  ...createContextSlice(...args),
  ...createUpdateSlice(...args),
}));

// =============================================================================
// Re-exports
// =============================================================================

// =============================================================================
// Store Initialization - Subscribe to IPC Events
// =============================================================================

/**
 * Initialize notification event listeners and fetch initial notification count.
 * Call this once when the app starts (e.g., in App.tsx useEffect).
 */
export function initializeNotificationListeners(): () => void {
  const cleanupFns: (() => void)[] = [];
  const pendingSessionRefreshTimers = new Map<
    string,
    ReturnType<typeof setTimeout>
  >();
  const pendingProjectRefreshTimers = new Map<
    string,
    ReturnType<typeof setTimeout>
  >();
  const SESSION_REFRESH_DEBOUNCE_MS = 150;
  const PROJECT_REFRESH_DEBOUNCE_MS = 300;
  const getBaseProjectId = (
    projectId: string | null | undefined,
  ): string | null => {
    if (!projectId) return null;
    const separatorIndex = projectId.indexOf("::");
    return separatorIndex >= 0 ? projectId.slice(0, separatorIndex) : projectId;
  };

  const scheduleSessionRefresh = (
    projectId: string,
    sessionId: string,
  ): void => {
    const key = `${projectId}/${sessionId}`;
    // Throttle (not trailing debounce): keep at most one pending refresh per session.
    // Debounce can delay updates indefinitely while the file is continuously appended.
    if (pendingSessionRefreshTimers.has(key)) {
      return;
    }
    const timer = setTimeout(() => {
      pendingSessionRefreshTimers.delete(key);
      const state = useStore.getState();
      void state.refreshSessionInPlace(projectId, sessionId);
    }, SESSION_REFRESH_DEBOUNCE_MS);
    pendingSessionRefreshTimers.set(key, timer);
  };

  const scheduleProjectRefresh = (projectId: string): void => {
    const existingTimer = pendingProjectRefreshTimers.get(projectId);
    if (existingTimer) {
      clearTimeout(existingTimer);
    }
    const timer = setTimeout(() => {
      pendingProjectRefreshTimers.delete(projectId);
      const state = useStore.getState();
      void state.refreshSessionsInPlace(projectId);
    }, PROJECT_REFRESH_DEBOUNCE_MS);
    pendingProjectRefreshTimers.set(projectId, timer);
  };

  // Listen for new notifications from main process
  if (api.notifications?.onNew) {
    const cleanup = api.notifications.onNew(
      (_event: unknown, error: unknown) => {
        // Cast the error to DetectedError type
        const notification = error as DetectedError;
        if (notification?.id) {
          // Keep list in sync immediately; unread count is synced via notification:updated/fetch.
          useStore.setState((state) => {
            if (state.notifications.some((n) => n.id === notification.id)) {
              return {};
            }
            return {
              notifications: [notification, ...state.notifications].slice(
                0,
                200,
              ),
            };
          });
        }
      },
    );
    if (typeof cleanup === "function") {
      cleanupFns.push(cleanup);
    }
  }

  // Listen for notification updates from main process
  if (api.notifications?.onUpdated) {
    const cleanup = api.notifications.onUpdated(
      (_event: unknown, payload: { total: number; unreadCount: number }) => {
        const unreadCount =
          typeof payload.unreadCount === "number" &&
          Number.isFinite(payload.unreadCount)
            ? Math.max(0, Math.floor(payload.unreadCount))
            : 0;
        useStore.setState({ unreadCount });
      },
    );
    if (typeof cleanup === "function") {
      cleanupFns.push(cleanup);
    }
  }

  // Navigate to error when user clicks a native OS notification
  if (api.notifications?.onClicked) {
    const cleanup = api.notifications.onClicked(
      (_event: unknown, data: unknown) => {
        const error = data as DetectedError;
        if (error?.id && error?.sessionId && error?.projectId) {
          useStore.getState().navigateToError(error);
        }
      },
    );
    if (typeof cleanup === "function") {
      cleanupFns.push(cleanup);
    }
  }

  // Fetch after listeners are attached so startup events do not get overwritten by a stale response.
  void useStore.getState().fetchNotifications();

  /**
   * Check if a session is visible in any pane (not just the focused pane's active tab).
   * This ensures file change and task-list listeners refresh sessions shown in any split pane.
   */
  const isSessionVisibleInAnyPane = (sessionId: string): boolean => {
    const { paneLayout } = useStore.getState();
    return paneLayout.panes.some(
      (pane) =>
        pane.activeTabId != null &&
        pane.tabs.some(
          (tab) =>
            tab.id === pane.activeTabId &&
            tab.type === "session" &&
            tab.sessionId === sessionId,
        ),
    );
  };

  // Listen for task-list file changes to refresh currently viewed session metadata
  if (api.onTodoChange) {
    const cleanup = api.onTodoChange((event) => {
      if (!event.sessionId) {
        return;
      }

      const state = useStore.getState();
      const isViewingSession =
        state.selectedSessionId === event.sessionId ||
        isSessionVisibleInAnyPane(event.sessionId);

      if (isViewingSession) {
        // Find the project ID from any pane's tab that shows this session
        const allTabs = state.getAllPaneTabs();
        const sessionTab = allTabs.find(
          (t) => t.type === "session" && t.sessionId === event.sessionId,
        );
        if (sessionTab?.projectId) {
          scheduleSessionRefresh(sessionTab.projectId, event.sessionId);
        }
      }

      // Refresh project sessions list if applicable
      const activeTab = state.getActiveTab();
      const activeProjectId =
        activeTab?.type === "session" && typeof activeTab.projectId === "string"
          ? activeTab.projectId
          : null;
      if (activeProjectId && activeProjectId === state.selectedProjectId) {
        scheduleProjectRefresh(activeProjectId);
      }
    });
    if (typeof cleanup === "function") {
      cleanupFns.push(cleanup);
    }
  }

  // Listen for file changes to auto-refresh current session and detect new sessions
  if (api.onFileChange) {
    const cleanup = api.onFileChange((event) => {
      // Skip unlink events
      if (event.type === "unlink") {
        return;
      }

      const state = useStore.getState();
      const selectedProjectId = state.selectedProjectId;
      const selectedProjectBaseId = getBaseProjectId(selectedProjectId);
      const eventProjectBaseId = getBaseProjectId(event.projectId);
      const matchesSelectedProject =
        !!selectedProjectId &&
        (eventProjectBaseId == null ||
          selectedProjectBaseId === eventProjectBaseId);
      const isTopLevelSessionEvent = !event.isSubagent;
      const isUnknownSessionInSidebar =
        event.sessionId == null ||
        !state.sessions.some((session) => session.id === event.sessionId);
      const shouldRefreshForPotentialNewSession =
        isTopLevelSessionEvent &&
        matchesSelectedProject &&
        isUnknownSessionInSidebar &&
        (event.type === "add" ||
          (state.connectionMode === "local" && event.type === "change"));

      // Refresh sidebar session list only when a truly new top-level session appears.
      // Local fs.watch can report "change" before/without "add" for newly created files.
      if (shouldRefreshForPotentialNewSession) {
        if (matchesSelectedProject && selectedProjectId) {
          scheduleProjectRefresh(selectedProjectId);
        }
      }

      // Keep opened session view in sync on content changes.
      // Some local writers emit rename/add for in-place updates, so include "add".
      if (
        (event.type === "change" || event.type === "add") &&
        selectedProjectId
      ) {
        const activeSessionId = state.selectedSessionId;
        const eventSessionId = event.sessionId;
        const isViewingEventSession =
          !!eventSessionId &&
          (activeSessionId === eventSessionId ||
            isSessionVisibleInAnyPane(eventSessionId));
        const shouldFallbackRefreshActiveSession =
          matchesSelectedProject && !eventSessionId && !!activeSessionId;
        const sessionIdToRefresh =
          (isViewingEventSession ? eventSessionId : null) ??
          (shouldFallbackRefreshActiveSession ? activeSessionId : null);

        if (sessionIdToRefresh) {
          const allTabs = state.getAllPaneTabs();
          const visibleSessionTab = allTabs.find(
            (tab) =>
              tab.type === "session" && tab.sessionId === sessionIdToRefresh,
          );
          const refreshProjectId =
            visibleSessionTab?.projectId ?? selectedProjectId;

          // Use refreshSessionInPlace to avoid flickering and preserve UI state
          scheduleSessionRefresh(refreshProjectId, sessionIdToRefresh);
        }
      }
    });
    if (typeof cleanup === "function") {
      cleanupFns.push(cleanup);
    }
  }

  // Listen for Ctrl+R / Cmd+R session refresh from main process (fixes #85)
  if (api.onSessionRefresh) {
    const cleanup = api.onSessionRefresh(() => {
      const state = useStore.getState();
      const activeTabId = state.activeTabId;
      const activeTab = activeTabId
        ? state.openTabs.find((t) => t.id === activeTabId)
        : null;
      if (
        activeTab?.type === "session" &&
        activeTab.projectId &&
        activeTab.sessionId
      ) {
        void Promise.all([
          state.refreshSessionInPlace(activeTab.projectId, activeTab.sessionId),
          state.fetchSessions(activeTab.projectId),
        ]).then(() => {
          window.dispatchEvent(
            new CustomEvent("session-refresh-scroll-bottom"),
          );
        });
      }
    });
    if (typeof cleanup === "function") {
      cleanupFns.push(cleanup);
    }
  }

  // Listen for SSH connection status changes from main process
  // NOTE: Only syncs connection status here. Data fetching is handled by
  // connectionSlice.connectSsh/disconnectSsh and contextSlice.switchContext.
  if (api.ssh?.onStatus) {
    const cleanup = api.ssh.onStatus((_event: unknown, status: unknown) => {
      const s = status as {
        state: string;
        host: string | null;
        error: string | null;
      };
      useStore
        .getState()
        .setConnectionStatus(
          s.state as "disconnected" | "connecting" | "connected" | "error",
          s.host,
          s.error,
        );
    });
    if (typeof cleanup === "function") {
      cleanupFns.push(cleanup);
    }
  }

  // Listen for context changes from main process (e.g., SSH disconnect)
  if (api.context?.onChanged) {
    const cleanup = api.context.onChanged((_event: unknown, data: unknown) => {
      const { id } = data as { id: string; type: string };
      const currentContextId = useStore.getState().activeContextId;
      if (id !== currentContextId) {
        // Main process switched context externally (e.g., SSH disconnect)
        // Trigger renderer-side context switch to sync state
        void useStore.getState().switchContext(id);
      }
    });
    if (typeof cleanup === "function") {
      cleanupFns.push(cleanup);
    }
  }

  // Handle native notification clicks via window focus (desktop only).
  // notify_rust doesn't support cross-platform click callbacks, so we store
  // the last shown error in Rust and detect clicks via a JS focus listener
  // that calls handle_notification_click to emit notification:clicked.
  if (isDesktopMode()) {
    import("@tauri-apps/api/core")
      .then(({ invoke }) => {
        const onFocus = () => {
          void invoke<boolean>("handle_notification_click");
        };
        window.addEventListener("focus", onFocus);
        cleanupFns.push(() => window.removeEventListener("focus", onFocus));
      })
      .catch((err) => {
        console.error("Failed to set up notification click handler:", err);
      });
  }

  // Listen for tray:open-session events (desktop only)
  if (isDesktopMode()) {
    import("@tauri-apps/api/event")
      .then(({ listen }) =>
        listen<{ projectId: string; sessionId: string }>(
          "tray:open-session",
          (event) => {
            const { projectId, sessionId } = event.payload;
            const store = useStore.getState();
            const project = store.projects.find((p) => p.id === projectId);
            const label = project?.name ?? sessionId;
            store.openTab({ type: "session", label, projectId, sessionId });
          },
        ),
      )
      .then((unlisten) => {
        cleanupFns.push(unlisten);
      })
      .catch((err) => {
        console.error("Failed to listen to tray:open-session event:", err);
      });
  }

  // Zoom keyboard listener (Cmd/Ctrl + =/+/-/0)
  // Handle zoom ourselves since we disabled the browser's default zoom hotkeys.
  // This keeps the zoom factor in sync with the backend and repositions traffic lights.
  if (isDesktopMode()) {
    const ZOOM_STEP = 0.1;
    const ZOOM_MIN = 0.5;
    const ZOOM_MAX = 3.0;

    const onZoomKeyDown = (e: KeyboardEvent) => {
      if (
        !(
          (e.metaKey || e.ctrlKey) &&
          (e.key === "=" || e.key === "+" || e.key === "-" || e.key === "0")
        )
      ) {
        return;
      }
      e.preventDefault();

      api
        .getZoomFactor()
        .then((current) => {
          let newFactor: number;
          if (e.key === "0") {
            newFactor = 1.0;
          } else if (e.key === "-" || e.key === "_") {
            newFactor = Math.max(ZOOM_MIN, +(current - ZOOM_STEP).toFixed(1));
          } else {
            newFactor = Math.min(ZOOM_MAX, +(current + ZOOM_STEP).toFixed(1));
          }
          return api.setZoomFactor(newFactor).then(() => newFactor);
        })
        .then((newFactor) => {
          if (newFactor == null) return;
          // Update traffic light padding for macOS overlay title bar
          const padding = Math.ceil(12 + (52 + 16) / newFactor);
          document.documentElement.style.setProperty(
            "--macos-traffic-light-padding-left",
            `${padding}px`,
          );
        })
        .catch(() => {
          // Silently ignore zoom errors
        });
    };

    window.addEventListener("keydown", onZoomKeyDown);
    cleanupFns.push(() => window.removeEventListener("keydown", onZoomKeyDown));
  }

  // Return cleanup function
  return () => {
    for (const timer of pendingSessionRefreshTimers.values()) {
      clearTimeout(timer);
    }
    pendingSessionRefreshTimers.clear();
    for (const timer of pendingProjectRefreshTimers.values()) {
      clearTimeout(timer);
    }
    pendingProjectRefreshTimers.clear();
    cleanupFns.forEach((fn) => fn());
  };
}
