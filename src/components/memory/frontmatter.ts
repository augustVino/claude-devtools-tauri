export interface MemoryFrontmatter {
  name?: string;
  description?: string;
  metadata: Record<string, string>;
  raw: string;
}

export interface FrontmatterSplit {
  frontmatter: MemoryFrontmatter | null;
  body: string;
}

const FRONTMATTER_RE = /^---\r?\n([\s\S]*?)\r?\n---\r?\n?/;

function unquote(value: string): string {
  const trimmed = value.trim();
  if (
    (trimmed.startsWith('"') && trimmed.endsWith('"')) ||
    (trimmed.startsWith("'") && trimmed.endsWith("'"))
  ) {
    return trimmed.slice(1, -1);
  }
  return trimmed;
}

export function splitFrontmatter(content: string): FrontmatterSplit {
  const match = FRONTMATTER_RE.exec(content);
  if (!match) return { frontmatter: null, body: content };

  const raw = match[1];
  const body = content.slice(match[0].length);

  const frontmatter: MemoryFrontmatter = { metadata: {}, raw };
  let inMetadata = false;
  for (const lineRaw of raw.split(/\r?\n/)) {
    if (!lineRaw.trim()) continue;
    const isIndented = /^\s+/.test(lineRaw);

    if (!isIndented) {
      inMetadata = false;
      const colonIdx = lineRaw.indexOf(":");
      if (colonIdx === -1) continue;
      const key = lineRaw.slice(0, colonIdx).trim();
      const value = lineRaw.slice(colonIdx + 1).trim();
      if (key === "metadata") {
        inMetadata = true;
        continue;
      }
      if (key === "name") frontmatter.name = unquote(value);
      else if (key === "description") frontmatter.description = unquote(value);
      continue;
    }

    if (!inMetadata) continue;
    const colonIdx = lineRaw.indexOf(":");
    if (colonIdx === -1) continue;
    const key = lineRaw.slice(0, colonIdx).trim();
    const value = lineRaw.slice(colonIdx + 1).trim();
    if (key) frontmatter.metadata[key] = unquote(value);
  }

  return { frontmatter, body };
}
