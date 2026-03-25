/**
 * SettingsSectionHeader - Section header component.
 * Linear-style subtle label.
 */

interface SettingsSectionHeaderProps {
  readonly title: string;
}

export const SettingsSectionHeader = ({ title }: SettingsSectionHeaderProps): React.JSX.Element => {
  return (
    <h3
      className="mb-2 mt-6 text-xs font-medium uppercase tracking-widest first:mt-0"
      style={{ color: 'var(--color-text-muted)' }}
    >
      {title}
    </h3>
  );
};
