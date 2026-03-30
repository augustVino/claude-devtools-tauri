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
  AgentConfig,
  MentionedFileInfo,
  HttpServerStatus,
  ContextInfo,
  SshConnectionConfig,
  SshConnectionStatus,
  SshConfigHostEntry,
  SshLastConnection,
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
  ConversationGroup,
} from "@main/types";
import type { WaterfallData } from "@shared/types/visualization";
import {
  adaptWaterfallData,
  type WaterfallDataRust,
  adaptConversationGroup,
} from "@renderer/types/data";
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

/**
 * Context API — delegates to Tauri invoke commands.
 */
function createContextAPI() {
  return {
    getActive: (): Promise<string> => invoke<string>("context_active"),
    list: (): Promise<ContextInfo[]> => invoke<ContextInfo[]>("context_list"),
    switch: (contextId: string): Promise<{ contextId: string }> =>
      invoke<{ contextId: string }>("context_switch", { contextId }),
    onChanged: (callback: (event: unknown, data: ContextInfo) => void) => {
      const unlisten = listen<ContextInfo>("context:changed", (event) => {
        callback(event, event.payload);
      });
      return () => {
        unlisten.then((fn) => fn());
      };
    },
  };
}

/**
 * HTTP Server API — delegates to Tauri invoke commands.
 */
function createHttpServerAPI() {
  return {
    getStatus: (): Promise<HttpServerStatus> =>
      invoke<HttpServerStatus>("get_status"),
    start: (): Promise<HttpServerStatus> => invoke<HttpServerStatus>("start"),
    stop: (): Promise<HttpServerStatus> => invoke<HttpServerStatus>("stop"),
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
    relaunch: (): Promise<void> => invoke("relaunch"),
  };

  readonly autoStart = {
    enable: (): Promise<void> => invoke("plugin:autostart|enable"),
    disable: (): Promise<void> => invoke("plugin:autostart|disable"),
    isEnabled: (): Promise<boolean> =>
      invoke<boolean>("plugin:autostart|is_enabled"),
  };

  readonly platform = {
    setDockVisible: (visible: boolean): Promise<void> =>
      invoke("set_dock_visible", { visible }),
  };

  readonly getZoomFactor = (): Promise<number> =>
    invoke<ZoomFactorResult>("get_zoom_factor").then((r) => r.factor);
  readonly setZoomFactor = (factor: number): Promise<void> =>
    invoke("set_zoom_factor", { factor });
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
  // Waterfall and Conversation Group commands
  // ===========================================================================

  readonly getWaterfallData = (
    projectId: string,
    sessionId: string,
  ): Promise<WaterfallData | null> =>
    invoke<WaterfallDataRust>("get_waterfall_data", { projectId, sessionId })
      .then(adaptWaterfallData)
      .catch((e) => {
        // Return null if session not found (matches Electron behavior)
        if (String(e).includes("not found")) return null;
        throw e;
      });

  readonly getSessionGroups = (
    projectId: string,
    sessionId: string,
  ): Promise<ConversationGroup[]> =>
    invoke<Record<string, unknown>[]>("get_session_groups", {
      projectId,
      sessionId,
    }).then((groups) => groups.map(adaptConversationGroup));

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
    maxTokens?: number,
  ): Promise<MentionedFileInfo | null> =>
    invoke("read_mentioned_file", { filePath, projectRoot, maxTokens });

  readonly readAgentConfigs = (
    projectRoot: string,
  ): Promise<Record<string, AgentConfig>> =>
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
  readonly session = {
    scrollToLine: (sessionId: string, lineNumber: number): Promise<void> =>
      invoke("scroll_to_line", { sessionId, lineNumber }),
  };
  readonly ssh = {
    connect: (config: SshConnectionConfig): Promise<SshConnectionStatus> =>
      invoke<SshConnectionStatus>("ssh_connect", { config }),
    disconnect: (): Promise<SshConnectionStatus> =>
      invoke<SshConnectionStatus>("ssh_disconnect"),
    getState: (): Promise<SshConnectionStatus> =>
      invoke<SshConnectionStatus>("ssh_get_state"),
    test: (
      config: SshConnectionConfig,
    ): Promise<{ success: boolean; error?: string }> =>
      invoke<{ success: boolean; error?: string }>("ssh_test", { config }),
    getConfigHosts: (): Promise<SshConfigHostEntry[]> =>
      invoke<SshConfigHostEntry[]>("ssh_get_config_hosts"),
    resolveHost: (alias: string): Promise<SshConfigHostEntry | null> =>
      invoke<SshConfigHostEntry | null>("ssh_resolve_host", { alias }),
    saveLastConnection: (config: SshLastConnection): Promise<void> =>
      invoke<void>("ssh_save_last_connection", { connection: config }),
    getLastConnection: (): Promise<SshLastConnection | null> =>
      invoke<SshLastConnection | null>("ssh_get_last_connection"),
    // IPC signature: (event: unknown, status: SshConnectionStatus) => void
    onStatus: (
      cb: (_event: unknown, status: SshConnectionStatus) => void,
    ): (() => void) => {
      let unlisten: UnlistenFn | null = null;
      listen<{ status: SshConnectionStatus }>("ssh:status", (e) =>
        cb(e, e.payload.status),
      )
        .then((fn) => {
          unlisten = fn;
        })
        .catch((err) => {
          console.error("Failed to listen to ssh:status event:", err);
        });
      return () => {
        if (unlisten) unlisten();
      };
    },
  };
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
        {
          path: selectedPath,
        },
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
