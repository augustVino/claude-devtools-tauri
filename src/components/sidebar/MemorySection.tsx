import { useEffect } from "react";
import { useStore } from "@renderer/store";
import { Brain } from "lucide-react";
import { useShallow } from "zustand/react/shallow";

import { OpenInMenu } from "./memory/OpenInMenu";

export const MemorySection = () => {
  const { selectedProjectId, hasMemory, indexEntryCount, loading, memoryError, loadMemoryForProject, openMemoryTab } =
    useStore(
      useShallow((s) => {
        const projectId = s.selectedProjectId;
        const index = projectId ? s.indexByProjectId[projectId] : null;
        const entryCount = (index?.entries.length ?? 0) + (index?.orphanFiles.length ?? 0);
        return {
          selectedProjectId: projectId,
          hasMemory: projectId ? s.hasMemoryByProjectId[projectId] : undefined,
          indexEntryCount: entryCount,
          loading: projectId ? (s.memoryLoadingByProjectId[projectId] ?? false) : false,
          memoryError: projectId ? s.memoryError : null,
          loadMemoryForProject: s.loadMemoryForProject,
          openMemoryTab: s.openMemoryTab,
        };
      }),
    );

  useEffect(() => {
    if (!selectedProjectId) return;
    if (hasMemory === undefined) void loadMemoryForProject(selectedProjectId);
  }, [selectedProjectId, hasMemory, loadMemoryForProject]);

  if (!selectedProjectId) return null;
  if (hasMemory === undefined && loading) return null;
  if (hasMemory === undefined && memoryError) return null;
  if (!hasMemory) return null;

  return (
    <div className="flex items-center justify-between px-4 py-3" style={{ marginTop: 8 }}>
      <div className="flex items-center gap-2">
        <Brain className="size-4" style={{ color: 'var(--color-text-muted)' }} />
        <button
          type="button"
          onClick={() => openMemoryTab(selectedProjectId)}
          className="text-xs uppercase tracking-wider text-left"
          style={{ color: 'var(--color-text-muted)' }}
        >
          Memory
          {indexEntryCount > 0 && (
            <span style={{ opacity: 0.6 }}> ({indexEntryCount})</span>
          )}
        </button>
      </div>
      <OpenInMenu projectId={selectedProjectId} fileName={null} variant="dots" />
    </div>
  );
};
