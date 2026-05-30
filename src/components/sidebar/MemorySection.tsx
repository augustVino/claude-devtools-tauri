import { useEffect } from "react";
import { useStore } from "@renderer/store";
import { Brain } from "lucide-react";
import { useShallow } from "zustand/react/shallow";

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
    <div
      className="flex w-full items-center gap-1 border-b px-3 py-2 text-[11px] font-semibold uppercase tracking-wider text-text-muted"
      style={{ borderColor: "var(--color-border)" }}
    >
      <button
        type="button"
        onClick={() => openMemoryTab(selectedProjectId)}
        className="flex flex-1 items-center gap-1.5 text-left hover:text-text-secondary"
      >
        <Brain size={13} className="shrink-0" aria-hidden="true" />
        <span>Memory</span>
        {indexEntryCount > 0 && <span className="text-text-muted">({indexEntryCount})</span>}
      </button>
    </div>
  );
};
