// A keyboard shortcut shown as a key cap: Settings → Key Bindings (as a
// button you click to rebind) and Settings → Code (fixed shortcuts). One look
// in both, cut in Terminal and rounded in Glass like other small controls.

import React from 'react'
import { KeyCombo } from '@/components/KeySymbol'
import { cx } from '@/components/ui'

export const KEY_CHIP =
  'inline-flex items-center justify-center min-w-[2.5rem] px-2 py-0.5 type-control font-mono whitespace-nowrap border rounded-[var(--radius-field)]'
export const KEY_CHIP_REST = 'bg-[var(--color-bg-elevated)] border-[var(--color-control-edge)] text-[var(--color-text-primary)]'

/** A shortcut that can't be changed here. */
export function KeyChip({ combo, className }: { combo: string; className?: string }): React.JSX.Element {
  return (
    <kbd data-k2-chamfer="chip" className={cx(KEY_CHIP, KEY_CHIP_REST, className)}>
      <KeyCombo combo={combo} />
    </kbd>
  )
}
