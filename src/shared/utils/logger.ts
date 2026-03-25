/**
 * Centralized logging utility for the application.
 *
 * Provides namespace-prefixed logging with environment-based filtering:
 * - Development: All log levels (DEBUG, INFO, WARN, ERROR)
 * - Production: Only ERROR logs are shown
 *
 * Usage:
 * ```typescript
 * import { createLogger } from '@shared/utils/logger';
 * const logger = createLogger('IPC:config');
 * logger.info('Config loaded');
 * logger.error('Failed to load config', error);
 * ```
 */

const LogLevel = {
  DEBUG: 0,
  INFO: 1,
  WARN: 2,
  ERROR: 3,
  NONE: 4,
} as const;

type LogLevelType = (typeof LogLevel)[keyof typeof LogLevel];

let globalLevel: LogLevelType = import.meta.env.PROD ? LogLevel.ERROR : LogLevel.WARN;

function createLogger(namespace: string) {
  return {
    debug: (...args: unknown[]): void => {
      if (globalLevel <= LogLevel.DEBUG) {
        console.debug(`[${namespace}]`, ...args);
      }
    },
    info: (...args: unknown[]): void => {
      if (globalLevel <= LogLevel.INFO) {
        console.log(`[${namespace}]`, ...args);
      }
    },
    warn: (...args: unknown[]): void => {
      if (globalLevel <= LogLevel.WARN) {
        console.warn(`[${namespace}]`, ...args);
      }
    },
    error: (...args: unknown[]): void => {
      if (globalLevel <= LogLevel.ERROR) {
        console.error(`[${namespace}]`, ...args);
      }
    },
  };
}

export { createLogger, LogLevel, globalLevel };
export type { LogLevelType };