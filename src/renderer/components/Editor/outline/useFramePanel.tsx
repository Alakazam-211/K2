// The frame's side panel (build-editors.md E3; editors-notes.md §2 Chrome):
//   - a plain markdown file: the Outline, open by default where there's room
//     (remembered on this computer), closed at 800 until you open it;
//   - a live markdown file: Outline and Live in one tab strip, Live first
//     chosen (it opens itself when someone comes in);
//   - code: the live panel only, as before (no outline).

import { useEffect, useState } from 'react'
import type { EditorView } from '@codemirror/view'
import { cx } from '@/components/ui'
import { useLocalFlag } from '@/lib/editor/local-flag'
import type { EditorKind } from '@/lib/editor/preview-switch'
import type { FrameLive } from '../frameLive'
import { OutlinePanel } from './OutlinePanel'
import { PanelTabsContext, type PanelTab, type PanelTabsValue } from './PanelTabs'

const OUTLINE_OPEN_KEY = 'k2.editor.outlineOpen.v1'

export interface FramePanel {
  panel: React.ReactNode
  panelOpen: boolean
  onTogglePanel?: () => void
  onClosePanel: () => void
  panelLabel?: string
}

export function useFramePanel(o: { kind: EditorKind; live: FrameLive | null; view: EditorView | null; version: number; narrow: boolean }): FramePanel {
  const { kind, live } = o
  const [wideOpen, setWideOpen] = useLocalFlag(OUTLINE_OPEN_KEY, true)
  const [narrowOpen, setNarrowOpen] = useState(false)
  const [tab, setTab] = useState<PanelTab['id']>(live ? 'live' : 'outline')
  const isLive = !!live
  useEffect(() => setTab(isLive ? 'live' : 'outline'), [isLive])

  if (kind !== 'markdown') {
    return { panel: live?.panel ?? null, panelOpen: !!live?.panelOpen, onTogglePanel: live?.onTogglePanel, onClosePanel: live?.onClosePanel ?? (() => {}), panelLabel: live?.panelLabel }
  }

  const outlineTab: PanelTab = { id: 'outline', label: 'Outline' }
  if (!live) {
    const open = o.narrow ? narrowOpen : wideOpen
    const set = (v: boolean): void => (o.narrow ? setNarrowOpen(v) : setWideOpen(v))
    const tabs: PanelTabsValue = { tabs: [outlineTab], selected: 'outline', onSelect: () => {} }
    return {
      panel: <OutlinePanel view={o.view} version={o.version} tabs={tabs} />,
      panelOpen: open,
      onTogglePanel: () => set(!open),
      onClosePanel: () => set(false),
      panelLabel: 'the outline',
    }
  }

  const here = live.here ?? 0
  const down = !!live.hereStale
  const liveTab: PanelTab = {
    id: 'live',
    label: 'Live',
    count: (
      <span data-testid="ed-live-tab-count" data-stale={down ? '' : undefined} title={down ? 'As you last saw it' : undefined} className={cx('font-normal text-[var(--color-text-secondary)]', down ? 'line-through' : '')}>
        {here}
      </span>
    ),
  }
  const tabs: PanelTabsValue = { tabs: [outlineTab, liveTab], selected: tab, onSelect: setTab }
  return {
    panel: tab === 'outline' ? <OutlinePanel view={o.view} version={o.version} tabs={tabs} /> : <PanelTabsContext.Provider value={tabs}>{live.panel}</PanelTabsContext.Provider>,
    panelOpen: live.panelOpen,
    onTogglePanel: live.onTogglePanel,
    onClosePanel: live.onClosePanel,
    panelLabel: 'the outline and Live',
  }
}
