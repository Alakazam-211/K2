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

import { useCallback, useEffect, useLayoutEffect, useRef } from 'react'
import { ChatMessageBody } from '@/components/common/ChatMessage'
import { ChoiceCard, SecretCard } from '@/components/SessionView/ThreadOverlayPane'
import { formatRelativeTime } from '@/lib/format-relative-time'
import type { OverlayThreadItem } from '@/components/SessionView/overlayThread'
import type { ZenAgentRow } from '@/lib/zen/zen-data'
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
        style={{
          padding: '9px 14px',
          borderRadius: 'var(--zen-bubble-radius)',
          borderBottomRightRadius: mine ? 6 : 'var(--zen-bubble-radius)',
          borderBottomLeftRadius: mine ? 'var(--zen-bubble-radius)' : 6,
          background: mine ? 'var(--zen-bubble-me)' : 'var(--zen-bubble-agent)',
          color: mine ? 'var(--zen-bubble-me-text)' : 'var(--zen-bubble-agent-text)',
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

function Conversation({ bridge, row }: { bridge: ZenWidgetBridge; row: ZenAgentRow }): React.JSX.Element {
  const view = useZenThread(bridge, row.address)
  const nowSec = useNowSec()
  const listRef = useRef<HTMLDivElement | null>(null)
  const pinBottom = useRef(true)
  const heightBeforeOlder = useRef<number | null>(null)
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
  const loadOlder = useCallback(() => {
    if (!view || !view.hasMore || view.loadingOlder || items.length === 0) return
    const el = listRef.current
    if (el) heightBeforeOlder.current = el.scrollHeight
    const minSeq = items.reduce((m, it) => Math.min(m, it.seq), Number.POSITIVE_INFINITY)
    void (bridge.call('thread.read', row.address, { beforeSeq: minSeq }) as Promise<unknown>).catch((err: unknown) =>
      console.warn('[zen] load older failed:', err),
    )
  }, [bridge, row.address, view, items])

  // Keep the newest message in view unless the user scrolled up; keep the
  // reading position when older messages arrive on top.
  useLayoutEffect(() => {
    const el = listRef.current
    if (!el) return
    if (heightBeforeOlder.current !== null) {
      el.scrollTop += el.scrollHeight - heightBeforeOlder.current
      heightBeforeOlder.current = null
      return
    }
    if (pinBottom.current) el.scrollTop = el.scrollHeight
  }, [items.length, row.working, items])

  useEffect(() => {
    pinBottom.current = true
  }, [row.address])

  const note = view?.note ?? null
  const showEmpty = ready && view.loaded && !view.error && items.length === 0
  return (
    <div className="flex h-full min-h-0 w-full flex-col" data-zen-conversation={row.address} style={CARD_TOKENS}>
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
            className="flex-shrink-0"
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
        ref={listRef}
        data-zen-conversation-body=""
        className="relative flex min-h-0 flex-1 flex-col overflow-y-auto"
        style={{ padding: '14px var(--zen-gap) 8px', gap: 10 }}
        onScroll={(e) => {
          const el = e.currentTarget
          pinBottom.current = el.scrollHeight - el.scrollTop - el.clientHeight <= 32
          if (el.scrollTop <= 16) loadOlder()
        }}
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
        {ready && view.hasMore && (
          <button
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

      <ZenCompose bridge={bridge} address={row.address} label={row.label} disabled={!ready || note !== null} />
    </div>
  )
}

export function ZenConversationWidget({ bridge }: ZenWidgetProps): React.JSX.Element {
  const rows = useZenRows(bridge)
  const row = rows.find((r) => r.selected) ?? null
  return (
    <div className="flex h-full min-h-0 w-full flex-col" data-zen-widget="conversation">
      <ZenWidgetStyles />
      {row ? (
        <Conversation key={row.address} bridge={bridge} row={row} />
      ) : (
        <div
          className="flex flex-1 items-center justify-center text-center"
          data-zen-conversation-none=""
          style={{ padding: 24, color: 'var(--zen-text-muted)' }}
        >
          {rows.length > 0 ? 'Pick an agent to message.' : ''}
        </div>
      )}
    </div>
  )
}
