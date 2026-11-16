// The live tab's frame: the header, then the sample banner and the notice
// row, then the hero (the picture, the monitor and lanes; whatever the view
// is about) with the closable side panel beside it. When the tab is narrow
// the panel starts closed and opens as an opaque sheet over the hero, so the
// hero never squeezes. The panel scrolls with soft ends (ScrollEdgeFrame).

import { ScrollEdgeFrame, cx } from '@/components/ui'
import { CloseGlyph } from './glyphs'
import { LIVE_SHELL_CSS } from './shellCss'

export function LiveTabFrame({
  header,
  banner,
  notice,
  hero,
  foot,
  panel,
  panelOpen,
  narrow,
  sample,
  onClosePanel,
}: {
  header: React.ReactNode
  banner?: React.ReactNode
  notice?: React.ReactNode
  hero: React.ReactNode
  /** Under the hero, beside the panel (the say row). */
  foot?: React.ReactNode
  panel: React.ReactNode
  panelOpen: boolean
  narrow: boolean
  sample: boolean
  onClosePanel: () => void
}): React.JSX.Element {
  return (
    <div data-testid="lv-tab" data-sample={sample ? '' : undefined} data-narrow={narrow ? '' : undefined} className="lv-shell flex h-full min-h-0 flex-col overflow-hidden">
      <style>{LIVE_SHELL_CSS}</style>
      {header}
      {banner}
      {notice}
      <div className="relative flex min-h-0 flex-1">
        <div className="flex min-w-0 flex-1 flex-col">
          <div className="relative flex min-h-0 flex-1 flex-col">{hero}</div>
          {foot}
        </div>
        {panelOpen ? (
          <aside
            data-testid="lv-panel"
            data-sheet={narrow ? '' : undefined}
            aria-label="Who’s watching and the changes"
            className={cx('lv-panel flex flex-col', narrow ? 'absolute inset-y-0 right-0 z-20 w-[min(340px,100%)]' : 'w-[300px] flex-none')}
          >
            {narrow ? (
              <div className="flex flex-none justify-end px-2 pt-1.5">
                <button
                  type="button"
                  data-k2-focus=""
                  aria-label="Close the side panel"
                  onClick={onClosePanel}
                  className="lv-icon-btn no-drag inline-flex h-[26px] w-[26px] cursor-pointer items-center justify-center rounded-[var(--radius-field)] text-[var(--color-text-secondary)]"
                >
                  <CloseGlyph size={14} />
                </button>
              </div>
            ) : null}
            <ScrollEdgeFrame className="min-h-0 flex-1" scrollClassName="overflow-y-auto" testId="lv-panel-scroll">
              {panel}
            </ScrollEdgeFrame>
          </aside>
        ) : null}
      </div>
    </div>
  )
}
