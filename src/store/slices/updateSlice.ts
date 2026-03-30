/**
 * Update slice - manages OTA auto-update state and actions.
 *
 * Uses @tauri-apps/plugin-updater JS API directly (no custom Rust commands).
 */

import { createLogger } from '@shared/utils/logger';

import type { AppState } from '../types';
import type { StateCreator } from 'zustand';

const logger = createLogger('Store:update');

// Module-level reference to the pending update (between check and download)
let pendingUpdate: import('@tauri-apps/plugin-updater').Update | null = null;

// =============================================================================
// Slice Interface
// =============================================================================

export interface UpdateSlice {
  // State
  updateStatus:
    | 'idle'
    | 'checking'
    | 'available'
    | 'not-available'
    | 'downloading'
    | 'downloaded'
    | 'error';
  availableVersion: string | null;
  releaseNotes: string | null;
  downloadProgress: number;
  updateError: string | null;
  showUpdateDialog: boolean;
  showUpdateBanner: boolean;

  // Actions
  checkForUpdates: () => void;
  downloadUpdate: () => void;
  installUpdate: () => void;
  dismissUpdateDialog: () => void;
  dismissUpdateBanner: () => void;
}

// =============================================================================
// Slice Creator
// =============================================================================

export const createUpdateSlice: StateCreator<AppState, [], [], UpdateSlice> = (set, get) => ({
  // Initial state
  updateStatus: 'idle',
  availableVersion: null,
  releaseNotes: null,
  downloadProgress: 0,
  updateError: null,
  showUpdateDialog: false,
  showUpdateBanner: false,

  checkForUpdates: () => {
    set({ updateStatus: 'checking', updateError: null });

    import('@tauri-apps/plugin-updater')
      .then(({ check }) => check())
      .then((update) => {
        if (update) {
          pendingUpdate = update;
          set({
            updateStatus: 'available',
            availableVersion: update.version,
            releaseNotes: update.body ?? null,
            showUpdateDialog: true,
          });
        } else {
          set({ updateStatus: 'not-available' });
        }
      })
      .catch((error: unknown) => {
        logger.error('Failed to check for updates:', error);
        set({
          updateStatus: 'error',
          updateError: error instanceof Error ? error.message : String(error),
        });
      });
  },

  downloadUpdate: () => {
    const update = pendingUpdate;
    if (!update) {
      logger.error('No pending update to download');
      return;
    }

    set({ showUpdateDialog: false, showUpdateBanner: true, downloadProgress: 0 });

    let downloaded = 0;
    let contentLength = 0;

    update
      .downloadAndInstall((event) => {
        if (event.event === 'Started') {
          contentLength = event.data.contentLength ?? 0;
          downloaded = 0;
        } else if (event.event === 'Progress') {
          downloaded += event.data.chunkLength;
          if (contentLength > 0) {
            set({ downloadProgress: (downloaded / contentLength) * 100 });
          }
        } else if (event.event === 'Finished') {
          set({ downloadProgress: 100 });
        }
      })
      .then(() => {
        set({
          updateStatus: 'downloaded',
          downloadProgress: 100,
          availableVersion: get().availableVersion ?? update.version,
        });
        pendingUpdate = null;
      })
      .catch((error: unknown) => {
        logger.error('Failed to download update:', error);
        set({
          updateStatus: 'error',
          updateError: error instanceof Error ? error.message : String(error),
          showUpdateBanner: false,
        });
      });
  },

  installUpdate: () => {
    import('@tauri-apps/plugin-process')
      .then(({ relaunch }) => relaunch())
      .catch((error: unknown) => {
        logger.error('Failed to relaunch:', error);
      });
  },

  dismissUpdateDialog: () => {
    set({ showUpdateDialog: false });
  },

  dismissUpdateBanner: () => {
    set({ showUpdateBanner: false });
  },
});
