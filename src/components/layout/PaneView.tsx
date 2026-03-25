/**
 * PaneView - Single pane wrapper with focus management.
 * Handles click-to-focus, visual focus indicator, width,
 * and edge split drop zones for DnD.
 */

import { useDndContext } from '@dnd-kit/core';
import { useStore } from '@renderer/store';
import { MAX_PANES } from '@renderer/types/panes';
import { useShallow } from 'zustand/react/shallow';

import { PaneContent } from './PaneContent';
import { PaneSplitDropZone } from './PaneSplitDropZone';
import { TabBar } from './TabBar';

interface PaneViewProps {
  paneId: string;
}

export const PaneView = ({ paneId }: PaneViewProps): React.JSX.Element => {
  const { pane, isFocused, paneCount, focusPane } = useStore(
    useShallow((s) => ({
      pane: s.paneLayout.panes.find((p) => p.id === paneId),
      isFocused: s.paneLayout.focusedPaneId === paneId,
      paneCount: s.paneLayout.panes.length,
      focusPane: s.focusPane,
    }))
  );

  // Check if a drag is active to show/hide edge drop zones
  const { active } = useDndContext();
  const isDragging = active !== null;
  const canSplit = paneCount < MAX_PANES;
  const showSplitZones = isDragging && canSplit;

  if (!pane) return <div />;

  const handleMouseDown = (): void => {
    if (!isFocused) {
      focusPane(paneId);
    }
  };

  return (
    // eslint-disable-next-line jsx-a11y/no-static-element-interactions -- pane focus management requires mousedown
    <div
      className="relative flex min-w-0 flex-col"
      style={{
        width: `${pane.widthFraction * 100}%`,
      }}
      onMouseDown={handleMouseDown}
    >
      {/* Focus indicator - accent border on top of focused pane's TabBar */}
      <div
        style={{
          borderTop:
            isFocused && paneCount > 1
              ? '2px solid var(--color-accent, #6366f1)'
              : '2px solid transparent',
        }}
      >
        <TabBar paneId={paneId} />
      </div>

      <PaneContent pane={pane} />

      {/* Edge split drop zones - visible only during active drag when under MAX_PANES */}
      <PaneSplitDropZone paneId={paneId} side="left" isActive={showSplitZones} />
      <PaneSplitDropZone paneId={paneId} side="right" isActive={showSplitZones} />

      {/* Max pane indicator - shown during drag when at limit */}
      {isDragging && !canSplit && (
        <div className="pointer-events-none absolute inset-x-0 bottom-3 z-40 flex justify-center">
          <div
            className="rounded-md px-3 py-1.5 text-xs font-medium"
            style={{
              backgroundColor: 'var(--color-surface-overlay)',
              border: '1px solid var(--color-border-emphasis)',
              color: 'var(--color-text-muted)',
            }}
          >
            Maximum {MAX_PANES} panes reached
          </div>
        </div>
      )}
    </div>
  );
};
