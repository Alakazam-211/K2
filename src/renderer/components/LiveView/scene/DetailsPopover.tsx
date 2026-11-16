// ⓘ Picture details: what the debug line used to say, for whoever wants
// it, in plain rows (Picture, Size, Looking at, Selected…); the raw facts
// (revs, addresses, codes) never show, "Copy for an agent" puts them on the
// clipboard. Its height is capped and the rows scroll, fading at the foot
// while there is more below; when narrow
// it is a sheet across the stage.

import { useLayoutEffect, useRef, useState } from 'react'
import { Button } from '@/components/ui'
import { useBrowserOverlay } from '@/lib/browser-overlay'
import type { Details } from '@/lib/live-view/scene/picture-model'
import { CopyGlyph, InfoGlyph } from '../shell/glyphs'

export function InfoButton({ open, onToggle }: { open: boolean; onToggle: () => void }): React.JSX.Element {
  return (
    <button type="button" data-k2-focus="" data-k2-chamfer="control" className="lv-infob lv-plate" aria-label="Picture details" title="Picture details" aria-expanded={open} aria-controls="lv-scene-info" onClick={onToggle}>
      <InfoGlyph size={15} />
    </button>
  )
}

/** More rows than fit: the list fades at its foot until scrolled to the end. */
function useMoreBelow(ref: React.RefObject<HTMLElement | null>, key: unknown): boolean {
  const [more, setMore] = useState(false)
  useLayoutEffect(() => {
    const el = ref.current
    if (!el) return
    const check = (): void => setMore(el.scrollHeight - el.scrollTop - el.clientHeight > 2)
    check()
    el.addEventListener('scroll', check, { passive: true })
    return () => el.removeEventListener('scroll', check)
  }, [ref, key])
  return more
}

export function DetailsPopover({ details, narrow }: { details: Details; narrow: boolean }): React.JSX.Element {
  const [copied, setCopied] = useState(false)
  const rows = useRef<HTMLDivElement>(null)
  const more = useMoreBelow(rows, details)
  // The popover claims its box, so a Browser page beside the stage gives way while it's open.
  const box = useRef<HTMLDivElement>(null)
  useBrowserOverlay({ active: true, ref: box, kind: 'popover' })
  const copy = (): void => {
    void navigator.clipboard?.writeText(details.forAgents).then(
      () => setCopied(true),
      () => setCopied(false),
    )
  }
  return (
    <div ref={box} id="lv-scene-info" role="dialog" aria-label="Picture details" className="lv-pop lv-info" data-sheet={narrow ? '' : undefined} data-k2-chamfer="surface">
      <h3>Picture details</h3>
      <div ref={rows} className="lv-info-rows" data-more={more ? '' : undefined} data-testid="lv-scene-info-scroll">
        <dl>
          {details.rows.map((r) => (
            <div key={r.label} className="contents">
              <dt>{r.label}</dt>
              <dd>{r.value}</dd>
            </div>
          ))}
        </dl>
      </div>
      <div className="lv-forag">
        <span>For agents</span>
        <Button size="sm" data-k2-focus="" data-k2-chamfer="control" className="ml-auto inline-flex items-center gap-1.5 rounded-[var(--radius-field)]" title="Copies the view’s and the selection’s addresses, to paste to an agent" onClick={copy}>
          <CopyGlyph size={12} />
          {copied ? 'Copied' : 'Copy for an agent'}
        </Button>
      </div>
    </div>
  )
}
