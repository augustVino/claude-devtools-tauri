/**
 * OpenInMenu - "Open in..." dropdown for Memory files/directories.
 * Two variants: 'dots' (sidebar overflow button) and 'iconMenu' (toolbar).
 * Follows SessionContextMenu pattern (no radix-ui dependency).
 */

import { useCallback, useEffect, useRef, useState } from 'react';

import { api } from '@renderer/api';
import { useClickOutside } from '@renderer/hooks/useClickOutside';
import {
  Bot,
  Check,
  Clipboard,
  FileCode,
  FileText,
  Folder,
  Hammer,
  MoreVertical,
  Smartphone,
  SquareCode,
  Terminal as TerminalIcon,
} from 'lucide-react';

import type { OpenTarget } from '@shared/types/api';

type OpenInMenuVariant = 'dots' | 'iconMenu';

interface OpenInMenuProps {
  projectId: string;
  fileName: string | null;
  variant?: OpenInMenuVariant;
}

// 图标映射 — 对齐上游 OpenInMenu.tsx 的 ICON_BY_ID
const ICON_BY_ID: Record<
  string,
  React.ComponentType<{ size?: number; className?: string }>
> = {
  finder: Folder,
  cursor: Bot,
  vscode: FileCode,
  zed: SquareCode,
  xcode: Hammer,
  ghostty: TerminalIcon,
  iterm: TerminalIcon,
  terminal: TerminalIcon,
  'android-studio': Smartphone,
  antigravity: SquareCode,
};

const FALLBACK_ICON = FileText;

export const OpenInMenu = ({
  projectId,
  fileName,
  variant = 'dots',
}: OpenInMenuProps) => {
  const [isOpen, setIsOpen] = useState(false);
  const [openers, setOpeners] = useState<OpenTarget[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);
  const menuRef = useRef<HTMLDivElement>(null);
  const mountedRef = useRef(true);
  const errorTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const copiedTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  // Clear any pending timers on unmount
  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
      if (errorTimerRef.current) clearTimeout(errorTimerRef.current);
      if (copiedTimerRef.current) clearTimeout(copiedTimerRef.current);
    };
  }, []);

  // Helper: set error with auto-clear after 2s, safe on unmount
  const showError = useCallback((message: string) => {
    if (errorTimerRef.current) clearTimeout(errorTimerRef.current);
    setError(message);
    errorTimerRef.current = setTimeout(() => {
      if (mountedRef.current) {
        setError(null);
        errorTimerRef.current = null;
      }
    }, 2000);
  }, []);

  // 加载 opener 列表
  const loadOpeners = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const list = await api.memory.listAvailableOpeners();
      setOpeners(list);
    } catch {
      setOpeners([]);
    } finally {
      setLoading(false);
    }
  }, []);

  // 打开时加载；projectId 变化时刷新
  useEffect(() => {
    if (isOpen) {
      void loadOpeners();
    }
  }, [isOpen, projectId, loadOpeners]);

  // Close on outside click or Escape
  useClickOutside(menuRef, () => setIsOpen(false), isOpen);

  // 点击 opener 项 — 调用 openIn 并检查返回结果
  const handleOpen = useCallback(
    async (targetId: string) => {
      setError(null);
      try {
        const result = await api.memory.openIn(projectId, fileName, targetId);
        if (!result.success) {
          showError(result.error ?? 'Failed to open');
          return;
        }
      } catch (err) {
        showError(err instanceof Error ? err.message : 'Unknown error');
        return;
      }
      setIsOpen(false);
    },
    [projectId, fileName, showError]
  );

  // Copy Path 菜单项 — 后端负责写入剪贴板
  const handleCopyPath = useCallback(async () => {
    setError(null);
    setCopied(false);
    try {
      const result = await api.memory.copyPath(projectId, fileName);
      if (result.success) {
        setCopied(true);
        if (copiedTimerRef.current) clearTimeout(copiedTimerRef.current);
        copiedTimerRef.current = setTimeout(() => {
          if (mountedRef.current) setCopied(false);
          copiedTimerRef.current = null;
        }, 1500);
      } else {
        showError(result.error ?? 'Failed to copy');
      }
    } catch (err) {
      showError(err instanceof Error ? err.message : 'Failed to copy');
    }
  }, [projectId, fileName, showError]);

  const toggleOpen = useCallback(() => {
    setIsOpen((prev) => !prev);
  }, []);

  // 触发按钮
  const triggerButton = (
    <button
      type="button"
      onClick={toggleOpen}
      aria-expanded={isOpen}
      aria-label="Open in..."
      title="Open in..."
      className="flex items-center justify-center rounded-md p-1 transition-colors hover:bg-[var(--color-surface-raised)]"
    >
      {variant === 'dots' ? <MoreVertical size={16} /> : <Folder size={16} />}
    </button>
  );

  if (!isOpen) return triggerButton;

  return (
    <div className="relative inline-block" ref={menuRef}>
      {triggerButton}
      <div
        className="absolute right-0 z-50 min-w-[200px] rounded-md border py-1 shadow-lg"
        style={{
          backgroundColor: 'var(--color-surface-overlay)',
          borderColor: 'var(--color-border-emphasis)',
          color: 'var(--color-text)',
        }}
      >
        {loading && (
          <div className="px-3 py-1.5 text-xs text-[var(--color-text-muted)]">
            Detecting apps...
          </div>
        )}

        {/* 错误提示 — 红色文本在菜单顶部 */}
        {error && (
          <div
            className="mx-2 mb-1 rounded px-2 py-1 text-xs"
            style={{
              backgroundColor: 'var(--color-red-900, rgba(220,38,38,0.15))',
              color: 'var(--color-red-400, #f87171)',
            }}
          >
            {error}
          </div>
        )}

        {!loading && openers.length === 0 && !error && (
          <div className="px-3 py-1.5 text-xs text-[var(--color-text-muted)]">
            No apps detected
          </div>
        )}

        {!loading &&
          openers.map((target) => {
            const IconComponent = ICON_BY_ID[target.id] ?? FALLBACK_ICON;
            return (
              <button
                key={target.id}
                type="button"
                onClick={() => void handleOpen(target.id)}
                className="flex w-full items-center justify-between px-3 py-1.5 text-left text-sm transition-colors hover:bg-[var(--color-surface-raised)]"
              >
                <span className="flex items-center gap-2">
                  <IconComponent
                    size={16}
                    className="text-[var(--color-text-secondary)]"
                  />
                  <span>{target.label}</span>
                </span>
                {target.shortcutKey && (
                  <span
                    className="ml-4 text-xs"
                    style={{ color: 'var(--color-text-muted)' }}
                  >
                    {target.shortcutKey}
                  </span>
                )}
              </button>
            );
          })}

        {/* 分割线 + Copy Path */}
        {openers.length > 0 && (
          <div
            className="mx-2 my-1 border-t"
            style={{ borderColor: 'var(--color-border)' }}
          />
        )}
        <button
          type="button"
          onClick={() => void handleCopyPath()}
          className="flex w-full items-center justify-between px-3 py-1.5 text-left text-sm transition-colors hover:bg-[var(--color-surface-raised)]"
        >
          <span className="flex items-center gap-2">
            {copied ? (
              <Check size={16} className="text-green-400" />
            ) : (
              <Clipboard
                size={16}
                className="text-[var(--color-text-secondary)]"
              />
            )}
            <span>{copied ? 'Copied' : 'Copy Path'}</span>
          </span>
          {!copied && (
            <span
              className="ml-4 text-xs"
              style={{ color: 'var(--color-text-muted)' }}
            >
              ⌘⇧C
            </span>
          )}
        </button>
      </div>
    </div>
  );
};
