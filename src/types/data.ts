/**
 * Type definitions for the renderer process.
 *
 * This module re-exports types from the main process types and adds
 * renderer-specific types and utilities. For most uses, import from
 * the index.ts barrel file instead.
 *
 * Import hierarchy:
 * - Main types: Domain models, JSONL format, parsed messages, chunks
 * - Renderer types: API interfaces, notifications, visualization
 */

// =============================================================================
// Re-exports from Main Process Types
// =============================================================================

// Domain types
export type {
  PhaseTokenBreakdown,
  Project,
  RepositoryGroup,
  SearchResult,
  Session,
  SessionMetrics,
  Worktree,
  WorktreeSource,
} from "@shared/types";

// Message types
export type { ParsedMessage } from "@shared/types";

// Chunk types
export type {
  Chunk,
  EnhancedAIChunk,
  EnhancedChunk,
  EnhancedCompactChunk,
  EnhancedSystemChunk,
  EnhancedUserChunk,
  Process,
  SemanticStep,
  SessionDetail,
  SubagentDetail,
} from "@shared/types";

// Chunk type guards
export { isEnhancedAIChunk } from "@shared/types";

// JSONL types (for components that need content block types)
export type { ToolUseResultData } from "@shared/types";

// =============================================================================
// Re-exports from Renderer-Specific Types
// =============================================================================

// API types
export type { ClaudeMdFileInfo } from "./api";

// Notification types
export type {
  AppConfig,
  DetectedError,
  NotificationTrigger,
  TriggerContentType,
  TriggerMatchField,
  TriggerMode,
  TriggerTestResult,
  TriggerTokenType,
  TriggerToolName,
} from "./notifications";

// =============================================================================
// Session Sort Mode
// =============================================================================

/** Sort mode for session list in sidebar */
export type SessionSortMode = "recent" | "most-context";

// =============================================================================
// Renderer-Specific Type Guards
// =============================================================================

import type {
  Chunk,
  ConversationGroup,
  EnhancedChunk,
  EnhancedCompactChunk,
  EnhancedSystemChunk,
  EnhancedUserChunk,
  ParsedMessage,
  Process,
  TaskExecution,
  ToolExecution,
} from "@shared/types";
import type { WaterfallData, WaterfallItem } from "@shared/types/visualization";

/**
 * Type guard: Check if message is an assistant message.
 */
export function isAssistantMessage(msg: ParsedMessage): boolean {
  return msg.type === "assistant";
}

/**
 * Type guard to check if a chunk is an EnhancedUserChunk.
 */
export function isEnhancedUserChunk(
  chunk: Chunk | EnhancedChunk,
): chunk is EnhancedUserChunk {
  return (
    "chunkType" in chunk && chunk.chunkType === "user" && "rawMessages" in chunk
  );
}

/**
 * Type guard to check if a chunk is an EnhancedSystemChunk.
 */
export function isEnhancedSystemChunk(
  chunk: Chunk | EnhancedChunk,
): chunk is EnhancedSystemChunk {
  return (
    "chunkType" in chunk &&
    chunk.chunkType === "system" &&
    "rawMessages" in chunk
  );
}

/**
 * Type guard to check if a chunk is an EnhancedCompactChunk.
 */
export function isEnhancedCompactChunk(
  chunk: Chunk | EnhancedChunk,
): chunk is EnhancedCompactChunk {
  return (
    "chunkType" in chunk &&
    chunk.chunkType === "compact" &&
    "rawMessages" in chunk
  );
}

/**
 * Type guard to check if a single chunk is an EnhancedChunk.
 * Enhanced chunks have 'chunkType' and 'rawMessages' properties.
 * Plain chunks (from Tauri backend) have 'chunkType' but no 'rawMessages'.
 */
function isEnhancedChunk(chunk: Chunk | EnhancedChunk): chunk is EnhancedChunk {
  return "chunkType" in chunk && "rawMessages" in chunk;
}

/**
 * Type guard to check if an array of chunks are all EnhancedChunks.
 * Returns the array typed as EnhancedChunk[] if valid.
 * For plain chunks from Tauri backend (without rawMessages/semanticSteps),
 * augments them with empty arrays so the conversation transformer works.
 */
export function asEnhancedChunkArray(chunks: Chunk[]): EnhancedChunk[] | null {
  if (chunks.length === 0) {
    return [];
  }
  // Check first chunk - if it has enhanced properties, assume all do
  // (they come from the same builder)
  if (isEnhancedChunk(chunks[0])) {
    return chunks as EnhancedChunk[];
  }
  // Plain chunks from Tauri backend — add missing enhanced fields
  const enhanced = chunks.map((chunk) => {
    const base = { ...chunk };
    if (!("rawMessages" in chunk)) {
      (base as Record<string, unknown>).rawMessages = [];
    }
    if (chunk.chunkType === "ai" && !("semanticSteps" in chunk)) {
      (base as Record<string, unknown>).semanticSteps = [];
      (base as Record<string, unknown>).semanticStepGroups = [];
    }
    // Tauri backend sends millisecond timestamps as numbers — convert to Date objects
    if (typeof base.startTime === "number") {
      base.startTime = new Date(
        base.startTime,
      ) as unknown as EnhancedChunk["startTime"];
    }
    if (typeof base.endTime === "number") {
      base.endTime = new Date(
        base.endTime,
      ) as unknown as EnhancedChunk["endTime"];
    }
    // SemanticStep times arrive as ISO strings from Rust — convert to Date objects
    if (chunk.chunkType === "ai") {
      const record = base as Record<string, unknown>;
      if (
        Array.isArray(record.semanticSteps) &&
        record.semanticSteps.length > 0
      ) {
        record.semanticSteps = (
          record.semanticSteps as Array<Record<string, unknown>>
        ).map((step) => ({
          ...step,
          startTime:
            typeof step.startTime === "string"
              ? new Date(step.startTime)
              : step.startTime,
          endTime:
            step.endTime != null && typeof step.endTime === "string"
              ? new Date(step.endTime)
              : step.endTime,
          effectiveEndTime:
            step.effectiveEndTime != null &&
            typeof step.effectiveEndTime === "string"
              ? new Date(step.effectiveEndTime)
              : step.effectiveEndTime,
        }));
      }
      // ToolExecution times also arrive as ISO strings — convert to Date objects
      if (Array.isArray(record.toolExecutions)) {
        record.toolExecutions = (
          record.toolExecutions as Array<Record<string, unknown>>
        ).map((te) => ({
          ...te,
          startTime:
            typeof te.startTime === "string"
              ? new Date(te.startTime)
              : te.startTime,
          endTime:
            te.endTime != null && typeof te.endTime === "string"
              ? new Date(te.endTime)
              : te.endTime,
        }));
      }
    }
    return base as EnhancedChunk;
  });
  return enhanced;
}

// =============================================================================
// Tauri Backend Adapter Types
// =============================================================================

/**
 * Raw WaterfallItem as received from the Rust backend.
 * Fields use number timestamps (u64 milliseconds) and `itemType` instead of `type`.
 * No `tokenUsage` or `groupId` fields — the Rust struct does not include them.
 */
export interface WaterfallItemRust {
  id: string;
  label: string;
  startTime: number;
  endTime: number;
  durationMs: number;
  level: number;
  itemType: string;
  parentId?: string;
  isParallel?: boolean;
  metadata?: {
    subagentType?: string;
    toolName?: string;
    messageCount?: number;
  };
}

/**
 * Raw WaterfallData as received from the Rust backend.
 * Uses number timestamps (u64 milliseconds) for minTime/maxTime.
 */
export interface WaterfallDataRust {
  items: WaterfallItemRust[];
  minTime: number;
  maxTime: number;
  totalDurationMs: number;
}

/**
 * Empty TokenUsage default for waterfall items (Rust backend does not include it).
 */
const EMPTY_TOKEN_USAGE = {
  input_tokens: 0,
  output_tokens: 0,
};

/**
 * Convert a raw WaterfallItemRust from the Tauri backend to the frontend WaterfallItem.
 * - Number timestamps → Date objects
 * - `itemType` → `type`
 * - `isParallel` Option → boolean (default false)
 * - Adds empty `tokenUsage` (not provided by Rust)
 */
function adaptWaterfallItem(item: WaterfallItemRust): WaterfallItem {
  return {
    id: item.id,
    label: item.label,
    startTime: new Date(item.startTime),
    endTime: new Date(item.endTime),
    durationMs: item.durationMs,
    tokenUsage: { ...EMPTY_TOKEN_USAGE },
    level: item.level,
    type: item.itemType as WaterfallItem["type"],
    isParallel: item.isParallel ?? false,
    parentId: item.parentId,
    groupId: undefined,
    metadata: item.metadata,
  };
}

/**
 * Convert raw WaterfallData from the Tauri backend to the frontend WaterfallData.
 * - Number timestamps → Date objects for minTime/maxTime and all items
 */
export function adaptWaterfallData(data: WaterfallDataRust): WaterfallData {
  return {
    items: data.items.map(adaptWaterfallItem),
    minTime: new Date(data.minTime),
    maxTime: new Date(data.maxTime),
    totalDurationMs: data.totalDurationMs,
  };
}

// =============================================================================
// ConversationGroup Adapter
// =============================================================================

/**
 * Convert a number (milliseconds since epoch) or ISO string to a Date.
 * Returns the value as-is if it is already a Date.
 */
function toTimestamp(value: unknown): Date | undefined {
  if (value instanceof Date) return value;
  if (typeof value === "number") return new Date(value);
  if (typeof value === "string" && value.length > 0) return new Date(value);
  return undefined;
}

/**
 * Adapt a raw Process object from the Rust backend.
 * Rust sends startTime/endTime as u64 milliseconds.
 */
function adaptProcess(process: Record<string, unknown>): Process {
  return {
    ...process,
    startTime: toTimestamp(process.startTime) ?? new Date(0),
    endTime: toTimestamp(process.endTime) ?? new Date(0),
  } as unknown as Process;
}

/**
 * Adapt a raw ToolExecution from the Rust backend.
 * Rust sends startTime as ISO string, endTime as optional ISO string.
 */
function adaptToolExecution(te: Record<string, unknown>): ToolExecution {
  return {
    ...te,
    startTime: toTimestamp(te.startTime) ?? new Date(0),
    endTime: toTimestamp(te.endTime),
  } as unknown as ToolExecution;
}

/**
 * Adapt a raw TaskExecution from the Rust backend.
 * Rust sends taskCallTimestamp/resultTimestamp as f64 milliseconds.
 */
function adaptTaskExecution(taskExec: Record<string, unknown>): TaskExecution {
  const adapted = { ...taskExec };
  adapted.taskCallTimestamp =
    toTimestamp(taskExec.taskCallTimestamp) ?? new Date(0);
  adapted.resultTimestamp =
    toTimestamp(taskExec.resultTimestamp) ?? new Date(0);
  // Adapt nested subagent Process
  if (taskExec.subagent && typeof taskExec.subagent === "object") {
    adapted.subagent = adaptProcess(
      taskExec.subagent as Record<string, unknown>,
    );
  }
  return adapted as unknown as TaskExecution;
}

/**
 * Convert a raw ConversationGroup from the Tauri backend to the frontend type.
 * - Top-level startTime/endTime (f64 ms) → Date
 * - Nested Process timestamps (u64 ms) → Date
 * - Nested ToolExecution timestamps (ISO strings) → Date
 * - Nested TaskExecution timestamps (f64 ms) → Date
 */
export function adaptConversationGroup(
  group: Record<string, unknown>,
): ConversationGroup {
  const adapted = { ...group };
  adapted.startTime = toTimestamp(group.startTime) ?? new Date(0);
  adapted.endTime = toTimestamp(group.endTime) ?? new Date(0);

  // Adapt nested Process objects
  if (Array.isArray(group.processes)) {
    adapted.processes = (group.processes as Record<string, unknown>[]).map(
      adaptProcess,
    );
  }

  // Adapt nested ToolExecution objects
  if (Array.isArray(group.toolExecutions)) {
    adapted.toolExecutions = (
      group.toolExecutions as Record<string, unknown>[]
    ).map(adaptToolExecution);
  }

  // Adapt nested TaskExecution objects
  if (Array.isArray(group.taskExecutions)) {
    adapted.taskExecutions = (
      group.taskExecutions as Record<string, unknown>[]
    ).map(adaptTaskExecution);
  }

  return adapted as unknown as ConversationGroup;
}
