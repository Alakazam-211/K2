// What a live tab tells the shared shell about itself: its look in the
// header, its side panel's sections in order (one PANEL array, so the order
// is one change), where Back lives, when it counts as narrow, what the say
// row points at, and its hero (the picture, the monitor and lanes). The
// scene and the timeline each have one (scene/scene-shell.ts,
// timeline/timeline-shell.ts); the editors will too.

import type { FollowAction, FollowState } from '@/lib/live-view/shared/follow'
import type { LiveNotice, NoticeActionId, NoticeView } from '@/lib/live-view/shared/notices'
import type { SayTime } from '@/lib/live-view/shared/say-time'
import type { LivePerson } from '@/lib/live-view/shared/people'
import type { LiveShellSnapshot } from '@/lib/live-view/shared/source'
import type { LiveTabState } from '@/lib/live-view/shared/state'
import type { Edge } from './EdgeStack'

export interface PanelEntry {
  id: 'watching' | 'changes' | 'messages'
  title: string
}

/** A refusal for the view to draw: the notice, its words (lib/live-view/shared/notices.ts), and its actions. */
export interface HeroRefusal {
  notice: Extract<LiveNotice, { kind: 'refused' }>
  view: NoticeView
  onAction: (id: NoticeActionId) => void
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
  /** The view's own actions (a card's Open the saved file) go to the same handler as the notices'. */
  onAction?: (id: NoticeActionId) => void
  /** The file's name as the header shows it (a card says "Opening rough-cut"). */
  fileName?: string
  /** A refusal the view draws on its picture (`refusalOnPicture`): the shell's words and actions ("Ask Atlas"). */
  refusal?: HeroRefusal | null
  /** Ask someone: opens the say row to them, about `address` when given. */
  ask?: (id: string, address?: SayAt | null) => void
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
  /** The view draws a refusal on its picture (the scene's callout by the object), so the notice bar doesn't. */
  refusalOnPicture?: boolean
  /** Open the copy, History, Save it again, Ask: wired by the view (Open the saved file the shell does itself: it switches the tab to File). `n` is null for an action from the view's own content (a card in the picture). */
  onNoticeAction?: (id: NoticeActionId, n: LiveNotice | null) => void
}
