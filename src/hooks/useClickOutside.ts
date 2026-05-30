import { useEffect, useRef } from 'react';

/**
 * Detect clicks outside a referenced element and/or Escape key press.
 * Callers supply a single handler that fires for either event.
 */
export function useClickOutside(
  ref: React.RefObject<HTMLElement | null>,
  onClickOutside: (e: MouseEvent | KeyboardEvent) => void,
  enabled: boolean = true,
) {
  const callbackRef = useRef(onClickOutside);
  callbackRef.current = onClickOutside;

  useEffect(() => {
    if (!enabled) return;

    const handle = (e: MouseEvent | KeyboardEvent) => {
      if (e instanceof MouseEvent) {
        if (ref.current && ref.current.contains(e.target as Node)) return;
      } else {
        if (e.key !== 'Escape') return;
      }
      callbackRef.current(e);
    };

    document.addEventListener('mousedown', handle);
    document.addEventListener('keydown', handle);
    return () => {
      document.removeEventListener('mousedown', handle);
      document.removeEventListener('keydown', handle);
    };
  }, [ref, enabled]);
}
