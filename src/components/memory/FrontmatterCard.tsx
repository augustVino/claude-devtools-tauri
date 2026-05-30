import type { MemoryFrontmatter } from "./frontmatter";

interface FrontmatterCardProps {
  frontmatter: MemoryFrontmatter;
}

const ROW_LABEL_CLASS = "shrink-0 text-[10px] uppercase tracking-wider text-text-muted";

export const FrontmatterCard = ({ frontmatter }: FrontmatterCardProps) => {
  const metadataEntries = Object.entries(frontmatter.metadata);
  return (
    <div
      className="mb-4 rounded-md border px-3 py-2 text-xs"
      style={{
        backgroundColor: "var(--color-surface-overlay)",
        borderColor: "var(--color-border)",
        color: "var(--prose-body)",
      }}
    >
      {frontmatter.name && (
        <div className="flex items-baseline gap-2">
          <span className={ROW_LABEL_CLASS} style={{ minWidth: "5rem" }}>
            name
          </span>
          <span className="font-mono text-[11px] text-text">{frontmatter.name}</span>
        </div>
      )}
      {frontmatter.description && (
        <div className="mt-1 flex items-baseline gap-2">
          <span className={ROW_LABEL_CLASS} style={{ minWidth: "5rem" }}>
            description
          </span>
          <span className="text-text-secondary">{frontmatter.description}</span>
        </div>
      )}
      {metadataEntries.length > 0 && (
        <div
          className="mt-2 flex flex-wrap gap-x-3 gap-y-1 border-t pt-2"
          style={{ borderColor: "var(--color-border)" }}
        >
          {metadataEntries.map(([key, value]) => (
            <span key={key} className="inline-flex items-baseline gap-1">
              <span className="text-[10px] uppercase tracking-wider text-text-muted">{key}</span>
              <span className="font-mono text-[11px] text-text-secondary">{value}</span>
            </span>
          ))}
        </div>
      )}
    </div>
  );
};
