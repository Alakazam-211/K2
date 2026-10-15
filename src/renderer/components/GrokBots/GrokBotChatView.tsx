// A Grok Bot's chat in the main area (0.45.7): the bot's collected K2
// Thread (`gb:<name>`, read through `/cli/grokbots/thread`), plus a message
// box when the server's owner turned Send on. No terminal, no session, no
// tabs — a Grok Bot is not a K2 workspace.
//
// The daemon collects every message it sees into that Thread, starting the
// day collection started (the first line says so): new lines from Grok
// Bot's store on this computer, each new latest line for a stale bot (its
// live conversation runs outside this computer), and every send made from
// K2. Older history in Grok Bot's own store is not shown. For a stale bot
// only the latest line at each check is visible, so a gap line stands
// where lines may be missing.
//
// Interactive lines (a question with choices, a connect request, a
// credential request, a shared template) show as read-only cards: they are
// answered in Grok Bot itself. K2 never asks for or shows a secret.

import { useCallback, useEffect, useRef, useState } from 'react'
import { ChatMessage } from '@/components/common/ChatMessage'
import { ChoiceCard } from '@/components/SessionView/ThreadOverlayPane'
import ProjectAvatar from '@/components/Sidebar/ProjectAvatar'
import { Button } from '@/components/ui/Button'
import { Callout } from '@/components/ui/Callout'
import { formatRelativeTime } from '@/lib/format-relative-time'
import {
  fetchGrokThread,
  grokAgentState,
  GROK_BOTS_SETTINGS_PATH,
  GROK_LIVE_GAP_TEXT,
  GROK_STALE_COMPOSE_NOTE,
  GROK_STATE_LABEL,
  grokK2Sender,
  grokStaleBannerText,
  grokStartText,
  mergeGrokMessages,
  sendToGrokBot,
  type GrokCard,
  type GrokMessage,
} from '@/lib/grokbots'
import { grokAgentOn, scopeForGrokHost, useGrokBotsStore, useGrokBotsSync } from '@/stores/grokbots'
import { GrokBotTag } from './GrokBotTag'

const PAGE = 100
const TAIL = 50
/** Other servers' rows have no event socket here: poll the chat. */
const REMOTE_POLL_MS = 6000

const muted = 'text-[var(--color-text-muted)]'

function OpenInGrokBot({ what }: { what: string }): React.JSX.Element {
  return (
    <div data-testid="grokbot-open-in-app" className={`text-[10px] ${muted}`}>
      {what} Open Grok Bot to see or answer it.
    </div>
  )
}

/** A K2 note in the Thread: where collection started, or a gap. */
export function GrokSystemLine({ m, agentName }: { m: GrokMessage; agentName: string }): React.JSX.Element {
  if (m.system === 'start') {
    return (
      <div data-testid="grokbot-start-line" className={`my-2 flex items-center gap-2 text-[10px] ${muted}`}>
        <div className="h-px flex-1 bg-[var(--color-border)]" />
        <span className="shrink-0">{m.atMs !== null ? grokStartText(agentName, m.atMs) : m.text}</span>
        <div className="h-px flex-1 bg-[var(--color-border)]" />
      </div>
    )
  }
  return (
    <div data-testid="grokbot-gap-line" className={`py-1 text-center text-[10px] ${muted}`}>
      {m.system === 'gap' ? GROK_LIVE_GAP_TEXT : m.text}
    </div>
  )
}

/** A read-only card for an interactive Grok Bot line. */
export function GrokCardView({ card }: { card: GrokCard }): React.JSX.Element {
  const box =
    'mt-1 flex flex-col gap-1 rounded-[var(--radius-box)] border border-[var(--color-border)] px-2.5 py-2 text-[12px]'
  switch (card.type) {
    case 'choice': {
      const chosen = card.options.find((o) => o.chosen)?.label ?? null
      const stateLine =
        card.state === 'answered'
          ? card.customAnswer
            ? `Answered: “${card.customAnswer}”`
            : 'Answered'
          : card.state === 'skipped'
            ? 'Skipped'
            : card.state === 'dismissed'
              ? 'Dismissed'
              : 'Waiting for an answer in Grok Bot'
      return (
        <div data-testid="grokbot-card-choice" className={box}>
          <div className="font-medium text-[var(--color-text-primary)]">{card.prompt}</div>
          {card.helpText && <div className={`text-[11px] ${muted}`}>{card.helpText}</div>}
          <ChoiceCard
            options={card.options.map((o) => ({ label: o.label }))}
            allowCustom={false}
            status={card.state === 'open' ? 'open' : card.state}
            answer={chosen}
            onPick={() => {}}
          />
          <div data-testid="grokbot-card-state" className={`text-[10px] ${muted}`}>
            {stateLine}
          </div>
          <OpenInGrokBot what="Questions are answered in Grok Bot." />
        </div>
      )
    }
    case 'connector':
      return (
        <div data-testid="grokbot-card-connector" className={box}>
          <div className="font-medium text-[var(--color-text-primary)]">
            {card.variant === 'connect' || !card.variant ? 'Connect' : card.variant} {card.connector}
          </div>
          <OpenInGrokBot what="Connections are made in Grok Bot." />
        </div>
      )
    case 'secret-request':
      return (
        <div data-testid="grokbot-card-secret" className={box}>
          <div className="font-medium text-[var(--color-text-primary)]">
            {card.label}
            {card.platform ? ` · ${card.platform}` : ''}
          </div>
          {card.description && <div className={`text-[11px] ${muted}`}>{card.description}</div>}
          <div data-testid="grokbot-card-state" className={`text-[10px] ${muted}`}>
            {card.provided ? 'Provided in Grok Bot' : 'Not provided yet'}
          </div>
          <OpenInGrokBot what="K2 never asks for secrets." />
        </div>
      )
    case 'template-share':
      return (
        <div data-testid="grokbot-card-template" className={box}>
          <div className="font-medium text-[var(--color-text-primary)]">Shared bot template: {card.name}</div>
          {card.body && <div className={`text-[11px] ${muted}`}>{card.body}</div>}
          {card.shareUrl && <div className="select-text break-all text-[11px] text-[var(--color-accent)]">{card.shareUrl}</div>}
        </div>
      )
  }
}

/** The banner on a bot whose live conversation runs elsewhere, with Grok
 *  Bot's current preview. */
export function GrokStaleBanner({
  name,
  preview,
  lastActivityAt,
  nowSec,
}: {
  name: string
  preview: string | null
  lastActivityAt: number | null
  nowSec: number
}): React.JSX.Element {
  const when = lastActivityAt !== null ? ` (${formatRelativeTime(Math.floor(lastActivityAt / 1000), nowSec)})` : ''
  return (
    <Callout tone="info" data-testid="grokbot-stale-banner">
      <div>{grokStaleBannerText(name)}</div>
      {preview && (
        <div data-testid="grokbot-stale-preview" className="mt-1">
          {`Latest${when}: “${preview}”`}
        </div>
      )}
    </Callout>
  )
}

function authorLabel(m: GrokMessage, agentName: string, k2From: string | null): string {
  if (m.author === 'human') return k2From ?? 'You'
  if (m.author === 'peer') return m.fromAgent?.name ?? 'Another agent'
  return agentName
}

function MessageRow({ m, agentName, nowSec }: { m: GrokMessage; agentName: string; nowSec: number }): React.JSX.Element {
  if (m.author === 'system') return <GrokSystemLine m={m} agentName={agentName} />
  const previewOnly = m.previewOnly
  const sent = grokK2Sender(m)
  const time = m.atMs !== null ? formatRelativeTime(Math.floor(m.atMs / 1000), nowSec) : ''
  const notes: string[] = []
  if (m.toAgent) notes.push(`to ${m.toAgent.name}`)
  if (m.author === 'peer') notes.push(`from ${m.fromAgent?.name ?? 'another agent'}`)
  if (m.viaK2) notes.push('sent from K2')
  if (previewOnly) notes.push('preview, open Grok Bot for the full message')
  const footer = (
    <>
      {m.card && <GrokCardView card={m.card} />}
      {!m.card && m.openInApp && <OpenInGrokBot what={`This is a ${m.openInApp} K2 can't show.`} />}
      {notes.length > 0 && <div className={`text-[10px] ${muted}`}>{notes.join(' · ')}</div>}
    </>
  )
  return (
    <div data-testid="grokbot-message" data-author={m.author}>
      <ChatMessage
        author={authorLabel(m, agentName, sent.from)}
        isOwner={m.author === 'human'}
        timeLabel={time}
        body={sent.text}
        footer={footer}
      />
    </div>
  )
}

export default function GrokBotChatView({ hostKey, agentId }: { hostKey: string; agentId: string }): React.JSX.Element {
  const scope = scopeForGrokHost(hostKey)
  const remote = !scope.isPrimary
  useGrokBotsSync(scope, remote ? REMOTE_POLL_MS * 2 : 0)
  const hostState = useGrokBotsStore((s) => s.byHost[hostKey])
  const changeSeq = useGrokBotsStore((s) => s.changeSeq[hostKey] ?? 0)
  const agent = grokAgentOn(hostState, agentId)
  const list = hostState?.list ?? null
  const realId = agent?.id ?? agentId

  const [messages, setMessages] = useState<GrokMessage[]>([])
  const [supported, setSupported] = useState(true)
  const [reason, setReason] = useState<string | null>(null)
  const [loadError, setLoadError] = useState<string | null>(null)
  const [loaded, setLoaded] = useState(false)
  const [draft, setDraft] = useState('')
  const [sending, setSending] = useState(false)
  const [sendError, setSendError] = useState<string | null>(null)
  const listRef = useRef<HTMLDivElement | null>(null)

  const load = useCallback(
    async (limit: number, reset: boolean) => {
      try {
        const page = await fetchGrokThread(scope, realId, { limit })
        setSupported(page.supported)
        setReason(page.reason)
        setMessages((prev) => (reset ? page.messages : mergeGrokMessages(prev, page.messages)))
        setLoadError(null)
      } catch (e) {
        setLoadError(e instanceof Error ? e.message : String(e))
      } finally {
        setLoaded(true)
      }
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [hostKey, realId],
  )

  // First page when the chat opens (or switches agent).
  useEffect(() => {
    setMessages([])
    setLoaded(false)
    setSendError(null)
    void load(PAGE, true)
  }, [load])

  // The server said this agent's chat (or the roster) moved: re-read the
  // newest lines and merge by seq (a line updated in place is replaced).
  useEffect(() => {
    if (changeSeq > 0) void load(TAIL, false)
  }, [changeSeq, load])

  useEffect(() => {
    if (!remote) return
    const t = setInterval(() => void load(TAIL, false), REMOTE_POLL_MS)
    return () => clearInterval(t)
  }, [remote, load])

  // Keep the newest line in view.
  useEffect(() => {
    const el = listRef.current
    if (el) el.scrollTop = el.scrollHeight
  }, [messages.length])

  const send = async (): Promise<void> => {
    const text = draft.trim()
    if (!text || sending) return
    setSending(true)
    setSendError(null)
    try {
      await sendToGrokBot(scope, realId, text)
      setDraft('')
      void load(TAIL, false)
    } catch (e) {
      setSendError(e instanceof Error ? e.message : String(e))
    } finally {
      setSending(false)
    }
  }

  const name = agent?.name ?? 'Grok Bot agent'
  const state = agent && list ? grokAgentState(agent, list.status) : null
  const nowSec = Math.floor(Date.now() / 1000)
  const sendOn = list?.sendEnabled === true
  const stale = agent?.stale === true

  return (
    <div className="flex h-full w-full flex-1 flex-col overflow-hidden" data-testid="grokbot-chat">
      <div className="flex items-center gap-2.5 border-b border-[var(--color-border)] px-4 py-2.5">
        <ProjectAvatar
          projectPath={`grokbot:${realId}`}
          projectName={name}
          projectColor={agent?.color ?? 'var(--color-text-muted)'}
          iconUrl={agent?.avatar ?? null}
          size={28}
          fetchIcon={false}
        />
        <div className="flex min-w-0 flex-col">
          <div className="flex items-center gap-2 text-sm text-[var(--color-text-primary)]">
            <span className="truncate">{name}</span>
            <GrokBotTag />
          </div>
          <div className={`truncate text-[11px] ${muted}`} data-testid="grokbot-chat-status">
            {[agent?.title, state ? GROK_STATE_LABEL[state] : null].filter(Boolean).join(' · ')}
          </div>
        </div>
      </div>

      {/* Pinned above the Thread so it stays in view while scrolling. */}
      {stale && agent && (
        <div className="px-2 pt-2">
          <GrokStaleBanner
            name={name}
            preview={agent.lastMessagePreview}
            lastActivityAt={agent.lastActivityAt}
            nowSec={nowSec}
          />
        </div>
      )}

      <div ref={listRef} className="flex-1 overflow-y-auto px-2 py-2">
        {list && !list.enabled && (
          <Callout tone="info">Local Grok Bots are off on this server. Turn on Show in {GROK_BOTS_SETTINGS_PATH}.</Callout>
        )}
        {loadError && list?.enabled !== false && <Callout tone="error">Could not read this chat: {loadError}</Callout>}
        {!supported && (
          <Callout tone="warn" data-testid="grokbot-unsupported">
            K2 can't read this chat yet{reason ? ` (${reason})` : ''}. Open it in Grok Bot.
          </Callout>
        )}
        {messages.map((m) => (
          <MessageRow key={m.seq} m={m} agentName={name} nowSec={nowSec} />
        ))}
        {loaded && supported && !loadError && !messages.some((m) => m.author !== 'system') && (
          <div data-testid="grokbot-empty" className={`py-6 text-center text-xs ${muted}`}>
            No messages yet
          </div>
        )}
      </div>

      <div className="border-t border-[var(--color-border)] px-3 py-2">
        {sendOn ? (
          <div className="flex flex-col gap-1.5" data-testid="grokbot-compose">
            {state === 'working' && (
              <div className={`text-[11px] ${muted}`}>{name} is busy. Your message may wait until it finishes.</div>
            )}
            <div className="flex items-end gap-2">
              <textarea
                data-testid="grokbot-compose-input"
                value={draft}
                onChange={(e) => setDraft(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === 'Enter' && !e.shiftKey) {
                    e.preventDefault()
                    void send()
                  }
                }}
                rows={2}
                placeholder={`Message ${name}`}
                className="flex-1 resize-none rounded-[var(--radius-box)] border border-[var(--color-border)] bg-transparent px-2 py-1.5 text-[13px] text-[var(--color-text-primary)] outline-none"
              />
              <Button
                variant="accent"
                data-testid="grokbot-compose-send"
                disabled={sending || !draft.trim()}
                onClick={() => void send()}
              >
                Send
              </Button>
            </div>
            <div className={`text-[10px] ${muted}`}>
              Each message runs a billed Grok turn and reaches the bot as [from your name].
            </div>
            {stale && (
              <div data-testid="grokbot-compose-stale-note" className={`text-[10px] ${muted}`}>
                {GROK_STALE_COMPOSE_NOTE}
              </div>
            )}
            {sendError && <Callout tone="error">{sendError}</Callout>}
          </div>
        ) : (
          <div data-testid="grokbot-send-off" className={`text-[11px] ${muted}`}>
            Sending is off. Turn on Send in {GROK_BOTS_SETTINGS_PATH}.
          </div>
        )}
      </div>
    </div>
  )
}
