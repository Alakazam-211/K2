// prd-zen-mode-v1 Z42–Z45 (answers 6, 10) — the Conversation widget: the
// selected agent's Thread, like a text conversation.
//
// The Thread comes through the bridge (`thread.subscribe`, `thread.read`),
// which runs the existing overlay Thread hook on the agent's own server.
// Rows reuse the Thread's own pieces in a new layout: the message body
// renderer (`ChatMessageBody`), the choice card and the secret card. The
// user's messages sit right, the agent's left; cards are inline and
// tappable (`thread.answer`, `thread.void`). While the agent works, a typing
// indicator shows under the last message. When it waits on a permission
// prompt, a status line says so (a fact, no button: K2 never steers people
// to answer in the terminal). Older messages load on scroll. The message
// box is `ZenCompose`.
//
// prd-zen-gardens-v1 G38 (Rosson's answer 5): it follows an Agents widget
// (`agents` prop, else the page's first), or, with `agent` (and `home`),
// is pinned to that one agent with no list needed. Display props:
// `compose`, `attachments`, `load-older` (the daemon sends every prop).
//
// 0.45.6 (Rosson 2026-10-15, from K2 Companion): with `activity-drawer` (on
// in Garden 1) the live Thread turn shows in `ZenActivityDrawer`, pulled
// out of the message box's top edge, instead of the typing dots. 0.46.5
// (Rosson 2026-10-27): always, on any server that reports Thread turns
// (with no message box, as a tile under the last message): the dots said
// "typing" while the turn was held by a subagent or a background task.
//
// 0.46.1 (thread-read-v1, Zen defaults): the conversation marks its Thread
// read while it is looked at, says how many cards wait on you ("2
// waiting", a click scrolls to the first) and, with `mute` (on in Garden
// 1), has a bell to mute the Thread (`ZenThreadReadControls`).
//
// Fleet Ops 2 (Rosson 2026-10-24): with `pick` (2 to 8 agents) the header
// is the drop-down, one layer: the shown agent plus a chevron and a badge
// with the others' unread and waiting counts. A click (Enter, Space, ↓)
// opens a panel over the Thread, from the header's top edge down, with one
// card per agent in `pick` order, each drawn like the header
// (`ZenConversationHead`). ↑/↓ move, Enter picks, Esc / a click outside
// closes. An agent nothing answers to is a disabled card. Which one shows
// is per Garden and widget on this computer (`zen-conversation-pick.ts`).
//
// The Kanban's Thread (Rosson 2026-10-24, for 0.46.4): `pick = "all"` lists
// every agent the conversation can show (what it follows with `agents`, a
// custom widget too, else the Garden's reach), with a search field on top
// of the menu and no badge. The shown agent is the view's selection, so a
// custom widget it follows reads it as its `selected` row (the Kanban's
// Send board) and moves it with `conversation.open`; the saved pick follows
// it and reopens it after a reload. Before anyone is picked the header is
// "Pick an agent" with the same menu.

import { useCallback, useEffect, useId, useLayoutEffect, useMemo, useRef, useState } from 'react'
import { useStore } from 'zustand'
import { zenThreadFeeds } from '@/lib/zen/zen-data'
import { ThreadAttachmentSourceProvider, type ThreadAttachmentSource } from '@/components/Thread/ThreadImageObjects'
import { useStickToBottom } from '@/hooks/useStickToBottom'
import { formatRelativeTime } from '@/lib/format-relative-time'
import { isOwnThreadPost, type ThreadViewer } from '@/lib/thread-viewer'
import type { OverlayThreadItem, ThreadAnswerPayload } from '@/components/SessionView/overlayThread'
import {
  FOOTER_FADE_PX,
  FOOTER_FADE_TARGETS,
  THREAD_CARD_TOKENS,
  ThreadBubble,
  ThreadLoadOlder,
  ThreadTypingIndicator,
  threadFadeTop,
  zenEmptyThreadText,
} from '@/components/Thread/ThreadZenKit'
import { ThreadMessageColumn } from '@/components/Thread/ThreadColumn'
import { WorkspaceFileLinks } from '@/components/common/WorkspaceFileLinks'
import { useZenMaterials } from '@/lib/zen/zen-materials'
import type { ZenAgentRow } from '@/lib/zen/zen-data'
import { ZEN_TILE_RAISED_PROPS } from '@/lib/zen/zen-materials'
import { useZenConversationPicksStore, zenConversationPick, zenConversationPickAll, zenFindPickRow } from '@/lib/zen/zen-conversation-pick'
import { zenGardenHomeKey } from '@/lib/zen/zen-garden-homes'
import type { ZenWidgetBridge } from '@/lib/zen/zen-bridge'
import type { ZenWidgetProps } from '../zen-registry'
import { ZenCompose } from './ZenCompose'
import { ZenActivityDrawer } from './ZenActivityDrawer'
import { ZenStatusDot, ZenUnreadPills } from './ZenAgentsWidget'
import { ZEN_DROPDOWN_STYLE } from './ZenChromeControls'
import { ZenProgressiveBlur } from './ZenProgressiveBlur'
import { ZenMuteButton, ZenWaitingChip, useZenThreadAutoRead, useZenThreadEntry, zenCardWaiting } from './ZenThreadReadControls'
import { ZenAgentAvatar, ZenWidgetStyles, initials, useNowSec, useZenRows, useZenThread } from './zen-widget-kit'

// The bubbles, cards, dots and "Earlier messages" are the shared Thread
// pieces (`components/Thread/ThreadZenKit`), which the desktop Thread draws
// too, so the two stay the same.
export { zenEmptyThreadText }
const CARD_TOKENS = THREAD_CARD_TOKENS

export const zenPermissionText = (label: string): string => `${label} is stuck on a permission prompt.`

function Bubble({
  item,
  viewer,
  nowSec,
  server,
  onAnswer,
  onVoid,
  onMoreOptions,
}: {
  item: OverlayThreadItem
  /** Who is looking: "You" only on their own posts (a Thread is shared). */
  viewer: ThreadViewer | null
  nowSec: number
  server?: string
  /** Resolves when the daemon took the answer; throws its refusal. */
  onAnswer(payload: ThreadAnswerPayload): Promise<void>
  onVoid(): void
  /** The full secret card, in the agent's Thread pane. */
  onMoreOptions(): void
}): React.JSX.Element {
  const mine = isOwnThreadPost(item.doc, viewer)
  return (
    <ThreadBubble
      item={item}
      mine={mine}
      author={mine ? 'You' : item.doc.from || 'agent'}
      timeLabel={formatRelativeTime(item.doc.created_at, nowSec)}
      viewer={viewer}
      nowSec={nowSec}
      onAnswer={onAnswer}
      onVoid={onVoid}
      // prd-k2-vault-v1 KV9: the compact card (the proposal's defaults),
      // same answer body; More options opens the full one.
      secret={{ compact: true, server, onMoreOptions }}
    />
  )
}

const TypingIndicator = ThreadTypingIndicator

/** One agent's conversation (also the empty Garden's Ask my agent). */
export interface ZenConversationOptions {
  compose: boolean
  attachments: boolean
  loadOlder: boolean
  /** 0.45.6 Garden 1: the activity drawer above the box (`activity-drawer`).
   *  From 0.46.5 the drawer always replaces the typing dots on a server
   *  that reports Thread turns; the prop is still read, for older Gardens. */
  activityDrawer?: boolean
  /** 0.46.1 Garden 1: the bell that mutes this Thread (`mute`). */
  mute?: boolean
}

const ALL_ON: ZenConversationOptions = { compose: true, attachments: true, loadOlder: true }

/** How far (px) the top bar's blur reaches below the bar, fading out. */
export const ZEN_HEADER_BLUR_TAIL_PX = 16

interface ZenFloatInsets {
  /** The floating top bar's height: the list pads its top by it. */
  top: number
  /** The floating message box's slot height: the list pads its bottom by it. */
  bottom: number
  /** The footer blur's top in the slot (`threadFadeTop`), null unmeasured. */
  fade: number | null
}
const NO_INSETS: ZenFloatInsets = { top: 0, bottom: 0, fade: null }

/**
 * 0.46.0, Glass (K2 Companion's materials): the top bar and the message box
 * float over the list, each on a progressive blur, and the list runs under
 * both (padded by their heights, so the first and newest messages still rest
 * clear of them). Measured with one ResizeObserver on each. Other themes:
 * nothing floats (all zero).
 */
function useZenFloatInsets(
  on: boolean,
  header: React.RefObject<HTMLDivElement | null>,
  slot: React.RefObject<HTMLDivElement | null>,
): ZenFloatInsets {
  const [insets, setInsets] = useState<ZenFloatInsets>(NO_INSETS)
  useLayoutEffect(() => {
    if (!on) {
      setInsets(NO_INSETS)
      return
    }
    const measure = (): void => {
      const top = Math.ceil(header.current?.getBoundingClientRect().height ?? 0)
      const el = slot.current
      let bottom = 0
      let fade: number | null = null
      if (el) {
        const r = el.getBoundingClientRect()
        bottom = Math.ceil(r.height)
        const tops = FOOTER_FADE_TARGETS.flatMap((sel) => {
          const node = el.querySelector(sel)
          return node ? [node.getBoundingClientRect().top] : []
        })
        fade = threadFadeTop(r.top, tops)
      }
      setInsets((cur) => (cur.top === top && cur.bottom === bottom && cur.fade === fade ? cur : { top, bottom, fade }))
    }
    measure()
    if (typeof ResizeObserver === 'undefined') return
    const ro = new ResizeObserver(measure)
    if (header.current) ro.observe(header.current)
    if (slot.current) ro.observe(slot.current)
    return () => ro.disconnect()
  }, [on, header, slot])
  return insets
}

/** 0.46.3: paths in a Garden Thread that are files in the agent's
 *  workspace are links (and its file chips open), like the main Thread.
 *  The live feed for this row names the server and the Thread address;
 *  before it exists the messages render unlinked. */
export function ZenFileLinks({ address, children }: { address: string; children: React.ReactNode }): React.JSX.Element {
  const feed = useStore(zenThreadFeeds, (s) => s.feeds.find((f) => f.address === address))
  if (!feed) return <>{children}</>
  return (
    <WorkspaceFileLinks workspace={feed.threadAddr} scope={feed.scope}>
      {children}
    </WorkspaceFileLinks>
  )
}

/** The header's second line: the server, then the state or activity. */
function zenConversationStatusText(row: ZenAgentRow): string {
  const second =
    row.state !== 'ok'
      ? row.stateLabel
      : row.activity === 'needs-you'
        ? 'needs you'
        : row.activity === 'unverifiable'
          ? 'no update'
          : row.activity
  return [row.server ?? 'this computer', second].filter(Boolean).join(' · ')
}

/** One agent as the conversation header draws it: the 34 px picture, the
 *  name, the status dot and the server / activity line. The header and
 *  each card of the `pick` menu (`card`: the card's own marks). Its parent
 *  lays it out (`flex items-center gap-3`). */
export function ZenConversationHead({ row, card = false }: { row: ZenAgentRow; card?: boolean }): React.JSX.Element {
  const status = row.activity ?? row.state
  return (
    <>
      <ZenAgentAvatar
        label={row.label}
        url={row.avatarUrl}
        size={34}
        agentColor={row.color}
        background="var(--zen-accent)"
        color="var(--zen-accent-text)"
        fontSize="0.9em"
      />
      <span className="flex min-w-0 flex-col">
        <span className="truncate" style={{ fontWeight: 600 }} {...(card ? { 'data-zen-pick-title': '' } : { 'data-zen-conversation-title': '' })}>
          {row.label}
        </span>
        <span className="flex items-center gap-1.5 truncate" style={{ fontSize: '0.8em', color: 'var(--zen-text-muted)' }}>
          <ZenStatusDot row={row} size={8} />
          <span {...(card ? { 'data-zen-pick-status': status } : { 'data-zen-conversation-status': status })}>
            {zenConversationStatusText(row)}
          </span>
        </span>
      </span>
    </>
  )
}

/** One agent of `pick`: its row, or null when nothing answers to the name. */
export interface ZenPickEntry {
  name: string
  row: ZenAgentRow | null
}

/** A `pick` conversation's agents and the switch. */
export interface ZenConversationPicker {
  entries: readonly ZenPickEntry[]
  onPick(address: string): void
  /** Set by a pick: the next conversation (a new mount) focuses its header,
   *  so the keyboard stays where it was. */
  refocus?: { current: boolean }
  /** `pick = "all"`: a search field tops the menu (it may list every agent). */
  search?: boolean
  /** The header's badge of the other agents' unread and waiting (default
   *  on; off for `pick = "all"`, where "the others" is everyone). */
  badge?: boolean
}

/** The second line of a card for an agent that isn't found. */
export const zenPickMissingText = (name: string): string => `${name} isn’t on this Home.`

/** `pick = "all"` before anyone is picked: the header and the body. */
export const ZEN_PICK_NONE_LABEL = 'Pick an agent'
export const ZEN_PICK_NONE_TEXT = 'Pick an agent above to talk to them here.'
/** The search field's empty answer. */
export const zenPickNoMatchText = (q: string): string => (q ? `No agent matches “${q}”.` : 'No agents here yet.')

/** Does the card for `row` match the search `q` (lower case)? Its name, its
 *  address or its server. */
export function zenPickMatches(row: ZenAgentRow, q: string): boolean {
  if (!q) return true
  return [row.label, row.address, row.server ?? 'this computer'].some((s) => s.toLocaleLowerCase().includes(q))
}

/** The other agents' unread and waiting, summed for the header's badge. A
 *  muted Thread's unread is left out (it never nags); its waiting cards
 *  still count. */
export function zenPickBadge(entries: readonly ZenPickEntry[], shown: string): NonNullable<ZenAgentRow['unread']> {
  let unread = 0
  let waiting = 0
  for (const e of entries) {
    if (!e.row || e.row.address === shown || !e.row.unread) continue
    if (!e.row.unread.muted) unread += e.row.unread.unread
    waiting += e.row.unread.waiting
  }
  return { unread, waiting, muted: false }
}

function Chevron({ open }: { open: boolean }): React.JSX.Element {
  return (
    <svg
      width="12"
      height="12"
      viewBox="0 0 12 12"
      aria-hidden
      data-zen-pick-chevron=""
      style={{ flex: 'none', color: 'var(--zen-text-muted)', transform: open ? 'rotate(180deg)' : undefined }}
    >
      <path d="M2.5 4.5 L6 8 L9.5 4.5" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round" />
    </svg>
  )
}

/** A card's box: the header row's padding, so each card sits like it. */
const CARD_STYLE: React.CSSProperties = { padding: '12px 18px', cursor: 'pointer' }

/** The header's drop-down button: the header row's look, a hover wash. */
const PICK_BUTTON_STYLE: React.CSSProperties = {
  margin: '-4px -6px',
  padding: '4px 6px',
  border: 'none',
  borderRadius: 'var(--zen-radius)',
  background: 'transparent',
  color: 'inherit',
  font: 'inherit',
  textAlign: 'left',
  cursor: 'pointer',
}

/** The `pick` menu: a panel over the Thread from the header's top edge
 *  down, one card per agent; a listbox (↑/↓ move, Home/End, Enter or Space
 *  pick, Esc or Tab close). With `search` (`pick = "all"`) a search field
 *  sits on top and keeps the focus (a combobox: the same keys, but Space
 *  types and Home/End move in the field); it filters by name, address or
 *  server. */
function ZenPickMenu({
  id,
  picker,
  shown,
  buttonRef,
  onClose,
}: {
  id: string
  picker: ZenConversationPicker
  shown: string
  buttonRef: React.RefObject<HTMLButtonElement | null>
  onClose(focusButton: boolean): void
}): React.JSX.Element {
  const { onPick, search } = picker
  const [query, setQuery] = useState('')
  const q = query.trim().toLocaleLowerCase()
  // Each card keeps its index in `picker.entries` (the option ids), so the
  // search only hides cards.
  const visible = picker.entries.flatMap((e, i) => (!search || (e.row && zenPickMatches(e.row, q)) ? [{ e, i }] : []))
  const enabled = visible.flatMap(({ e, i }) => (e.row ? [i] : []))
  const entries = picker.entries
  const [active, setActive] = useState(() => {
    const cur = entries.findIndex((e) => e.row?.address === shown)
    return cur >= 0 ? cur : (enabled[0] ?? -1)
  })
  // A new search: the first match is the keyboard's.
  const onQuery = (next: string): void => {
    setQuery(next)
    const nq = next.trim().toLocaleLowerCase()
    const first = entries.findIndex((e) => e.row !== null && zenPickMatches(e.row, nq))
    setActive(first)
  }
  const panelRef = useRef<HTMLDivElement | null>(null)
  const listRef = useRef<HTMLDivElement | null>(null)
  const searchRef = useRef<HTMLInputElement | null>(null)
  useEffect(() => {
    if (search) searchRef.current?.focus()
    else listRef.current?.focus()
  }, [search])
  // A click outside the panel and the header's button closes it.
  useEffect(() => {
    const onDown = (e: MouseEvent): void => {
      const t = e.target as Node | null
      if (t && (panelRef.current?.contains(t) || buttonRef.current?.contains(t))) return
      onClose(false)
    }
    document.addEventListener('mousedown', onDown)
    return () => document.removeEventListener('mousedown', onDown)
  }, [buttonRef, onClose])
  useEffect(() => {
    if (active < 0) return
    listRef.current?.querySelector<HTMLElement>(`[data-zen-pick-index="${active}"]`)?.scrollIntoView?.({ block: 'nearest' })
  }, [active])
  const pick = (i: number): void => {
    const row = entries[i]?.row
    if (!row || !enabled.includes(i)) return
    onClose(true)
    if (row.address !== shown) onPick(row.address)
  }
  const move = (dir: 1 | -1): void => {
    if (enabled.length === 0) return
    const at = enabled.indexOf(active)
    const next = at < 0 ? (dir === 1 ? 0 : enabled.length - 1) : (at + dir + enabled.length) % enabled.length
    setActive(enabled[next])
  }
  const onKeyDown = (e: React.KeyboardEvent): void => {
    if (e.key === 'Tab') {
      onClose(false)
      return
    }
    if (e.key === 'ArrowDown') move(1)
    else if (e.key === 'ArrowUp') move(-1)
    // In the search field Home, End and Space are the field's.
    else if (!search && e.key === 'Home') setActive(enabled[0] ?? -1)
    else if (!search && e.key === 'End') setActive(enabled[enabled.length - 1] ?? -1)
    else if (e.key === 'Enter' || (!search && e.key === ' ')) pick(active)
    else if (e.key === 'Escape') onClose(true)
    else return
    e.preventDefault()
    e.stopPropagation()
  }
  const activeId = enabled.includes(active) ? `${id}-${active}` : undefined
  return (
    <div
      ref={panelRef}
      data-zen-pick-menu=""
      data-zen-pick-search={search ? '' : undefined}
      {...ZEN_TILE_RAISED_PROPS}
      className="absolute overflow-y-auto"
      // Zen's drop-down look (the Garden switcher's), edge to edge.
      style={{ ...ZEN_DROPDOWN_STYLE, padding: 0, top: 0, left: 0, right: 0, maxHeight: '100%', zIndex: 4 }}
    >
      {search && (
        <div
          className="sticky top-0"
          style={{ zIndex: 1, padding: '10px 12px', background: 'var(--zen-surface-raised)', borderBottom: '1px solid var(--zen-border)' }}
        >
          <input
            ref={searchRef}
            type="text"
            role="combobox"
            aria-expanded="true"
            aria-controls={id}
            aria-autocomplete="list"
            aria-activedescendant={activeId}
            aria-label="Search agents"
            placeholder="Search agents…"
            autoComplete="off"
            spellCheck={false}
            data-zen-pick-search-input=""
            value={query}
            onChange={(e) => onQuery(e.target.value)}
            onKeyDown={onKeyDown}
            className="w-full"
            style={{
              padding: '6px 10px',
              border: '1px solid var(--zen-border)',
              borderRadius: 'var(--zen-radius)',
              background: 'var(--zen-surface)',
              color: 'var(--zen-text)',
              font: 'inherit',
              outline: 'none',
            }}
          />
        </div>
      )}
      <div
        ref={listRef}
        id={id}
        role="listbox"
        aria-label="Switch agent"
        tabIndex={-1}
        aria-activedescendant={search ? undefined : activeId}
        onKeyDown={search ? undefined : onKeyDown}
        className="flex flex-col"
        style={{ outline: 'none' }}
      >
        {search && visible.length === 0 && (
          <div data-zen-pick-empty="" style={{ ...CARD_STYLE, cursor: 'default', color: 'var(--zen-text-muted)', textAlign: 'center' }}>
            {zenPickNoMatchText(query.trim())}
          </div>
        )}
        {visible.map(({ e, i }, at) => {
          const sep = at > 0 ? { borderTop: '1px solid var(--zen-border)' } : null
          if (!e.row) {
            return (
              <div
                key={`missing:${i}`}
                id={`${id}-${i}`}
                role="option"
                aria-selected={false}
                aria-disabled="true"
                data-zen-pick-option=""
                data-zen-pick-missing={e.name}
                data-zen-pick-index={i}
                className="flex items-center gap-3"
                style={{ ...CARD_STYLE, ...sep, cursor: 'default', opacity: 0.55 }}
              >
                <ZenAgentAvatar label={e.name} url={null} size={34} background="var(--zen-surface)" color="var(--zen-text-muted)" fontSize="0.9em" />
                <span className="flex min-w-0 flex-col">
                  <span className="truncate" style={{ fontWeight: 600 }}>
                    {e.name}
                  </span>
                  <span className="truncate" style={{ fontSize: '0.8em', color: 'var(--zen-text-muted)' }}>
                    {zenPickMissingText(e.name)}
                  </span>
                </span>
              </div>
            )
          }
          const row = e.row
          const selected = row.address === shown
          return (
            <div
              key={row.address}
              id={`${id}-${i}`}
              role="option"
              aria-selected={selected}
              data-zen-pick-option={row.address}
              data-zen-pick-index={i}
              data-active={i === active ? '' : undefined}
              className="flex items-center gap-3"
              style={{ ...CARD_STYLE, ...sep, boxShadow: selected ? 'inset 3px 0 0 var(--zen-accent)' : undefined }}
              onMouseEnter={() => setActive(i)}
              onClick={() => pick(i)}
            >
              <ZenConversationHead row={row} card />
              <span className="ml-auto flex flex-shrink-0 items-center gap-2">
                {row.unread ? <ZenUnreadPills unread={row.unread} /> : null}
                {selected && (
                  <svg width="14" height="14" viewBox="0 0 14 14" aria-hidden data-zen-pick-check="" style={{ color: 'var(--zen-accent)' }}>
                    <path d="M3 7.4 L5.8 10 L11 4.5" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round" />
                  </svg>
                )}
              </span>
            </div>
          )
        })}
      </div>
    </div>
  )
}

export function ZenConversation({
  bridge,
  row,
  options = ALL_ON,
  picker,
}: {
  bridge: ZenWidgetBridge
  row: ZenAgentRow
  options?: ZenConversationOptions
  /** `pick`: the header is the drop-down that switches agents. */
  picker?: ZenConversationPicker
}): React.JSX.Element {
  const view = useZenThread(bridge, row.address)
  const nowSec = useNowSec()
  const rootRef = useRef<HTMLDivElement | null>(null)
  const headerRef = useRef<HTMLDivElement | null>(null)
  const slotRef = useRef<HTMLDivElement | null>(null)
  const floating = useZenMaterials().materials === 'mobile'
  const insets = useZenFloatInsets(floating, headerRef, slotRef)
  const items = view?.items ?? []
  const ready = view?.phase === 'ready'
  // 0.46.5 (Rosson 2026-10-27: "the stuck typing bubble shouldn't be a
  // bubble, it should show what's actually going on"): a server that
  // reports Thread turns gets the activity drawer, never the typing dots:
  // the step, the subagents and background tasks by name, the clock. On
  // the message box when there is one, else as a tile under the last
  // message. Nothing tells us the agent is composing its reply, so there
  // are no dots for it. Only an older server (no turns, only the row's
  // `working` rollup) keeps the dots: it has nothing else to show.
  const drawerTurn = view?.liveTurn !== undefined ? view.liveTurn : undefined
  const drawerInList = drawerTurn !== undefined && !options.compose
  const typing = drawerTurn === undefined && view?.turn === undefined && row.working

  // The card awaits this: a secret card's refusal (a red warning, a scope
  // only an admin may pick) shows on the card. Never logs the payload.
  const answer = useCallback(
    async (id: string, payload: ThreadAnswerPayload): Promise<void> => {
      await (bridge.call('thread.answer', row.address, id, payload) as Promise<unknown>)
    },
    [bridge, row.address],
  )
  const moreOptions = useCallback(() => {
    void Promise.resolve()
      .then(() => bridge.call('conversation.open', row.address, { where: 'agents' }))
      .catch((err: unknown) => console.warn('[zen] open in Agents failed:', err))
  }, [bridge, row.address])
  const dismiss = useCallback(
    (id: string) => {
      void (bridge.call('thread.void', row.address, id) as Promise<unknown>).catch((err: unknown) =>
        console.warn('[zen] dismiss failed:', err),
      )
    },
    [bridge, row.address],
  )
  // Stick to the list's true bottom (the working dots included) while the
  // person is there; never snap them once they scrolled up. Resizes — the
  // message box growing as they type — re-pin through a ResizeObserver
  // (useStickToBottom), not through renders.
  const holdRef = useRef<() => void>(() => {})
  const loadOlder = useCallback(() => {
    if (!options.loadOlder || !view || !view.hasMore || view.loadingOlder || items.length === 0) return
    holdRef.current()
    const minSeq = items.reduce((m, it) => Math.min(m, it.seq), Number.POSITIVE_INFINITY)
    void (bridge.call('thread.read', row.address, { beforeSeq: minSeq }) as Promise<unknown>).catch((err: unknown) =>
      console.warn('[zen] load older failed:', err),
    )
  }, [bridge, row.address, view, items, options.loadOlder])

  const stick = useStickToBottom<HTMLDivElement>({
    deps: [items, typing, ready, view?.note, view?.hasMore, view?.error, insets.top, insets.bottom],
    extraTargets: () => [rootRef.current?.querySelector('[data-zen-compose]')],
    onNearTop: loadOlder,
  })
  holdRef.current = stick.holdForPrepend

  // A new conversation (the empty Garden reuses this one) starts at its end.
  const { scrollToBottom } = stick
  useLayoutEffect(() => {
    scrollToBottom()
  }, [row.address, scrollToBottom])

  // S3 (thread-images-v1): image attachments read their pictures from the
  // agent's own server, by its Thread address (the feed Zen mounted for
  // this conversation). No feed yet: images stay chips.
  const feed = useStore(zenThreadFeeds, (s) => s.feeds.find((f) => f.address === row.address) ?? null)
  const feedScope = feed?.scope ?? null
  const feedAddr = feed?.threadAddr ?? ''
  const attachmentSource = useMemo<ThreadAttachmentSource | null>(
    () => (feedScope && feedAddr ? { scope: feedScope, addr: feedAddr } : null),
    [feedScope, feedAddr],
  )

  const note = view?.note ?? null
  const showEmpty = ready && view.loaded && !view.error && items.length === 0
  // thread-read-v1: read while looked at; the cards waiting on you.
  useZenThreadAutoRead(view, note === null)
  const readEntry = useZenThreadEntry(view)
  const waitingInView = ready ? items.filter(zenCardWaiting) : []
  const waiting = Math.max(readEntry?.waiting ?? 0, waitingInView.length)
  const showWaiting = useCallback(() => {
    const first = waitingInView[0]
    if (!first) {
      // Older than the loaded page: page back toward it.
      loadOlder()
      return
    }
    const el = Array.from(rootRef.current?.querySelectorAll<HTMLElement>('[data-zen-item]') ?? []).find(
      (n) => n.getAttribute('data-zen-item') === first.id,
    )
    const box = el?.firstElementChild as HTMLElement | null | undefined
    ;(box ?? el)?.scrollIntoView?.({ block: 'center', behavior: 'smooth' })
  }, [waitingInView, loadOlder])
  // `pick`: the header's drop-down.
  const menuId = useId()
  const pickButtonRef = useRef<HTMLButtonElement | null>(null)
  const [menuOpen, setMenuOpen] = useState(false)
  const closeMenu = useCallback((focusButton: boolean) => {
    setMenuOpen(false)
    if (focusButton) pickButtonRef.current?.focus()
  }, [])
  const badge = picker && picker.badge !== false ? zenPickBadge(picker.entries, row.address) : null
  const refocus = picker?.refocus
  useEffect(() => {
    if (!refocus?.current) return
    refocus.current = false
    pickButtonRef.current?.focus()
  }, [refocus])
  return (
    <div
      ref={rootRef}
      className="relative flex h-full min-h-0 w-full flex-col"
      data-zen-conversation={row.address}
      data-zen-floating={floating ? '' : undefined}
      style={CARD_TOKENS}
    >
      <div
        ref={headerRef}
        data-zen-conversation-header=""
        className="flex-shrink-0"
        style={floating ? { position: 'absolute', top: 0, left: 0, right: 0, zIndex: 2 } : undefined}
      >
      {floating && <ZenProgressiveBlur edge="header" style={{ top: 0, bottom: -ZEN_HEADER_BLUR_TAIL_PX }} />}
      <div
        className={`${floating ? 'relative ' : ''}flex flex-shrink-0 items-center gap-3`}
        style={floating ? { zIndex: 1, padding: '12px 18px' } : { padding: '12px 18px', borderBottom: '1px solid var(--zen-border)' }}
      >
        {picker ? (
          <button
            ref={pickButtonRef}
            type="button"
            data-zen-pick-button=""
            aria-haspopup="listbox"
            aria-expanded={menuOpen}
            aria-controls={menuOpen ? menuId : undefined}
            title="Switch agent"
            className="flex min-w-0 items-center gap-3"
            style={PICK_BUTTON_STYLE}
            onClick={() => setMenuOpen((o) => !o)}
            onKeyDown={(e) => {
              if (e.key === 'ArrowDown' && !menuOpen) {
                e.preventDefault()
                setMenuOpen(true)
              }
            }}
          >
            <ZenConversationHead row={row} />
            <Chevron open={menuOpen} />
            {badge && (badge.unread > 0 || badge.waiting > 0) && (
              <span data-zen-pick-badge="" title="Unread and waiting with your other agents here">
                <ZenUnreadPills unread={badge} />
              </span>
            )}
          </button>
        ) : (
          <ZenConversationHead row={row} />
        )}
        <span className="ml-auto flex flex-shrink-0 items-center gap-2" data-zen-conversation-actions="">
        <ZenWaitingChip waiting={waiting} onShow={showWaiting} />
        {options.mute && <ZenMuteButton view={view} />}
        {row.people.length > 0 && (
          <span className="flex -space-x-1.5" data-zen-people="">
            {row.people.slice(0, 4).map((p) => (
              <span
                key={p.user}
                title={`${p.name} is here`}
                className="flex items-center justify-center"
                style={{
                  width: 24,
                  height: 24,
                  borderRadius: 999,
                  fontSize: '0.7em',
                  background: 'var(--zen-surface-raised)',
                  border: '2px solid var(--zen-surface)',
                  color: 'var(--zen-text)',
                }}
              >
                {initials(p.name)}
              </span>
            ))}
          </span>
        )}
        </span>
      </div>

      {row.needsYou && ready && (
        // States the fact only. K2 never steers people to answer prompts in
        // a terminal (no human-in-the-loop flows), so there is no button.
        <div
          role="status"
          data-zen-permission=""
          className={`${floating ? 'relative ' : ''}flex flex-shrink-0 items-center gap-2`}
          style={{
            ...(floating ? { zIndex: 1 } : {}),
            margin: floating ? '0 var(--zen-gap) 6px' : '10px var(--zen-gap) 0',
            padding: '7px 12px',
            borderRadius: 'var(--zen-radius)',
            background: 'var(--zen-surface-raised)',
            borderLeft: '3px solid var(--zen-needs-you)',
            color: 'var(--zen-text)',
            fontSize: '0.85em',
          }}
        >
          <span
            aria-hidden="true"
            className="flex-shrink-0"
            style={{ width: 7, height: 7, borderRadius: 999, background: 'var(--zen-needs-you)' }}
          />
          <span className="min-w-0 flex-1 truncate">{zenPermissionText(row.label)}</span>
        </div>
      )}
      </div>

      <div
        ref={stick.ref}
        data-zen-conversation-body=""
        className="relative flex min-h-0 flex-1 flex-col overflow-y-auto"
        // The browser's own scroll anchoring would fight the pin.
        style={{
          padding: '14px var(--zen-gap) 8px',
          paddingTop: 14 + insets.top,
          paddingBottom: 8 + insets.bottom,
          gap: 10,
          overflowAnchor: 'none',
        }}
        onScroll={stick.onScroll}
      >
        {!ready && (
          <div
            className="m-auto text-center"
            data-zen-conversation-note={view?.phase ?? 'opening'}
            style={{ color: 'var(--zen-text-muted)', maxWidth: '36ch' }}
          >
            {note ?? `Opening ${row.label}…`}
          </div>
        )}
        {ready && note && (
          <div data-zen-conversation-note="refused" style={{ color: 'var(--zen-danger)', textAlign: 'center' }}>
            {note}
          </div>
        )}
        {ready && view.hasMore && options.loadOlder && <ThreadLoadOlder loading={view.loadingOlder} onClick={loadOlder} />}
        {ready && view.error && !note && (
          <div style={{ color: 'var(--zen-text-muted)', fontSize: '0.85em', textAlign: 'center' }}>{view.error}</div>
        )}
        {showEmpty && (
          <div className="m-auto text-center" data-zen-thread-empty="" style={{ color: 'var(--zen-text-muted)' }}>
            {zenEmptyThreadText(row.label)}
          </div>
        )}
        {/* Narrow (< 480px, a phone): bubbles use the column (ThreadColumn). */}
        {ready && (items.length > 0 || typing) && (
          <ThreadAttachmentSourceProvider value={attachmentSource}>
          <ThreadMessageColumn gap={10} className="flex flex-shrink-0 flex-col" testId="zen-conversation-column">
          {items.map((it) => (
            // `display: contents`: the list's gap and layout see the bubble;
            // the id lets "N waiting" scroll to a card.
            <div key={it.id} data-zen-item={it.id} data-zen-item-waiting={zenCardWaiting(it) ? '' : undefined} style={{ display: 'contents' }}>
              <Bubble
                item={it}
                viewer={view?.viewer ?? null}
                nowSec={nowSec}
                server={row.server ?? 'this computer'}
                onAnswer={(payload) => answer(it.id, payload)}
                onVoid={() => dismiss(it.id)}
                onMoreOptions={moreOptions}
              />
            </div>
          ))}
          {typing && <TypingIndicator />}
          {drawerInList && ready && <ZenActivityDrawer turn={drawerTurn ?? null} attached={false} />}
          </ThreadMessageColumn>
          </ThreadAttachmentSourceProvider>
        )}
      </div>

      {options.compose && (
        <div
          ref={slotRef}
          data-zen-compose-slot=""
          className="flex-shrink-0"
          style={floating ? { position: 'absolute', left: 0, right: 0, bottom: 0, zIndex: 2 } : undefined}
        >
          {floating && <ZenProgressiveBlur edge="footer" style={{ top: insets.fade ?? -FOOTER_FADE_PX, bottom: 0 }} />}
          <div className={floating ? 'relative' : undefined} style={floating ? { zIndex: 1 } : undefined}>
            <ZenCompose
              bridge={bridge}
              address={row.address}
              label={row.label}
              disabled={!ready || note !== null}
              attachments={options.attachments}
              onSend={scrollToBottom}
              drawer={drawerTurn !== undefined && ready ? <ZenActivityDrawer turn={drawerTurn} /> : undefined}
            />
          </div>
        </div>
      )}
      {picker && menuOpen && (
        <ZenPickMenu id={menuId} picker={picker} shown={row.address} buttonRef={pickButtonRef} onClose={closeMenu} />
      )}
    </div>
  )
}

/** `pick = "all"` with no one shown yet: the header is the drop-down
 *  ("Pick an agent"), the same menu as a shown agent's header. */
function ZenConversationPickNone({ picker }: { picker: ZenConversationPicker }): React.JSX.Element {
  const menuId = useId()
  const buttonRef = useRef<HTMLButtonElement | null>(null)
  const [open, setOpen] = useState(false)
  const close = useCallback((focusButton: boolean) => {
    setOpen(false)
    if (focusButton) buttonRef.current?.focus()
  }, [])
  return (
    <div className="relative flex h-full min-h-0 w-full flex-col" data-zen-pick-none="" style={CARD_TOKENS}>
      <div className="flex flex-shrink-0 items-center gap-3" style={{ padding: '12px 18px', borderBottom: '1px solid var(--zen-border)' }}>
        <button
          ref={buttonRef}
          type="button"
          data-zen-pick-button=""
          aria-haspopup="listbox"
          aria-expanded={open}
          aria-controls={open ? menuId : undefined}
          title="Pick an agent"
          className="flex min-w-0 items-center gap-3"
          style={PICK_BUTTON_STYLE}
          onClick={() => setOpen((o) => !o)}
          onKeyDown={(e) => {
            if (e.key === 'ArrowDown' && !open) {
              e.preventDefault()
              setOpen(true)
            }
          }}
        >
          <ZenAgentAvatar label="?" url={null} size={34} background="var(--zen-surface)" color="var(--zen-text-muted)" fontSize="0.9em" />
          <span className="truncate" style={{ fontWeight: 600 }} data-zen-conversation-title="">
            {ZEN_PICK_NONE_LABEL}
          </span>
          <Chevron open={open} />
        </button>
      </div>
      <div
        className="flex flex-1 items-center justify-center text-center"
        data-zen-conversation-none=""
        style={{ padding: 24, color: 'var(--zen-text-muted)' }}
      >
        {ZEN_PICK_NONE_TEXT}
      </div>
      {open && <ZenPickMenu id={menuId} picker={picker} shown="" buttonRef={buttonRef} onClose={close} />}
    </div>
  )
}

/** Is this conversation pinned to one agent (the `agent` prop)? */
function zenConversationPinned(props: Record<string, unknown>): boolean {
  return typeof props.agent === 'string' && props.agent.trim() !== ''
}

/** The row a Conversation widget shows: pinned, the one agent (its list is
 *  that agent); else the followed widget's selection. */
export function zenConversationRow(rows: readonly ZenAgentRow[], props: Record<string, unknown>): ZenAgentRow | null {
  return zenConversationPinned(props) ? (rows[0] ?? null) : (rows.find((r) => r.selected) ?? null)
}

/** `pick`: each name with its row (the view's rows are exactly the matched
 *  ones, in `pick` order), and the one shown: the saved pick while it is
 *  listed, else the first agent found. */
export function zenPickState(
  rows: readonly ZenAgentRow[],
  pick: readonly string[],
  saved: string | null,
): { entries: ZenPickEntry[]; shown: ZenAgentRow | null } {
  const entries = pick.map((name) => ({ name, row: zenFindPickRow(rows, name) }))
  const found = entries.flatMap((e) => (e.row ? [e.row] : []))
  return { entries, shown: found.find((r) => r.address === saved) ?? found[0] ?? null }
}

/** `pick = "all"`: every agent the view lists (not a list's off-list
 *  extras) is a card, and the one shown is the view's selection, else the
 *  saved pick while it is listed (it is opened, so it becomes the
 *  selection), else no one. */
export function zenPickAllState(
  rows: readonly ZenAgentRow[],
  saved: string | null,
): { entries: ZenPickEntry[]; shown: ZenAgentRow | null } {
  const listed = rows.filter((r) => !r.offList)
  const entries = listed.map((r) => ({ name: r.label, row: r }))
  const shown = listed.find((r) => r.selected) ?? listed.find((r) => r.address === saved) ?? null
  return { entries, shown }
}

export function ZenConversationWidget({ bridge, decl }: ZenWidgetProps): React.JSX.Element {
  const rows = useZenRows(bridge)
  const pick = zenConversationPick(decl.props)
  // `pick = "all"` (0.46.4): any agent; the view's selection is the pick, so
  // a custom widget this follows sees it (`selected`) and can change it.
  const all = zenConversationPickAll(decl.props)
  const pinned = !pick && !all && zenConversationPinned(decl.props)
  // `pick`: the shown agent is per Garden and widget, on this computer.
  const pickKey = zenGardenHomeKey(bridge.gardens?.current?.()?.id ?? '', decl.id)
  const saved = useZenConversationPicksStore((s) => s.picks[pickKey] ?? null)
  const setPick = useZenConversationPicksStore((s) => s.setPick)
  const refocus = useRef(false)
  // The last agent this widget opened (a pinned or picked one opens itself).
  const opened = useRef<string | null>(null)
  const onPick = useCallback(
    (address: string) => {
      refocus.current = true
      setPick(pickKey, address)
      if (!all) return
      // The shared selection: the conversation, and a widget it follows.
      // Opened here, once (not again by the effect below).
      opened.current = address
      void Promise.resolve()
        .then(() => bridge.call('conversation.open', address))
        .catch((err: unknown) => console.warn('[zen] pick conversation failed:', err))
    },
    [setPick, pickKey, all, bridge],
  )
  const picked = pick ? zenPickState(rows, pick, saved) : all ? zenPickAllState(rows, saved) : null
  const picker: ZenConversationPicker | undefined = picked
    ? { entries: picked.entries, onPick, refocus, ...(all ? { search: true, badge: false } : {}) }
    : undefined
  const row = picked ? picked.shown : zenConversationRow(rows, decl.props)
  // `pick = "all"`: a pick made elsewhere (the widget it follows) is saved
  // too, so it comes back after a reload.
  const selectedAddress = all ? (rows.find((r) => r.selected && !r.offList)?.address ?? null) : null
  const lastSelected = useRef<string | null>(null)
  useEffect(() => {
    if (!all || selectedAddress === lastSelected.current) return
    lastSelected.current = selectedAddress
    if (selectedAddress) setPick(pickKey, selectedAddress)
  }, [all, selectedAddress, pickKey, setPick])
  const options: ZenConversationOptions = {
    compose: decl.props.compose !== false,
    attachments: decl.props.attachments !== false,
    loadOlder: decl.props['load-older'] !== false,
    activityDrawer: decl.props['activity-drawer'] === true,
    mute: decl.props.mute === true,
  }
  // A pinned conversation opens itself; so does a `pick` one's shown agent
  // (with `pick = "all"`, the saved pick after a reload).
  const opens = pinned || pick !== null || all
  useEffect(() => {
    if (!opens || !row || row.selected || opened.current === row.address) return
    opened.current = row.address
    void Promise.resolve()
      .then(() => bridge.call('conversation.open', row.address))
      .catch((err: unknown) => console.warn('[zen] open pinned conversation failed:', err))
  }, [bridge, opens, row])
  return (
    <div className="flex h-full min-h-0 w-full flex-col" data-zen-widget="conversation">
      <ZenWidgetStyles />
      {row ? (
        <ZenFileLinks address={row.address}>
          <ZenConversation key={row.address} bridge={bridge} row={row} options={options} picker={picker} />
        </ZenFileLinks>
      ) : all && picker ? (
        <ZenConversationPickNone picker={picker} />
      ) : (
        <div
          className="flex flex-1 items-center justify-center text-center"
          data-zen-conversation-none=""
          style={{ padding: 24, color: 'var(--zen-text-muted)' }}
        >
          {pick
            ? `None of ${pick.join(', ')} is on this Home.`
            : pinned
            ? `${String(decl.props.agent)} isn’t on this Home.`
            : rows.length > 0
              ? 'Pick an agent to message.'
              : ''}
        </div>
      )}
    </div>
  )
}
