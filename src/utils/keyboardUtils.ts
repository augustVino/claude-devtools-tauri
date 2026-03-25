/**
 * Keyboard utility functions for platform-aware shortcuts
 */

/**
 * Detect if running on macOS
 */
export function isMacOS(): boolean {
  return navigator.userAgent.toLowerCase().includes('mac');
}

/**
 * Get the primary modifier key name for the current platform
 * @returns 'Cmd' on macOS, 'Ctrl' on other platforms
 */
export function getModifierKeyName(): string {
  return isMacOS() ? 'Cmd' : 'Ctrl';
}

/**
 * Get the primary modifier key symbol for the current platform
 * @returns '⌘' on macOS, 'Ctrl' on other platforms
 */
export function getModifierKeySymbol(): string {
  return isMacOS() ? '⌘' : 'Ctrl';
}

/**
 * Format a keyboard shortcut for display
 * @param key - The key to press (e.g., 'K', 'G', 'Enter')
 * @param useSymbol - Whether to use symbols (⌘) or text (Cmd)
 * @returns Formatted shortcut string (e.g., '⌘K' or 'Ctrl+K')
 */
export function formatModifierShortcut(key: string, useSymbol = true): string {
  const modifier = useSymbol ? getModifierKeySymbol() : getModifierKeyName();
  const separator = useSymbol && isMacOS() ? '' : '+';
  return `${modifier}${separator}${key}`;
}
