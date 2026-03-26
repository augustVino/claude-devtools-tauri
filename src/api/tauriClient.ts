/**
 * Tauri API client — implements ElectronAPI via @tauri-apps/api invoke/listen.
 *
 * Window controls, version, sessions, config, search, validation, notifications,
 * updater, and trigger commands are implemented. Session and SSH features are stubbed.
 */

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import type { UnlistenFn } from "@tauri-apps/api/event";

import type {
  ElectronAPI,
  ConfigAPI,
  ClaudeMdFileInfo,
  AgentConfigEntry,
} from "@shared/types/api";
import type {
  Session,
  SessionDetail,
  SessionMetrics,
  Project,
  SearchSessionsResult,
  PaginatedSessionsResult,
  FileChangeEvent,
  TodoChangeEvent,
  RepositoryGroup,
  RawSubagentDetail,
} from "@main/types";
import type {
  AppConfig,
  NotificationTrigger,
  TriggerTestResult,
  StoredNotification,
  DetectedError,
  GetNotificationsOptions,
  GetNotificationsResult,
  NotificationCountResult,
  NotificationStats,
} from "@shared/types/notifications";

const NOT_IMPLEMENTED = new Error("Not yet implemented in Tauri backend");

function notImplemented(): Promise<never> {
  return Promise.reject(NOT_IMPLEMENTED);
}

/**
 * Creates a stub object that returns safe no-op values for event listeners.
 * Event listener methods (onXxx) return a no-op cleanup function.
 * Other methods return a rejected Promise.
 *
 * This allows frontend code to safely check `if (!api.ssh?.onStatus)` and
 * use event listeners without breaking useEffect cleanup patterns.
 */
function stubEventAPI(): Record<string, unknown> {
  return new Proxy({} as Record<string, unknown>, {
    get(_target, prop) {
      // Event listener methods return a no-op cleanup function
      if (typeof prop === "string" && prop.startsWith("on")) {
        return () => () => {};
      }
      // Other methods return rejected Promise
      return notImplemented;
    },
  });
}

/**
 * Context API stub for V1 - returns local-only context.
 * SSH context switching is V2 scope, so we return safe defaults.
 */
// eslint-disable-next-line @typescript-eslint/no-explicit-any
function createContextAPI(): any {
  return {
    getActive: () => Promise.resolve("local"),
    list: () => Promise.resolve([{ id: "local", type: "local" }]),
    switch: notImplemented,
    onChanged: () => () => {},
  };
}

/**
 * HTTP Server API stub for V1 - HTTP server is V2 scope.
 * Returns safe defaults to prevent startup errors.
 */
// eslint-disable-next-line @typescript-eslint/no-explicit-any
function createHttpServerAPI(): any {
  return {
    getStatus: () => Promise.resolve({ running: false, port: 3456 }),
    start: notImplemented,
    stop: notImplemented,
  };
}

// =============================================================================
// Validation result types (match Rust structs)
// =============================================================================

interface PathValidationResult {
  valid: boolean;
  error?: string;
  resolved_path?: string;
}

interface MentionsValidationResult {
  valid: boolean;
  error?: string;
}

interface ZoomFactorResult {
  factor: number;
}

/**
 * Updater status events emitted on the `updater:status` channel.
 * Matches the Rust UpdaterStatus enum (serde tag = "status", rename_all = "camelCase").
 */
type UpdaterStatus =
  | { status: "checking" }
  | { status: "available"; version: string }
  | { status: "downloading"; progress: number; contentLength: number | null }
  | { status: "downloaded" }
  | { status: "upToDate" }
  | { status: "error"; message: string };

export class TauriAPIClient implements ElectronAPI {
  // ===========================================================================
  // Implemented commands
  // ===========================================================================

  readonly getAppVersion = (): Promise<string> => invoke("get_app_version");

  readonly windowControls = {
    minimize: (): Promise<void> => invoke("minimize"),
    maximize: (): Promise<void> => invoke("maximize"),
    close: (): Promise<void> => invoke("close"),
    isMaximized: (): Promise<boolean> => invoke<boolean>("is_maximized"),
    relaunch: async (): Promise<void> => {
      try {
        await invoke("process_relaunch");
      } catch {
        window.location.reload();
      }
    },
  };

  readonly getZoomFactor = async (): Promise<number> => {
    const result = await invoke<ZoomFactorResult>("get_zoom_factor");
    return result.factor;
  };
  readonly onZoomFactorChanged = (): (() => void) => () => {};

  // ===========================================================================
  // Project and Session commands
  // ===========================================================================

  readonly getProjects = (): Promise<Project[]> => invoke("get_projects");

  readonly getSessions = (projectId: string): Promise<Session[]> =>
    invoke("get_sessions", { projectId });

  readonly getSessionsPaginated = (
    projectId: string,
    cursor: string | null,
    limit?: number,
    options?: unknown,
  ): Promise<PaginatedSessionsResult> =>
    invoke("get_sessions_paginated", { projectId, cursor, limit, options });

  readonly getSessionDetail = (
    projectId: string,
    sessionId: string,
  ): Promise<SessionDetail | null> =>
    invoke("get_session_detail", { projectId, sessionId });

  readonly getSessionMetrics = (
    projectId: string,
    sessionId: string,
  ): Promise<SessionMetrics | null> =>
    invoke("get_session_metrics", { projectId, sessionId });

  // ===========================================================================
  // Search commands
  // ===========================================================================

  readonly searchSessions = (
    projectId: string,
    query: string,
    maxResults?: number,
  ): Promise<SearchSessionsResult> =>
    invoke("search_sessions", { projectId, query, maxResults });

  readonly searchAllProjects = (
    query: string,
    maxResults?: number,
  ): Promise<SearchSessionsResult> =>
    invoke("search_all_projects", { query, maxResults });

  // ===========================================================================
  // Validation commands
  // ===========================================================================

  readonly validatePath = async (
    relativePath: string,
    _projectPath: string,
  ): Promise<{ exists: boolean; isDirectory?: boolean }> => {
    const result = await invoke<PathValidationResult>("validate_path", {
      path: relativePath,
    });
    return {
      exists: result.valid,
      isDirectory: undefined,
    };
  };

  readonly validateMentions = async (
    mentions: { type: "path"; value: string }[],
    _projectPath: string,
  ): Promise<Record<string, boolean>> => {
    const paths = mentions.map((m) => m.value);
    const result = await invoke<MentionsValidationResult>("validate_mentions", {
      mentions: paths,
    });
    // Return a map where all paths have the same validity
    const map: Record<string, boolean> = {};
    for (const m of mentions) {
      map[m.value] = result.valid;
    }
    return map;
  };

  // ===========================================================================
  // Utility commands
  // ===========================================================================

  readonly openPath = async (
    targetPath: string,
    _projectRoot?: string,
  ): Promise<{ success: boolean; error?: string }> => {
    try {
      await invoke("open_path", { path: targetPath });
      return { success: true };
    } catch (e) {
      return { success: false, error: String(e) };
    }
  };

  readonly openExternal = async (
    url: string,
  ): Promise<{ success: boolean; error?: string }> => {
    try {
      await invoke("open_external", { url });
      return { success: true };
    } catch (e) {
      return { success: false, error: String(e) };
    }
  };

  // ===========================================================================
  // Stubbed — will be implemented in later phases
  // ===========================================================================

  readonly getWaterfallData = notImplemented; // Complex visualization, defer
  readonly getSessionGroups = notImplemented; // Requires ConversationGroupBuilder

  // ===========================================================================
  // Repository and Worktree commands
  // ===========================================================================

  readonly getRepositoryGroups = (): Promise<RepositoryGroup[]> =>
    invoke("get_repository_groups");

  readonly getWorktreeSessions = (worktreeId: string): Promise<Session[]> =>
    invoke("get_worktree_sessions", { worktreeId });

  // ===========================================================================
  // Session commands (additional)
  // ===========================================================================

  readonly getSubagentDetail = (
    projectId: string,
    sessionId: string,
    subagentId: string,
  ): Promise<RawSubagentDetail | null> =>
    invoke("get_subagent_detail", { projectId, sessionId, subagentId });

  readonly getSessionsByIds = (
    projectId: string,
    sessionIds: string[],
    _options?: unknown,
  ): Promise<Session[]> =>
    invoke("get_sessions_by_ids", { projectId, sessionIds });

  // ===========================================================================
  // CLAUDE.md and Agent Config commands
  // ===========================================================================

  readonly readClaudeMdFiles = (
    projectRoot: string,
  ): Promise<Record<string, ClaudeMdFileInfo>> =>
    invoke("read_claude_md_files", { projectRoot });

  readonly readDirectoryClaudeMd = (
    directory: string,
  ): Promise<ClaudeMdFileInfo> =>
    invoke("read_directory_claude_md", { directory });

  readonly readMentionedFile = async (
    filePath: string,
    projectRoot: string,
    _maxTokens?: number,
  ): Promise<string | null> =>
    invoke("read_mentioned_file", { filePath, projectRoot });

  readonly readAgentConfigs = (
    projectRoot: string,
  ): Promise<AgentConfigEntry[]> =>
    invoke("read_agent_configs", { projectRoot });

  // Nested API objects — partially implemented
  readonly notifications = {
    get: (options?: GetNotificationsOptions): Promise<GetNotificationsResult> =>
      invoke("get_notifications", { options: options ?? null }),
    markRead: (id: string): Promise<boolean> =>
      invoke<boolean>("mark_notification_read", { notificationId: id }),
    markAllRead: (): Promise<boolean> =>
      invoke<boolean>("mark_all_notifications_read"),
    delete: (id: string): Promise<boolean> =>
      invoke<boolean>("delete_notification", { notificationId: id }),
    clear: (): Promise<boolean> => invoke<boolean>("clear_notifications"),
    getUnreadCount: (): Promise<number> =>
      invoke<NotificationCountResult>("get_notification_count").then(
        (r) => r.unreadCount,
      ),
    getStats: (): Promise<NotificationStats> =>
      invoke<NotificationStats>("get_notification_stats"),
    onNew: (cb: (_event: unknown, error: unknown) => void): (() => void) => {
      let unlisten: UnlistenFn | null = null;
      listen<StoredNotification>("notification:new", (e) => cb(e, e.payload))
        .then((fn) => {
          unlisten = fn;
        })
        .catch((err) => {
          console.error("Failed to listen to notification:new event:", err);
        });
      return () => {
        if (unlisten) unlisten();
      };
    },
    onUpdated: (
      cb: (
        _event: unknown,
        payload: { total: number; unreadCount: number },
      ) => void,
    ): (() => void) => {
      let unlisten: UnlistenFn | null = null;
      listen<{ total: number; unreadCount: number }>(
        "notification:updated",
        (e) => cb(e, e.payload),
      )
        .then((fn) => {
          unlisten = fn;
        })
        .catch((err) => {
          console.error("Failed to listen to notification:updated event:", err);
        });
      return () => {
        if (unlisten) unlisten();
      };
    },
    onClicked: (cb: (_event: unknown, data: unknown) => void): (() => void) => {
      let unlisten: UnlistenFn | null = null;
      listen<DetectedError>("notification:clicked", (e) => cb(e, e.payload))
        .then((fn) => {
          unlisten = fn;
        })
        .catch((err) => {
          console.error("Failed to listen to notification:clicked event:", err);
        });
      return () => {
        if (unlisten) unlisten();
      };
    },
  };
  readonly session = stubEventAPI() as any;
  readonly updater = {
    check: () => invoke("check_for_updates"),
    download: () => invoke("download_and_install_update"),
    install: () => invoke("install_update"),
    onStatus: (cb: (status: UpdaterStatus) => void): (() => void) => {
      let unlisten: UnlistenFn | null = null;
      listen<UpdaterStatus>("updater:status", (e) => cb(e.payload))
        .then((fn) => {
          unlisten = fn;
        })
        .catch((err) => {
          console.error("Failed to listen to updater:status event:", err);
        });
      return () => {
        if (unlisten) unlisten();
      };
    },
  };
  readonly ssh = stubEventAPI() as any;
  readonly context = createContextAPI();
  readonly httpServer = createHttpServerAPI();

  // Config API — implemented
  readonly config: ConfigAPI = {
    get: (): Promise<AppConfig> => invoke<AppConfig>("get_config"),
    update: (section: string, data: object): Promise<AppConfig> =>
      invoke<AppConfig>("update_config", { section, data }),
    addIgnoreRegex: (pattern: string): Promise<AppConfig> =>
      invoke<AppConfig>("add_ignore_regex", { pattern }),
    removeIgnoreRegex: (pattern: string): Promise<AppConfig> =>
      invoke<AppConfig>("remove_ignore_regex", { pattern }),
    addIgnoreRepository: (repositoryId: string): Promise<AppConfig> =>
      invoke<AppConfig>("add_ignore_repository", { repositoryId }),
    removeIgnoreRepository: (repositoryId: string): Promise<AppConfig> =>
      invoke<AppConfig>("remove_ignore_repository", { repositoryId }),
    snooze: (minutes: number): Promise<AppConfig> =>
      invoke<AppConfig>("snooze", { minutes }),
    clearSnooze: (): Promise<AppConfig> => invoke<AppConfig>("clear_snooze"),
    addTrigger: (
      trigger: Omit<NotificationTrigger, "isBuiltin">,
    ): Promise<AppConfig> => invoke<AppConfig>("add_trigger", { trigger }),
    updateTrigger: (
      triggerId: string,
      updates: Partial<NotificationTrigger>,
    ): Promise<AppConfig> =>
      invoke<AppConfig>("update_trigger", { triggerId, updates }),
    removeTrigger: (triggerId: string): Promise<AppConfig> =>
      invoke<AppConfig>("remove_trigger", { triggerId }),
    getTriggers: (): Promise<NotificationTrigger[]> =>
      invoke<NotificationTrigger[]>("get_triggers"),
    testTrigger: (trigger: NotificationTrigger): Promise<TriggerTestResult> =>
      invoke<TriggerTestResult>("test_trigger", { trigger }),
    selectFolders: async (): Promise<string[]> => {
      const result = await open({
        multiple: true,
        directory: true,
        title: "Select Project Folders",
      });
      if (result === null) return [];
      return result as string[];
    },
    selectClaudeRootFolder: async (): Promise<{
      path: string;
      isClaudeDirName: boolean;
      hasProjectsDir: boolean;
    } | null> => {
      const { homeDir } = await import("@tauri-apps/api/path");
      const home = await homeDir();
      const result = await open({
        directory: true,
        title: "Select Claude Root Folder",
        defaultPath: home,
      });
      if (result === null) return null;
      const selectedPath = Array.isArray(result) ? result[0] : result;
      const hasProjectsDir = await invoke<boolean>(
        "check_projects_dir_exists",
        { path: selectedPath },
      );
      return {
        path: selectedPath,
        isClaudeDirName: selectedPath.endsWith(".claude"),
        hasProjectsDir,
      };
    },
    getClaudeRootInfo: () =>
      invoke<{
        defaultPath: string;
        resolvedPath: string;
        customPath: string | null;
      }>("get_claude_root_info"),
    findWslClaudeRoots: () => Promise.resolve([]),
    openInEditor: (): Promise<void> => invoke("open_in_editor"),
    pinSession: (projectId: string, sessionId: string): Promise<void> =>
      invoke("pin_session", { projectId, sessionId }),
    unpinSession: (projectId: string, sessionId: string): Promise<void> =>
      invoke("unpin_session", { projectId, sessionId }),
    hideSession: (projectId: string, sessionId: string): Promise<void> =>
      invoke("hide_session", { projectId, sessionId }),
    unhideSession: (projectId: string, sessionId: string): Promise<void> =>
      invoke("unhide_session", { projectId, sessionId }),
    hideSessions: (projectId: string, sessionIds: string[]): Promise<void> =>
      invoke("hide_sessions", { projectId, sessionIds }),
    unhideSessions: (projectId: string, sessionIds: string[]): Promise<void> =>
      invoke("unhide_sessions", { projectId, sessionIds }),
  };

  // Event listeners — wired to Tauri events
  readonly onFileChange = (
    callback: (event: FileChangeEvent) => void,
  ): (() => void) => {
    let unlisten: UnlistenFn | null = null;
    listen<FileChangeEvent>("file-change", (e) => callback(e.payload))
      .then((fn) => {
        unlisten = fn;
      })
      .catch((err) => {
        console.error("Failed to listen to file-change event:", err);
      });
    return () => {
      if (unlisten) unlisten();
    };
  };

  readonly onTodoChange = (
    callback: (event: TodoChangeEvent) => void,
  ): (() => void) => {
    let unlisten: UnlistenFn | null = null;
    listen<TodoChangeEvent>("todo-change", (e) => callback(e.payload))
      .then((fn) => {
        unlisten = fn;
      })
      .catch((err) => {
        console.error("Failed to listen to todo-change event:", err);
      });
    return () => {
      if (unlisten) unlisten();
    };
  };

  readonly onSessionRefresh = (): (() => void) => () => {};
}
