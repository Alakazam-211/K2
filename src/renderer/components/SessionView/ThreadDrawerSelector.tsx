import { useEffect, useRef, type JSX, type KeyboardEvent, type ReactNode } from 'react'
import { useRoom } from '@/components/Room/RoomContext'
import { PageTab } from '@/components/TopBar/PageTab'
import { useSettingsStore } from '@/stores/settings'
import { isMacPlatform } from '@/lib/desktop-chrome'
import { effectiveKeybinding, formatKeyCombo } from '@shared/hotkeys'
import { TILE_VIEWS, type TileView } from '@/components/Projects/tile-view'
import { registerRevealTarget } from './threadReveal'
import { floatProps } from './threadFloats'
import {
  THREAD_PANEL_KINDS,
  THREAD_PANEL_LABEL,
  threadDrawerOpen,
  type ThreadDrawerKind,
  type ThreadPanelKind,
} from './threadPanels'

const PANEL_TOOLTIP: Record<ThreadPanelKind, string> = {
  chatter: 'messages between agents',
  heartbeats: 'recent heartbeats and other outside input',
}

/** `switch` mode's segments: the words are the accessible names. */
const VIEW_LABEL: Record<TileView, string> = { terminal: 'Terminal', thread: 'Thread', chatter: 'Chatter', heartbeats: 'Heartbeats' }
const VIEW_TOOLTIP: Record<TileView, string> = {
  terminal: 'Terminal',
  thread: 'Thread',
  chatter: 'Chatter: messages between agents',
  heartbeats: 'Heartbeats: recent heartbeats and other outside input',
}

/** Where focus must be for Ctrl+` to switch a tile: the element carrying
 *  the tile's view (`data-tile-view`, ProjectDashboard). */
const TILE_SCOPE_SELECTOR = '[data-tile-view]'

type SegmentKind = ThreadDrawerKind | TileView

interface Segment {
  kind: SegmentKind
  on: boolean
  disabled: boolean
  label: string
  title: string
  icon: ReactNode
}

/** Thread view's drawers (today's mode). */
export interface ThreadDrawerSelectorDrawersProps {
  mode?: 'drawers'
  terminalRevealed: boolean
  /** No terminal yet (the session is starting or has ended). */
  terminalAvailable: boolean
  onToggleTerminal: () => void
  panel: ThreadPanelKind | null
  /** No `onPanel`: the terminal's segment only. */
  onPanel?: (next: ThreadPanelKind | null) => void
}

/** An agent tile's view picker (prd-projects-threads-v1 §3.1): Terminal,
 *  Thread, Chatter and Heartbeats switch the whole tile. Exactly one is
 *  selected. */
export interface ThreadDrawerSelectorSwitchProps {
  mode: 'switch'
  view: TileView
  onView: (next: TileView) => void
}

export type ThreadDrawerSelectorProps = ThreadDrawerSelectorDrawersProps | ThreadDrawerSelectorSwitchProps

/** Thread view's drawers in one capsule, beside the view menu: the
 *  terminal, Chatter and Heartbeats (Rosson, 2026-11-10: "merge those three
 *  into the selector system"). The page tabs' pattern (`PageTab`): a Glass
 *  capsule of 24px circles, square cells in Terminal. The open drawer's
 *  segment is selected; pressing it closes the drawer, pressing another
 *  switches (one drawer at a time, `useSessionViewTab`). One Tab stop; the
 *  arrow keys move between segments without opening anything. The terminal's
 *  segment is also what the Ctrl+` chord finds (`threadReveal.ts`).
 *
 *  `mode: 'switch'` is the same capsule in an agent tile's bar (Projects):
 *  Terminal, Thread, Chatter and Heartbeats as a radio group, one always
 *  selected.
 *  Pressing the selected one does nothing; pressing another switches the
 *  whole tile. It sits in the bar, so it is not a float. Ctrl+` with focus
 *  in the tile switches between Terminal and the last other view. */
export function ThreadDrawerSelector(props: ThreadDrawerSelectorProps): JSX.Element {
  const room = useRoom()
  const group = useRef<HTMLDivElement>(null)
  const terminalRef = useRef<HTMLButtonElement>(null)
  const switching = props.mode === 'switch'
  // The view Ctrl+` returns to from Terminal (Thread until another is picked).
  const lastOther = useRef<TileView>('thread')
  if (props.mode === 'switch' && props.view !== 'terminal') lastOther.current = props.view
  const latest = useRef(props)
  latest.current = props
  const combo = useSettingsStore((s) => effectiveKeybinding(s.keybindings, 'toggleThreadTerminal', isMacPlatform()))

  useEffect(() => {
    const element = terminalRef.current
    if (!element) return
    if (switching) {
      const pane = (): HTMLElement | null => element.closest<HTMLElement>(TILE_SCOPE_SELECTOR)
      return registerRevealTarget({
        room,
        element,
        pane,
        // Only the tile holding focus: a tile never takes the chord from
        // another tile or from the Agents page.
        enabled: () => {
          const active = typeof document === 'undefined' ? null : document.activeElement
          return !!active && !!pane()?.contains(active)
        },
        toggle: () => {
          const p = latest.current
          if (p.mode !== 'switch') return
          p.onView(p.view === 'terminal' ? lastOther.current : 'terminal')
        },
      })
    }
    return registerRevealTarget({
      room,
      element,
      enabled: () => latest.current.mode !== 'switch' && latest.current.terminalAvailable,
      toggle: () => {
        const p = latest.current
        if (p.mode !== 'switch') p.onToggleTerminal()
      },
    })
  }, [room, switching])

  const key = combo ? ` (${formatKeyCombo(combo, isMacPlatform())})` : ''
  let segments: Segment[]
  let choose: (kind: SegmentKind) => void
  if (props.mode === 'switch') {
    const { view, onView } = props
    segments = TILE_VIEWS.map((kind) => ({
      kind,
      on: view === kind,
      disabled: false,
      label: VIEW_LABEL[kind],
      title: VIEW_TOOLTIP[kind],
      icon: <ViewIcon kind={kind} />,
    }))
    choose = (kind) => {
      if (kind !== view) onView(kind as TileView)
    }
  } else {
    const { terminalRevealed, terminalAvailable, onToggleTerminal, panel, onPanel } = props
    const open = threadDrawerOpen(terminalRevealed, onPanel ? panel : null)
    const terminalOn = open === 'terminal'
    const terminalAction = terminalOn ? 'Hide terminal' : 'Reveal terminal'
    segments = [
      {
        kind: 'terminal',
        on: terminalOn,
        disabled: !terminalAvailable,
        label: terminalAction,
        title: terminalAvailable ? `${terminalAction}${key}` : 'No terminal is running yet',
        icon: <TerminalIcon />,
      },
      ...(onPanel ? THREAD_PANEL_KINDS : []).map((kind): Segment => {
        const on = open === kind
        const name = THREAD_PANEL_LABEL[kind]
        return {
          kind,
          on,
          disabled: false,
          label: on ? `Hide ${name}` : `Show ${name}`,
          title: on ? `Hide ${name}` : `${name}: ${PANEL_TOOLTIP[kind]}`,
          icon: kind === 'chatter' ? <ChatterIcon /> : <HeartbeatIcon />,
        }
      }),
    ]
    choose = (kind) => {
      if (kind === 'terminal') onToggleTerminal()
      else onPanel?.(open === kind ? null : (kind as ThreadPanelKind))
    }
  }

  const usable = segments.filter((s) => !s.disabled)
  // The Tab stop: the open drawer's (or the selected view's) segment, else
  // the first that works.
  const stop = usable.find((s) => s.on)?.kind ?? usable[0]?.kind

  // Arrows move focus only (Segmented's manual activation): opening a drawer
  // or switching a view is a click, Enter or Space (switching to Terminal
  // can reopen a socket).
  const onKeyDown = (e: KeyboardEvent<HTMLButtonElement>, from: SegmentKind): void => {
    const step = e.key === 'ArrowRight' || e.key === 'ArrowDown' ? 1 : e.key === 'ArrowLeft' || e.key === 'ArrowUp' ? -1 : 0
    if (!step) return
    e.preventDefault()
    const at = usable.findIndex((s) => s.kind === from)
    if (at < 0) return
    const next = usable[(at + step + usable.length) % usable.length]
    group.current?.querySelector<HTMLElement>(`[data-thread-drawer-segment="${next.kind}"]`)?.focus()
  }

  // A bar's capsule (the top bar's page tabs, PageTabs.tsx) in switch mode;
  // a floating object over the Thread in drawers mode.
  const form = switching
    ? { 'data-material': 'frost', 'data-frost-shape': 'pill', 'data-tab-mode': 'switch' }
    : floatProps(true, 'capsule')

  return (
    <div
      ref={group}
      role={switching ? 'radiogroup' : 'toolbar'}
      aria-label={switching ? 'View' : 'Side panels'}
      data-testid={switching ? 'tile-view-picker' : 'thread-drawer-selector'}
      data-page-tabs=""
      {...form}
      className="no-drag flex items-center flex-shrink-0"
    >
      {segments.map((s) => (
        <PageTab
          key={s.kind}
          ref={s.kind === 'terminal' ? terminalRef : undefined}
          selected={s.on}
          onSelect={() => choose(s.kind)}
          title={s.title}
          ariaLabel={s.label}
          disabled={s.disabled}
          tabIndex={s.kind === stop ? 0 : -1}
          onKeyDown={(e) => onKeyDown(e, s.kind)}
          // Keep focus where it is: a click must not pull it out of the
          // terminal before the hide decides where focus goes.
          onMouseDown={!switching && s.kind === 'terminal' ? (e) => e.preventDefault() : undefined}
          {...(switching ? { role: 'radio', 'aria-checked': s.on } : {})}
          data-thread-drawer-segment={s.kind}
          data-thread-panel-toggle={switching || s.kind === 'terminal' ? undefined : s.kind}
          data-testid={
            switching
              ? `tile-view-${s.kind}`
              : s.kind === 'terminal'
                ? 'thread-terminal-toggle'
                : `thread-panel-toggle-${s.kind}`
          }
        >
          {s.icon}
        </PageTab>
      ))}
    </div>
  )
}

function ViewIcon({ kind }: { kind: TileView }): JSX.Element {
  if (kind === 'terminal') return <TerminalIcon />
  if (kind === 'thread') return <ThreadIcon />
  if (kind === 'chatter') return <ChatterIcon />
  return <HeartbeatIcon />
}

function Icon({ children }: { children: ReactNode }): JSX.Element {
  return (
    <svg
      xmlns="http://www.w3.org/2000/svg"
      width="14"
      height="14"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="2"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      {children}
    </svg>
  )
}

function TerminalIcon(): JSX.Element {
  return (
    <Icon>
      <path d="M5 8l5 4-5 4" />
      <path d="M12 18h7" />
    </Icon>
  )
}

/** One speech bubble: you and the agent. */
function ThreadIcon(): JSX.Element {
  return (
    <Icon>
      <path d="M21 12a8 8 0 0 1-11.6 7.1L4 20l1-4.6A8 8 0 1 1 21 12z" />
    </Icon>
  )
}

function ChatterIcon(): JSX.Element {
  return (
    <Icon>
      <path d="M3 5h11v8H8l-3 3v-3H3z" />
      <path d="M17 9h4v8h-2v3l-3-3h-5v-1" />
    </Icon>
  )
}

function HeartbeatIcon(): JSX.Element {
  return (
    <Icon>
      <path d="M2 12h5l2-5 4 10 2-5h7" />
    </Icon>
  )
}
