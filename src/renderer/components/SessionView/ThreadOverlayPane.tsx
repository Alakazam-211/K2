import { memo, useEffect, useLayoutEffect, useRef, useState, type JSX } from 'react'
import { ChatMessage } from '@/components/common/ChatMessage'
import { useIsTabVisible } from '@/contexts/TabVisibilityContext'
import { formatRelativeTime } from '@/lib/format-relative-time'
import { isEffectivelyHidden } from '@/lib/workspace-switch-focus'
import { useSettingsStore } from '@/stores/settings'
import { useOverlayThread } from './useOverlayThread'
import { useRoom } from '@/components/Room/RoomContext'
import { ThreadWorkingStrip } from './ThreadWorkingStrip'
import { displayThreadAddress } from '@/lib/thread-address-bus'
import {
  cardExpiryLabel,
  isVoidedHitl,
  type OverlayDoc,
  type OverlayThreadItem,
  type ThreadAnswerPayload,
} from './overlayThread'
import { SecretCard } from './SecretCard'
import {
  cardAnswererLabel,
  isOwnThreadPost,
  OWNER_VIEWER,
  parseThreadAuthor,
  useThreadViewer,
  type ThreadViewer,
} from '@/lib/thread-viewer'

// The vault secret card lives in its own file; Zen imports it from here.
export { SecretCard } from './SecretCard'

/** Who a secret card names in its K2-drawn header. */
export interface SecretCardContext {
  workspace?: string
  server?: string
}

interface ThreadOverlayPaneProps {
  addr: string
  conversationId: string | null
  /** Pause GET/WS and the relative-time interval when the pane is hidden. */
  active?: boolean
  /** Agent display name for the empty state. Falls back to "the agent". */
  agentName?: string
  /** The working strip's Stop (Esc to the agent's PTY). Absent: no Stop. */
  onStop?: () => void
}

/** Overlay log only — Message-the-agent compose stays on TerminalPane. */
export const ThreadOverlayPane = memo(function ThreadOverlayPane({
  addr,
  conversationId,
  active = true,
  agentName,
  onStop,
}: ThreadOverlayPaneProps): JSX.Element {
  const roomScope = useRoom().scope
  // A Thread is shared: "You" only on this viewer's own posts.
  const viewer = useThreadViewer(roomScope)
  const isTabVisible = useIsTabVisible()
  const { items, error, answer, voidCard, hasMore, loadOlder, loadingOlder, loaded, turn, currentAddr, pastAddresses } = useOverlayThread({
    // Home M4: the room's server (B's thread in B's room).
    scope: roomScope,
    addr,
    conversationId,
    enabled: active,
    // Thread push §6.4: shown here (and the window looked at) = quiet pushes.
    viewing: active && isTabVisible,
  })

  const visible = items.filter((it) => !isVoidedHitl(it.doc))
  const editorFontSize = useSettingsStore((s) => s.editor.fontSize) || 13
  const [nowSec, setNowSec] = useState(() => Math.floor(Date.now() / 1000))
  useEffect(() => {
    if (!active) return
    setNowSec(Math.floor(Date.now() / 1000))
    const id = setInterval(() => setNowSec(Math.floor(Date.now() / 1000)), 30_000)
    return () => clearInterval(id)
  }, [active])

  // Re-pin on show/resize (retainer 0-box, pane-group reveal, compose grow).
  // Do not latch the initial pin against a 0-height list; hidden/layout
  // scrollTop===0 is not load-older and does not leave the bottom.
  const listRef = useRef<HTMLDivElement>(null)
  /** The messages + the working strip: a strip that grows or goes keeps a
   *  pinned list at the bottom (prd-daemon-activity-and-thread-working-v1 S7). */
  const contentRef = useRef<HTMLDivElement>(null)
  const didInitialScroll = useRef(false)
  const prevHeightRef = useRef<number | null>(null)
  const pinBottomRef = useRef(true)
  const savedScrollTopRef = useRef<number | null>(null)
  const lastClientHeightRef = useRef(0)
  const itemsLenRef = useRef(items.length)
  itemsLenRef.current = items.length
  const isTabVisibleRef = useRef(isTabVisible)
  isTabVisibleRef.current = isTabVisible

  useEffect(() => {
    didInitialScroll.current = false
    pinBottomRef.current = true
    prevHeightRef.current = null
    savedScrollTopRef.current = null
    lastClientHeightRef.current = 0
  }, [addr])

  const syncListScrollRef = useRef<(el: HTMLElement) => void>(() => {})
  syncListScrollRef.current = (el: HTMLElement) => {
    if (el.clientHeight === 0) return
    lastClientHeightRef.current = el.clientHeight
    if (!didInitialScroll.current && itemsLenRef.current > 0) {
      el.scrollTop = el.scrollHeight
      didInitialScroll.current = true
      pinBottomRef.current = true
      savedScrollTopRef.current = el.scrollTop
      return
    }
    if (prevHeightRef.current != null) {
      el.scrollTop += el.scrollHeight - prevHeightRef.current
      prevHeightRef.current = null
      savedScrollTopRef.current = el.scrollTop
      return
    }
    if (pinBottomRef.current) {
      el.scrollTop = el.scrollHeight
      savedScrollTopRef.current = el.scrollTop
      return
    }
    if (savedScrollTopRef.current != null) {
      el.scrollTop = savedScrollTopRef.current
    }
  }

  useLayoutEffect(() => {
    const el = listRef.current
    if (!el) return
    syncListScrollRef.current(el)
  }, [items, turn, isTabVisible])

  useEffect(() => {
    if (!isTabVisible) return
    const el = listRef.current
    if (!el) return
    const run = (): void => {
      syncListScrollRef.current(el)
    }
    run()
    const id = requestAnimationFrame(() => {
      requestAnimationFrame(run)
    })
    return () => cancelAnimationFrame(id)
  }, [isTabVisible])

  useEffect(() => {
    const list = listRef.current
    const pane = list?.parentElement
    if (!list || typeof ResizeObserver === 'undefined') return
    const ro = new ResizeObserver(() => {
      syncListScrollRef.current(list)
    })
    ro.observe(list)
    if (pane) ro.observe(pane)
    if (contentRef.current) ro.observe(contentRef.current)
    return () => ro.disconnect()
  }, [])

  function listScrollLive(el: HTMLElement): boolean {
    if (el.clientHeight === 0) return false
    if (!isTabVisibleRef.current) return false
    if (isEffectivelyHidden(el)) return false
    return true
  }

  function requestOlder(): void {
    if (!hasMore || loadingOlder) return
    const el = listRef.current
    if (!el || !listScrollLive(el)) return
    prevHeightRef.current = el.scrollHeight
    void loadOlder()
  }

  return (
    <div
      className="flex-1 min-h-0 overflow-hidden flex flex-col bg-[var(--color-bg)] selectable-copy chat-thread-selectable"
      data-testid="thread-overlay-pane"
    >
      <div
        ref={listRef}
        data-testid="thread-overlay-list"
        className="relative flex-1 min-h-0 overflow-y-auto overflow-x-hidden px-2 py-2"
        onScroll={(e) => {
          const el = e.currentTarget
          if (!listScrollLive(el)) return
          // Compose grow / split shrink fires scroll before (or instead of)
          // ResizeObserver. Old scrollTop is no longer the bottom; treating
          // that as a user scroll unpins and the latest messages stay put.
          if (lastClientHeightRef.current !== el.clientHeight) {
            lastClientHeightRef.current = el.clientHeight
            if (pinBottomRef.current) {
              el.scrollTop = el.scrollHeight
              savedScrollTopRef.current = el.scrollTop
            }
            return
          }
          if (el.scrollTop <= 16) requestOlder()
          pinBottomRef.current =
            el.scrollHeight - el.scrollTop - el.clientHeight <= 32
          savedScrollTopRef.current = el.scrollTop
        }}
      >
        {hasMore && (
          <button
            type="button"
            data-testid="overlay-load-older"
            disabled={loadingOlder}
            onClick={() => requestOlder()}
            className="w-full mb-2 px-2 py-1 text-[11px] text-[var(--color-text-muted)] border border-[var(--color-border)]"
          >
            {loadingOlder ? 'Loading…' : 'Load older'}
          </button>
        )}
        {error && (
          <div className="text-[11px] text-[var(--color-text-muted)] px-2.5">{error}</div>
        )}
        {loaded && !error && visible.length === 0 && (
          // Absolute over the list so it centers without adding scroll
          // height; pointer-events-none keeps Load older clickable.
          <div
            data-testid="thread-overlay-empty"
            className="pointer-events-none absolute inset-0 flex flex-col items-center justify-center gap-1 px-6 text-center"
          >
            <div className="text-[13px] font-medium text-[var(--color-text-primary)]">
              No Thread messages yet
            </div>
            <div className="text-[12px] text-[var(--color-text-muted)] max-w-[40ch]">
              Send a message to {agentName?.trim() || 'the agent'} in the box below.
            </div>
          </div>
        )}
        <div ref={contentRef} className="flex flex-col gap-2.5">
          {visible.map((it) => (
            <ThreadItemRow
              key={it.id}
              item={it}
              viewer={viewer}
              nowSec={nowSec}
              fontSize={editorFontSize}
              currentAddr={currentAddr}
              pastAddresses={pastAddresses}
              onAnswer={(payload) => answer(it.id, payload)}
              onVoid={() => void voidCard(it.id)}
              cardContext={{ workspace: addr.split('/')[0], server: roomScope.label }}
            />
          ))}
          {/* TW12: live only, under the last message; never stored (TW13). */}
          {turn && <ThreadWorkingStrip key={turn.turnId} turn={turn} onStop={onStop} />}
        </div>
      </div>
    </div>
  )
})

/** A person's post (the compose bar, a Garden widget, an app guest): the
 *  daemon stamped who sent it. Older docs only say `via=compose`. */
function isPersonPost(doc: OverlayDoc): boolean {
  return parseThreadAuthor(doc.author) !== null || doc.via === 'compose'
}

/** The author line: "You" on the viewer's own post, else the sender's
 *  name (a person's `from` is their name: the owner's display name, the
 *  Connect username, the guest's username; an agent's is its address). */
export function threadAuthorLabel(
  doc: OverlayDoc,
  viewer: ThreadViewer | null,
  currentAddr?: string,
  pastAddresses?: readonly string[],
): { label: string; mine: boolean } {
  const mine = isOwnThreadPost(doc, viewer)
  // UWB12a: a post a Garden widget sent for a person says which widget;
  // Design notes (D-A10) say "via Design".
  const viaWidget = isPersonPost(doc) ? doc.widget?.widget?.trim() || (doc.design ? 'Design' : '') : ''
  const name = mine
    ? 'You'
    : isPersonPost(doc)
      ? doc.from?.trim() || 'someone'
      : displayThreadAddress(doc.from || '', currentAddr, pastAddresses) || 'unknown'
  return { label: viaWidget ? `${name} · via ${viaWidget}` : name, mine }
}

export function ThreadItemRow({
  item,
  viewer = OWNER_VIEWER,
  nowSec,
  fontSize,
  currentAddr,
  pastAddresses,
  onAnswer,
  onVoid,
  cardContext,
}: {
  item: OverlayThreadItem
  /** Who is looking (`useThreadViewer` on the room's server). Defaults to
   *  the owner on this computer. */
  viewer?: ThreadViewer | null
  nowSec?: number
  fontSize?: number
  /** Q5: an old row stamped with this chat's earlier address shows the
   *  current one (storage keeps what was stamped). */
  currentAddr?: string
  pastAddresses?: readonly string[]
  /** Resolves when the daemon took the answer; throws its refusal. */
  onAnswer?: (payload: ThreadAnswerPayload) => unknown
  onVoid?: () => void
  cardContext?: SecretCardContext
}): JSX.Element {
  const kind = item.doc.kind
  const { label: author, mine: owner } = threadAuthorLabel(item.doc, viewer, currentAddr, pastAddresses)
  const timeLabel = formatRelativeTime(
    item.doc.created_at,
    nowSec ?? Math.floor(Date.now() / 1000),
  )
  const prompt =
    kind === 'choice'
      ? item.doc.choice?.prompt || item.doc.body || ''
      : kind === 'secret' && item.doc.secret
        ? // The card shows the agent's prompt as plain text (VS13), never markdown.
          ''
        : item.doc.body || ''
  return (
    <div
      data-testid="thread-item"
      data-kind={kind}
      data-seq={item.seq}
      data-status={item.doc.choice?.status || item.doc.secret?.status || ''}
    >
      <ChatMessage
        author={author}
        isOwner={owner}
        timeLabel={timeLabel}
        body={prompt}
        fontSize={fontSize}
        footer={
          kind === 'choice' && item.doc.choice ? (
            <ChoiceCard
              options={item.doc.choice.options}
              allowCustom={item.doc.choice.allow_custom}
              status={item.doc.choice.status}
              answer={item.doc.choice.answer}
              answeredBy={cardAnswererLabel(item.doc.choice.answeredBy, viewer)}
              expiresAt={item.doc.choice.expiresAt}
              nowSec={nowSec}
              onPick={(label) => onAnswer?.({ answer: label })}
            />
          ) : kind === 'secret' && item.doc.secret ? (
            <SecretCard
              secret={item.doc.secret}
              answeredBy={cardAnswererLabel(item.doc.secret.answeredBy, viewer)}
              agent={item.doc.from || ''}
              workspace={cardContext?.workspace}
              server={cardContext?.server}
              onSubmit={async (payload) => {
                await onAnswer?.(payload)
              }}
              onDismiss={() => onVoid?.()}
            />
          ) : null
        }
      />
    </div>
  )
}

/** A, B, … Z, AA — UI letter only; the posted answer is still the option label. */
export function choiceLetter(index: number): string {
  let n = index
  let s = ''
  do {
    s = String.fromCharCode(65 + (n % 26)) + s
    n = Math.floor(n / 26) - 1
  } while (n >= 0)
  return s
}

export function ChoiceCard({
  options,
  allowCustom,
  status,
  answer,
  answeredBy,
  expiresAt,
  nowSec,
  onPick,
}: {
  options: { label: string }[]
  allowCustom: boolean
  status: string
  answer?: string | null
  /** `cardAnswererLabel` of the card's `answeredBy` ("You", a login). */
  answeredBy?: string | null
  /** The card's time frame (unix seconds); the daemon expires it. */
  expiresAt?: number | null
  nowSec?: number
  onPick: (label: string) => void
}): JSX.Element {
  const pending = status === 'pending'
  const expiry =
    status === 'expired'
      ? 'expired'
      : pending
        ? cardExpiryLabel(expiresAt, nowSec ?? Math.floor(Date.now() / 1000))
        : null
  const [custom, setCustom] = useState('')
  return (
    <div
      data-testid="thread-choice-card"
      className="mt-1 w-full box-border flex flex-col gap-1.5 rounded-[var(--thread-card-radius,0px)] border border-[var(--thread-card-border,var(--color-border))] bg-[var(--thread-card-bg,var(--color-bg))] px-2 py-1.5"
    >
      {options.map((opt, i) => {
          const selected = answer === opt.label
          const letter = choiceLetter(i)
          return (
            <button
              key={`${opt.label}-${i}`}
              type="button"
              data-testid="thread-choice-chip"
              data-label={opt.label}
              data-letter={letter}
              data-primary={i === 0 ? 'true' : 'false'}
              disabled={!pending}
              onClick={() => onPick(opt.label)}
              className={`flex w-full items-center gap-2 box-border rounded-[var(--thread-card-radius,0px)] px-2 py-1.5 text-left text-[11px] font-medium border transition-colors ${
                selected
                  ? 'border-[var(--thread-card-accent,var(--color-accent))] bg-[var(--thread-card-accent,var(--color-accent))]/15 text-[var(--thread-card-text,var(--color-text-primary))]'
                  : pending
                    ? 'border-[var(--thread-card-border,var(--color-border))] text-[var(--thread-card-text-secondary,var(--color-text-secondary))] hover:border-[var(--thread-card-accent,var(--color-accent))] hover:text-[var(--thread-card-text,var(--color-text-primary))] cursor-pointer'
                    : 'border-[var(--thread-card-border,var(--color-border))] text-[var(--thread-card-muted,var(--color-text-muted))] opacity-50'
              } disabled:cursor-not-allowed`}
            >
              <span
                className={`w-4 shrink-0 text-[10px] font-semibold ${
                  selected
                    ? 'text-[var(--thread-card-accent,var(--color-accent))]'
                    : 'text-[var(--thread-card-muted,var(--color-text-muted))]'
                }`}
              >
                {letter}
              </span>
              <span className="min-w-0 flex-1">{opt.label}</span>
            </button>
          )
      })}
      {expiry && (
        <div
          data-testid="thread-choice-expiry"
          className="text-[10px] text-[var(--thread-card-muted,var(--color-text-muted))]"
        >
          {expiry}
        </div>
      )}
      {status === 'answered' && answeredBy && (
        <div
          data-testid="thread-choice-answered-by"
          className="text-[10px] text-[var(--thread-card-muted,var(--color-text-muted))]"
        >
          answered by {answeredBy}
        </div>
      )}
      {allowCustom && pending && (
        <div className="mt-0.5 flex gap-2">
          <input
            data-testid="thread-choice-custom"
            value={custom}
            onChange={(e) => setCustom(e.target.value)}
            placeholder="Custom…"
            className="flex-1 bg-transparent text-[12px] text-[var(--thread-card-text,var(--color-text-primary))] outline-none border border-[var(--thread-card-border,var(--color-border))] px-2 py-1"
          />
          <button
            type="button"
            data-testid="thread-choice-custom-submit"
            disabled={!custom.trim()}
            onClick={() => onPick(custom.trim())}
            className="px-2 py-1 text-[11px] border border-[var(--thread-card-border,var(--color-border))]"
          >
            Send
          </button>
        </div>
      )}
    </div>
  )
}
