/**
 * IPC API type definitions for Electron preload bridge.
 *
 * These types define the interface exposed to the renderer process
 * via contextBridge. The actual implementation lives in src/preload/index.ts.
 *
 * Shared between preload and renderer processes.
 */

import type {
  AppConfig,
  DetectedError,
  NotificationStats,
  NotificationTrigger,
  TriggerTestResult,
} from "./notifications";
import type { WaterfallData } from "./visualization";
import type {
  ConversationGroup,
  FileChangeEvent,
  TodoChangeEvent,
  MemoryChangeEvent,
  PaginatedSessionsResult,
  Project,
  RepositoryGroup,
  SearchSessionsResult,
  Session,
  SessionDetail,
  SessionMetrics,
  SessionsByIdsOptions,
  SessionsPaginationOptions,
  RawSubagentDetail,
  FindSessionByIdResult,
  FindSessionsByPartialIdResult,
} from "@main/types";

// =============================================================================
// Session Detail Response Types
// =============================================================================

/** Sentinel returned when the renderer's known fingerprint matches the file on disk. */
export interface SessionDetailUnchanged {
  unchanged: true;
  fingerprint: string;
}

// =============================================================================
// Agent Config
// =============================================================================

export interface AgentConfig {
  name: string;
  color?: string;
}

/**
 * Information about a mentioned file returned from IPC.
 */
export interface MentionedFileInfo {
  /** Absolute file path */
  path: string;
  /** Whether the file exists on disk */
  exists: boolean;
  /** Character count of file content */
  charCount: number;
  /** Estimated token count (typically charCount / 4) */
  estimatedTokens: number;
}

// =============================================================================
// Notifications API
// =============================================================================

/**
 * Result of notifications:get with pagination.
 */
interface NotificationsResult {
  notifications: DetectedError[];
  total: number;
  totalCount: number;
  unreadCount: number;
  hasMore: boolean;
}

/**
 * Notifications API exposed via preload.
 * Note: Event callbacks use `unknown` types because IPC data cannot be typed at the preload layer.
 * Consumers should cast to DetectedError or NotificationClickData as appropriate.
 */
export interface NotificationsAPI {
  get: (options?: {
    limit?: number;
    offset?: number;
  }) => Promise<NotificationsResult>;
  markRead: (id: string) => Promise<boolean>;
  markAllRead: () => Promise<boolean>;
  delete: (id: string) => Promise<boolean>;
  clear: () => Promise<boolean>;
  getUnreadCount: () => Promise<number>;
  getStats: () => Promise<NotificationStats>;
  onNew: (callback: (event: unknown, error: unknown) => void) => () => void;
  onUpdated: (
    callback: (
      event: unknown,
      payload: { total: number; unreadCount: number },
    ) => void,
  ) => () => void;
  onClicked: (callback: (event: unknown, data: unknown) => void) => () => void;
}

// =============================================================================
// Config API
// =============================================================================

/**
 * Config API exposed via preload.
 */
export interface ConfigAPI {
  get: () => Promise<AppConfig>;
  update: (section: string, data: object) => Promise<AppConfig>;
  addIgnoreRegex: (pattern: string) => Promise<AppConfig>;
  removeIgnoreRegex: (pattern: string) => Promise<AppConfig>;
  addIgnoreRepository: (repositoryId: string) => Promise<AppConfig>;
  removeIgnoreRepository: (repositoryId: string) => Promise<AppConfig>;
  snooze: (minutes: number) => Promise<AppConfig>;
  clearSnooze: () => Promise<AppConfig>;
  // Trigger management methods
  addTrigger: (
    trigger: Omit<NotificationTrigger, "isBuiltin">,
  ) => Promise<AppConfig>;
  updateTrigger: (
    triggerId: string,
    updates: Partial<NotificationTrigger>,
  ) => Promise<AppConfig>;
  removeTrigger: (triggerId: string) => Promise<AppConfig>;
  getTriggers: () => Promise<NotificationTrigger[]>;
  testTrigger: (trigger: NotificationTrigger) => Promise<TriggerTestResult>;
  /** Opens native folder selection dialog and returns selected paths */
  selectFolders: () => Promise<string[]>;
  /** Open native dialog to select local Claude root folder */
  selectClaudeRootFolder: () => Promise<ClaudeRootFolderSelection | null>;
  /** Get resolved Claude root path info for local mode */
  getClaudeRootInfo: () => Promise<ClaudeRootInfo>;
  /** Find Windows WSL Claude root candidates (UNC paths) */
  findWslClaudeRoots: () => Promise<WslClaudeRootCandidate[]>;
  /** Opens the config JSON file in an external editor */
  openInEditor: () => Promise<void>;
  /** Pin a session for a project */
  pinSession: (projectId: string, sessionId: string) => Promise<void>;
  /** Unpin a session for a project */
  unpinSession: (projectId: string, sessionId: string) => Promise<void>;
  /** Hide a session for a project */
  hideSession: (projectId: string, sessionId: string) => Promise<void>;
  /** Unhide a session for a project */
  unhideSession: (projectId: string, sessionId: string) => Promise<void>;
  /** Bulk hide sessions for a project */
  hideSessions: (projectId: string, sessionIds: string[]) => Promise<void>;
  /** Bulk unhide sessions for a project */
  unhideSessions: (projectId: string, sessionIds: string[]) => Promise<void>;
}

export interface ClaudeRootInfo {
  /** Auto-detected default Claude root path for this machine */
  defaultPath: string;
  /** Effective path currently used by local context */
  resolvedPath: string;
  /** Custom override path from settings (null means auto-detect) */
  customPath: string | null;
}

export interface ClaudeRootFolderSelection {
  /** Selected directory absolute path */
  path: string;
  /** Whether the selected folder name is exactly ".claude" */
  isClaudeDirName: boolean;
  /** Whether selected folder contains a "projects" directory */
  hasProjectsDir: boolean;
}

export interface WslClaudeRootCandidate {
  /** WSL distribution name (e.g. Ubuntu) */
  distro: string;
  /** Candidate Claude root path in UNC format */
  path: string;
  /** True if this root contains "projects" directory */
  hasProjectsDir: boolean;
}

// =============================================================================
// Session API
// =============================================================================

/**
 * Session navigation API exposed via preload.
 */
export interface SessionAPI {
  scrollToLine: (sessionId: string, lineNumber: number) => Promise<void>;
}

// =============================================================================
// CLAUDE.md File Info
// =============================================================================

/**
 * CLAUDE.md file information returned from reading operations.
 */
export interface ClaudeMdFileInfo {
  path: string;
  exists: boolean;
  charCount: number;
  estimatedTokens: number;
}

// =============================================================================
// Updater API
// =============================================================================

// NOTE: Updater now uses @tauri-apps/plugin-updater JS API directly.
// UpdaterStatus and UpdaterAPI types removed — no longer needed.

// =============================================================================
// Context API
// =============================================================================

/**
 * Context information for listing available contexts.
 */
export interface ContextInfo {
  id: string;
  type: "local" | "ssh";
}

// =============================================================================
// SSH API
// =============================================================================

/**
 * SSH connection state.
 */
export type SshConnectionState =
  | "disconnected"
  | "connecting"
  | "connected"
  | "error";

/**
 * SSH authentication method.
 *
 * Tauri 后端 SshAuthMethod enum 已支持多 casing alias（sshConfig / ssh_config /
 * SshConfig / private_key / identity_file 等老值全部归一到 "auto"，对齐
 * Electron `sshConfig` 语义）。前端通过 `normalizeSshAuthMethod` 在读取老 profile
 * 时统一归一，避免类型不匹配。
 */
export type SshAuthMethod = "password" | "privateKey" | "agent" | "auto";

/**
 * 归一化老 profile 的 authMethod 值。
 *
 * - "password" / "privateKey" / "agent" 原样返回（合法值）
 * - 其他值（包括 "sshConfig" / "ssh_config" / "SshConfig" / "identity_file" /
 *   "IdentityFile" / undefined / unknown）统一归一到 "auto"
 *
 * 用于：
 * - 读取老 profile / lastSshConfig 时（ConnectionSection）
 * - 写入 lastSshConfig 前（connectionSlice saveLastConnection）
 * - WorkspaceSection handleAdd/handleEdit 写入前
 *
 * 与后端 Rust 端 `SshAuthMethod` enum 的 serde alias 列表保持同步。
 */
export function normalizeSshAuthMethod(raw: unknown): SshAuthMethod {
  if (raw === "password" || raw === "privateKey" || raw === "agent") {
    return raw;
  }
  return "auto";
}

/**
 * SSH config host entry resolved from ~/.ssh/config.
 */
export interface SshConfigHostEntry {
  alias: string;
  hostName?: string;
  user?: string;
  port?: number;
  /** Resolved IdentityFile paths (~ expanded to absolute). Empty if not configured. */
  identityFiles?: string[];
}

/**
 * SSH connection configuration sent from renderer.
 */
export interface SshConnectionConfig {
  host: string;
  port: number;
  username: string;
  authMethod: SshAuthMethod;
  password?: string;
  privateKeyPath?: string;
}

/**
 * Saved SSH connection profile (no password stored).
 */
export interface SshConnectionProfile {
  id: string;
  name: string;
  host: string;
  port: number;
  username: string;
  authMethod: SshAuthMethod;
  privateKeyPath?: string;
}

/**
 * SSH connection status returned from main process.
 */
export interface SshConnectionStatus {
  state: SshConnectionState;
  host: string | null;
  error: string | null;
  remoteProjectsPath: string | null;
}

/**
 * SSH API exposed via preload.
 */
/**
 * Saved SSH connection config (no password).
 */
export interface SshLastConnection {
  host: string;
  port: number;
  username: string;
  authMethod: SshAuthMethod;
  privateKeyPath?: string;
}

export interface SshAPI {
  connect: (config: SshConnectionConfig) => Promise<SshConnectionStatus>;
  disconnect: () => Promise<SshConnectionStatus>;
  getState: () => Promise<SshConnectionStatus>;
  test: (
    config: SshConnectionConfig,
  ) => Promise<{ success: boolean; error?: string }>;
  getConfigHosts: () => Promise<SshConfigHostEntry[]>;
  resolveHost: (alias: string) => Promise<SshConfigHostEntry | null>;
  saveLastConnection: (config: SshLastConnection) => Promise<void>;
  getLastConnection: () => Promise<SshLastConnection | null>;
  onStatus: (
    callback: (event: unknown, status: SshConnectionStatus) => void,
  ) => () => void;
}

// =============================================================================
// HTTP Server API
// =============================================================================

/**
 * HTTP server status returned from main process.
 */
export interface HttpServerStatus {
  running: boolean;
  port: number;
}

/**
 * HTTP Server API for controlling the sidecar server.
 */
export interface HttpServerAPI {
  start: () => Promise<HttpServerStatus>;
  stop: () => Promise<HttpServerStatus>;
  getStatus: () => Promise<HttpServerStatus>;
}

// =============================================================================
// Main Electron API
// =============================================================================

/**
 * Complete Electron API exposed to the renderer process via preload script.
 */
export interface ElectronAPI {
  getAppVersion: () => Promise<string>;
  getProjects: () => Promise<Project[]>;
  getSessions: (projectId: string) => Promise<Session[]>;
  getSessionsPaginated: (
    projectId: string,
    cursor: string | null,
    limit?: number,
    options?: SessionsPaginationOptions,
  ) => Promise<PaginatedSessionsResult>;
  searchSessions: (
    projectId: string,
    query: string,
    maxResults?: number,
  ) => Promise<SearchSessionsResult>;
  searchAllProjects: (
    query: string,
    maxResults?: number,
  ) => Promise<SearchSessionsResult>;
  findSessionById: (sessionId: string) => Promise<FindSessionByIdResult>;
  findSessionsByPartialId: (
    fragment: string,
  ) => Promise<FindSessionsByPartialIdResult>;
  getSessionDetail: (
    projectId: string,
    sessionId: string,
    knownFingerprint?: string,
  ) => Promise<SessionDetail | SessionDetailUnchanged | null>;
  /** Fetch full session detail with chunks and processes for export */
  getSessionDetailForExport: (
    projectId: string,
    sessionId: string,
  ) => Promise<SessionDetail | null>;
  getSessionMetrics: (
    projectId: string,
    sessionId: string,
  ) => Promise<SessionMetrics | null>;
  getWaterfallData: (
    projectId: string,
    sessionId: string,
  ) => Promise<WaterfallData | null>;
  getSubagentDetail: (
    projectId: string,
    sessionId: string,
    subagentId: string,
  ) => Promise<RawSubagentDetail | null>;
  getSessionGroups: (
    projectId: string,
    sessionId: string,
  ) => Promise<ConversationGroup[]>;
  getSessionsByIds: (
    projectId: string,
    sessionIds: string[],
    options?: SessionsByIdsOptions,
  ) => Promise<Session[]>;
  /** Delete a session and all associated files */
  deleteSession: (
    projectId: string,
    sessionId: string,
  ) => Promise<{ mainFileDeleted: boolean; associatedDeleted: number; errors: number }>;

  // Repository grouping (worktree support)
  getRepositoryGroups: () => Promise<RepositoryGroup[]>;
  getWorktreeSessions: (worktreeId: string) => Promise<Session[]>;

  // Validation methods
  validatePath: (
    relativePath: string,
    projectPath: string,
  ) => Promise<{ exists: boolean; isDirectory?: boolean }>;
  validateMentions: (
    mentions: { type: "path"; value: string }[],
    projectPath: string,
  ) => Promise<Record<string, boolean>>;

  // CLAUDE.md reading methods
  readClaudeMdFiles: (
    projectRoot: string,
  ) => Promise<Record<string, ClaudeMdFileInfo>>;
  readDirectoryClaudeMd: (dirPath: string) => Promise<ClaudeMdFileInfo>;
  readMentionedFile: (
    absolutePath: string,
    projectRoot: string,
    maxTokens?: number,
  ) => Promise<MentionedFileInfo | null>;

  // Agent config reading
  readAgentConfigs: (
    projectRoot: string,
  ) => Promise<Record<string, AgentConfig>>;

  // Notifications API
  notifications: NotificationsAPI;

  // Config API
  config: ConfigAPI;

  // Deep link navigation
  session: SessionAPI;

  // Window zoom sync (for traffic-light-safe layout)
  getZoomFactor: () => Promise<number>;
  setZoomFactor: (factor: number) => Promise<void>;
  onZoomFactorChanged: (callback: (zoomFactor: number) => void) => () => void;

  // File change events (real-time updates)
  onFileChange: (callback: (event: FileChangeEvent) => void) => () => void;
  onTodoChange: (callback: (event: TodoChangeEvent) => void) => () => void;
  /**
   * Phase 3A: Subscribe to memory file changes (projects/<id>/memory/*.md).
   * Fired when MEMORY.md or any other .md file in the memory directory changes.
   */
  onMemoryChange: (callback: (event: MemoryChangeEvent) => void) => () => void;

  // Session refresh (Ctrl+R / Cmd+R intercepted by main process)
  onSessionRefresh: (callback: () => void) => () => void;

  // Shell operations
  openPath: (
    targetPath: string,
    projectRoot?: string,
  ) => Promise<{ success: boolean; error?: string }>;
  openExternal: (url: string) => Promise<{ success: boolean; error?: string }>;

  /** Write text content to a file at the given path (desktop-only). */
  writeTextFile: (path: string, content: string) => Promise<void>;

  // Window controls (when title bar is hidden, e.g. Windows / Linux)
  windowControls: {
    minimize: () => Promise<void>;
    maximize: () => Promise<void>;
    close: () => Promise<void>;
    isMaximized: () => Promise<boolean>;
    relaunch: () => Promise<void>;
  };

  // Auto-start API (launch at login)
  autoStart: {
    enable: () => Promise<void>;
    disable: () => Promise<void>;
    isEnabled: () => Promise<boolean>;
  };

  // Platform-specific API (macOS dock, etc.)
  platform: {
    setDockVisible: (visible: boolean) => Promise<void>;
  };

  // SSH API
  ssh: SshAPI;

  // Context API
  context: {
    list: () => Promise<ContextInfo[]>;
    getActive: () => Promise<string>;
    switch: (contextId: string) => Promise<{ contextId: string }>;
    onChanged: (
      callback: (event: unknown, data: ContextInfo) => void,
    ) => () => void;
  };

  // HTTP Server API
  httpServer: HttpServerAPI;

  // Memory API
  memory: MemoryAPI;
}

// =============================================================================
// Memory API types
// =============================================================================

export interface MemoryEntry {
  title: string;
  file: string;
  hook: string;
  lineNumber: number;
}

export interface MemoryIndex {
  rawMarkdown: string;
  entries: MemoryEntry[];
  orphanFiles: string[];
}

export type MemoryReadFileResult =
  | { success: true; content: string; path: string }
  | { success: false; error: string };

export interface MemoryOpenResult {
  success: boolean;
  path?: string;
  error?: string;
}

export interface OpenTarget {
  id: string;
  label: string;
  iconName: string;
  available: boolean;
  shortcutKey?: string;
}

export interface MemoryAPI {
  hasMemory: (projectId: string) => Promise<boolean>;
  getIndex: (projectId: string) => Promise<MemoryIndex | null>;
  readFile: (
    projectId: string,
    fileName: string,
  ) => Promise<MemoryReadFileResult>;
  copyPath: (
    projectId: string,
    fileName: string | null,
  ) => Promise<MemoryOpenResult>;
  listAvailableOpeners: () => Promise<OpenTarget[]>;
  openIn: (
    projectId: string,
    fileName: string | null,
    openerId: string,
  ) => Promise<MemoryOpenResult>;
}

// =============================================================================
// Window Type Extension
// =============================================================================

declare global {
  interface Window {
    electronAPI: ElectronAPI;
  }
}
