/**
 * Tauri API client — implements ElectronAPI via @tauri-apps/api invoke/listen.
 *
 * Window controls, version, sessions, config, search, validation, and utility
 * commands are implemented. Notifications and advanced features are stubbed.
 */

import { invoke } from '@tauri-apps/api/core';

import type { ElectronAPI, ConfigAPI } from '@shared/types/api';
import type {
  Session,
  SessionDetail,
  SessionMetrics,
  Project,
  SearchSessionsResult,
  PaginatedSessionsResult,
} from '@main/types';
import type { AppConfig } from '@shared/types/notifications';

const NOT_IMPLEMENTED = new Error('Not yet implemented in Tauri backend');

function notImplemented(): Promise<never> {
  return Promise.reject(NOT_IMPLEMENTED);
}

function stub(): Record<string, unknown> {
  return new Proxy({} as Record<string, unknown>, {
    get() {
      return notImplemented;
    },
  });
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

  readonly getAppVersion = (): Promise<string> => invoke('get_app_version');

  readonly windowControls = {
    minimize: (): Promise<void> => invoke('minimize'),
    maximize: (): Promise<void> => invoke('maximize'),
    close: (): Promise<void> => invoke('close'),
    isMaximized: (): Promise<boolean> => invoke<boolean>('is_maximized'),
    relaunch: async (): Promise<void> => {
      try {
        await invoke('process_relaunch');
      } catch {
        window.location.reload();
      }
    },
  };

  readonly getZoomFactor = async (): Promise<number> => {
    const result = await invoke<ZoomFactorResult>('get_zoom_factor');
    return result.factor;
  };
  readonly onZoomFactorChanged = (): (() => void) => () => {};

  // ===========================================================================
  // Project and Session commands
  // ===========================================================================

  readonly getProjects = (): Promise<Project[]> => invoke('get_projects');

  readonly getSessions = (projectId: string): Promise<Session[]> =>
    invoke('get_sessions', { projectId });

  readonly getSessionsPaginated = (
    projectId: string,
    cursor: string | null,
    limit?: number,
    options?: unknown
  ): Promise<PaginatedSessionsResult> =>
    invoke('get_sessions_paginated', { projectId, cursor, limit, options });

  readonly getSessionDetail = (
    projectId: string,
    sessionId: string
  ): Promise<SessionDetail | null> =>
    invoke('get_session_detail', { projectId, sessionId });

  readonly getSessionMetrics = (
    projectId: string,
    sessionId: string
  ): Promise<SessionMetrics | null> =>
    invoke('get_session_metrics', { projectId, sessionId });

  // ===========================================================================
  // Search commands
  // ===========================================================================

  readonly searchSessions = (
    projectId: string,
    query: string,
    maxResults?: number
  ): Promise<SearchSessionsResult> =>
    invoke('search_sessions', { projectId, query, maxResults });

  readonly searchAllProjects = (
    query: string,
    maxResults?: number
  ): Promise<SearchSessionsResult> =>
    invoke('search_all_projects', { query, maxResults });

  // ===========================================================================
  // Validation commands
  // ===========================================================================

  readonly validatePath = async (
    relativePath: string,
    _projectPath: string
  ): Promise<{ exists: boolean; isDirectory?: boolean }> => {
    const result = await invoke<PathValidationResult>('validate_path', {
      path: relativePath,
    });
    return {
      exists: result.valid,
      isDirectory: undefined,
    };
  };

  readonly validateMentions = async (
    mentions: { type: 'path'; value: string }[],
    _projectPath: string
  ): Promise<Record<string, boolean>> => {
    const paths = mentions.map((m) => m.value);
    const result = await invoke<MentionsValidationResult>('validate_mentions', {
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
    _projectRoot?: string
  ): Promise<{ success: boolean; error?: string }> => {
    try {
      await invoke('open_path', { path: targetPath });
      return { success: true };
    } catch (e) {
      return { success: false, error: String(e) };
    }
  };

  readonly openExternal = async (
    url: string
  ): Promise<{ success: boolean; error?: string }> => {
    try {
      await invoke('open_external', { url });
      return { success: true };
    } catch (e) {
      return { success: false, error: String(e) };
    }
  };

  // ===========================================================================
  // Stubbed — will be implemented in later phases
  // ===========================================================================

  readonly getWaterfallData = notImplemented;
  readonly getSubagentDetail = notImplemented;
  readonly getSessionGroups = notImplemented;
  readonly getSessionsByIds = notImplemented;
  readonly getRepositoryGroups = notImplemented;
  readonly getWorktreeSessions = notImplemented;
  readonly readClaudeMdFiles = notImplemented;
  readonly readDirectoryClaudeMd = notImplemented;
  readonly readMentionedFile = notImplemented;
  readonly readAgentConfigs = notImplemented;

  // Nested API objects — partially implemented
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  readonly notifications = stub() as any;
  readonly session = stub() as any;
  readonly updater = stub() as any;
  readonly ssh = stub() as any;
  readonly context = stub() as any;
  readonly httpServer = stub() as any;

  // Config API — implemented
  readonly config: ConfigAPI = {
    get: (): Promise<AppConfig> => invoke<AppConfig>('get_config'),
    update: (section: string, data: object): Promise<AppConfig> =>
      invoke<AppConfig>('update_config', { section, data }),
    addIgnoreRegex: (pattern: string): Promise<AppConfig> =>
      invoke<AppConfig>('add_ignore_regex', { pattern }),
    removeIgnoreRegex: (pattern: string): Promise<AppConfig> =>
      invoke<AppConfig>('remove_ignore_regex', { pattern }),
    addIgnoreRepository: notImplemented,
    removeIgnoreRepository: notImplemented,
    snooze: (minutes: number): Promise<AppConfig> => invoke<AppConfig>('snooze', { minutes }),
    clearSnooze: (): Promise<AppConfig> => invoke<AppConfig>('clear_snooze'),
    addTrigger: notImplemented,
    updateTrigger: notImplemented,
    removeTrigger: notImplemented,
    getTriggers: notImplemented,
    testTrigger: notImplemented,
    selectFolders: notImplemented,
    selectClaudeRootFolder: notImplemented,
    getClaudeRootInfo: notImplemented,
    findWslClaudeRoots: notImplemented,
    openInEditor: notImplemented,
    pinSession: (projectId: string, sessionId: string): Promise<void> =>
      invoke('pin_session', { projectId, sessionId }),
    unpinSession: (projectId: string, sessionId: string): Promise<void> =>
      invoke('unpin_session', { projectId, sessionId }),
    hideSession: (projectId: string, sessionId: string): Promise<void> =>
      invoke('hide_session', { projectId, sessionId }),
    unhideSession: (projectId: string, sessionId: string): Promise<void> =>
      invoke('unhide_session', { projectId, sessionId }),
    hideSessions: notImplemented,
    unhideSessions: notImplemented,
  };

  // Event listeners — return no-op unlisten
  readonly onFileChange = (): (() => void) => () => {};
  readonly onTodoChange = (): (() => void) => () => {};
  readonly onSessionRefresh = (): (() => void) => () => {};
}