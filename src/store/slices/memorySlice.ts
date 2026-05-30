import { api } from "@renderer/api";
import type { MemoryIndex } from "@shared/types/api";
import type { AppState } from "../types";
import type { StateCreator } from "zustand";

function fileCacheKey(projectId: string, fileName: string): string {
  return `${projectId}::${fileName}`;
}

export interface MemorySlice {
  hasMemoryByProjectId: Record<string, boolean | undefined>;
  indexByProjectId: Record<string, MemoryIndex | null | undefined>;
  expandedEntriesByProjectId: Record<string, string[] | undefined>;
  fileContents: Record<string, string | undefined>;
  memoryLoadingByProjectId: Record<string, boolean | undefined>;
  memoryError: string | null;

  loadMemoryForProject: (projectId: string) => Promise<void>;
  toggleMemoryEntry: (projectId: string, fileName: string) => Promise<void>;
  refreshMemoryForProject: (projectId: string) => Promise<void>;
  openMemoryTab: (projectId: string) => void;
}

export const createMemorySlice: StateCreator<AppState, [], [], MemorySlice> = (
  set,
  get,
) => ({
  hasMemoryByProjectId: {},
  indexByProjectId: {},
  expandedEntriesByProjectId: {},
  fileContents: {},
  memoryLoadingByProjectId: {},
  memoryError: null,

  loadMemoryForProject: async (projectId: string) => {
    if (!projectId) return;
    if (get().memoryLoadingByProjectId[projectId]) return;
    set((state) => ({
      memoryLoadingByProjectId: {
        ...state.memoryLoadingByProjectId,
        [projectId]: true,
      },
      memoryError: null,
    }));
    try {
      const has = await api.memory.hasMemory(projectId);
      let index: MemoryIndex | null = null;
      if (has) {
        index = await api.memory.getIndex(projectId);
      }
      set((state) => ({
        hasMemoryByProjectId: {
          ...state.hasMemoryByProjectId,
          [projectId]: has,
        },
        indexByProjectId: { ...state.indexByProjectId, [projectId]: index },
        memoryLoadingByProjectId: {
          ...state.memoryLoadingByProjectId,
          [projectId]: false,
        },
      }));
    } catch (error) {
      set((state) => ({
        memoryError:
          error instanceof Error ? error.message : "Failed to load memory",
        memoryLoadingByProjectId: {
          ...state.memoryLoadingByProjectId,
          [projectId]: false,
        },
      }));
    }
  },

  toggleMemoryEntry: async (projectId: string, fileName: string) => {
    const state = get();
    const expanded = state.expandedEntriesByProjectId[projectId] ?? [];
    const isOpen = expanded.includes(fileName);

    if (isOpen) {
      set((s) => ({
        expandedEntriesByProjectId: {
          ...s.expandedEntriesByProjectId,
          [projectId]: expanded.filter((f) => f !== fileName),
        },
      }));
      return;
    }

    set((s) => ({
      expandedEntriesByProjectId: {
        ...s.expandedEntriesByProjectId,
        [projectId]: [...expanded, fileName],
      },
    }));

    const cacheKey = fileCacheKey(projectId, fileName);
    if (state.fileContents[cacheKey] !== undefined) return;

    try {
      const result = await api.memory.readFile(projectId, fileName);
      if (result.success) {
        set((s) => ({
          fileContents: { ...s.fileContents, [cacheKey]: result.content },
        }));
      } else {
        set((s) => ({
          fileContents: {
            ...s.fileContents,
            [cacheKey]: `> Failed to read ${fileName}: ${result.error}`,
          },
        }));
      }
    } catch (error) {
      const message =
        error instanceof Error ? error.message : String(error);
      set((s) => ({
        fileContents: {
          ...s.fileContents,
          [cacheKey]: `> Failed to read ${fileName}: ${message}`,
        },
      }));
    }
  },

  openMemoryTab: (projectId: string) => {
    if (!projectId) return;
    const state = get();
    for (const pane of state.paneLayout.panes) {
      const existing = pane.tabs.find(
        (t) => t.type === "memory" && t.projectId === projectId,
      );
      if (existing) {
        state.setActiveTab(existing.id);
        return;
      }
    }
    state.openTab({
      type: "memory",
      projectId,
      label: "Memory",
    });
    // Trigger loading immediately so the tab doesn't briefly show "no memory"
    void state.loadMemoryForProject(projectId);
  },

  refreshMemoryForProject: async (projectId: string) => {
    if (!projectId) return;
    set((state) => {
      const next: Record<string, string | undefined> = {};
      const prefix = `${projectId}::`;
      for (const [key, value] of Object.entries(state.fileContents)) {
        if (!key.startsWith(prefix)) next[key] = value;
      }
      return {
        fileContents: next,
        expandedEntriesByProjectId: {
          ...state.expandedEntriesByProjectId,
          [projectId]: [],
        },
      };
    });
    await get().loadMemoryForProject(projectId);
  },
});
