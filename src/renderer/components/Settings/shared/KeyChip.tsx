// A keyboard shortcut: Settings → Key Bindings (a key cap you click to
// change, cut in Terminal and rounded in Glass like other small controls) and
// fixed shortcuts there and in Settings → Code (the same keys, no cap).

import React from 'react'
import { KeyCombo } from '@/components/KeySymbol'
import { cx } from '@/components/ui'

export const KEY_CHIP =
  'inline-flex items-center justify-center min-w-[2.5rem] px-2 py-1 type-control font-mono whitespace-nowrap border rounded-[var(--radius-field)]'
/** A key cap you can click to change. */
export const KEY_CHIP_REST = 'bg-[var(--color-bg-elevated)] border-[var(--color-control-edge)] text-[var(--color-text-primary)]'
/** A shortcut that can't be changed here: the same keys, with no cap, so it
 *  doesn't look like a button. */
export const KEY_CHIP_FIXED = 'bg-transparent border-transparent text-[var(--color-text-secondary)]'

/** A shortcut that can't be changed here. */
export function KeyChip({ combo, className }: { combo: string; className?: string }): React.JSX.Element {
  return (
    <kbd className={cx(KEY_CHIP, KEY_CHIP_FIXED, className)}>
      <KeyCombo combo={combo} />
    </kbd>
  )
}
