import { useEffect, useRef } from 'react';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { isTauriMode } from '@renderer/api';

/**
 * Enables Tauri window dragging on the referenced element.
 *
 * Attach this ref to the drag-region container div. In Tauri, it adds
 * a mousedown listener that calls `getCurrentWindow().startDragging()`.
 * In Electron, `-webkit-app-region: drag` CSS handles this instead.
 *
 * Interactive children (buttons, inputs, links) are automatically excluded
 * from the drag region. Double-click toggles maximize.
 */
export function useWindowDrag<T extends HTMLElement>() {
  const ref = useRef<T | null>(null);

  useEffect(() => {
    const el = ref.current;
    if (!el || !isTauriMode()) return;

    const handleMouseDown = (e: MouseEvent) => {
      if (e.button !== 0) return;

      const target = e.target as HTMLElement;
      if (
        target.closest('button') ||
        target.closest('a') ||
        target.closest('input') ||
        target.closest('select') ||
        target.closest('textarea') ||
        target.closest('[data-no-drag]')
      ) {
        return;
      }

      if (e.detail === 2) {
        void getCurrentWindow().toggleMaximize();
        return;
      }

      void getCurrentWindow().startDragging();
    };

    el.addEventListener('mousedown', handleMouseDown);
    return () => el.removeEventListener('mousedown', handleMouseDown);
  }, []);

  return ref;
}
