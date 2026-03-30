/**
 * Update slice - manages update check and directs to GitHub releases.
 *
 * Uses @tauri-apps/plugin-updater to check for updates only.
 * Download is handled manually via GitHub releases page.
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
  updateStatus: 'idle' | 'checking' | 'available' | 'not-available' | 'error';
  availableVersion: string | null;
  releaseNotes: string | null;
  updateError: string | null;
  showUpdateDialog: boolean;

  // Actions
  checkForUpdates: () => void;
  downloadUpdate: () => void;
  dismissUpdateDialog: () => void;
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

  checkForUpdates: () => {
    set({ updateStatus: 'checking', updateError: null });

    import('@tauri-apps/plugin-updater')
      .then(({ check }) => check())
      .then((update) => {
        if (update) {
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
    // 打开 GitHub Release 页面让用户手动下载
    window.open('https://github.com/augustVino/claude-devtools-tauri/releases/latest', '_blank');
    set({ showUpdateDialog: false });
  },

  dismissUpdateDialog: () => {
    set({ showUpdateDialog: false });
  },
});
