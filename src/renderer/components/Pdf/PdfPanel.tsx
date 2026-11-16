// The PDF tab's side panel (the live shell's panel, sliding like the
// drawers): three tabs, Layers, Annotations and Pages, in the editors' tab
// strip style, each with a quiet count. Arrow keys move between tabs.

import { useRef } from 'react'
import { cx } from '@/components/ui'
import type { PdfAnnot } from '@/lib/pdf/annotations'
import type { Coverage, LayerView } from '@/lib/pdf/layer-coverage'
import type { OcConfig } from '@/lib/pdf/layers'
import { PdfAnnotList } from './PdfAnnotList'
import { PdfLayersList } from './PdfLayersList'
import { PdfThumbs } from './PdfThumbs'
import type { PdfDoc } from './usePdfDocument'

export type PdfPanelTab = 'layers' | 'annotations' | 'pages'
const TABS: { id: PdfPanelTab; label: string }[] = [
  { id: 'layers', label: 'Layers' },
  { id: 'annotations', label: 'Annotations' },
  { id: 'pages', label: 'Pages' },
]

export function PdfPanel(props: {
  tab: PdfPanelTab
  onTab: (t: PdfPanelTab) => void
  doc: PdfDoc
  layers: LayerView
  coverage: Coverage | null
  onLayer: (id: string, visible: boolean) => void
  onAllLayers: (visible: boolean) => void
  annots: readonly PdfAnnot[] | null
  selectedKey: string | null
  onOpenAnnot: (a: PdfAnnot) => void
  cfg: OcConfig | null
  cfgKey: number
  page: number
  onGo: (n: number) => void
}): React.JSX.Element {
  const strip = useRef<HTMLDivElement>(null)
  const counts: Record<PdfPanelTab, number | null> = { layers: props.layers.used.length, annotations: props.annots?.length ?? null, pages: props.doc.numPages }
  const onKey = (e: React.KeyboardEvent): void => {
    const step = e.key === 'ArrowRight' ? 1 : e.key === 'ArrowLeft' ? -1 : 0
    if (!step) return
    e.preventDefault()
    const i = (TABS.findIndex((t) => t.id === props.tab) + step + TABS.length) % TABS.length
    props.onTab(TABS[i].id)
    strip.current?.querySelectorAll<HTMLElement>('[role="tab"]')[i]?.focus()
  }
  return (
    <div data-testid="pdf-panel" className="flex min-h-full flex-col">
      <div ref={strip} role="tablist" aria-label="Side panel" className="flex flex-none items-center gap-0.5 px-3 pb-1 pt-2" onKeyDown={onKey}>
        {TABS.map((t) => {
          const on = t.id === props.tab
          return (
            <button
              key={t.id}
              type="button"
              role="tab"
              id={`pdf-tab-${t.id}`}
              aria-selected={on}
              aria-controls="pdf-tabpanel"
              tabIndex={on ? 0 : -1}
              data-k2-focus=""
              data-k2-chamfer="control"
              onClick={() => props.onTab(t.id)}
              className={cx('pdf-panel-tab no-drag', on ? 'ed-panel-tab font-semibold' : '')}
            >
              {t.label}
              {counts[t.id] !== null ? <span className="pdf-tab-count lv-num font-normal">{counts[t.id]}</span> : null}
            </button>
          )
        })}
      </div>
      <div role="tabpanel" id="pdf-tabpanel" aria-labelledby={`pdf-tab-${props.tab}`} className="min-h-0 flex-1 pb-2">
        {props.tab === 'layers' ? (
          <PdfLayersList view={props.layers} coverage={props.coverage} onSet={props.onLayer} onAll={props.onAllLayers} />
        ) : props.tab === 'annotations' ? (
          <PdfAnnotList annots={props.annots} selectedKey={props.selectedKey} onOpen={props.onOpenAnnot} />
        ) : (
          <PdfThumbs doc={props.doc} cfg={props.cfg} cfgKey={props.cfgKey} page={props.page} onGo={props.onGo} />
        )}
      </div>
    </div>
  )
}
