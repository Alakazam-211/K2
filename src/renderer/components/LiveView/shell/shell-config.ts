// What a live tab tells the shared shell about itself: its look in the
// header, its side panel's sections in order (one PANEL array, so the order
// is one change), where Back lives, when it counts as narrow, what the say
// row points at, and its hero (the picture, the monitor and lanes). The
// scene and the timeline each have one (scene/scene-shell.ts,
// timeline/timeline-shell.ts); the editors will too.

import type { FollowAction, FollowState } from '@/lib/live-view/shared/follow'
import type { LiveNotice, NoticeActionId } from '@/lib/live-view/shared/notices'
import type { SayTime } from '@/lib/live-view/shared/say-time'
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
  /** The say row, when the view places it (`placesSayAndPanel`). */
  say?: React.ReactNode
  /** The side panel (null when closed), when the view places it (`placesSayAndPanel`). */
  panel?: React.ReactNode
  /** Whom you follow, the follow state and its dispatch: the view follows their place and looks away on its own keys and scrolling. */
  follow?: { id: string | null; state: FollowState; dispatch: (a: FollowAction) => void }
  /** The view says what the say row points at (the timeline: the clip under its playhead); null goes back to the config's. */
  setSayAt?: (a: SayAt | null) => void
}

/** What a message points at: the chip's words and tooltip, the address it carries, and a time (the playhead's time position) where the view has one. */
export interface SayAt {
  label: string
  title: string
  address: string
  time?: SayTime | null
}

/** What a view's own side panel gets from the shell (the scene's outliner and properties). */
export interface ShellPanelProps {
  snap: LiveShellSnapshot
  followingId: string | null
  onFollow: (id: string) => void
  /** Not connected: people read "as you last saw them". */
  offline: boolean
  narrow: boolean
}

export interface ShellConfig {
  kind: 'scene' | 'timeline' | 'editor'
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
  sayAddress: (snap: LiveShellSnapshot, to: string) => SayAt | null
  Hero: (p: ShellHeroProps) => React.JSX.Element
  /** The hero lays out the say row and the side panel itself (the timeline: its lanes run full width under both). */
  placesSayAndPanel?: boolean
  /** The view's own side panel, for the snapshots it knows (`when`); the shell's sections otherwise. */
  Panel?: { when: (snap: LiveShellSnapshot) => boolean; View: (p: ShellPanelProps) => React.JSX.Element | null }
  /** Open the copy, History, Open saved versions, Save it again, Ask: wired by the view. */
  onNoticeAction?: (id: NoticeActionId, n: LiveNotice) => void
}
