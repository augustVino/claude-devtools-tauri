/**
 * SettingRow - Setting row component for consistent layout.
 * Linear-style clean row without icons.
 */

interface SettingRowProps {
  readonly label: string;
  readonly description?: string;
  readonly children: React.ReactNode;
}

export const SettingRow = ({
  label,
  description,
  children,
}: SettingRowProps): React.JSX.Element => {
  return (
    <div
      className="flex items-center justify-between border-b py-3"
      style={{ borderColor: 'var(--color-border-subtle)' }}
    >
      <div>
        <div className="text-sm font-medium" style={{ color: 'var(--color-text)' }}>
          {label}
        </div>
        {description && (
          <div className="text-xs" style={{ color: 'var(--color-text-muted)' }}>
            {description}
          </div>
        )}
      </div>
      <div className="shrink-0">{children}</div>
    </div>
  );
};
