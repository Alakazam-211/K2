import { KeyCombo } from '@/components/KeySymbol'

/** A nav section header's modifier hint beside its count (`⌥⌘ 1-9`): the
 *  muted text colour at full strength, which clears 4.5:1 on the nav in every
 *  look (at half opacity it fell to about 2.0–2.7:1). Shared by the pinned
 *  area's header and the Active bar. */
export function ShortcutRangeHint({ combo }: { combo: string }): React.JSX.Element {
  return (
    <span className="text-[9px] font-mono text-[var(--color-text-muted)]" data-shortcut-hint={combo}>
      <KeyCombo combo={combo} />
    </span>
  )
}
