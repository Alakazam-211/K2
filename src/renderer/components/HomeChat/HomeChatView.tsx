// A DM or a channel in Home's main area (prd-home-people-channels-v1 §8).
//
// It LOOKS like the Thread (Rosson: "almost the same as today's Thread"):
// `ThreadChatScope`, the Thread's bubbles and runs, its message box, the
// render window and the reply chip, all from the Thread kit. Its DATA is
// not the Thread's: DMs and channels come from Sew through this server's
// daemon (`/cli/home-chat/*` and `WS /cli/home-chat/events`), never the
// redb Thread store. Agent Threads stay on redb (§2 non-goals); nothing here
// reads or writes them.
//
// What differs from the Thread:
//   - the activity bar shows who is typing (§9), not agent activity;
//   - reactions under bubbles (§10) and a hover row: React, Reply, ⋯;
//   - Reply: the agent Thread's reply bar on the box's edge (× or Esc), sent as
//     Sew's inline `reference {message_id, kind:"reply"}` (§11); the sent
//     bubble's quote strip jumps to the original, paging back for it;
//   - names over every run in a channel; in a DM only the other person's;
//   - no @mentions: `@name` is plain text (D18);
//   - a DM has a Call button, the caller's "Calling…" bar, the in-call strip,
//     and each call's record as a line ("Missed call", "Call · 12:04") with
//     Call back, never a bubble (§11a.4).
//
// Opening tells the daemon the chat is on screen (`view`, which subscribes
// typing and joins an open channel not joined yet); it acks the newest
// message while the window has focus. A reopened chat paints this window's
// last copy at once, then reads what came after it.

import { useCallback, useEffect, useMemo, useRef, useState, type JSX } from 'react'
import {
  ThreadChatDraftComposer,
  ThreadChatEmpty,
  ThreadChatNote,
  ThreadChatScope,
  type ThreadChatDraftApi,
} from '@/components/Thread/ThreadChat'
import { THREAD_LIST_GAP, THREAD_LIST_PADDING, ThreadLoadOlder, ThreadWindowSpacer } from '@/components/Thread/ThreadZenKit'
import { ThreadMessageColumn } from '@/components/Thread/ThreadColumn'
import { useThreadTextCapPx } from '@/components/SessionView/useThreadTextCap'
import { threadComposeCapWidth, threadTextColumnStyle } from '@/components/SessionView/threadTextWidth'
import { ThreadReplyBar } from '@/components/Thread/ThreadReplyBar'
import { useRenderWindow } from '@/components/Thread/useRenderWindow'
import { usePrependHold } from '@/components/Thread/usePrependHold'
import { threadRunWindow } from '@/components/Thread/threadRuns'
import { SelectableRegion } from '@/components/common/SelectableText'
import { formatRelativeTime } from '@/lib/format-relative-time'
import { daemonErrorText } from '@/lib/daemon-error'
import { useConnectHostStore } from '@/stores/connect-host'
import { activeHomeHostKey } from '@/lib/home-address'
import type { HomeChatTarget } from '@/lib/home-chat/address'
import {
  ackHomeChat,
  fetchHomeChatMembers,
  fetchHomeChatMessages,
  fetchHomeChatReactors,
  openHomeChatDm,
  parseHomeChatMessage,
  reactHomeChat,
  sendHomeChatMessage,
  typingHomeChat,
  viewHomeChat,
  type HomeChatMembers,
  type HomeChatMessage,
} from '@/lib/home-chat/api'
import {
  JUMP_MAX_PAGES,
  TYPING_INTERVAL_MS,
  applyReactionFrame,
  authorName,
  cachedHomeChat,
  firstLine,
  markDeleted,
  memberNames,
  mergeHomeChatMessages,
  newNonce,
  rememberHomeChat,
  replyPreview,
  toggleReaction,
} from '@/lib/home-chat/chat'
import { noteHomeChatSend } from '@/lib/home-chat/stamp'
import { useHomesStore } from '@/stores/homes'
import {
  homeChatRowInviteOnly,
  onHomeChatFrame,
  refreshHomeChatPeople,
  scopeForChatHost,
  typingMembers,
  useHomeChatHost,
  useHomeChatStore,
} from '@/stores/home-chat'
import { HomeChatMessageRow, JUMP_WASH_MS, type HomeChatRowActions } from './HomeChatMessageRow'
import { HomeChatMembersSheet } from './HomeChatMembersSheet'
import { HomeChatTyping } from './HomeChatTyping'
import { HomeChatChannelGlyph, HomeChatPersonAvatar } from './HomeChatGlyphs'
import { EyeIcon } from 'lucide-react'
import { K2_ICON_STROKE } from '@/components/icons/K2Icons'
import { HomeChatCallButton, HomeChatCallRecord } from './HomeChatCall'
import { HomeChatDmCallBars, useDmCall } from './HomeChatCallControls'
import { callRecordLine } from '@/lib/home-chat/calls'
import { MachineChip } from '@/components/Home/MachineChip'
import { serverLabelFor } from './serverLabel'

const PAGE = 50
const DELTA = 100
/** How long a jumped-to message keeps its wash (it fades over JUMP_WASH_MS). */
const FLASH_MS = JUMP_WASH_MS + 200

/** The line above the box when a send fails: the reason, and that the text
 *  waits there (Enter sends it again with the same nonce, so never twice). */
export function homeChatSendFailedLine(reason: string): string {
  const why = reason.trim().replace(/[.\s]+$/, '')
  return `Not sent${why ? ` (${why})` : ''}. It's still in the box: press Enter to retry.`
}

interface PendingReply {
  id: string
  from: string
  text: string
}

export default function HomeChatView({ target }: { target: HomeChatTarget }): JSX.Element {
  const hostKey = target.hostKey
  const scope = useMemo(() => scopeForChatHost(hostKey), [hostKey])
  const host = useHomeChatHost(hostKey)
  const noteDm = useHomeChatStore((s) => s.noteDm)
  const storedDm = useHomeChatStore((s) => (target.kind === 'person' ? s.byHost[hostKey]?.dms[target.ext] : undefined))
  const conv = target.kind === 'channel' ? target.conv : (storedDm ?? null)
  const [convError, setConvError] = useState<string | null>(null)

  // A person row opens (or makes) the DM on first open (§6.2).
  useEffect(() => {
    if (target.kind !== 'person' || storedDm) return
    let alive = true
    setConvError(null)
    openHomeChatDm(scope, target.ext).then(
      (c) => alive && noteDm(hostKey, target.ext, c),
      (e: unknown) => alive && setConvError(daemonErrorText(e) || String(e)),
    )
    return () => {
      alive = false
    }
  }, [target, storedDm, scope, hostKey, noteDm])

  // Names: this server's people (who wrote each message).
  useEffect(() => {
    void refreshHomeChatPeople(hostKey)
  }, [hostKey])
  const people = host?.people ?? null
  const me = people?.me.memberId ?? null
  const names = useMemo(() => memberNames(people), [people])
  const person = target.kind === 'person' ? (people?.people.find((p) => p.ext === target.ext) ?? null) : null

  // A channel's own facts: its name, kind, and who may do what.
  const [channel, setChannel] = useState<HomeChatMembers | null>(null)
  const [membersOpen, setMembersOpen] = useState(false)
  const reloadChannel = useCallback(() => {
    if (target.kind !== 'channel') return
    fetchHomeChatMembers(scope, { conv: target.conv }).then(setChannel, (e: unknown) =>
      console.warn('[home-chat] channel facts:', e),
    )
  }, [scope, target])
  useEffect(() => {
    setChannel(null)
    reloadChannel()
  }, [reloadChannel])

  // ── Messages ─────────────────────────────────────────────────────────
  const [messages, setMessages] = useState<HomeChatMessage[] | null>(null)
  const messagesRef = useRef<HomeChatMessage[]>([])
  messagesRef.current = messages ?? []
  const [hasMore, setHasMore] = useState(true)
  const hasMoreRef = useRef(true)
  hasMoreRef.current = hasMore
  const [loadError, setLoadError] = useState<string | null>(null)
  const [loadingOlder, setLoadingOlder] = useState(false)

  useEffect(() => {
    if (!conv) return
    let alive = true
    const cached = cachedHomeChat(hostKey, conv)
    setMessages(cached)
    setHasMore(true)
    setLoadError(null)
    const newest = cached && cached.length > 0 ? cached[cached.length - 1].id : null
    void (async () => {
      try {
        if (newest) {
          // Phone style: paint the copy, then read only what came after it.
          const delta = await fetchHomeChatMessages(scope, conv, { after: newest, limit: DELTA })
          if (!alive) return
          if (delta.messages.length >= DELTA) {
            const page = await fetchHomeChatMessages(scope, conv, { limit: PAGE })
            if (!alive) return
            setMessages(page.messages)
            setHasMore(page.hasMore)
          } else {
            setMessages((prev) => mergeHomeChatMessages(prev ?? [], delta.messages))
          }
        } else {
          const page = await fetchHomeChatMessages(scope, conv, { limit: PAGE })
          if (!alive) return
          setMessages(page.messages)
          setHasMore(page.hasMore)
        }
      } catch (e) {
        if (alive) setLoadError(daemonErrorText(e) || String(e))
      }
    })()
    viewHomeChat(scope, conv, true).catch((e: unknown) => console.warn('[home-chat] view:', e))
    return () => {
      alive = false
      viewHomeChat(scope, conv, false).catch(() => undefined)
    }
  }, [conv, hostKey, scope])

  useEffect(() => {
    if (conv && messages) rememberHomeChat(hostKey, conv, messages)
  }, [conv, hostKey, messages])

  // Live frames for this conversation.
  useEffect(() => {
    if (!conv) return
    return onHomeChatFrame(hostKey, (f) => {
      if (!('conv' in f) || f.conv !== conv) return
      if (f.type === 'message' || f.type === 'message_updated') {
        // A call's record updates in place (§11a.4).
        const m = parseHomeChatMessage(f.message)
        if (m) setMessages((prev) => mergeHomeChatMessages(prev ?? [], [m]))
      } else if (f.type === 'deleted') {
        setMessages((prev) => (prev ? markDeleted(prev, f.id) : prev))
      } else if (f.type === 'reaction') {
        setMessages((prev) => (prev ? applyReactionFrame(prev, f) : prev))
      } else if (f.type === 'member_removed') {
        reloadChannel()
      }
    })
  }, [conv, hostKey, reloadChannel])

  // Read: ack the newest while this window has focus.
  const newestId = messages && messages.length > 0 ? messages[messages.length - 1].id : null
  useEffect(() => {
    if (!conv || !newestId) return
    if (typeof document !== 'undefined' && !document.hasFocus()) return
    ackHomeChat(scope, conv).catch((e: unknown) => console.warn('[home-chat] ack:', e))
  }, [conv, newestId, scope])

  // ── The list (the Thread's render window and runs) ───────────────────
  const scrollRef = useRef<HTMLDivElement | null>(null)
  const followingRef = useRef(true)
  const keys = useMemo(() => (messages ?? []).map((m) => m.id), [messages])
  const win = useRenderWindow({
    room: `home-chat:${hostKey}:${conv ?? ''}`,
    keys,
    scroller: () => scrollRef.current,
    list: () => scrollRef.current?.querySelector<HTMLElement>('[data-thread-column]') ?? null,
    following: () => followingRef.current,
    stick: (el) => {
      el.scrollTop = el.scrollHeight
    },
  })
  // An older page (Earlier messages, a reply's original read back) keeps
  // the message being read where it is.
  usePrependHold({
    keys,
    scroller: () => scrollRef.current,
    rows: () => scrollRef.current?.querySelector<HTMLElement>('[data-thread-column]') ?? null,
    following: () => followingRef.current,
    wrote: win.wrote,
  })
  const runInputs = useMemo(
    () => (messages ?? []).map((m) => ({ mine: !!me && m.authorId === me, from: m.authorId, at: m.createdAt })),
    [messages, me],
  )
  const runs = useMemo(() => threadRunWindow(runInputs, win.lo, win.hi), [runInputs, win.lo, win.hi])

  const toNewest = win.toNewest
  const sentRef = useRef(false)
  const prevNewest = useRef<string | null>(null)
  useEffect(() => {
    if (newestId === null || newestId === prevNewest.current) return
    const opening = prevNewest.current === null
    prevNewest.current = newestId
    const sent = sentRef.current
    sentRef.current = false
    if (!opening && !sent && !followingRef.current) return
    followingRef.current = true
    toNewest(true)
    requestAnimationFrame(() => {
      const el = scrollRef.current
      if (el) el.scrollTop = el.scrollHeight
    })
  }, [newestId, toNewest])
  useEffect(() => {
    prevNewest.current = null
  }, [conv])

  /** Read one page older than the oldest loaded; the merged list. */
  const loadOlder = useCallback(async (): Promise<HomeChatMessage[]> => {
    const list = messagesRef.current
    if (!conv || list.length === 0 || !hasMoreRef.current) return list
    const page = await fetchHomeChatMessages(scope, conv, { before: list[0].id, limit: PAGE })
    const merged = mergeHomeChatMessages(messagesRef.current, page.messages)
    messagesRef.current = merged
    hasMoreRef.current = page.hasMore
    setMessages(merged)
    setHasMore(page.hasMore)
    return merged
  }, [conv, scope])

  const growTop = win.growTop
  const onLoadOlder = useCallback(async () => {
    if (growTop() || loadingOlder) return
    setLoadingOlder(true)
    try {
      // The page keeps the message being read where it is (usePrependHold).
      await loadOlder()
    } catch (e) {
      setLoadError(daemonErrorText(e) || String(e))
    } finally {
      setLoadingOlder(false)
    }
  }, [growTop, loadingOlder, loadOlder])

  // ── Jump to a reply's original (§11) ─────────────────────────────────
  const [flashId, setFlashId] = useState<string | null>(null)
  const [jumpMiss, setJumpMiss] = useState<string | null>(null)
  const show = win.show
  const jump = useCallback(
    async (id: string, pages = JUMP_MAX_PAGES) => {
      setJumpMiss(null)
      let list = messagesRef.current
      let read = 0
      try {
        while (!list.some((m) => m.id === id) && hasMoreRef.current && read < pages) {
          list = await loadOlder()
          read += 1
        }
      } catch (e) {
        setLoadError(daemonErrorText(e) || String(e))
        return
      }
      if (!list.some((m) => m.id === id)) {
        // Read back to the first message without it: it is gone.
        setJumpMiss(hasMoreRef.current ? id : `gone:${id}`)
        return
      }
      followingRef.current = false
      show(id)
      requestAnimationFrame(() => {
        const el = scrollRef.current?.querySelector<HTMLElement>(`[data-home-chat-message][data-zen-message="${id}"]`)
        el?.scrollIntoView?.({ block: 'center' })
        setFlashId(id)
        setTimeout(() => setFlashId((cur) => (cur === id ? null : cur)), FLASH_MS)
      })
    },
    [loadOlder, show],
  )

  // ── Reply, react, who ────────────────────────────────────────────────
  const [reply, setReply] = useState<PendingReply | null>(null)
  const inputRef = useRef<HTMLTextAreaElement | null>(null)
  const actions = useMemo<HomeChatRowActions>(
    () => ({
      reply: (m) => {
        setReply({ id: m.id, from: authorName(m.authorId, me, names), text: m.body })
        requestAnimationFrame(() => inputRef.current?.focus())
      },
      react: (msgId, emoji) => {
        if (!conv) return
        const t = toggleReaction(messagesRef.current, msgId, emoji)
        setMessages(t.messages)
        reactHomeChat(scope, { conv, msg: msgId, emoji, on: t.on }).then(
          (m) => m && setMessages((prev) => mergeHomeChatMessages(prev ?? [], [m])),
          (e: unknown) => {
            // The server's truth back: undo the optimistic toggle.
            setMessages((prev) => (prev ? toggleReaction(prev, msgId, emoji).messages : prev))
            setSendError(daemonErrorText(e) || String(e))
          },
        )
      },
      jump: (id) => void jump(id),
      who: async (msgId, emoji) => {
        const ids = await fetchHomeChatReactors(scope, msgId, emoji)
        return ids.map((id) => authorName(id, me, names))
      },
    }),
    [conv, scope, me, names, jump],
  )

  // ── Typing (§9) ──────────────────────────────────────────────────────
  const typingNow = useHomeChatStore((s) => (conv ? s.typing[`${hostKey}|${conv}`] : undefined))
  const typingNames = useMemo(() => {
    if (!conv || !typingNow) return []
    return typingMembers({ [`${hostKey}|${conv}`]: typingNow }, hostKey, conv, Date.now())
      .filter((m) => m !== me)
      .map((m) => authorName(m, me, names))
  }, [typingNow, hostKey, conv, me, names])
  const lastTyping = useRef(0)
  const onDraftChange = useCallback(
    (text: string) => {
      if (!conv || !text.trim()) return
      const now = Date.now()
      if (now - lastTyping.current < TYPING_INTERVAL_MS) return
      lastTyping.current = now
      typingHomeChat(scope, conv).catch(() => undefined)
    },
    [conv, scope],
  )

  // ── Send ─────────────────────────────────────────────────────────────
  const draftApi = useRef<ThreadChatDraftApi>(null)
  const [busy, setBusy] = useState(false)
  const [sendError, setSendError] = useState<string | null>(null)
  const retryRef = useRef<{ conv: string; text: string; nonce: string } | null>(null)
  const send = useCallback(
    (draft: string) => {
      const text = draft.trim()
      if (!text || busy || !conv) return
      const kept = retryRef.current
      const nonce = kept && kept.conv === conv && kept.text === text ? kept.nonce : newNonce()
      retryRef.current = { conv, text, nonce }
      setBusy(true)
      setSendError(null)
      sendHomeChatMessage(scope, { conv, text, nonce, replyTo: reply?.id ?? null })
        .then((m) => {
          retryRef.current = null
          draftApi.current?.set('')
          setReply(null)
          sentRef.current = true
          if (m) setMessages((prev) => mergeHomeChatMessages(prev ?? [], [m]))
          noteHomeChatSend(target)
        })
        .catch((e: unknown) => setSendError(daemonErrorText(e) || String(e)))
        .finally(() => setBusy(false))
    },
    [busy, conv, scope, reply, target],
  )

  // ── Header and state ─────────────────────────────────────────────────
  const connectedKey = useConnectHostStore((s) => activeHomeHostKey(s.activeHost))
  const hosts = useConnectHostStore((s) => s.hosts)
  const place = hostKey === connectedKey ? null : serverLabelFor(hostKey, hosts)
  const status = host?.status ?? null
  const chatOff = status !== null && !status.on
  const archived = channel?.state === 'archived'
  // A DM whose other side left this server's list (disabled or removed) is
  // read-only (HP-13); the daemon refuses its sends too.
  const personGone = target.kind === 'person' && people !== null && person === null && target.ext !== people.me.ext
  const readOnly = archived || personGone
  const title =
    target.kind === 'person' ? (person?.displayName ?? labelOf(target)) : (channel?.name ?? labelOf(target))
  const nowSec = Math.floor(Date.now() / 1000)
  const dm = target.kind === 'person'
  // DM calls (§11a): the Call button says why when it can't call.
  const dmCall = useDmCall(hostKey, dm ? conv : null, title, me)
  const callBlocked = !dm
    ? null
    : chatOff
      ? status?.reason === 'mac'
        ? 'calls need a Linux server'
        : 'chat is off on this server'
      : readOnly
        ? 'this conversation is read-only'
        : !conv || !me
          ? 'opening the conversation'
          : null
  // The agent Thread's text width (Settings: Thread text width), one limit.
  const textCap = useThreadTextCapPx()
  const composeCap = threadComposeCapWidth(textCap)
  const chatState: 'loading' | 'error' | 'off' | null = chatOff
    ? 'off'
    : convError || loadError
      ? 'error'
      : messages === null
        ? 'loading'
        : null

  return (
    <div className="relative flex h-full w-full flex-1 flex-col overflow-hidden" data-testid="home-chat-view" data-kind={target.kind}>
      <div className="flex items-center gap-2 border-b border-[var(--color-border)] px-4 py-2.5" data-testid="home-chat-header">
        {dm ? (
          <HomeChatPersonAvatar username={person?.username ?? title} name={title} picture={person?.picture ?? null} size={22} />
        ) : (
          // The row's glyph: `#` open, a lock invite-only.
          <span className="inline-flex text-[var(--color-text-secondary)]" data-testid="home-chat-header-glyph">
            <HomeChatChannelGlyph inviteOnly={channel ? !channel.open : homeChatRowInviteOnly(host, target)} size={15} />
          </span>
        )}
        <span className="truncate text-sm text-[var(--color-text-primary)]" data-testid="home-chat-title">
          {title}
        </span>
        {place !== null && <MachineChip place={place} />}
        {channel && channel.agents.length > 0 && (
          <span className="inline-flex items-center gap-1 type-hint text-[var(--color-text-secondary)]" data-testid="home-chat-agent-eye" title="Agents with access can read this channel">
            <EyeIcon size={13} strokeWidth={K2_ICON_STROKE} aria-hidden="true" />
            {channel.agents.length === 1 ? '1 agent can read' : `${channel.agents.length} agents can read`}
          </span>
        )}
        <div className="flex-1" />
        {dm && <HomeChatCallButton disabledReason={callBlocked} onCall={dmCall.call} />}
        {target.kind === 'channel' && (
          <button
            type="button"
            className="k2-focus rounded-[var(--radius-field)] px-2 py-1 text-[11px] text-[var(--color-text-secondary)] hover:bg-[var(--color-row-hover)]"
            data-testid="home-chat-members-button"
            onClick={() => setMembersOpen(true)}
          >
            Members{channel ? ` · ${channel.people.length}` : ''}
          </button>
        )}
      </div>
      {dm && <HomeChatDmCallBars view={dmCall} name={title} />}
      {membersOpen && target.kind === 'channel' && (
        <HomeChatMembersSheet target={target} onClose={() => setMembersOpen(false)} onChanged={reloadChannel} />
      )}

      <ThreadChatScope className="flex min-h-0 flex-1 flex-col" style={{ background: 'var(--zen-canvas)' }} testId="home-chat-thread">
        <SelectableRegion className="flex min-h-0 flex-1 flex-col">
          <div
            ref={scrollRef}
            className="min-h-0 flex-1 overflow-y-auto overflow-x-hidden"
            data-zen-conversation-body=""
            style={{ padding: THREAD_LIST_PADDING, overflowAnchor: 'none' }}
            onScroll={(e) => {
              const el = e.currentTarget
              if (el.clientHeight > 0) followingRef.current = el.scrollHeight - el.scrollTop - el.clientHeight <= 32
              win.onScroll()
            }}
          >
            {chatState === 'off' ? (
              <ThreadChatEmpty testId="home-chat-off">
                <p>No chat on this server.</p>
              </ThreadChatEmpty>
            ) : chatState === 'error' ? (
              <ThreadChatEmpty testId="home-chat-error">
                <p>Couldn't open this chat.</p>
                <p className="mt-1 opacity-70">{convError ?? loadError}</p>
              </ThreadChatEmpty>
            ) : chatState === 'loading' ? (
              <ThreadChatEmpty testId="home-chat-loading">
                <p>Loading…</p>
              </ThreadChatEmpty>
            ) : messages && messages.length === 0 ? (
              <ThreadChatEmpty testId="home-chat-empty">
                <p style={{ color: 'var(--zen-text)' }}>No messages yet</p>
                <p className="mt-1">{dm ? `Send ${title} a message.` : `Messages here go to everyone in ${title}.`}</p>
              </ThreadChatEmpty>
            ) : messages ? (
              <ThreadMessageColumn gap={THREAD_LIST_GAP} testId="home-chat-column" style={threadTextColumnStyle(textCap)}>
                {hasMore && win.drawsOldest && (
                  <ThreadLoadOlder loading={loadingOlder} onClick={() => void onLoadOlder()} testId="home-chat-load-older" />
                )}
                <ThreadWindowSpacer px={win.above} gap={THREAD_LIST_GAP} edge="top" />
                {messages.slice(win.lo, win.hi).map((m, j) => {
                  if (m.call) {
                    return (
                      <div key={m.id} data-home-chat-message="" data-zen-message={m.id}>
                        <HomeChatCallRecord
                          line={callRecordLine(m.call, me)}
                          timeLabel={formatRelativeTime(m.createdAt, nowSec)}
                          onCallBack={dm && callBlocked === null ? dmCall.call : undefined}
                        />
                      </div>
                    )
                  }
                  const mine = !!me && m.authorId === me
                  // In a DM only the other person's name shows (§8.2).
                  const author = mine ? 'You' : authorName(m.authorId, me, names)
                  const original = m.replyTo ? messages.find((x) => x.id === m.replyTo) : undefined
                  return (
                    <HomeChatMessageRow
                      key={m.id}
                      m={m}
                      mine={mine}
                      author={author}
                      timeLabel={formatRelativeTime(m.createdAt, nowSec)}
                      still={win.wasDrawn(m.id)}
                      runStart={runs.starts[j]}
                      tail={runs.ends[j]}
                      reply={m.replyTo ? replyPreview(original, me, names) : null}
                      flash={flashId === m.id}
                      readOnly={readOnly || chatOff}
                      actions={actions}
                    />
                  )
                })}
                <ThreadWindowSpacer px={win.below} gap={THREAD_LIST_GAP} edge="bottom" />
              </ThreadMessageColumn>
            ) : null}
          </div>
        </SelectableRegion>

        {/* The box caps with the column, as the agent Thread's does. */}
        <div className="flex-shrink-0" data-testid="home-chat-compose-cap" style={{ width: '100%', maxWidth: composeCap, marginInline: 'auto' }}>
        {readOnly ? (
          <ThreadChatNote testId="home-chat-readonly">
            {archived ? 'This channel is archived: it is read-only.' : 'This person is no longer on this server: the conversation is read-only.'}
          </ThreadChatNote>
        ) : (
          <ThreadChatDraftComposer
            key={`${hostKey}|${conv ?? target.address}`}
            apiRef={draftApi}
            initialValue=""
            onDraftChange={onDraftChange}
            testId="home-chat-compose"
            inputTestId="home-chat-input"
            sendTestId="home-chat-send"
            onSend={send}
            busy={busy}
            disabled={chatState !== null}
            inputRef={inputRef}
            placeholder={dm ? `Message ${title}` : `Message #${title}`}
            onKeyDown={(e) => {
              if (e.key === 'Escape' && reply) {
                e.preventDefault()
                e.stopPropagation()
                setReply(null)
              }
            }}
            // The activity bar's place: who is typing (§9), never agent work.
            // Always there and of no height, so the box never moves.
            drawer={<HomeChatTyping names={typingNames} under={reply !== null} />}
            // The agent Thread's reply bar, on the box's edge (× or Esc).
            chips={
              reply ? (
                <ThreadReplyBar
                  target={{ id: reply.id, seq: 0, from: reply.from, addr: '', text: firstLine(reply.text) || reply.text, part: false }}
                  onCancel={() => setReply(null)}
                />
              ) : undefined
            }
            above={
              <>
                {jumpMiss && (
                  <ThreadChatNote testId="home-chat-jump-miss">
                    {jumpMiss.startsWith('gone:') ? (
                      'Original message deleted.'
                    ) : (
                      <>
                        Original not loaded.{' '}
                        <button type="button" className="underline" data-testid="home-chat-jump-more" onClick={() => void jump(jumpMiss)}>
                          Load it
                        </button>
                      </>
                    )}
                  </ThreadChatNote>
                )}
                {sendError && (
                  <ThreadChatNote error testId="home-chat-send-error">
                    {homeChatSendFailedLine(sendError)}
                  </ThreadChatNote>
                )}
              </>
            }
          />
        )}
        </div>
      </ThreadChatScope>
    </div>
  )
}

/** The name the row was added with (a hint, until the server answers). */
function labelOf(target: HomeChatTarget): string {
  for (const h of useHomesStore.getState().homes) {
    const row = h.rows.find((r) => r.address === target.address)
    if (row?.label) return row.label
  }
  return target.kind === 'person' ? 'this person' : 'this channel'
}
