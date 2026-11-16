// What a live tab tells the shared shell about itself: its look in the
// header, its side panel's sections in order (one PANEL array, so the order
// is one change), where Back lives, when it counts as narrow, what the say
// row points at, and its hero (the picture, the monitor and lanes). The
// scene and the timeline each have one (scene/scene-shell.ts,
// timeline/timeline-shell.ts); the editors will too.

import type { LiveNotice, NoticeActionId } from '@/lib/live-view/shared/notices'
import type { LivePerson } from '@/lib/live-view/shared/people'
import type { LiveShellSnapshot } from '@/lib/live-view/shared/source'
import type { LiveTabState } from '@/lib/live-view/shared/state'
import type { Edge } from './EdgeStack'

export interface PanelEntry {
  id: 'watching' | 'changes' | 'messages'
  title: string
}

export interface ShellHeroProps {
  snap: LiveShellSnapshot
  state: LiveTabState
  /** People off screen, by edge (the hero places the stacks). */
  offscreen: { side: Edge; people: LivePerson[] }[]
  onJump: (side: Edge) => void
  /** The last edge jumped to (the sample shows it). */
  jumped: Edge | null
  /** "Back to X", when this view keeps Back out of the header. */
  back: React.ReactNode
  narrow: boolean
}

export interface ShellConfig {
  kind: 'scene' | 'timeline'
  kindLabel: string
  fileGlyph: React.ReactNode
  /** The sample banner's line. */
  bannerLine: string
  panel: readonly PanelEntry[]
  /** Narrower than this (px) the panel starts closed and opens as a sheet. */
  narrowBelow: number
  /** At narrow widths the header's sentence takes its own row. */
  sentenceRowWhenNarrow: boolean
  /** "Back to X" in the header (the scene) or in the view's own place (the timeline's transport). */
  backInHeader: boolean
  /** The say row shows whenever live (the timeline: the lanes don't jump), else only while following. */
  sayAlways: boolean
  /** What stays on screen while reconnecting, in words. */
  kept: string
  /** What a message to `to` points at: words for the chip, the address it carries. */
  sayAddress: (snap: LiveShellSnapshot, to: string) => { label: string; title: string; address: string } | null
  Hero: (p: ShellHeroProps) => React.JSX.Element
  /** Open the copy, History, Open saved versions, Save it again, Ask: wired by the view. */
  onNoticeAction?: (id: NoticeActionId, n: LiveNotice) => void
}
