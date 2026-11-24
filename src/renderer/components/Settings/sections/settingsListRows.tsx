import React from 'react'
import { Spinner } from '@/components/ui'
import { SETTING_ROW_HINT } from '../controls/SettingControls'

// Moved out of ProjectsSection.tsx as they were (0.46.18 queue, ratchet).

/** A card row while something loads: the spinner and what is loading. */
export function LoadingRow({ label, testId }: { label: string; testId?: string }): React.JSX.Element {
  return (
    <div className="px-4 py-3 flex items-center gap-2 type-row text-[var(--color-text-secondary)]" role="status" data-testid={testId}>
      <Spinner />
      {label}&hellip;
    </div>
  )
}

/** A card row for an empty list: what would show here, and how to add one. */
export function EmptyRow({ title, hint, testId }: { title: string; hint: string; testId?: string }): React.JSX.Element {
  return (
    <div className="px-4 py-3" data-testid={testId}>
      <div className="type-row text-[var(--color-text-primary)]">{title}</div>
      <div className={SETTING_ROW_HINT}>{hint}</div>
    </div>
  )
}
