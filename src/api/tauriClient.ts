/**
 * Tauri API client — implements ElectronAPI via @tauri-apps/api invoke/listen.
 *
 * Currently only window controls and version are implemented.
 * Remaining commands will be wired in subsequent phases.
 */

import { invoke } from '@tauri-apps/api/core';

import type { ElectronAPI } from '@shared/types/api';

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

export class TauriAPIClient implements ElectronAPI {
  // =========================================================================
  // Implemented commands
  // =========================================================================

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

  readonly getZoomFactor = (): Promise<number> => Promise.resolve(1);
  readonly onZoomFactorChanged = (): (() => void) => () => {};

  // =========================================================================
  // Stubbed — will be implemented in later phases
  // =========================================================================

  readonly getProjects = notImplemented;
  readonly getSessions = notImplemented;
  readonly getSessionsPaginated = notImplemented;
  readonly searchSessions = notImplemented;
  readonly searchAllProjects = notImplemented;
  readonly getSessionDetail = notImplemented;
  readonly getSessionMetrics = notImplemented;
  readonly getWaterfallData = notImplemented;
  readonly getSubagentDetail = notImplemented;
  readonly getSessionGroups = notImplemented;
  readonly getSessionsByIds = notImplemented;
  readonly getRepositoryGroups = notImplemented;
  readonly getWorktreeSessions = notImplemented;
  readonly validatePath = notImplemented;
  readonly validateMentions = notImplemented;
  readonly readClaudeMdFiles = notImplemented;
  readonly readDirectoryClaudeMd = notImplemented;
  readonly readMentionedFile = notImplemented;
  readonly readAgentConfigs = notImplemented;
  readonly openPath = notImplemented;
  readonly openExternal = notImplemented;

  // Nested API objects — fully stubbed
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  readonly notifications = stub() as any;
  readonly config = stub() as any;
  readonly session = stub() as any;
  readonly updater = stub() as any;
  readonly ssh = stub() as any;
  readonly context = stub() as any;
  readonly httpServer = stub() as any;

  // Event listeners — return no-op unlisten
  readonly onFileChange = (): (() => void) => () => {};
  readonly onTodoChange = (): (() => void) => () => {};
  readonly onSessionRefresh = (): (() => void) => () => {};
}
