import { useEffect, useRef, type JSX, type KeyboardEvent, type ReactNode } from 'react'
import { useRoom } from '@/components/Room/RoomContext'
import { PageTab } from '@/components/TopBar/PageTab'
import { useSettingsStore } from '@/stores/settings'
import { isMacPlatform } from '@/lib/desktop-chrome'
import { effectiveKeybinding, formatKeyCombo } from '@shared/hotkeys'
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

interface Segment {
  kind: ThreadDrawerKind
  on: boolean
  disabled: boolean
  label: string
  title: string
  icon: ReactNode
}

/** Thread view's drawers in one capsule, beside the view menu: the
 *  terminal, Chatter and Heartbeats (Rosson, 2026-11-10: "merge those three
 *  into the selector system"). The page tabs' pattern (`PageTab`): a Glass
 *  capsule of 24px circles, square cells in Terminal. The open drawer's
 *  segment is selected; pressing it closes the drawer, pressing another
 *  switches (one drawer at a time, `useSessionViewTab`). One Tab stop; the
 *  arrow keys move between segments without opening anything. The terminal's
 *  segment is also what the Ctrl+` chord finds (`threadReveal.ts`). */
export function ThreadDrawerSelector({
  terminalRevealed,
  terminalAvailable,
  onToggleTerminal,
  panel,
  onPanel,
}: {
  terminalRevealed: boolean
  /** No terminal yet (the session is starting or has ended). */
  terminalAvailable: boolean
  onToggleTerminal: () => void
  panel: ThreadPanelKind | null
  /** No `onPanel`: the terminal's segment only. */
  onPanel?: (next: ThreadPanelKind | null) => void
}): JSX.Element {
  const room = useRoom()
  const group = useRef<HTMLDivElement>(null)
  const terminalRef = useRef<HTMLButtonElement>(null)
  const latest = useRef({ terminalAvailable, onToggleTerminal })
  latest.current = { terminalAvailable, onToggleTerminal }
  const combo = useSettingsStore((s) => effectiveKeybinding(s.keybindings, 'toggleThreadTerminal', isMacPlatform()))

  useEffect(() => {
    const element = terminalRef.current
    if (!element) return
    return registerRevealTarget({
      room,
      element,
      enabled: () => latest.current.terminalAvailable,
      toggle: () => latest.current.onToggleTerminal(),
    })
  }, [room])

  const open = threadDrawerOpen(terminalRevealed, onPanel ? panel : null)
  const terminalOn = open === 'terminal'
  const terminalAction = terminalOn ? 'Hide terminal' : 'Reveal terminal'
  const key = combo ? ` (${formatKeyCombo(combo, isMacPlatform())})` : ''
  const segments: Segment[] = [
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

  const usable = segments.filter((s) => !s.disabled)
  // The Tab stop: the open drawer's segment, else the first that works.
  const stop = usable.find((s) => s.on)?.kind ?? usable[0]?.kind

  const choose = (kind: ThreadDrawerKind): void => {
    if (kind === 'terminal') onToggleTerminal()
    else onPanel?.(open === kind ? null : kind)
  }

  // Arrows move focus only (Segmented's manual activation): opening a drawer
  // is a click, Enter or Space.
  const onKeyDown = (e: KeyboardEvent<HTMLButtonElement>, from: ThreadDrawerKind): void => {
    const step = e.key === 'ArrowRight' || e.key === 'ArrowDown' ? 1 : e.key === 'ArrowLeft' || e.key === 'ArrowUp' ? -1 : 0
    if (!step) return
    e.preventDefault()
    const at = usable.findIndex((s) => s.kind === from)
    if (at < 0) return
    const next = usable[(at + step + usable.length) % usable.length]
    group.current?.querySelector<HTMLElement>(`[data-thread-drawer-segment="${next.kind}"]`)?.focus()
  }

  return (
    <div
      ref={group}
      role="toolbar"
      aria-label="Side panels"
      data-testid="thread-drawer-selector"
      data-page-tabs=""
      {...floatProps(true, 'capsule')}
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
          onMouseDown={s.kind === 'terminal' ? (e) => e.preventDefault() : undefined}
          data-thread-drawer-segment={s.kind}
          data-thread-panel-toggle={s.kind === 'terminal' ? undefined : s.kind}
          data-testid={s.kind === 'terminal' ? 'thread-terminal-toggle' : `thread-panel-toggle-${s.kind}`}
        >
          {s.icon}
        </PageTab>
      ))}
    </div>
  )
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
