// Find in the PDF, in the editors' find bar: docked under the header, full
// width, pushing the page down (never over it). The query, the count ("2 of
// 9" or "No matches"), previous and next, Match case, and close. Enter goes
// to the next match, Shift+Enter the previous, Escape closes.

import { useEffect, useRef } from 'react'
import { ChevronLeftIcon, ChevronRightIcon, SearchIcon } from 'lucide-react'
import { K2_ICON_STROKE } from '@/components/icons/K2Icons'
import { CloseGlyph } from '@/components/LiveView/shell/glyphs'

export function PdfFindBar({
  query,
  onQuery,
  matchCase,
  onMatchCase,
  count,
  hasMatches,
  reading,
  onNext,
  onPrev,
  onClose,
}: {
  query: string
  onQuery: (q: string) => void
  matchCase: boolean
  onMatchCase: () => void
  count: string
  hasMatches: boolean
  /** Still reading the pages' text. */
  reading: boolean
  onNext: () => void
  onPrev: () => void
  onClose: () => void
}): React.JSX.Element {
  const input = useRef<HTMLInputElement>(null)
  useEffect(() => {
    input.current?.focus()
    input.current?.select()
  }, [])
  const off = !hasMatches
  const nav = (label: string, onClick: () => void, Icon: typeof ChevronLeftIcon): React.JSX.Element => (
    <button type="button" data-k2-focus="" data-k2-chamfer="control" aria-label={label} title={off ? 'No matches' : label} aria-disabled={off || undefined} onClick={() => !off && onClick()} className="lv-icon-btn pdf-find-btn no-drag">
      <Icon size={15} strokeWidth={K2_ICON_STROKE} aria-hidden="true" />
    </button>
  )
  return (
    <div role="search" aria-label="Find in this PDF" data-testid="pdf-find" className="pdf-find">
      <span aria-hidden="true" className="pdf-find-icon">
        <SearchIcon size={15} strokeWidth={K2_ICON_STROKE} />
      </span>
      <input
        ref={input}
        type="search"
        aria-label="Find"
        placeholder="Find"
        value={query}
        data-k2-chamfer="control"
        className="pdf-find-field no-drag"
        onChange={(e) => onQuery(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === 'Enter') {
            e.preventDefault()
            if (!off) (e.shiftKey ? onPrev : onNext)()
          } else if (e.key === 'Escape') {
            e.preventDefault()
            e.stopPropagation()
            onClose()
          }
        }}
      />
      <span data-testid="pdf-find-count" aria-live="polite" data-none={query.trim() && !hasMatches && !reading ? '' : undefined} className="pdf-find-count lv-num">
        {reading && query.trim() ? 'Reading…' : count}
      </span>
      {nav('Previous match', onPrev, ChevronLeftIcon)}
      {nav('Next match', onNext, ChevronRightIcon)}
      <button type="button" data-k2-focus="" data-k2-chamfer="control" aria-pressed={matchCase} aria-label="Match case" title="Match case" onClick={onMatchCase} className="lv-icon-btn pdf-find-btn pdf-find-case no-drag">
        Aa
      </button>
      <span className="flex-1" />
      <button type="button" data-k2-focus="" data-k2-chamfer="control" aria-label="Close find" title="Close find (Esc)" onClick={onClose} className="lv-icon-btn pdf-find-btn no-drag">
        <CloseGlyph size={15} />
      </button>
    </div>
  )
}
