// prd-zen-mode-v1 Z42–Z45 (answers 6, 10) — the Conversation widget: the
// selected agent's Thread, like a text conversation.
//
// The Thread comes through the bridge (`thread.subscribe`, `thread.read`),
// which runs the existing overlay Thread hook on the agent's own server.
// Rows reuse the Thread's own pieces in a new layout: the message body
// renderer (`ChatMessageBody`), the choice card and the secret card. The
// user's messages sit right, the agent's left; cards are inline and
// tappable (`thread.answer`, `thread.void`). While the agent works, a typing
// indicator shows under the last message. When it waits for a permission
// prompt Zen can't show, a banner offers "Open in Agents". Older messages
// load on scroll. The message box is `ZenCompose`.
//
// prd-zen-gardens-v1 G38 (Rosson's answer 5): it follows an Agents widget
// (`agents` prop, else the page's first), or, with `agent` (and `home`),
// is pinned to that one agent with no list needed. Display props:
// `compose`, `attachments`, `load-older` (the daemon sends every prop).

import { useCallback, useEffect, useLayoutEffect, useRef } from 'react'
import { useStickToBottom } from '@/hooks/useStickToBottom'
import { ChatMessageBody } from '@/components/common/ChatMessage'
import { ChoiceCard, SecretCard } from '@/components/SessionView/ThreadOverlayPane'
import { formatRelativeTime } from '@/lib/format-relative-time'
import type { OverlayThreadItem } from '@/components/SessionView/overlayThread'
import type { ZenAgentRow } from '@/lib/zen/zen-data'
import { zenBubbleShield } from '@/lib/zen/zen-style-shield'
import type { ZenWidgetBridge } from '@/lib/zen/zen-bridge'
import type { ZenWidgetProps } from '../zen-registry'
import { ZenCompose } from './ZenCompose'
import { ZenStatusDot } from './ZenAgentsWidget'
import { ZenWidgetStyles, initials, useNowSec, useZenRows, useZenThread } from './zen-widget-kit'

/** The Thread cards take their look from these (Styles values otherwise). */
const CARD_TOKENS = {
  '--thread-card-border': 'var(--zen-border)',
  '--thread-card-bg': 'var(--zen-surface)',
  '--thread-card-accent': 'var(--zen-accent)',
  '--thread-card-text': 'var(--zen-text)',
  '--thread-card-text-secondary': 'var(--zen-text)',
  '--thread-card-muted': 'var(--zen-text-muted)',
  '--thread-card-radius': 'calc(var(--zen-radius) / 2)',
} as React.CSSProperties

export const zenEmptyThreadText = (label: string): string => `Message ${label}. It reads this like a text.`
export const zenPermissionText = (label: string): string => `${label} is waiting for permission in its terminal.`

function itemText(item: OverlayThreadItem): string {
  const d = item.doc
  if (d.kind === 'choice') return d.choice?.prompt || d.body || ''
  if (d.kind === 'secret') return d.secret?.prompt || d.body || ''
  return d.body || ''
}

function Bubble({
  item,
  nowSec,
  onAnswer,
  onVoid,
}: {
  item: OverlayThreadItem
  nowSec: number
  onAnswer(payload: { answer?: string; secret?: string }): void
  onVoid(): void
}): React.JSX.Element {
  const mine = item.doc.via === 'compose'
  const kind = item.doc.kind
  const text = itemText(item)
  return (
    <div
      data-zen-message={item.id}
      data-kind={kind}
      data-mine={mine ? '' : undefined}
      className="flex flex-col"
      style={{ alignSelf: mine ? 'flex-end' : 'flex-start', maxWidth: 'min(78%, 46rem)', gap: 3 }}
    >
      <div
        className="selectable-copy"
        data-zen-bubble={mine ? 'me' : 'agent'}
        style={{
          padding: '9px 14px',
          borderRadius: 'var(--zen-bubble-radius)',
          borderBottomRightRadius: mine ? 6 : 'var(--zen-bubble-radius)',
          borderBottomLeftRadius: mine ? 'var(--zen-bubble-radius)' : 6,
          background: mine ? 'var(--zen-bubble-me)' : 'var(--zen-bubble-agent)',
          color: mine ? 'var(--zen-bubble-me-text)' : 'var(--zen-bubble-agent-text)',
          // Markdown inside reads Styles variables; here they are this bubble's text.
          ...zenBubbleShield(mine ? 'me' : 'agent'),
          ...CARD_TOKENS,
        }}
      >
        {text && <ChatMessageBody text={text} style={{ color: 'inherit', fontSize: 'inherit' }} />}
        {kind === 'choice' && item.doc.choice && (
          <ChoiceCard
            options={item.doc.choice.options}
            allowCustom={item.doc.choice.allow_custom}
            status={item.doc.choice.status}
            answer={item.doc.choice.answer}
            onPick={(label) => onAnswer({ answer: label })}
          />
        )}
        {kind === 'secret' && item.doc.secret && (
          <div style={{ marginTop: 6 }}>
            <SecretCard
              name={item.doc.secret.name}
              status={item.doc.secret.status}
              onSubmit={(value) => onAnswer({ secret: value })}
              onDismiss={onVoid}
            />
          </div>
        )}
      </div>
      <span
        data-zen-message-meta=""
        style={{
          alignSelf: mine ? 'flex-end' : 'flex-start',
          padding: '0 6px',
          fontSize: '0.72em',
          color: 'var(--zen-text-muted)',
        }}
      >
        {mine ? 'You' : item.doc.from || 'agent'} · {formatRelativeTime(item.doc.created_at, nowSec)}
      </span>
    </div>
  )
}

function TypingIndicator(): React.JSX.Element {
  return (
    <div
      data-zen-typing=""
      aria-label="Working"
      className="flex items-center gap-1"
      style={{
        alignSelf: 'flex-start',
        padding: '10px 14px',
        borderRadius: 'var(--zen-bubble-radius)',
        background: 'var(--zen-bubble-agent)',
      }}
    >
      {[0, 1, 2].map((i) => (
        <span key={i} style={{ width: 6, height: 6, borderRadius: 999, background: 'var(--zen-working)', display: 'inline-block' }} />
      ))}
    </div>
  )
}

/** One agent's conversation (also the empty Garden's Ask my agent). */
export interface ZenConversationOptions {
  compose: boolean
  attachments: boolean
  loadOlder: boolean
}

const ALL_ON: ZenConversationOptions = { compose: true, attachments: true, loadOlder: true }

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
  const items = view?.items ?? []
  const ready = view?.phase === 'ready'

  const answer = useCallback(
    (id: string, payload: { answer?: string; secret?: string }) => {
      void (bridge.call('thread.answer', row.address, id, payload) as Promise<unknown>).catch((err: unknown) =>
        console.warn('[zen] answer failed:', err),
      )
    },
    [bridge, row.address],
  )
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
    deps: [items, row.working, ready, view?.note, view?.hasMore, view?.error],
    extraTargets: () => [rootRef.current?.querySelector('[data-zen-compose]')],
    onNearTop: loadOlder,
  })
  holdRef.current = stick.holdForPrepend

  // A new conversation (the empty Garden reuses this one) starts at its end.
  const { scrollToBottom } = stick
  useLayoutEffect(() => {
    scrollToBottom()
  }, [row.address, scrollToBottom])

  const note = view?.note ?? null
  const showEmpty = ready && view.loaded && !view.error && items.length === 0
  return (
    <div ref={rootRef} className="flex h-full min-h-0 w-full flex-col" data-zen-conversation={row.address} style={CARD_TOKENS}>
      <div
        className="flex flex-shrink-0 items-center gap-3"
        style={{ padding: '12px 18px', borderBottom: '1px solid var(--zen-border)' }}
      >
        <span
          className="flex flex-shrink-0 items-center justify-center"
          style={{
            width: 34,
            height: 34,
            borderRadius: 999,
            background: 'var(--zen-accent)',
            color: 'var(--zen-accent-text)',
            fontWeight: 600,
            fontSize: '0.9em',
          }}
        >
          {initials(row.label)}
        </span>
        <span className="flex min-w-0 flex-col">
          <span className="truncate" style={{ fontWeight: 600 }} data-zen-conversation-title="">
            {row.label}
          </span>
          <span className="flex items-center gap-1.5 truncate" style={{ fontSize: '0.8em', color: 'var(--zen-text-muted)' }}>
            <ZenStatusDot row={row} size={8} />
            <span data-zen-conversation-status={row.activity ?? row.state}>
              {[row.server ?? 'this computer', row.state !== 'ok' ? row.stateLabel : row.activity === 'needs-you' ? 'needs you' : row.activity]
                .filter(Boolean)
                .join(' · ')}
            </span>
          </span>
        </span>
        {row.people.length > 0 && (
          <span className="ml-auto flex -space-x-1.5" data-zen-people="">
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
      </div>

      {row.needsYou && ready && (
        <div
          role="status"
          data-zen-permission=""
          className="flex flex-shrink-0 items-center gap-3"
          style={{
            margin: '10px var(--zen-gap) 0',
            padding: '8px 12px',
            borderRadius: 'var(--zen-radius)',
            border: '1px solid var(--zen-needs-you)',
            color: 'var(--zen-text)',
            fontSize: '0.88em',
          }}
        >
          <span className="min-w-0 flex-1">{zenPermissionText(row.label)}</span>
          <button
            type="button"
            data-zen-open-in-agents=""
            data-zen-soft-button=""
            onClick={() => {
              void (bridge.call('conversation.open', row.address, { where: 'agents' }) as Promise<unknown>).catch(
                (err: unknown) => console.warn('[zen] open in Agents failed:', err),
              )
            }}
            className="flex-shrink-0 cursor-pointer disabled:cursor-default"
            style={{
              padding: '4px 12px',
              borderRadius: 999,
              border: '1px solid var(--zen-border)',
              color: 'var(--zen-text)',
              fontWeight: 600,
            }}
          >
            Open in Agents
          </button>
        </div>
      )}

      <div
        ref={stick.ref}
        data-zen-conversation-body=""
        className="relative flex min-h-0 flex-1 flex-col overflow-y-auto"
        // The browser's own scroll anchoring would fight the pin.
        style={{ padding: '14px var(--zen-gap) 8px', gap: 10, overflowAnchor: 'none' }}
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
        {ready && view.hasMore && options.loadOlder && (
          <button
            className="cursor-pointer disabled:cursor-default"
            type="button"
            data-zen-load-older=""
            data-zen-soft-button=""
            disabled={view.loadingOlder}
            onClick={loadOlder}
            style={{ alignSelf: 'center', padding: '3px 12px', borderRadius: 999, fontSize: '0.8em', color: 'var(--zen-text-muted)' }}
          >
            {view.loadingOlder ? 'Loading…' : 'Earlier messages'}
          </button>
        )}
        {ready && view.error && !note && (
          <div style={{ color: 'var(--zen-text-muted)', fontSize: '0.85em', textAlign: 'center' }}>{view.error}</div>
        )}
        {showEmpty && (
          <div className="m-auto text-center" data-zen-thread-empty="" style={{ color: 'var(--zen-text-muted)' }}>
            {zenEmptyThreadText(row.label)}
          </div>
        )}
        {ready &&
          items.map((it) => (
            <Bubble
              key={it.id}
              item={it}
              nowSec={nowSec}
              onAnswer={(payload) => answer(it.id, payload)}
              onVoid={() => dismiss(it.id)}
            />
          ))}
        {ready && row.working && <TypingIndicator />}
      </div>

      {options.compose && (
        <ZenCompose
          bridge={bridge}
          address={row.address}
          label={row.label}
          disabled={!ready || note !== null}
          attachments={options.attachments}
          onSend={scrollToBottom}
        />
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
