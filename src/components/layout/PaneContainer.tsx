/**
 * PaneContainer - Horizontal flex container that renders panes side by side.
 * Wraps children with @dnd-kit DndContext provider for tab drag-and-drop.
 *
 * DnD interactions:
 * - Drag within same TabBar → reorder tabs (reorderTabInPane)
 * - Drag to another pane's TabBar → move tab to target pane (moveTabToPane)
 * - Drag to pane edge zone → create new split pane (moveTabToNewPane)
 * - Drag last tab out of pane → source pane auto-closes
 */

import { Fragment, useCallback, useState } from 'react';

import {
  DndContext,
  DragOverlay,
  PointerSensor,
  pointerWithin,
  useSensor,
  useSensors,
} from '@dnd-kit/core';
import { useStore } from '@renderer/store';

import { PaneResizeHandle } from './PaneResizeHandle';
import { PaneView } from './PaneView';
import { DragOverlayTab } from './SortableTab';

import type { DragEndEvent, DragStartEvent } from '@dnd-kit/core';
import type { Tab } from '@renderer/types/tabs';

export const PaneContainer = (): React.JSX.Element => {
  const panes = useStore((s) => s.paneLayout.panes);

  // Track the currently dragged tab for DragOverlay
  const [activeTab, setActiveTab] = useState<Tab | null>(null);

  // Configure pointer sensor with activation distance to avoid conflict with clicks
  const sensors = useSensors(
    useSensor(PointerSensor, {
      activationConstraint: {
        distance: 8, // 8px drag distance before activating
      },
    })
  );

  const handleDragStart = useCallback(
    (event: DragStartEvent) => {
      const { active } = event;
      const data = active.data.current;

      if (data?.type === 'tab') {
        const sourcePaneId = data.paneId as string;
        const tabId = data.tabId as string;

        // Find the tab in the source pane
        const pane = panes.find((p) => p.id === sourcePaneId);
        const tab = pane?.tabs.find((t) => t.id === tabId);
        if (tab) {
          setActiveTab(tab);
        }
      }
    },
    [panes]
  );

  const handleDragEnd = useCallback(
    (event: DragEndEvent) => {
      const { active, over } = event;

      setActiveTab(null);

      if (!over || !active.data.current) return;

      const activeData = active.data.current;
      const overData = over.data.current;

      if (activeData.type !== 'tab') return;

      const draggedTabId = activeData.tabId as string;
      const sourcePaneId = activeData.paneId as string;
      const state = useStore.getState();

      // Case 1: Drop on a split-zone (edge of pane) → create new pane
      if (overData?.type === 'split-zone') {
        const targetPaneId = overData.paneId as string;
        const side = overData.side as 'left' | 'right';
        state.moveTabToNewPane(draggedTabId, sourcePaneId, targetPaneId, side);
        return;
      }

      // Case 2: Drop on a tabbar (different pane) → move tab to that pane
      if (overData?.type === 'tabbar') {
        const targetPaneId = overData.paneId as string;
        if (sourcePaneId !== targetPaneId) {
          state.moveTabToPane(draggedTabId, sourcePaneId, targetPaneId);
        }
        return;
      }

      // Case 3: Drop on another sortable tab
      // This can mean either reorder within same pane or move to another pane's tab position
      if (overData?.type === 'tab') {
        const overTabId = overData.tabId as string;
        const overPaneId = overData.paneId as string;

        if (sourcePaneId === overPaneId) {
          // Reorder within the same pane
          const pane = panes.find((p) => p.id === sourcePaneId);
          if (!pane) return;

          const fromIndex = pane.tabs.findIndex((t) => t.id === draggedTabId);
          const toIndex = pane.tabs.findIndex((t) => t.id === overTabId);

          if (fromIndex !== -1 && toIndex !== -1 && fromIndex !== toIndex) {
            state.reorderTabInPane(sourcePaneId, fromIndex, toIndex);
          }
        } else {
          // Move to another pane, inserting at the over tab's position
          const targetPane = panes.find((p) => p.id === overPaneId);
          if (!targetPane) return;

          const insertIndex = targetPane.tabs.findIndex((t) => t.id === overTabId);
          state.moveTabToPane(draggedTabId, sourcePaneId, overPaneId, insertIndex);
        }
      }
    },
    [panes]
  );

  return (
    <DndContext
      sensors={sensors}
      collisionDetection={pointerWithin}
      onDragStart={handleDragStart}
      onDragEnd={handleDragEnd}
    >
      <div id="pane-container" className="flex flex-1 overflow-hidden">
        {panes.map((pane, i) => (
          <Fragment key={pane.id}>
            {i > 0 && <PaneResizeHandle leftPaneId={panes[i - 1].id} rightPaneId={pane.id} />}
            <PaneView paneId={pane.id} />
          </Fragment>
        ))}
      </div>

      {/* Drag overlay - semi-transparent ghost of the dragged tab */}
      <DragOverlay dropAnimation={null}>
        {activeTab ? <DragOverlayTab tab={activeTab} /> : null}
      </DragOverlay>
    </DndContext>
  );
};
