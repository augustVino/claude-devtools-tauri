/**
 * Shared session ID validation utilities.
 * Used by both renderer (CommandPalette) and main (IPC/HTTP handlers).
 */

/** Matches a standard v4 UUID (case-insensitive). */
export const UUID_REGEX = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

/** Matches a session ID fragment: starts with hex, followed by 2+ hex-or-dash chars. */
export const SESSION_FRAGMENT_REGEX = /^[0-9a-f][0-9a-f-]{2,}$/i;

/** Minimum length for a valid session ID fragment. */
export const MIN_FRAGMENT_LENGTH = 3;

export function isUUID(value: string): boolean {
  return UUID_REGEX.test(value.trim());
}

/**
 * Detects a session ID fragment: 3+ hex-dash chars that aren't a full UUID.
 */
export function isSessionIdFragment(value: string): boolean {
  const trimmed = value.trim();
  return (
    trimmed.length >= MIN_FRAGMENT_LENGTH &&
    !isUUID(trimmed) &&
    SESSION_FRAGMENT_REGEX.test(trimmed)
  );
}
