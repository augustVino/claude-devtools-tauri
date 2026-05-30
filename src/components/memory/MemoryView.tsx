import { useCallback, useEffect, useMemo, useState } from "react";
import ReactMarkdown from "react-markdown";

import { api } from "@renderer/api";
import { markdownComponents } from "@renderer/components/chat/markdownComponents";
import { useStore } from "@renderer/store";
import { Check, Copy } from "lucide-react";
import remarkGfm from "remark-gfm";
import { useShallow } from "zustand/react/shallow";

import { splitFrontmatter } from "./frontmatter";
import { FrontmatterCard } from "./FrontmatterCard";

import type { Components } from "react-markdown";

interface MemoryViewProps {
  projectId: string;
}

interface ListRow {
  key: string;
  title: string;
  hook: string;
  fileName: string;
  kind: "index" | "linked" | "orphan";
}

const INDEX_FILE = "MEMORY.md";
const WIKILINK_PROTOCOL = "memory:";

// Bounded character class — linear-time, no backtracking.
const WIKILINK_RE = /\[\[([^\]\n]+)\]\]/g;

function transformProse(text: string): string {
  return text.replace(WIKILINK_RE, (match, raw: string) => {
    const slug = raw.trim();
    if (!slug) return match;
    return `[${slug}](${WIKILINK_PROTOCOL}${encodeURIComponent(slug)})`;
  });
}

function preprocessWikilinks(content: string): string {
  const segments: string[] = [];
  let cursor = 0;
  const codeRe = /(```[\s\S]*?```|`[^`\n]*`)/g;
  let m: RegExpExecArray | null;
  while ((m = codeRe.exec(content)) !== null) {
    segments.push(transformProse(content.slice(cursor, m.index)));
    segments.push(m[0]);
    cursor = m.index + m[0].length;
  }
  segments.push(transformProse(content.slice(cursor)));
  return segments.join("");
}

function resolveWikilink(rawSlug: string, rows: ListRow[]): string | null {
  const slug = rawSlug.trim();
  if (!slug) return null;
  const lower = slug.toLowerCase();

  const byExact = rows.find((r) => r.fileName === slug);
  if (byExact) return byExact.fileName;

  const withMd = `${slug}.md`;
  const byWithMd = rows.find((r) => r.fileName === withMd);
  if (byWithMd) return byWithMd.fileName;

  const byTitleCi = rows.find((r) => r.title.toLowerCase() === lower);
  if (byTitleCi) return byTitleCi.fileName;

  const slugified = lower.replace(/\s+/g, "-");
  const bySlugified = rows.find(
    (r) =>
      r.fileName.toLowerCase() === `${slugified}.md` ||
      r.title.toLowerCase().replace(/\s+/g, "-") === slugified,
  );
  if (bySlugified) return bySlugified.fileName;

  return null;
}

export const MemoryView = ({ projectId }: MemoryViewProps) => {
  const { index, hasMemory, fileContentsForProject, loadMemoryForProject, toggleMemoryEntry, expanded } =
    useStore(
      useShallow((s) => {
        const prefix = `${projectId}::`;
        const projectFiles: Record<string, string | undefined> = {};
        for (const [key, value] of Object.entries(s.fileContents)) {
          if (key.startsWith(prefix)) projectFiles[key] = value;
        }
        return {
          index: s.indexByProjectId[projectId] ?? null,
          hasMemory: s.hasMemoryByProjectId[projectId],
          fileContentsForProject: projectFiles,
          loadMemoryForProject: s.loadMemoryForProject,
          toggleMemoryEntry: s.toggleMemoryEntry,
          expanded: s.expandedEntriesByProjectId[projectId] ?? [],
        };
      }),
    );

  const [selectedFile, setSelectedFile] = useState<string | null>(null);
  const [copiedAt, setCopiedAt] = useState<number | null>(null);

  useEffect(() => {
    if (hasMemory === undefined) void loadMemoryForProject(projectId);
  }, [projectId, hasMemory, loadMemoryForProject]);

  const rows: ListRow[] = useMemo(() => {
    if (!index) return [];
    const indexRow: ListRow = {
      key: INDEX_FILE,
      title: "Index",
      hook: "MEMORY.md",
      fileName: INDEX_FILE,
      kind: "index",
    };
    const entryRows: ListRow[] = index.entries.map((e) => ({
      key: e.file,
      title: e.title,
      hook: e.hook,
      fileName: e.file,
      kind: "linked" as const,
    }));
    const orphanRows: ListRow[] = index.orphanFiles.map((f) => ({
      key: f,
      title: f,
      hook: "",
      fileName: f,
      kind: "orphan" as const,
    }));
    return [indexRow, ...entryRows, ...orphanRows];
  }, [index]);

  const displayedFile = useMemo<string | null>(() => {
    if (rows.length === 0) return null;
    if (selectedFile && rows.some((r) => r.fileName === selectedFile)) {
      return selectedFile;
    }
    return rows[0].fileName;
  }, [rows, selectedFile]);

  useEffect(() => {
    if (!displayedFile) return;
    const key = `${projectId}::${displayedFile}`;
    if (fileContentsForProject[key] !== undefined) return;
    if (!expanded.includes(displayedFile)) {
      void toggleMemoryEntry(projectId, displayedFile);
    }
  }, [displayedFile, projectId, fileContentsForProject, expanded, toggleMemoryEntry]);

  const content = displayedFile ? fileContentsForProject[`${projectId}::${displayedFile}`] : undefined;
  const { frontmatter, body } = useMemo(
    () => (content !== undefined ? splitFrontmatter(content) : { frontmatter: null, body: "" }),
    [content],
  );
  const rendered = useMemo(
    () => (content !== undefined ? preprocessWikilinks(body) : undefined),
    [content, body],
  );

  const resolveLayerHref = useCallback(
    (href: string | undefined): string | null => {
      if (!href) return null;
      if (href.startsWith(WIKILINK_PROTOCOL)) {
        return resolveWikilink(decodeURIComponent(href.slice(WIKILINK_PROTOCOL.length)), rows);
      }
      if (/^[a-z]+:/i.test(href)) return null;
      if (href.startsWith("#")) return null;
      const trimmed = href.replace(/^\.\//, "");
      if (trimmed.includes("/") || trimmed.includes("\\")) return null;
      if (!trimmed.toLowerCase().endsWith(".md")) return null;
      return resolveWikilink(trimmed.replace(/\.md$/i, ""), rows);
    },
    [rows],
  );

  const components = useMemo<Components>(
    () => ({
      ...markdownComponents,
      a: ({ href, children, ...rest }) => {
        const target = resolveLayerHref(typeof href === "string" ? href : undefined);
        const isLayerLink =
          typeof href === "string" &&
          (href.startsWith(WIKILINK_PROTOCOL) ||
            (/\.md(?:#.*)?$/i.test(href) && !/^[a-z]+:/i.test(href)));

        if (isLayerLink) {
          const resolved = target !== null;
          return (
            <a
              {...rest}
              href={href}
              onClick={(e) => {
                e.preventDefault();
                e.stopPropagation();
                if (target) setSelectedFile(target);
              }}
              className="cursor-pointer underline decoration-solid underline-offset-2 hover:opacity-80"
              style={{
                color: resolved ? "var(--prose-link)" : "var(--color-text-muted)",
                fontWeight: 500,
              }}
              title={resolved ? `Open ${target}` : "No matching memory layer"}
            >
              {children}
            </a>
          );
        }

        return (
          <a
            {...rest}
            href={href}
            onClick={(e) => {
              if (typeof href !== "string") return;
              e.preventDefault();
              void api.openExternal(href);
            }}
            target="_blank"
            rel="noreferrer noopener"
            className="underline decoration-dotted underline-offset-2"
            style={{ color: "var(--prose-link)" }}
          >
            {children}
          </a>
        );
      },
    }),
    [resolveLayerHref],
  );

  const handleCopy = useCallback(async () => {
    if (content === undefined) return;
    try {
      await navigator.clipboard.writeText(content);
      setCopiedAt(Date.now());
    } catch {
      // Best-effort
    }
  }, [content]);

  useEffect(() => {
    if (copiedAt === null) return;
    const timer = setTimeout(() => setCopiedAt(null), 1500);
    return () => clearTimeout(timer);
  }, [copiedAt]);

  if (!hasMemory) {
    return (
      <div className="flex flex-1 items-center justify-center text-text-muted">
        This project has no memory directory yet.
      </div>
    );
  }

  return (
    <div className="flex flex-1 overflow-hidden">
      {/* Layers list */}
      <div
        className="flex w-64 shrink-0 flex-col border-r"
        style={{
          backgroundColor: "var(--color-surface-sidebar)",
          borderColor: "var(--color-border)",
        }}
      >
        <div className="px-3 pb-1 pt-3 text-[11px] font-semibold uppercase tracking-wider text-text-muted">
          Layers {rows.length > 0 && <span>({rows.length})</span>}
        </div>
        <div className="flex-1 overflow-y-auto pb-2">
          {rows.length === 0 && (
            <div className="px-3 py-2 text-xs text-text-muted">No memory layers yet</div>
          )}
          {rows.map((row) => {
            const isActive = row.fileName === displayedFile;
            return (
              <button
                key={row.key}
                type="button"
                onClick={() => setSelectedFile(row.fileName)}
                className="flex w-full flex-col gap-0.5 px-3 py-2 text-left hover:bg-surface-raised"
                style={{
                  backgroundColor: isActive ? "var(--color-surface-raised)" : undefined,
                }}
              >
                <span
                  className="text-xs font-medium text-text"
                  style={row.kind === "index" ? { color: "var(--color-text-secondary)" } : undefined}
                >
                  {row.title}
                </span>
                {row.hook && (
                  <span className="line-clamp-2 text-[11px] text-text-muted">{row.hook}</span>
                )}
                {row.kind === "orphan" && (
                  <span className="text-[10px] uppercase tracking-wider text-text-muted">
                    Unlinked
                  </span>
                )}
              </button>
            );
          })}
        </div>
      </div>

      {/* Content viewer */}
      <div className="flex flex-1 flex-col overflow-hidden">
        <div
          className="flex items-center justify-end gap-2 border-b px-4 py-2"
          style={{ borderColor: "var(--color-border)" }}
        >
          {displayedFile && (
            <button
              type="button"
              aria-label={copiedAt === null ? "Copy content" : "Copied"}
              onClick={() => void handleCopy()}
              disabled={content === undefined}
              className="flex items-center gap-1.5 rounded-md border px-2 py-1 text-xs text-text hover:bg-surface-raised disabled:opacity-40"
              style={{
                backgroundColor: "var(--color-surface-overlay)",
                borderColor: "var(--color-border-emphasis)",
              }}
            >
              {copiedAt === null ? (
                <>
                  <Copy size={14} className="text-text-secondary" aria-hidden="true" />
                  <span>Copy</span>
                </>
              ) : (
                <>
                  <Check size={14} className="text-text-secondary" aria-hidden="true" />
                  <span>Copied</span>
                </>
              )}
            </button>
          )}
        </div>
        <div className="flex-1 overflow-y-auto px-6 py-4">
          {displayedFile === null ? (
            <div className="text-text-muted">Select a layer to view its content.</div>
          ) : rendered === undefined ? (
            <div className="text-text-muted">Loading…</div>
          ) : (
            <div className="max-w-3xl">
              {frontmatter && <FrontmatterCard frontmatter={frontmatter} />}
              <div className="prose-sm" style={{ color: "var(--prose-body)" }}>
                <ReactMarkdown
                  remarkPlugins={[remarkGfm]}
                  components={components}
                  urlTransform={(url) => {
                    if (!url) return "";
                    if (url.startsWith("memory:") || url.startsWith("#") || url.startsWith("/") || url.startsWith("./")) return url;
                    if (/^https?:\/\//i.test(url)) return url;
                    return "";
                  }}
                >
                  {rendered}
                </ReactMarkdown>
              </div>
            </div>
          )}
        </div>
      </div>
    </div>
  );
};
