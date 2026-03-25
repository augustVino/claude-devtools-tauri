/**
 * NotificationTriggerSettings - Component for managing notification triggers.
 * Allows users to configure when notifications should be generated.
 *
 * Uses intent-first design pattern with 4 sections:
 * 1. General Info (always visible)
 * 2. Trigger Condition (mode selector)
 * 3. Dynamic Configuration (based on mode)
 * 4. Advanced (collapsible)
 */

import { AddTriggerForm } from './components/AddTriggerForm';
import { SectionHeader } from './components/SectionHeader';
import { TriggerCard } from './components/TriggerCard';

import type { NotificationTriggerSettingsProps } from './types';

// Stable no-op function for builtin triggers that can't be removed
const noopRemove = (_triggerId: string): Promise<void> => Promise.resolve();

/**
 * Main component for managing notification triggers.
 */
export const NotificationTriggerSettings = ({
  triggers,
  saving,
  onUpdateTrigger,
  onAddTrigger,
  onRemoveTrigger,
}: Readonly<NotificationTriggerSettingsProps>): React.JSX.Element => {
  // Separate builtin and custom triggers
  const builtinTriggers = triggers.filter((t) => t.isBuiltin);
  const customTriggers = triggers.filter((t) => !t.isBuiltin);

  return (
    <div className="space-y-8">
      {/* Builtin Triggers */}
      {builtinTriggers.length > 0 && (
        <div>
          <SectionHeader title="Built-in Triggers" />
          <p className="mb-4 text-xs text-text-muted">
            Default triggers that come with the application. You can enable/disable them and
            customize their patterns.
          </p>
          <div>
            {builtinTriggers.map((trigger) => (
              <TriggerCard
                key={trigger.id}
                trigger={trigger}
                saving={saving}
                onUpdate={onUpdateTrigger}
                onRemove={noopRemove}
              />
            ))}
          </div>
        </div>
      )}

      {/* Custom Triggers */}
      <div>
        <SectionHeader title="Custom Triggers" />
        <p className="mb-4 text-xs text-text-muted">
          Create your own triggers to get notified for specific patterns or tool outputs.
        </p>

        {customTriggers.length > 0 && (
          <div className="mb-4">
            {customTriggers.map((trigger) => (
              <TriggerCard
                key={trigger.id}
                trigger={trigger}
                saving={saving}
                onUpdate={onUpdateTrigger}
                onRemove={onRemoveTrigger}
              />
            ))}
          </div>
        )}

        {customTriggers.length === 0 && (
          <p className="mb-4 text-sm italic text-text-muted">No custom triggers configured yet.</p>
        )}

        <AddTriggerForm saving={saving} onAdd={onAddTrigger} />
      </div>
    </div>
  );
};
