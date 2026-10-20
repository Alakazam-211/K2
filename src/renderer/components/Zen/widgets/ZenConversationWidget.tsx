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
// out of the message box's top edge, instead of the typing dots.
//
// 0.46.1 (thread-read-v1, Zen defaults): the conversation marks its Thread
// read while it is looked at, says how many cards wait on you ("2
// waiting", a click scrolls to the first) and, with `mute` (on in Garden
// 1), has a bell to mute the Thread (`ZenThreadReadControls`).

import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react'
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
import { useZenMaterials } from '@/lib/zen/zen-materials'
import type { ZenAgentRow } from '@/lib/zen/zen-data'
import type { ZenWidgetBridge } from '@/lib/zen/zen-bridge'
import type { ZenWidgetProps } from '../zen-registry'
import { ZenCompose } from './ZenCompose'
import { ZenActivityDrawer, zenDrawerShows } from './ZenActivityDrawer'
import { ZenStatusDot } from './ZenAgentsWidget'
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
  /** 0.45.6 Garden 1: the activity drawer above the box (`activity-drawer`). */
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

export function ZenConversation({
  bridge,
  row,
  options = ALL_ON,
}: {
  bridge: ZenWidgetBridge
  row: ZenAgentRow
  options?: ZenConversationOptions
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
  // TW14: the dots follow the Thread turn when the server reports turns,
  // else the row's rollup (an older server).
  // Garden 1's activity drawer (`activity-drawer`) stands in for the dots
  // while it has a turn to show (a server that reports Thread turns).
  const drawerTurn = options.activityDrawer && options.compose && view?.liveTurn !== undefined ? view.liveTurn : undefined
  const drawerOn = drawerTurn !== undefined && zenDrawerShows(drawerTurn)
  const typing =
    !drawerOn &&
    (view?.turn !== undefined ? view.turn !== null && (view.turn.state === 'working' || view.turn.state === 'monitoring') : row.working)

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
          <span className="truncate" style={{ fontWeight: 600 }} data-zen-conversation-title="">
            {row.label}
          </span>
          <span className="flex items-center gap-1.5 truncate" style={{ fontSize: '0.8em', color: 'var(--zen-text-muted)' }}>
            <ZenStatusDot row={row} size={8} />
            <span data-zen-conversation-status={row.activity ?? row.state}>
              {[row.server ?? 'this computer', row.state !== 'ok' ? row.stateLabel : row.activity === 'needs-you' ? 'needs you' : row.activity === 'unverifiable' ? 'no update' : row.activity]
                .filter(Boolean)
                .join(' · ')}
            </span>
          </span>
        </span>
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

export function ZenConversationWidget({ bridge, decl }: ZenWidgetProps): React.JSX.Element {
  const rows = useZenRows(bridge)
  const pinned = zenConversationPinned(decl.props)
  const row = zenConversationRow(rows, decl.props)
  const options: ZenConversationOptions = {
    compose: decl.props.compose !== false,
    attachments: decl.props.attachments !== false,
    loadOlder: decl.props['load-older'] !== false,
    activityDrawer: decl.props['activity-drawer'] === true,
    mute: decl.props.mute === true,
  }
  // A pinned conversation opens itself.
  const opened = useRef<string | null>(null)
  useEffect(() => {
    if (!pinned || !row || row.selected || opened.current === row.address) return
    opened.current = row.address
    void Promise.resolve()
      .then(() => bridge.call('conversation.open', row.address))
      .catch((err: unknown) => console.warn('[zen] open pinned conversation failed:', err))
  }, [bridge, pinned, row])
  return (
    <div className="flex h-full min-h-0 w-full flex-col" data-zen-widget="conversation">
      <ZenWidgetStyles />
      {row ? (
        <ZenConversation key={row.address} bridge={bridge} row={row} options={options} />
      ) : (
        <div
          className="flex flex-1 items-center justify-center text-center"
          data-zen-conversation-none=""
          style={{ padding: 24, color: 'var(--zen-text-muted)' }}
        >
          {pinned
            ? `${String(decl.props.agent)} isn’t on this Home.`
            : rows.length > 0
              ? 'Pick an agent to message.'
              : ''}
        </div>
      )}
    </div>
  )
}
