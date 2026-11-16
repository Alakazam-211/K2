// The live tab's frame: the header, then the sample banner, the notice row
// and the find bar, then the hero (the picture, the monitor and lanes, the
// text; whatever the view is about) with its foot and status line, and the
// closable side panel beside it. When the tab is narrow the panel starts
// closed and opens as an opaque sheet over the hero, so the hero never
// squeezes; or, in 'reflow' (the editor's), beside it, narrowing the text
// with no scrim. The panel scrolls with soft ends (ScrollEdgeFrame). A view
// that lays out its own rows (the timeline: the lanes run full width under
// the panel) places LivePanel itself and leaves `panelOpen` off here. The
// panel slides open and closed like the app's drawers (PanelMotion).

import { ScrollEdgeFrame, cx } from '@/components/ui'
import { CloseGlyph } from './glyphs'
import { PANEL_MOTION_CSS, SlidingPanel } from './PanelMotion'
import { LIVE_SHELL_CSS } from './shellCss'

export function LiveTabFrame({
  header,
  banner,
  notice,
  findBar,
  hero,
  foot,
  status,
  panel,
  panelOpen,
  panelMode = 'sheet',
  panelLabel = 'Who’s watching and the changes',
  narrow,
  sample,
  onClosePanel,
}: {
  header: React.ReactNode
  banner?: React.ReactNode
  notice?: React.ReactNode
  /** Under the notice, full width (the editor's find bar). */
  findBar?: React.ReactNode
  hero: React.ReactNode
  /** Under the hero, beside the panel (the say row). */
  foot?: React.ReactNode
  /** Under the hero and the foot, across the hero's column (the editor's status line). */
  status?: React.ReactNode
  panel: React.ReactNode
  panelOpen: boolean
  /** Narrow: a sheet over the hero (the scene, the timeline) or beside it (the editor). */
  panelMode?: 'sheet' | 'reflow'
  /** The panel's accessible name. */
  panelLabel?: string
  narrow: boolean
  sample: boolean
  onClosePanel: () => void
}): React.JSX.Element {
  return (
    <div data-testid="lv-tab" data-sample={sample ? '' : undefined} data-narrow={narrow ? '' : undefined} className="lv-shell flex h-full min-h-0 flex-col overflow-hidden">
      <style>{LIVE_SHELL_CSS + PANEL_MOTION_CSS}</style>
      {header}
      {banner}
      {notice}
      {findBar}
      <div className="relative flex min-h-0 flex-1">
        <div className="flex min-w-0 flex-1 flex-col">
          <div className="relative flex min-h-0 flex-1 flex-col">{hero}</div>
          {foot}
          {status}
        </div>
        {panel !== null && panel !== undefined && panel !== false ? (
          <LivePanel open={panelOpen} narrow={narrow} mode={panelMode} label={panelLabel} onClose={onClosePanel}>
            {panel}
          </LivePanel>
        ) : null}
      </div>
    </div>
  )
}

/** The closable side panel: beside the hero, or a sheet over it when narrow (the nearest positioned box holds it).
 *  It slides like the app's drawers: keep it rendered and flip `open`, so it can slide out. */
export function LivePanel({
  open = true,
  narrow,
  mode = 'sheet',
  label = 'Who’s watching and the changes',
  onClose,
  children,
}: {
  open?: boolean
  narrow: boolean
  mode?: 'sheet' | 'reflow'
  label?: string
  onClose: () => void
  children: React.ReactNode
}): React.JSX.Element {
  const sheet = narrow && mode === 'sheet'
  return (
    <SlidingPanel open={open} sheet={sheet}>
      <aside
      data-testid="lv-panel"
      data-sheet={sheet ? '' : undefined}
      aria-label={label}
      className={cx('lv-panel flex flex-col', sheet ? 'absolute inset-y-0 right-0 z-20 w-[min(340px,100%)]' : 'w-[300px] flex-none')}
    >
      {narrow ? (
        <div className="flex flex-none justify-end px-2 pt-1.5">
          <button
            type="button"
            data-k2-focus=""
            aria-label="Close the side panel"
            onClick={onClose}
            className="lv-icon-btn no-drag inline-flex h-[26px] w-[26px] cursor-pointer items-center justify-center rounded-[var(--radius-field)] text-[var(--color-text-secondary)]"
          >
            <CloseGlyph size={14} />
          </button>
        </div>
      ) : null}
      <ScrollEdgeFrame className="min-h-0 flex-1" scrollClassName="overflow-y-auto" testId="lv-panel-scroll">
        {children}
      </ScrollEdgeFrame>
      </aside>
    </SlidingPanel>
  )
}
