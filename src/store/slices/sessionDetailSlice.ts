/**
 * Session detail slice - manages session detail, conversation, and stats.
 */

import { api } from '@renderer/api';
import { asEnhancedChunkArray } from '@renderer/types/data';
import { findTabBySession, truncateLabel } from '@renderer/types/tabs';
import { processSessionClaudeMd } from '@renderer/utils/claudeMdTracker';
import { processSessionContextWithPhases } from '@renderer/utils/contextTracker';
import {
  extractFileReferences,
  incrementalUpdateConversation,
  transformChunksToConversation
} from '@renderer/utils/groupTransformer';
import { isSessionDetailUnchanged } from '@shared/utils/sessionDetailResponse';
import { createLogger } from '@shared/utils/logger';

import { batchAsync } from '../utils/batchAsync';
import { resolveFilePath } from '../utils/pathResolution';

const logger = createLogger('Store:sessionDetail');

/**
 * Tracks latest refresh generation per session to avoid stale overwrites when
 * many file-change events trigger concurrent in-place refreshes.
 */
const sessionRefreshGeneration = new Map<string, number>();
const sessionRefreshInFlight = new Set<string>();
const sessionRefreshQueued = new Set<string>();
/** Fingerprint cache for skipping unchanged session refreshes */
const sessionChunkFingerprint = new Map<string, string>();
/** File-level fingerprint cache for backend short-circuit */
const sessionFileFingerprint = new Map<string, string>();
let sessionDetailFetchGeneration = 0;
let agentConfigsCachedForProject = '';

/**
 * Cleanup coordination entries for a session when its tab is closed.
 * Safe to call at any time — entries are only stale data, not live refs.
 */
export function cleanupSessionRefreshCoordination(refreshKey: string): void {
  sessionRefreshGeneration.delete(refreshKey);
  sessionRefreshInFlight.delete(refreshKey);
  sessionRefreshQueued.delete(refreshKey);
  sessionChunkFingerprint.delete(refreshKey);
  sessionFileFingerprint.delete(refreshKey);
}

import { getAllTabs } from '../utils/paneHelpers';

import type { AppState } from '../types';
import type { ClaudeMdStats } from '@renderer/types/claudeMd';
import type {
  ContextPhaseInfo,
  ContextStats,
  MentionedFileInfo
} from '@renderer/types/contextInjection';
import type { ClaudeMdFileInfo, EnhancedChunk, Process, SessionDetail } from '@renderer/types/data';
import type { AIGroup, SessionConversation } from '@renderer/types/groups';
import type { AgentConfig } from '@shared/types/api';
import type { StateCreator } from 'zustand';

// =============================================================================
// Per-tab session data type
// =============================================================================

export interface TabSessionData {
  sessionDetail: SessionDetail | null;
  conversation: SessionConversation | null;
  conversationLoading: boolean;
  sessionDetailLoading: boolean;
  sessionDetailError: string | null;
  sessionClaudeMdStats: Map<string, ClaudeMdStats> | null;
  sessionContextStats: Map<string, ContextStats> | null;
  sessionPhaseInfo: ContextPhaseInfo | null;
  visibleAIGroupId: string | null;
  selectedAIGroup: AIGroup | null;
}

function createEmptyTabSessionData(): TabSessionData {
  return {
    sessionDetail: null,
    conversation: null,
    conversationLoading: false,
    sessionDetailLoading: false,
    sessionDetailError: null,
    sessionClaudeMdStats: null,
    sessionContextStats: null,
    sessionPhaseInfo: null,
    visibleAIGroupId: null,
    selectedAIGroup: null
  };
}

// =============================================================================
// Slice Interface
// =============================================================================

export interface SessionDetailSlice {
  // State
  sessionDetail: SessionDetail | null;
  sessionDetailLoading: boolean;
  sessionDetailError: string | null;

  // Conversation state
  conversation: SessionConversation | null;
  conversationLoading: boolean;

  // CLAUDE.md stats (injection tracking per AI group)
  sessionClaudeMdStats: Map<string, ClaudeMdStats> | null;
  // Unified context stats (CLAUDE.md + mentioned files + tool outputs)
  sessionContextStats: Map<string, ContextStats> | null;
  // Context phase info (compaction boundaries)
  sessionPhaseInfo: ContextPhaseInfo | null;

  // Agent configs from .claude/agents/ (keyed by agent name)
  agentConfigs: Record<string, AgentConfig>;

  // Visible AI Group
  visibleAIGroupId: string | null;
  selectedAIGroup: AIGroup | null;

  // Per-tab session data (keyed by tabId)
  tabSessionData: Record<string, TabSessionData>;

  // Actions
  fetchSessionDetail: (projectId: string, sessionId: string, tabId?: string) => Promise<void>;
  /** Refresh session without loading states or UI resets - for real-time updates */
  refreshSessionInPlace: (projectId: string, sessionId: string) => Promise<void>;
  setVisibleAIGroup: (aiGroupId: string | null) => void;
  /** Set visible AI group for a specific tab */
  setTabVisibleAIGroup: (tabId: string, aiGroupId: string | null) => void;
  /** Clean up per-tab session data when tab is closed */
  cleanupTabSessionData: (tabId: string) => void;
}

// =============================================================================
// Slice Creator
// =============================================================================

export const createSessionDetailSlice: StateCreator<AppState, [], [], SessionDetailSlice> = (
  set,
  get
) => ({
  // Initial state
  sessionDetail: null,
  sessionDetailLoading: false,
  sessionDetailError: null,

  conversation: null,
  conversationLoading: false,

  // CLAUDE.md stats (injection tracking per AI group)
  sessionClaudeMdStats: null,
  // Unified context stats (CLAUDE.md + mentioned files + tool outputs)
  sessionContextStats: null,
  // Context phase info (compaction boundaries)
  sessionPhaseInfo: null,

  agentConfigs: {},

  visibleAIGroupId: null,
  selectedAIGroup: null,

  // Per-tab session data
  tabSessionData: {},

  // Fetch full session detail with chunks and subagents — two-phase loading:
  //   Phase 1 (sync await): fetch + transform → immediate UI render (~200-500ms)
  //   Phase 2 (fire-and-forget): async context tracking (CLAUDE.md, mentioned files)
  fetchSessionDetail: async (projectId: string, sessionId: string, tabId?: string) => {
    const requestGeneration = ++sessionDetailFetchGeneration;
    set({
      sessionDetailLoading: true,
      sessionDetailError: null,
      conversationLoading: true
    });

    // Also set per-tab loading state
    if (tabId) {
      const prev = get().tabSessionData;
      set({
        tabSessionData: {
          ...prev,
          [tabId]: {
            ...(prev[tabId] ?? createEmptyTabSessionData()),
            sessionDetailLoading: true,
            sessionDetailError: null,
            conversationLoading: true
          }
        }
      });
    }
    try {
      // ═══════════════════════════════════════
      // PHASE 1: 即时渲染 (~200-500ms)
      // ═══════════════════════════════════════
      const response = await api.getSessionDetail(projectId, sessionId);
      if (requestGeneration !== sessionDetailFetchGeneration) {
        return;
      }
      if (!response) {
        const refreshKey = `${projectId}/${sessionId}`;
        sessionFileFingerprint.delete(refreshKey);
        sessionChunkFingerprint.delete(refreshKey);
        set({
          sessionDetail: null,
          sessionDetailLoading: false,
          conversation: null,
          conversationLoading: false,
          sessionClaudeMdStats: null,
          sessionContextStats: null,
          sessionPhaseInfo: null,
        });
        if (tabId) {
          const prev = get().tabSessionData;
          set({ tabSessionData: { ...prev, [tabId]: createEmptyTabSessionData() } });
        }
        return;
      }
      if (isSessionDetailUnchanged(response)) {
        set({ sessionDetailLoading: false, conversationLoading: false });
        if (tabId) {
          const prev = get().tabSessionData;
          set({
            tabSessionData: {
              ...prev,
              [tabId]: {
                ...(prev[tabId] ?? createEmptyTabSessionData()),
                sessionDetailLoading: false,
                conversationLoading: false,
              },
            },
          });
        }
        return;
      }
      const detail = response;
      if (detail.fingerprint) {
        sessionFileFingerprint.set(`${projectId}/${sessionId}`, detail.fingerprint);
      }

      // Transform chunks to conversation
      const isOngoing = detail?.session?.isOngoing ?? false;
      const enhancedChunks = detail ? asEnhancedChunkArray(detail.chunks) : null;
      const conversation: SessionConversation | null =
        detail && enhancedChunks
          ? transformChunksToConversation(enhancedChunks, detail.processes, isOngoing)
          : null;

      // slimDetail: strip raw data no longer needed after conversion to conversation
      const slimDetail = detail
        ? { ...detail, chunks: [] as SessionDetail['chunks'], processes: [] as Process[] }
        : null;

      // Initialize visibleAIGroupId to first AI Group if available
      const firstAIItem = conversation?.items?.find((item) => item.type === 'ai');
      const firstAIGroupId = firstAIItem?.type === 'ai' ? firstAIItem.group.id : null;
      const firstAIGroup = firstAIItem?.type === 'ai' ? firstAIItem.group : null;

      // [F-2] Check stillViewingSession before Phase 1 set() — prevent stale data pollution
      const phase1State = get();
      const activeTab = phase1State.getActiveTab();
      const stillViewingSession =
        phase1State.selectedSessionId === sessionId ||
        (activeTab?.type === 'session' &&
          activeTab.sessionId === sessionId &&
          activeTab.projectId === projectId);
      if (!stillViewingSession) {
        set({
          sessionDetailLoading: false,
          conversationLoading: false
        });
        if (tabId) {
          const prev = get().tabSessionData;
          set({
            tabSessionData: {
              ...prev,
              [tabId]: {
                ...(prev[tabId] ?? createEmptyTabSessionData()),
                sessionDetailLoading: false,
                conversationLoading: false
              }
            }
          });
        }
        return;
      }

      // Update tab label
      const existingTab = findTabBySession(phase1State.openTabs, sessionId);
      if (existingTab && detail) {
        const newLabel = detail.session.firstMessage
          ? truncateLabel(detail.session.firstMessage)
          : `Session ${sessionId.slice(0, 8)}`;
        phase1State.updateTabLabel(existingTab.id, newLabel);
      }

      // ── Immediate render: user sees conversation now ──
      set({
        sessionDetail: slimDetail,
        sessionDetailLoading: false,
        conversation,
        conversationLoading: false,
        visibleAIGroupId: firstAIGroupId,
        selectedAIGroup: firstAIGroup,
        // Phase 2 stats filled later
        sessionClaudeMdStats: null,
        sessionContextStats: null,
        sessionPhaseInfo: null,
      });

      // Per-tab data (Phase 1)
      if (tabId) {
        const prev = get().tabSessionData;
        set({
          tabSessionData: {
            ...prev,
            [tabId]: {
              ...(prev[tabId] ?? createEmptyTabSessionData()),
              sessionDetail: slimDetail,
              conversation,
              conversationLoading: false,
              sessionDetailLoading: false,
              sessionDetailError: null,
              // Phase 2 stats filled later
              sessionClaudeMdStats: null,
              sessionContextStats: null,
              sessionPhaseInfo: null,
              visibleAIGroupId: firstAIGroupId,
              selectedAIGroup: firstAIGroup,
            },
          },
        });
      }

      // [F-3] Auto-expand AI groups immediately after Phase 1, before Phase 2 starts
      if (tabId && conversation?.items && get().appConfig?.general?.autoExpandAIGroups) {
        for (const item of conversation.items) {
          if (item.type === 'ai') {
            get().expandAIGroupForTab(tabId, item.group.id);
          }
        }
      }

      // Fetch agent configs from .claude/agents/ (fire-and-forget)
      const projectRoot = detail?.session?.projectPath ?? '';
      const { connectionMode } = get();
      if (connectionMode !== 'ssh' && projectRoot && projectRoot !== agentConfigsCachedForProject) {
        agentConfigsCachedForProject = projectRoot;
        api
          .readAgentConfigs(projectRoot)
          .then((configs) => {
            set({ agentConfigs: configs });
          })
          .catch((err) => {
            logger.error('Failed to read agent configs:', err);
            agentConfigsCachedForProject = '';
          });
      }

      // ═══════════════════════════════════════
      // PHASE 2: 异步上下文追踪 (fire-and-forget)
      // ═══════════════════════════════════════
      void (async () => {
        try {
          // Skip for SSH mode (same as current behavior)
          if (connectionMode === 'ssh' || !conversation?.items) return;

          // Generation check (1 of 4): after SSH mode check
          if (requestGeneration !== sessionDetailFetchGeneration) return;

          // --- CLAUDE.md token data ---
          let claudeMdTokenData: Record<string, ClaudeMdFileInfo> = {};
          try {
            claudeMdTokenData = await api.readClaudeMdFiles(projectRoot);
          } catch (err) {
            logger.error('Failed to read CLAUDE.md files:', err);
            // [W-4] Don't return on error - continue to mentioned files collection
          }

          // Generation check (2 of 4): after CLAUDE.md files read
          if (requestGeneration !== sessionDetailFetchGeneration) return;

          const claudeMdStats = processSessionClaudeMd(conversation.items, projectRoot, claudeMdTokenData);

          // Directory CLAUDE.md files (using batchAsync instead of Promise.all)
          const directoryTokenData: Record<string, ClaudeMdFileInfo> = {};

          if (claudeMdStats && claudeMdStats.size > 0) {
            const directoryPaths = new Set<string>();
            for (const stats of claudeMdStats.values()) {
              for (const injection of stats.accumulatedInjections) {
                if (injection.source === 'directory') directoryPaths.add(injection.path);
              }
            }

            if (directoryPaths.size > 0) {
              const directoryTokens = new Map<string, number>();
              const nonExistentPaths = new Set<string>();

              const directoryResults = await batchAsync(
                Array.from(directoryPaths),
                async (fullPath) => {
                  try {
                    const dirPath = fullPath.replace(/[\\/]CLAUDE\.md$/, '');
                    const fileInfo = await api.readDirectoryClaudeMd(dirPath);
                    return { fullPath, fileInfo, error: false };
                  } catch (err) {
                    logger.error('Failed to read directory CLAUDE.md:', fullPath, err);
                    return { fullPath, fileInfo: null, error: true };
                  }
                },
                5, // concurrency
              );

              // Generation check (3 of 4): after directory CLAUDE.md files read
              if (requestGeneration !== sessionDetailFetchGeneration) return;

              for (const { fullPath, fileInfo, error } of directoryResults) {
                if (error || !fileInfo) {
                  nonExistentPaths.add(fullPath);
                } else if (fileInfo.exists && fileInfo.estimatedTokens > 0) {
                  directoryTokens.set(fullPath, fileInfo.estimatedTokens);
                  directoryTokenData[fullPath] = fileInfo;
                } else {
                  nonExistentPaths.add(fullPath);
                }
              }

              // Update stats: set real tokens and REMOVE non-existent files
              for (const [, stats] of claudeMdStats.entries()) {
                stats.accumulatedInjections = stats.accumulatedInjections.filter(
                  (inj) => inj.source !== 'directory' || !nonExistentPaths.has(inj.path)
                );
                stats.newInjections = stats.newInjections.filter(
                  (inj) => inj.source !== 'directory' || !nonExistentPaths.has(inj.path)
                );

                for (const injection of stats.accumulatedInjections) {
                  if (injection.source === 'directory' && directoryTokens.has(injection.path)) {
                    injection.estimatedTokens = directoryTokens.get(injection.path)!;
                  }
                }
                for (const injection of stats.newInjections) {
                  if (injection.source === 'directory' && directoryTokens.has(injection.path)) {
                    injection.estimatedTokens = directoryTokens.get(injection.path)!;
                  }
                }

                stats.totalEstimatedTokens = stats.accumulatedInjections.reduce(
                  (sum, inj) => sum + inj.estimatedTokens, 0
                );
                stats.accumulatedCount = stats.accumulatedInjections.length;
                stats.newCount = stats.newInjections.length;
              }
            }
          }

          // Mentioned files (using batchAsync instead of Promise.all)
          const mentionedFilePaths = new Set<string>();
          for (const item of conversation.items) {
            if (item.type === 'user' && item.group.content.fileReferences) {
              for (const ref of item.group.content.fileReferences) {
                const trimmedPath = ref.path?.trim();
                if (!trimmedPath || trimmedPath === '.' || trimmedPath === './') continue;
                const absolutePath = resolveFilePath(projectRoot, trimmedPath);
                if (absolutePath && absolutePath !== projectRoot) mentionedFilePaths.add(absolutePath);
              }
            }
          }

          for (const item of conversation.items) {
            if (item.type === 'ai') {
              for (const msg of item.group.responses) {
                if (msg.type !== 'user') continue;
                let text = '';
                if (typeof msg.content === 'string') {
                  text = msg.content;
                } else if (Array.isArray(msg.content)) {
                  for (const block of msg.content) {
                    if (block.type === 'text' && block.text) text += block.text;
                  }
                }
                if (text) {
                  for (const ref of extractFileReferences(text)) {
                    const trimmedPath = ref.path?.trim();
                    if (!trimmedPath || trimmedPath === '.' || trimmedPath === './') continue;
                    const absolutePath = resolveFilePath(projectRoot, trimmedPath);
                    if (absolutePath && absolutePath !== projectRoot) mentionedFilePaths.add(absolutePath);
                  }
                }
              }
            }
          }

          const mentionedFileTokenData = new Map<string, MentionedFileInfo>();
          const mentionedFileResults = await batchAsync(
            Array.from(mentionedFilePaths),
            async (filePath) => {
              try {
                const fileInfo = await api.readMentionedFile(filePath, projectRoot);
                return { filePath, fileInfo };
              } catch (err) {
                logger.error('Failed to read mentioned file:', filePath, err);
                return { filePath, fileInfo: null };
              }
            },
            5, // concurrency
          );

          // Generation check (4 of 4): final check before updating state
          if (requestGeneration !== sessionDetailFetchGeneration) return;

          for (const { filePath, fileInfo } of mentionedFileResults) {
            if (fileInfo) mentionedFileTokenData.set(filePath, fileInfo);
          }

          // Context processing
          const phaseResult = processSessionContextWithPhases(
            conversation.items, projectRoot, claudeMdTokenData,
            mentionedFileTokenData, directoryTokenData,
          );

          // Update store — only fill stats data
          set({
            sessionClaudeMdStats: claudeMdStats,
            sessionContextStats: phaseResult.statsMap,
            sessionPhaseInfo: phaseResult.phaseInfo,
          });

          // Update per-tab data
          if (tabId) {
            const prev = get().tabSessionData;
            const existing = prev[tabId];
            if (existing) {
              set({
                tabSessionData: {
                  ...prev,
                  [tabId]: {
                    ...existing,
                    sessionClaudeMdStats: claudeMdStats,
                    sessionContextStats: phaseResult.statsMap,
                    sessionPhaseInfo: phaseResult.phaseInfo,
                  },
                },
              });
            }
          }
        } catch (err) {
          // Phase 2 errors must not affect already-rendered UI
          logger.error('Phase 2 context tracking failed:', err);
        }
      })();
    } catch (error) {
      logger.error('fetchSessionDetail error:', error);
      if (requestGeneration !== sessionDetailFetchGeneration) {
        return;
      }
      const errorMsg = error instanceof Error ? error.message : 'Failed to fetch session detail';
      set({
        sessionDetailError: errorMsg,
        sessionDetailLoading: false,
        conversationLoading: false
      });

      // Store per-tab error state
      if (tabId) {
        const prev = get().tabSessionData;
        set({
          tabSessionData: {
            ...prev,
            [tabId]: {
              ...(prev[tabId] ?? createEmptyTabSessionData()),
              sessionDetailError: errorMsg,
              sessionDetailLoading: false,
              conversationLoading: false
            }
          }
        });
      }
    }
  },

  // Refresh session in place without loading states or UI resets
  // Used for real-time file change updates to avoid flickering
  refreshSessionInPlace: async (projectId: string, sessionId: string) => {
    const currentState = get();

    // Check if any tab is viewing this session (across all panes)
    const allTabs = getAllTabs(currentState.paneLayout);
    const tabsViewingSession = allTabs.filter(
      (t) => t.type === 'session' && t.sessionId === sessionId
    );

    // Only refresh if we're actually viewing this session
    if (currentState.selectedSessionId !== sessionId && tabsViewingSession.length === 0) {
      return;
    }

    const refreshKey = `${projectId}/${sessionId}`;

    // Coalesce duplicate in-flight refreshes for the same session.
    if (sessionRefreshInFlight.has(refreshKey)) {
      sessionRefreshQueued.add(refreshKey);
      return;
    }
    const generation = (sessionRefreshGeneration.get(refreshKey) ?? 0) + 1;
    sessionRefreshGeneration.set(refreshKey, generation);
    sessionRefreshInFlight.add(refreshKey);

    try {
      const knownFingerprint = sessionFileFingerprint.get(refreshKey);
      const response = await api.getSessionDetail(projectId, sessionId, knownFingerprint);

      if (sessionRefreshGeneration.get(refreshKey) !== generation) {
        return;
      }

      if (!response) {
        return;
      }

      // Fast path: file unchanged — skip transformation entirely.
      if (isSessionDetailUnchanged(response)) {
        sessionFileFingerprint.set(refreshKey, response.fingerprint);
        return;
      }

      const detail = response;
      if (detail.fingerprint) {
        sessionFileFingerprint.set(refreshKey, detail.fingerprint);
      }

      // Transform chunks to conversation - validate with type guard
      const isOngoing = detail.session?.isOngoing ?? false;
      const enhancedChunks = asEnhancedChunkArray(detail.chunks);
      if (!enhancedChunks) {
        return;
      }

      // ---------------------------------------------------------------
      // Fingerprint check: skip expensive transformation when content is
      // unchanged. Most file-watcher events are duplicates or metadata-only.
      // Includes endTime for Tauri chunks where rawMessages is always [].
      // ---------------------------------------------------------------
      const lastChunk = enhancedChunks[enhancedChunks.length - 1];
      const fingerprint =
        `${enhancedChunks.length}:${lastChunk?.rawMessages?.length ?? 0}` +
        `:${lastChunk?.endTime?.getTime() ?? 0}:${isOngoing}`;
      const prevFingerprint = sessionChunkFingerprint.get(refreshKey);
      if (fingerprint === prevFingerprint) {
        return; // Nothing changed — zero-cost skip
      }
      sessionChunkFingerprint.set(refreshKey, fingerprint);

      // ---------------------------------------------------------------
      // Early release: null out raw data from IPC before transformation
      // to reduce peak memory (detail + enhancedChunks + newConversation).
      // _subagents parameter is unused by the transformer.
      // ---------------------------------------------------------------
      const slimDetail = { ...detail, chunks: [] as EnhancedChunk[], processes: [] as Process[] };

      // Use incremental update when a previous conversation exists —
      // reuses unchanged ChatItem objects, only re-transforms the tail.
      const prevConversation = get().conversation;
      const newConversation =
        prevConversation && prevConversation.items.length > 0
          ? incrementalUpdateConversation(prevConversation, enhancedChunks, [], isOngoing)
          : transformChunksToConversation(enhancedChunks, [], isOngoing);

      if (!newConversation) {
        return;
      }

      const latestState = get();
      const latestAllTabs = getAllTabs(latestState.paneLayout);
      const stillViewingSession =
        latestState.selectedSessionId === sessionId ||
        latestAllTabs.some((tab) => tab.type === 'session' && tab.sessionId === sessionId);
      if (!stillViewingSession) {
        return;
      }

      // Preserve current visibleAIGroupId if it still exists in new conversation
      // Otherwise keep it (it might be scrolled to an item that still exists)
      const currentVisibleId = currentState.visibleAIGroupId;
      const currentSelectedGroup = currentState.selectedAIGroup;

      // Check if current visible group still exists
      const visibleGroupStillExists =
        currentVisibleId &&
        newConversation.items.some(
          (item) => item.type === 'ai' && item.group.id === currentVisibleId
        );

      // Find the updated group if it exists
      let updatedSelectedGroup = currentSelectedGroup;
      if (visibleGroupStillExists && currentVisibleId) {
        const foundItem = newConversation.items.find(
          (item) => item.type === 'ai' && item.group.id === currentVisibleId
        );
        if (foundItem?.type === 'ai') {
          updatedSelectedGroup = foundItem.group;
        }
      }

      // Snapshot existing AI group IDs before overwriting state, so the
      // auto-expand diff below can correctly identify which groups are new.
      const prevGroupIds = new Set(
        (latestState.conversation?.items ?? [])
          .filter((item) => item.type === 'ai')
          .map((item) => (item as { type: 'ai'; group: { id: string } }).group.id)
      );

      // Build per-tab session data before the single merged set() below
      const latestTabSessionData: Record<string, TabSessionData> = { ...get().tabSessionData };
      for (const tab of latestAllTabs) {
        if (tab.type === 'session' && tab.sessionId === sessionId && latestTabSessionData[tab.id]) {
          const tabData = latestTabSessionData[tab.id];
          // Preserve per-tab visibleAIGroupId
          const tabVisibleId = tabData.visibleAIGroupId;
          const tabGroupStillExists =
            tabVisibleId &&
            newConversation.items.some(
              (item) => item.type === 'ai' && item.group.id === tabVisibleId
            );
          let tabSelectedGroup = tabData.selectedAIGroup;
          if (tabGroupStillExists && tabVisibleId) {
            const found = newConversation.items.find(
              (item) => item.type === 'ai' && item.group.id === tabVisibleId
            );
            if (found?.type === 'ai') tabSelectedGroup = found.group;
          }

          latestTabSessionData[tab.id] = {
            ...tabData,
            sessionDetail: slimDetail, // Use slimDetail to avoid holding full chunks/processes in per-tab memory
            conversation: newConversation,
            ...(tabGroupStillExists ? { selectedAIGroup: tabSelectedGroup } : {})
          };
        }
      }

      // Update only the data, preserve UI states (single merged set — was dual set())
      set((state) => ({
        sessionDetail: slimDetail,
        conversation: newConversation,
        // Update on latest sessions state to avoid restoring stale sidebar snapshots.
        sessions: state.sessions.map((s) =>
          s.id === sessionId ? { ...s, isOngoing: detail.session?.isOngoing ?? false } : s
        ),
        // Preserve visible group if it still exists, otherwise keep current
        ...(visibleGroupStillExists
          ? {
              selectedAIGroup: updatedSelectedGroup
            }
          : {}),
        // Note: aiGroupExpansionLevels and expandedStepIds are NOT touched
        // so expansion states are preserved
        tabSessionData: latestTabSessionData,
      }));

      // Auto-expand newly arrived AI groups if the setting is enabled.
      // Uses prevGroupIds snapshotted before set() so the diff is accurate.
      if (get().appConfig?.general?.autoExpandAIGroups) {
        const oldGroupIds = prevGroupIds;
        const newGroupIds = newConversation.items
          .filter(
            (item) =>
              item.type === 'ai' &&
              !oldGroupIds.has((item as { type: 'ai'; group: { id: string } }).group.id)
          )
          .map((item) => (item as { type: 'ai'; group: { id: string } }).group.id);

        if (newGroupIds.length > 0) {
          for (const tab of latestAllTabs) {
            if (tab.type === 'session' && tab.sessionId === sessionId) {
              for (const groupId of newGroupIds) {
                get().expandAIGroupForTab(tab.id, groupId);
              }
            }
          }
        }
      }

    } catch (error) {
      logger.error('refreshSessionInPlace error:', error);
      // Don't set error state - this is a background refresh
    } finally {
      sessionRefreshInFlight.delete(refreshKey);
      // NOTE: Relies on cleanupSessionRefreshCoordination not being invoked between
      // the delete() above and this has() check — both run in the same event-loop tick.
      // Only re-trigger refresh if the key is still tracked (tab not closed mid-refresh)
      if (sessionRefreshQueued.has(refreshKey)) {
        sessionRefreshQueued.delete(refreshKey);
        // Guard: if generation was cleaned up (tab closed), skip the queued refresh
        if (sessionRefreshGeneration.has(refreshKey)) {
          void get().refreshSessionInPlace(projectId, sessionId);
        }
      }
    }
  },

  // Set visible AI Group (called by scroll observer)
  setVisibleAIGroup: (aiGroupId: string | null) => {
    const state = get();

    if (aiGroupId === state.visibleAIGroupId) return;

    // Find the AIGroup in the conversation
    let selectedAIGroup: AIGroup | null = null;
    if (aiGroupId && state.conversation) {
      for (const item of state.conversation.items) {
        if (item.type === 'ai' && item.group.id === aiGroupId) {
          selectedAIGroup = item.group;
          break;
        }
      }
    }

    set({
      visibleAIGroupId: aiGroupId,
      selectedAIGroup
    });
  },

  // Set visible AI Group for a specific tab
  setTabVisibleAIGroup: (tabId: string, aiGroupId: string | null) => {
    const state = get();
    const tabData = state.tabSessionData[tabId];
    if (!tabData) return;

    if (aiGroupId === tabData.visibleAIGroupId) return;

    // Find the AIGroup in the tab's conversation
    let selectedAIGroup: AIGroup | null = null;
    if (aiGroupId && tabData.conversation) {
      for (const item of tabData.conversation.items) {
        if (item.type === 'ai' && item.group.id === aiGroupId) {
          selectedAIGroup = item.group;
          break;
        }
      }
    }

    set({
      tabSessionData: {
        ...state.tabSessionData,
        [tabId]: {
          ...tabData,
          visibleAIGroupId: aiGroupId,
          selectedAIGroup
        }
      }
    });
  },

  // Clean up per-tab session data when tab is closed
  cleanupTabSessionData: (tabId: string) => {
    const prev = get().tabSessionData;
    if (!(tabId in prev)) return;
    const next = { ...prev };
    delete next[tabId];
    set({ tabSessionData: next });
  }
});
