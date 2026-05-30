import type { SessionDetailUnchanged } from '@shared/types/api';

export function isSessionDetailUnchanged(
  response: unknown
): response is SessionDetailUnchanged {
  return (
    !!response &&
    typeof response === 'object' &&
    (response as Record<string, unknown>).unchanged === true
  );
}
