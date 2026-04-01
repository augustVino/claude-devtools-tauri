/**
 * Update slice - manages app update check, download, and install flow.
 *
 * Uses @tauri-apps/plugin-updater for check + download + install.
 * Three-phase UI: available -> downloading -> downloaded (or download-error).
 */

import { createLogger } from '@shared/utils/logger';

import type { AppState } from '../types';
import type { StateCreator } from 'zustand';

const logger = createLogger('Store:update');

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
    | 'error'
    | 'downloading'
    | 'downloaded'
    | 'download-error';
  availableVersion: string | null;
  releaseNotes: string | null;
  updateError: string | null;
  showUpdateDialog: boolean;
  downloadProgress: number;
  downloadError: string | null;

  // Actions
  checkForUpdates: () => void;
  downloadUpdate: () => void;
  installAndRestart: () => void;
  retryDownload: () => void;
  dismissUpdateDialog: () => void;
}

// =============================================================================
// Helpers
// =============================================================================

/** Hold a reference to the resolved Update object across phases. */
let pendingUpdate: Awaited<ReturnType<typeof import('@tauri-apps/plugin-updater').check>> = null;

/**
 * Download the update using the Tauri updater plugin.
 * Tracks progress via chunk lengths against total content length.
 */
async function performDownload(
  set: (partial: Partial<AppState> | ((s: AppState) => Partial<AppState>)) => void,
): Promise<void> {
  if (!pendingUpdate) {
    set({
      updateStatus: 'download-error',
      downloadError: 'No update available. Please check for updates again.',
    });
    return;
  }

  set({ updateStatus: 'downloading', downloadProgress: 0, downloadError: null });

  let totalBytes = 0;

  try {
    await pendingUpdate.downloadAndInstall((event) => {
      switch (event.event) {
        case 'Started':
          totalBytes = event.data.contentLength ?? 0;
          break;
        case 'Progress': {
          if (totalBytes > 0) {
            // Estimate progress from cumulative bytes.
            // The plugin does not expose cumulative downloaded bytes,
            // so we accumulate from chunks as a reasonable approximation.
            set((s) => {
              const downloaded = (s.downloadProgress / 100) * totalBytes + event.data.chunkLength;
              const progress = Math.min(Math.round((downloaded / totalBytes) * 100), 100);
              return { downloadProgress: progress };
            });
          }
          break;
        }
        case 'Finished':
          set({ downloadProgress: 100 });
          break;
      }
    });

    set({ updateStatus: 'downloaded', downloadProgress: 100 });
  } catch (error: unknown) {
    logger.error('Failed to download update:', error);
    set({
      updateStatus: 'download-error',
      downloadError: error instanceof Error ? error.message : String(error),
    });
  }
}

// =============================================================================
// Slice Creator
// =============================================================================

export const createUpdateSlice: StateCreator<AppState, [], [], UpdateSlice> = (set) => ({
  // Initial state
  updateStatus: 'idle',
  availableVersion: null,
  releaseNotes: null,
  updateError: null,
  showUpdateDialog: false,
  downloadProgress: 0,
  downloadError: null,

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
          pendingUpdate = null;
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
    performDownload(set);
  },

  installAndRestart: async () => {
    try {
      // The update was already downloaded via downloadAndInstall(),
      // but the plugin requires relaunch to apply. Use process exit +
      // tauri relaunch utility.
      const { relaunch } = await import('@tauri-apps/plugin-process');
      await relaunch();
    } catch (error: unknown) {
      logger.error('Failed to restart:', error);
      // If relaunch fails, fall back to downloaded state so user can retry
      set({
        updateStatus: 'download-error',
        downloadError: error instanceof Error ? error.message : String(error),
      });
    }
  },

  retryDownload: () => {
    performDownload(set);
  },

  dismissUpdateDialog: () => {
    // Block dismiss during downloading / downloaded phases
    set((state) => {
      if (state.updateStatus === 'downloading' || state.updateStatus === 'downloaded') {
        return {};
      }
      return { showUpdateDialog: false };
    });
  },
});
