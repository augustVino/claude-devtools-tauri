/**
 * Connection Slice - Manages SSH connection state.
 *
 * Tracks connection mode (local/ssh), connection state,
 * and provides actions for connecting/disconnecting.
 */

import { api } from '@renderer/api';

import { getFullResetState } from '../utils/stateResetHelpers';
import { normalizeSshAuthMethod } from '@shared/types';

import type { AppState } from '../types';
import type {
  SshConfigHostEntry,
  SshConnectionConfig,
  SshConnectionState,
  SshLastConnection,
} from '@shared/types';
import type { StateCreator } from 'zustand';

// =============================================================================
// Slice Interface
// =============================================================================

export interface ConnectionSlice {
  // State
  connectionMode: 'local' | 'ssh';
  connectionState: SshConnectionState;
  connectedHost: string | null;
  connectionError: string | null;
  sshConfigHosts: SshConfigHostEntry[];
  lastSshConfig: SshLastConnection | null;

  // Actions
  connectSsh: (config: SshConnectionConfig) => Promise<void>;
  disconnectSsh: () => Promise<void>;
  testConnection: (config: SshConnectionConfig) => Promise<{ success: boolean; error?: string }>;
  setConnectionStatus: (
    state: SshConnectionState,
    host: string | null,
    error: string | null
  ) => void;
  fetchSshConfigHosts: () => Promise<void>;
  resolveConfigHost: (alias: string) => Promise<SshConfigHostEntry | null>;
  loadLastConnection: () => Promise<void>;
}

// =============================================================================
// Slice Creator
// =============================================================================

export const createConnectionSlice: StateCreator<AppState, [], [], ConnectionSlice> = (
  set,
  get
) => ({
  // Initial state
  connectionMode: 'local',
  connectionState: 'disconnected',
  connectedHost: null,
  connectionError: null,
  sshConfigHosts: [],
  lastSshConfig: null,

  // Actions
  connectSsh: async (config: SshConnectionConfig): Promise<void> => {
    set({
      isContextSwitching: true,
      // 与后端 ssh_context_id 同构（ssh-{host}）：切换 overlay 显示
      // 目标主机名而非 Unknown（此前漏设，overlay 只能 fallback 'Unknown'）
      targetContextId: `ssh-${config.host}`,
      connectionState: 'connecting',
      connectedHost: config.host,
      connectionError: null,
    });

    try {
      const status = await api.ssh.connect(config);
      set({
        connectionMode: status.state === 'connected' ? 'ssh' : 'local',
        connectionState: status.state,
        connectedHost: status.host,
        connectionError: status.error,
        // On connect: sync context ID + clear all stale local data including tabs
        ...(status.state === 'connected'
          ? {
              activeContextId: `ssh-${config.host}`,
              projects: [],
              repositoryGroups: [],
              openTabs: [],
              activeTabId: null,
              selectedTabIds: [],
              paneLayout: {
                panes: [
                  {
                    id: 'pane-default',
                    tabs: [],
                    activeTabId: null,
                    selectedTabIds: [],
                    widthFraction: 1,
                  },
                ],
                focusedPaneId: 'pane-default',
              },
              ...getFullResetState(),
            }
          : {}),
        isContextSwitching: false,
      });

      // Re-fetch all data and persist config when connected
      if (status.state === 'connected') {
        const state = get();
        void state.fetchProjects();
        void state.fetchRepositoryGroups();

        // Save connection config (without password) for form pre-fill on next launch
        const saved: SshLastConnection = {
          host: config.host,
          port: config.port,
          username: config.username,
          authMethod: normalizeSshAuthMethod(config.authMethod),
          privateKeyPath: config.privateKeyPath,
        };
        set({ lastSshConfig: saved });
        void api.ssh.saveLastConnection(saved);
      }
    } catch (err) {
      // Task 8: Tauri invoke reject 时 err 是 string（commands::ssh_connect 的
      // into_tauri_string()）；HTTP fetch 抛错时 err 是 Error。两个分支都要处理。
      // 注意：保留原 connectedHost 行为（不清空）—— 与 Electron ipc/ssh.ts 失败时
      // resolve {success:false} 等价，UI 仍显示用户输入的 host。
      const message =
        err instanceof Error
          ? err.message
          : typeof err === 'string'
            ? err
            : String(err);
      set({
        connectionState: 'error',
        connectionError: message,
        isContextSwitching: false,
        targetContextId: null,
      });
    }
  },

  disconnectSsh: async (): Promise<void> => {
    set({ isContextSwitching: true, targetContextId: 'local' });

    try {
      const status = await api.ssh.disconnect();
      set({
        connectionMode: 'local',
        connectionState: status.state,
        connectedHost: null,
        connectionError: null,
        activeContextId: 'local',
        // Clear all stale SSH data including tabs so dashboard shows fresh local data
        projects: [],
        repositoryGroups: [],
        openTabs: [],
        activeTabId: null,
        selectedTabIds: [],
        paneLayout: {
          panes: [
            {
              id: 'pane-default',
              tabs: [],
              activeTabId: null,
              selectedTabIds: [],
              widthFraction: 1,
            },
          ],
          focusedPaneId: 'pane-default',
        },
        ...getFullResetState(),
        isContextSwitching: false,
      });

      // Re-fetch local data
      const state = get();
      void state.fetchProjects();
      void state.fetchRepositoryGroups();
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      set({
        connectionError: message,
        isContextSwitching: false,
      });
    }
  },

  testConnection: async (
    config: SshConnectionConfig
  ): Promise<{ success: boolean; error?: string }> => {
    try {
      return await api.ssh.test(config);
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      return { success: false, error: message };
    }
  },

  setConnectionStatus: (
    state: SshConnectionState,
    host: string | null,
    error: string | null
  ): void => {
    set({
      connectionState: state,
      connectionMode: state === 'connected' ? 'ssh' : 'local',
      connectedHost: host,
      connectionError: error,
    });
  },

  fetchSshConfigHosts: async (): Promise<void> => {
    try {
      const hosts = await api.ssh.getConfigHosts();
      set({ sshConfigHosts: hosts });
    } catch {
      // Gracefully ignore - SSH config may not exist
      set({ sshConfigHosts: [] });
    }
  },

  resolveConfigHost: async (alias: string): Promise<SshConfigHostEntry | null> => {
    try {
      return await api.ssh.resolveHost(alias);
    } catch {
      return null;
    }
  },

  loadLastConnection: async (): Promise<void> => {
    try {
      const saved = await api.ssh.getLastConnection();
      set({ lastSshConfig: saved });
    } catch {
      // Gracefully ignore - no saved connection
    }
  },
});
