// The Layers tab: the PDF's optional content groups, each with a switch and
// a quiet count of what it covers ("214 items"). "3 of 4 visible", Show all
// and Hide all at the top. The choice is this tab's alone and never written
// to the file.
//   - Many CAD exports keep layer names that nothing is marked with: once
//     every page is counted (lib/pdf/layer-coverage.ts) those go in one
//     closed group at the foot, "N layers with nothing on them", whose rows
//     stay findable with their switches off.
//   - Until then: "Checking layers… 12 of 40 pages", and no layer is empty.
//   - When none covers anything, one plain line says so first.
//   - A PDF with no layers says that in one line.

import { useState } from 'react'
import { Toggle } from '@/components/ui'
import { ChevronGlyph } from '@/components/LiveView/shell/glyphs'
import { itemsWord, type Coverage, type CoveredLayer, type LayerView } from '@/lib/pdf/layer-coverage'

const NOTHING = 'Nothing on this layer'

function Row({ l, empty, onSet }: { l: CoveredLayer; empty?: boolean; onSet: (id: string, visible: boolean) => void }): React.JSX.Element {
  return (
    <li data-testid="pdf-layer-row" data-off={l.visible ? undefined : ''} data-empty={empty ? '' : undefined} title={empty ? NOTHING : undefined} className="pdf-layer-row" style={{ paddingLeft: 14 + l.depth * 14 }}>
      <span className="pdf-wrap type-row min-w-0 flex-1">{l.name}</span>
      {!empty && l.items > 0 ? (
        <span data-testid="pdf-layer-items" className="pdf-quiet type-hint lv-num flex-none">
          {itemsWord(l.items)}
        </span>
      ) : null}
      <Toggle size="sm" checked={!empty && l.visible} disabled={empty} aria-label={l.name} onChange={(v) => onSet(l.id, v)} />
    </li>
  )
}

export function PdfLayersList({ view, coverage, onSet, onAll }: { view: LayerView; coverage: Coverage | null; onSet: (id: string, visible: boolean) => void; onAll: (visible: boolean) => void }): React.JSX.Element {
  const [emptyOpen, setEmptyOpen] = useState(false)
  const { used, empty, checking } = view
  if (used.length === 0 && empty.length === 0) return <p className="pdf-empty type-row m-0">This PDF has no layers.</p>
  const shown = used.filter((l) => l.visible).length
  const n = empty.length
  return (
    <div data-testid="pdf-layers">
      {used.length === 0 ? (
        <p data-testid="pdf-layers-none" className="pdf-empty type-row m-0">
          This file names {n} {n === 1 ? 'layer' : 'layers'}, but none is attached to anything on the sheets, so there’s nothing to switch.
        </p>
      ) : (
        <div className="pdf-list-head">
          <span data-testid="pdf-layers-count" className="type-hint lv-num">
            {shown} of {used.length} visible
          </span>
          <span className="flex-1" />
          <button type="button" data-k2-focus="" data-k2-chamfer="control" className="ed-chip-btn pdf-chip type-hint" aria-disabled={shown === used.length || undefined} onClick={() => shown < used.length && onAll(true)}>
            Show all
          </button>
          <button type="button" data-k2-focus="" data-k2-chamfer="control" className="ed-chip-btn pdf-chip type-hint" aria-disabled={shown === 0 || undefined} onClick={() => shown > 0 && onAll(false)}>
            Hide all
          </button>
        </div>
      )}
      {checking ? (
        <p data-testid="pdf-layers-checking" aria-live="polite" className="pdf-checking type-hint m-0 lv-num">
          Checking layers… {coverage?.scanned ?? 0} of {coverage?.total ?? '…'} pages
        </p>
      ) : null}
      <ul className="m-0 list-none p-0">
        {used.map((l) => (
          <Row key={l.id} l={l} onSet={onSet} />
        ))}
      </ul>
      {n > 0 ? (
        <div data-testid="pdf-layers-empty" className="pdf-empty-group">
          <button type="button" data-k2-focus="" aria-expanded={emptyOpen} onClick={() => setEmptyOpen((o) => !o)} className="lv-row-btn pdf-group-btn no-drag">
            <span aria-hidden="true" className={emptyOpen ? 'inline-flex' : 'inline-flex -rotate-90'}>
              <ChevronGlyph size={13} />
            </span>
            {/* The rows' own size and weight (the button's font is reset by the shell's row buttons). */}
            <span className="pdf-group-words type-row">
              {n} {n === 1 ? 'layer' : 'layers'} with nothing on {n === 1 ? 'it' : 'them'}
            </span>
          </button>
          {emptyOpen ? (
            <ul className="m-0 list-none p-0">
              {empty.map((l) => (
                <Row key={l.id} l={l} empty onSet={onSet} />
              ))}
            </ul>
          ) : null}
        </div>
      ) : null}
    </div>
  )
}
