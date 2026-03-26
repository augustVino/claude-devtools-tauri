import { useCallback } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { isTauriMode } from "@renderer/api";

/**
 * Enables Tauri window dragging on the referenced element.
 *
 * Returns a callback ref that attaches a mousedown listener to the element.
 * In Tauri, it calls `getCurrentWindow().startDragging()`.
 * In Electron, `-webkit-app-region: drag` CSS handles this instead.
 *
 * Interactive children (buttons, inputs, links) are automatically excluded
 * from the drag region. Double-click toggles maximize.
 *
 * Usage — standalone:
 *   const dragRef = useWindowDrag<HTMLDivElement>();
 *   <div ref={dragRef}>...</div>
 *
 * Usage — merged with another ref:
 *   const dragRef = useWindowDrag<HTMLDivElement>();
 *   <div ref={(el) => { otherRef.current = el; dragRef(el); }}>...</div>
 */
export function useWindowDrag<T extends HTMLElement>() {
  const attachDrag = useCallback((el: T | null) => {
    if (!el || !isTauriMode()) return;

    // Apply grab cursor to the drag region
    el.style.cursor = "grab";

    const handleMouseDown = (e: MouseEvent) => {
      if (e.button !== 0) return;

      const target = e.target as HTMLElement;
      if (
        target.closest("button") ||
        target.closest("a") ||
        target.closest("input") ||
        target.closest("select") ||
        target.closest("textarea") ||
        target.closest("[data-no-drag]")
      ) {
        return;
      }

      e.preventDefault();

      if (e.detail === 2) {
        void getCurrentWindow().toggleMaximize();
        return;
      }

      el.style.cursor = "grabbing";
      void getCurrentWindow().startDragging();
    };

    const handleMouseUp = () => {
      el.style.cursor = "grab";
    };

    el.addEventListener("mousedown", handleMouseDown);
    el.addEventListener("mouseup", handleMouseUp);
  }, []);

  return attachDrag;
}
