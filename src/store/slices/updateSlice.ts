/**
 * Update slice - manages update check, download, and install.
 *
 * Uses @tauri-apps/plugin-updater to check, download, and install updates.
 * Download progress is shown in UpdateDialog with retry support.
 *
 * Note: tauri-plugin-updater's downloadAndInstall() does NOT support
 * AbortController/signal cancellation. Timeout is handled via
 * DownloadOptions.timeout at the Rust level.
 */

import { createLogger } from '@shared/utils/logger';

import type { AppState } from '../types';
import type { StateCreator } from 'zustand';

const logger = createLogger('Store:update');

// Module-level ref for the non-serializable Update object returned by check()
let pendingUpdate: Awaited<ReturnType<typeof import('@tauri-apps/plugin-updater').check>> | null = null;

// Rust-side download timeout in ms (5 minutes for ~10MB file)
const DOWNLOAD_TIMEOUT_MS = 300_000;

// =============================================================================
// Slice Interface
// =============================================================================

export interface UpdateSlice {
  // State
  updateStatus: 'idle' | 'checking' | 'available' | 'downloading' | 'downloaded' | 'download-error' | 'not-available' | 'error';
  availableVersion: string | null;
  releaseNotes: string | null;
  updateError: string | null;
  downloadProgress: number;
  downloadError: string | null;
  showUpdateDialog: boolean;

  // Actions
  checkForUpdates: () => void;
  downloadUpdate: () => void;
  installAndRestart: () => void;
  retryDownload: () => void;
  dismissUpdateDialog: () => void;
  resetUpdateStatus: () => void;
}

// =============================================================================
// Slice Creator
// =============================================================================

export const createUpdateSlice: StateCreator<AppState, [], [], UpdateSlice> = (set, get) => ({
  // Initial state
  updateStatus: 'idle',
  availableVersion: null,
  releaseNotes: null,
  updateError: null,
  downloadProgress: 0,
  downloadError: null,
  showUpdateDialog: false,

  checkForUpdates: () => {
    if (get().updateStatus === 'checking' || get().updateStatus === 'downloading') return;
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
    if (get().updateStatus === 'downloading') return;
    if (!pendingUpdate) {
      set({ updateStatus: 'error', updateError: 'No update available' });
      return;
    }

    let downloadedBytes = 0;
    let totalBytes = 0;

    set({ updateStatus: 'downloading', downloadProgress: 0, downloadError: null });

    pendingUpdate
      .downloadAndInstall(
        (event) => {
          switch (event.event) {
            case 'Started':
              totalBytes = event.data.contentLength ?? 0;
              break;
            case 'Progress':
              downloadedBytes += event.data.chunkLength;
              if (totalBytes > 0) {
                set({ downloadProgress: Math.round((downloadedBytes / totalBytes) * 100) });
              } else {
                // Unknown total: show progress capped at 99% based on MB downloaded
                set({ downloadProgress: Math.min(99, Math.round(downloadedBytes / 10_000_000 * 100)) });
              }
              break;
            case 'Finished':
              set({ downloadProgress: 100 });
              break;
          }
        },
        { timeout: DOWNLOAD_TIMEOUT_MS },
      )
      .then(() => {
        // downloadAndInstall resolves after both download AND install are complete
        set({ updateStatus: 'downloaded', downloadProgress: 100 });
      })
      .catch((error: unknown) => {
        logger.error('Failed to download update:', error);
        set({
          updateStatus: 'download-error',
          downloadError: error instanceof Error ? error.message : String(error),
          downloadProgress: 0,
        });
      });
  },

  installAndRestart: () => {
    import('@tauri-apps/plugin-process')
      .then(({ relaunch }) => relaunch())
      .catch((error: unknown) => {
        logger.error('Failed to relaunch:', error);
        set({
          updateStatus: 'download-error',
          downloadError: `Failed to restart application: ${error instanceof Error ? error.message : String(error)}`,
        });
      });
  },

  retryDownload: () => {
    if (get().updateStatus === 'checking' || get().updateStatus === 'downloading') return;
    set({ downloadError: null, updateStatus: 'checking' });
    // Re-run check first, then auto-download if update is still available
    import('@tauri-apps/plugin-updater')
      .then(({ check }) => check())
      .then((update) => {
        if (update) {
          pendingUpdate = update;
          // Auto-start download
          get().downloadUpdate();
        } else {
          set({ updateStatus: 'not-available', showUpdateDialog: false });
        }
      })
      .catch((error: unknown) => {
        logger.error('Failed to re-check for updates:', error);
        set({
          updateStatus: 'download-error',
          downloadError: error instanceof Error ? error.message : String(error),
        });
      });
  },

  dismissUpdateDialog: () => {
    // Block dismiss during active download or when update is ready to install
    const status = get().updateStatus;
    if (status === 'downloading' || status === 'downloaded') return;
    set({ showUpdateDialog: false, updateStatus: 'idle' });
  },

  resetUpdateStatus: () => {
    set({ updateStatus: 'idle' });
  },
});
