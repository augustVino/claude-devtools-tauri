import React, { useEffect, useId, useRef, useState } from 'react';

import { CopyButton } from '@renderer/components/common/CopyButton';
import {
  CODE_BG,
  CODE_BORDER,
  COLOR_TEXT,
  COLOR_TEXT_MUTED,
  PROSE_PRE_BG,
  PROSE_PRE_BORDER,
} from '@renderer/constants/cssVariables';
import { useTheme } from '@renderer/hooks/useTheme';
import { Code, GitBranch } from 'lucide-react';

import type mermaidApi from 'mermaid';

// =============================================================================
// Mermaid initialization (lazy-loaded to keep it out of the main bundle)
// =============================================================================

let mermaidPromise: Promise<typeof mermaidApi> | null = null;
let lastMermaidTheme: 'dark' | 'default' | null = null;

async function getMermaid(): Promise<typeof mermaidApi> {
  if (!mermaidPromise) {
    mermaidPromise = import('mermaid').then((mod) => mod.default);
  }
  return mermaidPromise;
}

async function ensureMermaidInit(isDark: boolean): Promise<typeof mermaidApi> {
  const m = await getMermaid();
  const theme: 'dark' | 'default' = isDark ? 'dark' : 'default';
  if (lastMermaidTheme !== theme) {
    m.initialize({
      startOnLoad: false,
      theme,
      securityLevel: 'strict',
      fontFamily: 'ui-sans-serif, system-ui, sans-serif',
    });
    lastMermaidTheme = theme;
  }
  return m;
}

// =============================================================================
// Component
// =============================================================================

interface MermaidViewerProps {
  code: string;
}

export const MermaidViewer: React.FC<MermaidViewerProps> = ({ code }) => {
  const uniqueId = useId().replace(/:/g, '-');
  const mermaidId = `mermaid-${uniqueId}`;
  const [showCode, setShowCode] = useState(false);
  const [svg, setSvg] = useState<string>('');
  const [error, setError] = useState<string | null>(null);
  const containerRef = useRef<HTMLDivElement>(null);
  const [isVisible, setIsVisible] = useState(false);
  const { isDark } = useTheme();

  // Mermaid renders error SVGs to document.body under `d{id}` and `{id}` wrapper divs
  // (errorRenderer.draw throws before removeTempElements cleanup in mermaid.core.mjs)
  // Clean up orphaned nodes to prevent accumulation of error icons at page bottom
  const cleanupOrphans = (): void => {
    document.getElementById(`d${mermaidId}`)?.remove();
    document.getElementById(mermaidId)?.remove();
  };

  // 第三层懒加载：IntersectionObserver 可见性检测
  useEffect(() => {
    const el = containerRef.current;
    if (!el) return;

    const observer = new IntersectionObserver(
      ([entry]) => {
        if (entry.isIntersecting) {
          setIsVisible(true);
          observer.disconnect(); // 渲染结果缓存于 state，无需重复观察
        }
      },
      { rootMargin: '200px' } // CPU 密集型渲染预留足够预热时间
    );

    observer.observe(el);
    return () => observer.disconnect();
  }, []);

  // isDark 切换且不可见时清除旧主题 SVG
  useEffect(() => {
    if (!isVisible && svg) {
      setSvg('');
    }
  }, [isDark, isVisible]);

  // Render mermaid diagram
  useEffect(() => {
    if (!isVisible) return;

    let cancelled = false;
    const render = async (): Promise<void> => {
      try {
        const m = await ensureMermaidInit(isDark);
        let rendered: string;
        try {
          const result = await m.render(mermaidId, code);
          rendered = result.svg;
        } catch (renderErr) {
          // ID conflict retry with timestamp suffix
          if (renderErr instanceof Error && renderErr.message?.includes('already in use')) {
            const retryId = `${mermaidId}-${Date.now()}`;
            const retryResult = await m.render(retryId, code);
            rendered = retryResult.svg;
          } else {
            throw renderErr;
          }
        }
        if (!cancelled) {
          setSvg(rendered);
          setError(null);
        }
      } catch (err) {
        if (!cancelled) {
          console.error('Failed to render mermaid diagram:', err);
          setError(err instanceof Error ? err.message : 'Failed to render mermaid diagram');
          setSvg('');
        }
      } finally {
        cleanupOrphans();
      }
    };
    void render();
    return () => {
      cancelled = true;
      cleanupOrphans();
    };
  }, [code, isDark, isVisible]);

  return (
    <div ref={containerRef} className="group relative overflow-hidden rounded-lg shadow-sm"
         style={{ backgroundColor: CODE_BG, border: `1px solid ${CODE_BORDER}` }}>
      {/* Header */}
      <div className="flex items-center gap-2 px-3 py-1.5"
           style={{ borderBottom: `1px solid ${CODE_BORDER}` }}>
        <GitBranch className="size-3.5 shrink-0" style={{ color: COLOR_TEXT_MUTED }} />
        <span className="text-xs font-medium" style={{ color: COLOR_TEXT_MUTED }}>
          Mermaid Diagram
        </span>
        <span className="flex-1" />
        <button onClick={() => setShowCode(!showCode)}
                className="flex items-center gap-1 rounded px-1.5 py-0.5 text-xs transition-colors hover:bg-white/10"
                style={{ color: COLOR_TEXT_MUTED }}
                title={showCode ? 'Show diagram' : 'Show code'}>
          <Code className="size-3" />
          {showCode ? 'Diagram' : 'Code'}
        </button>
        <CopyButton text={code} inline />
      </div>

      {/* Content */}
      {showCode ? (
        <pre className="overflow-x-auto p-3 text-xs leading-relaxed"
             style={{ backgroundColor: PROSE_PRE_BG, color: COLOR_TEXT }}>
          <code className="font-mono">{code}</code>
        </pre>
      ) : !isVisible ? (
        <div style={{
          minHeight: 200,
          display: 'flex',
          alignItems: 'center',
          justifyContent: 'center',
          opacity: 0.4,
        }}>
          <span style={{ color: 'var(--text-muted, #888)', fontSize: 11 }}>
            Mermaid diagram
          </span>
        </div>
      ) : error ? (
        <div className="p-3">
          <div className="mb-2 rounded px-2 py-1 text-xs"
               style={{ backgroundColor: 'rgba(239, 68, 68, 0.1)', color: '#ef4444' }}>
            {error}
          </div>
          <pre className="overflow-x-auto rounded p-2 text-xs leading-relaxed"
               style={{ backgroundColor: PROSE_PRE_BG, border: `1px solid ${PROSE_PRE_BORDER}`, color: COLOR_TEXT }}>
            <code className="font-mono">{code}</code>
          </pre>
        </div>
      ) : svg ? (
        <div className="flex justify-center overflow-auto p-4"
             dangerouslySetInnerHTML={{ __html: svg }} />
      ) : (
        <div className="flex items-center justify-center p-4">
          <span className="text-xs" style={{ color: COLOR_TEXT_MUTED }}>Rendering...</span>
        </div>
      )}
    </div>
  );
};
