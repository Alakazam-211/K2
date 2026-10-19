// The Thread's look for chats that are not Threads (Rosson 2026-10-18): a
// ticket's discussion (the Tickets page's rail) and the project chat (the
// Projects page). Their data, routes and keys stay their own; what they
// draw is the Thread's: the same bubbles ("author · time" under each,
// markdown inside), the same message box and round Send, the same list
// spacing, all from `ThreadZenKit`. Nothing here is a copy of the Thread's
// CSS: the pieces are the kit's, and so are the tokens.
//
//   - `ThreadChatScope`: the Thread box. In the app it is a desktop Thread
//     box (`data-k2-thread-zen` + `THREAD_ZEN_DESKTOP_TOKENS`), so every
//     K2 Style paints it the way it paints the Thread (Square's chamfers,
//     Glass and Bezel round, light and dark from the palette). Inside Zen
//     (`ThreadChatZenContext`) the Zen root already sets the `--zen-*`
//     tokens and the kit's rules, so the box adds neither.
//   - `ThreadChatBubble`: one message as the Thread's bubble.
//   - `ThreadChatComposer`: the Thread's message box (field, Send, the
//     stacking past 3 lines). Enter sends and Shift+Enter is a new line, as
//     in the Thread; ⌘/Ctrl+Enter still sends.
//   - `ThreadChatChip`: a soft pill above the box (a ticket's quick answers).

import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useImperativeHandle,
  useMemo,
  useRef,
  useState,
  type CSSProperties,
  type JSX,
  type KeyboardEvent,
  type ReactNode,
  type Ref,
} from 'react'
import { useSettingsStore } from '@/stores/settings'
import {
  applyComposeTextareaAutosize,
  COMPOSE_TEXTAREA_MAX_HEIGHT,
} from '@/components/Terminal/terminalCompose'
import type { OverlayThreadItem } from '@/components/SessionView/overlayThread'
import { ZEN_SOLID_PROPS } from '@/lib/zen/zen-materials'
import {
  THREAD_COMPOSE_BOX_EDGE,
  THREAD_ZEN_DESKTOP_TOKENS,
  THREAD_ZEN_SCOPE_ATTR,
  ThreadBubble,
  ThreadComposeBox,
  ThreadComposeStackSpacer,
  ThreadSendButton,
  ThreadZenStyles,
  chamferCutFor,
  chamferVars,
  threadComposeLines,
  threadComposeStackNext,
  threadComposeTextSlotStyle,
} from './ThreadZenKit'

/** True inside Zen (its Tickets view): the Zen root sets the `--zen-*`
 *  tokens, so the box must not set the app's over them. */
export const ThreadChatZenContext = createContext(false)

/** Send's square side (px): the desktop Thread box's (`COMPOSE_TOOL.height`
 *  in TerminalComposeBar), so the field is 26 + 2×6 + 2×1 = 40px tall. */
export const THREAD_CHAT_SEND_SIZE = 26

/** The one-row layout's width beside the textarea: the field's padding and
 *  border on both sides, the gap before Send, and Send. */
export const THREAD_CHAT_ROW_CHROME = 2 * THREAD_COMPOSE_BOX_EDGE + 8 + THREAD_CHAT_SEND_SIZE

/** The props a Thread chat box puts on its root (exported for tests). */
export function threadChatScopeProps(inZen: boolean, fontSize: number): Record<string, unknown> & { style?: CSSProperties } {
  if (inZen) return { 'data-thread-chat': 'zen' }
  return {
    [THREAD_ZEN_SCOPE_ATTR]: '',
    'data-thread-chat': 'desktop',
    style: { ...THREAD_ZEN_DESKTOP_TOKENS, fontSize },
  }
}

/** The Thread box around a chat: its tokens, its rules, the font size of
 *  Code Editor → Appearance (as the Thread). */
export function ThreadChatScope({
  children,
  className,
  style,
  testId,
}: {
  children: ReactNode
  className?: string
  style?: CSSProperties
  testId?: string
}): JSX.Element {
  const inZen = useContext(ThreadChatZenContext)
  const fontSize = useSettingsStore((s) => s.editor.fontSize) || 13
  const scope = threadChatScopeProps(inZen, fontSize)
  return (
    <div className={className} data-testid={testId} {...scope} style={{ ...scope.style, ...style }}>
      <ThreadZenStyles />
      {children}
    </div>
  )
}

/** A chat message as an overlay item, so it draws as the Thread's bubble.
 *  `message` is not a card kind: the body is the bubble's text. */
export function threadChatItem(m: { id: string; seq?: number; author: string; body: string; createdAt: number }): OverlayThreadItem {
  return {
    collection: 'thread-chat',
    seq: m.seq ?? 0,
    id: m.id,
    doc: { id: m.id, kind: 'message', from: m.author, body: m.body, created_at: m.createdAt },
  }
}

/** One message: mine on the right in the accent, others on the left;
 *  "author · time" under it, and `footer` under that. */
export function ThreadChatBubble({
  id,
  seq,
  author,
  body,
  createdAt,
  mine,
  timeLabel,
  footer,
  still,
}: {
  id: string
  seq?: number
  /** The author as stored (shown when it is not the viewer's own post). */
  author: string
  body: string
  createdAt: number
  mine: boolean
  timeLabel: string
  footer?: ReactNode
  /** Drawn again by the render window: no message-in animation. */
  still?: boolean
}): JSX.Element {
  // One item per message (not per render), so the memoized bubble skips.
  const item = useMemo(() => threadChatItem({ id, seq, author, body, createdAt }), [id, seq, author, body, createdAt])
  return (
    <ThreadBubble
      item={item}
      mine={mine}
      author={mine ? 'You' : author}
      timeLabel={timeLabel}
      viewer={null}
      footer={footer}
      still={still}
    />
  )
}

/** Enter sends (Shift+Enter is a new line), as in the Thread; ⌘/Ctrl+Enter
 *  sends too. Not while an IME is composing. */
export function threadChatSendKey(e: Pick<KeyboardEvent, 'key' | 'shiftKey' | 'altKey'> & { nativeEvent?: { isComposing?: boolean } }): boolean {
  if (e.key !== 'Enter') return false
  if (e.nativeEvent?.isComposing) return false
  return !e.shiftKey && !e.altKey
}

/** What a chat's code reads and writes of a `ThreadChatDraftComposer`'s
 *  text (on Send, after a send went through). */
export interface ThreadChatDraftApi {
  get: () => string
  set: (text: string) => void
}

/** The Thread's message box owning its draft (K2 Companion's
 *  `ThreadDraftComposer`, 0.46.2): a keystroke re-renders the box only,
 *  never the chat around it and its messages. The chat hears about the
 *  text on Send (`onSend(text)`), sets it through `apiRef` (clear after a
 *  send), and `onDraftChange` persists it. Key it by the chat (a ticket, a
 *  project) so another chat starts from its own `initialValue`. */
export function ThreadChatDraftComposer({
  initialValue,
  onDraftChange,
  onSend,
  apiRef,
  ...box
}: Omit<ThreadChatComposerProps, 'value' | 'onChange' | 'onSend'> & {
  initialValue: string
  onDraftChange?: (text: string) => void
  onSend: (text: string) => void
  apiRef?: Ref<ThreadChatDraftApi>
}): JSX.Element {
  const [value, setValue] = useState(initialValue)
  const valueRef = useRef(value)
  valueRef.current = value
  const persist = useRef(onDraftChange)
  persist.current = onDraftChange
  const change = useCallback((text: string) => {
    valueRef.current = text
    setValue(text)
    persist.current?.(text)
  }, [])
  useImperativeHandle(apiRef, () => ({ get: () => valueRef.current, set: change }), [change])
  return <ThreadChatComposer {...box} value={value} onChange={change} onSend={() => onSend(valueRef.current)} />
}

export interface ThreadChatComposerProps {
  value: string
  onChange: (text: string) => void
  onSend: () => void
  /** A send in flight: Send is off. */
  busy?: boolean
  placeholder: string
  inputTestId?: string
  sendTestId?: string
  sendLabel?: string
  inputRef?: Ref<HTMLTextAreaElement>
  /** Above the box: errors, notes, quick answers. */
  above?: ReactNode
  /** Beside the box, on its left (the project chat's Hide). */
  leading?: ReactNode
  /** On the box's top edge: the activity drawer (`ZenActivityDrawer`) of
   *  the session this chat posts to. */
  drawer?: ReactNode
  /** Runs first; `preventDefault` there skips the send keys. */
  onKeyDown?: (e: KeyboardEvent<HTMLTextAreaElement>) => void
  testId?: string
}

export function ThreadChatComposer({
  value,
  onChange,
  onSend,
  busy = false,
  placeholder,
  inputTestId,
  sendTestId,
  sendLabel = 'Send message',
  inputRef,
  above,
  leading,
  drawer,
  onKeyDown,
  testId,
}: ThreadChatComposerProps): JSX.Element {
  const fontSize = useSettingsStore((s) => s.editor.fontSize) || 13
  const ownRef = useRef<HTMLTextAreaElement | null>(null)
  const setRef = useCallback(
    (el: HTMLTextAreaElement | null) => {
      ownRef.current = el
      if (typeof inputRef === 'function') inputRef(el)
      else if (inputRef) (inputRef as { current: HTMLTextAreaElement | null }).current = el
    },
    [inputRef],
  )
  const canSend = !busy && value.trim().length > 0

  // Grow with the text; past 3 lines the box stacks (the kit's rule, the
  // Thread's box does the same).
  const [stacked, setStacked] = useState(false)
  const autoGrow = useCallback(() => {
    const el = ownRef.current
    if (!el) return
    applyComposeTextareaAutosize(el, fontSize)
    const lines = threadComposeLines(el, fontSize * 1.4, 8)
    setStacked((cur) => threadComposeStackNext({ stacked: cur, lines, width: el.clientWidth, rowChrome: THREAD_CHAT_ROW_CHROME }))
  }, [fontSize])
  useEffect(() => {
    autoGrow()
  }, [value, autoGrow])
  useEffect(() => {
    const el = ownRef.current
    if (!el || typeof ResizeObserver === 'undefined') return
    const ro = new ResizeObserver(() => autoGrow())
    ro.observe(el)
    return () => ro.disconnect()
  }, [autoGrow])

  return (
    <div
      className="flex min-w-0 w-full flex-shrink-0 flex-col"
      style={{ padding: '8px var(--zen-gap, 12px) var(--zen-gap, 12px)' }}
      data-zen-compose=""
      data-compose-look="zen"
      data-thread-chat-compose=""
      data-testid={testId}
    >
      {above}
      <div className="flex min-w-0 items-end" style={{ gap: 8 }}>
        {leading}
        <div className="flex min-w-0 flex-1 flex-col">
          <ThreadComposeBox stacked={stacked} drawer={drawer}>
            <div className="relative min-w-0 flex-1" style={{ alignSelf: 'center', ...threadComposeTextSlotStyle(stacked) }}>
              <textarea
                ref={setRef}
                data-testid={inputTestId}
                data-zen-compose-input=""
                value={value}
                onChange={(e) => onChange(e.target.value)}
                onKeyDown={(e) => {
                  onKeyDown?.(e)
                  if (e.defaultPrevented) return
                  if (threadChatSendKey(e)) {
                    e.preventDefault()
                    if (canSend) onSend()
                  }
                }}
                rows={1}
                placeholder={placeholder}
                title="Enter to send, Shift+Enter for a new line"
                className="block w-full resize-none overflow-x-hidden bg-transparent focus:outline-none selectable-copy"
                style={{
                  display: 'block',
                  width: '100%',
                  minWidth: 0,
                  fontFamily: 'inherit',
                  fontSize,
                  lineHeight: 1.4,
                  boxSizing: 'border-box',
                  border: 'none',
                  borderRadius: 0,
                  padding: '4px 6px',
                  maxHeight: COMPOSE_TEXTAREA_MAX_HEIGHT,
                  overflowY: 'auto',
                  overflowWrap: 'anywhere',
                  wordBreak: 'break-word',
                  color: 'var(--zen-text)',
                  caretColor: 'var(--zen-text)',
                  background: 'transparent',
                }}
              />
            </div>
            <ThreadComposeStackSpacer stacked={stacked} />
            <ThreadSendButton
              canSend={canSend}
              onClick={() => {
                if (canSend) onSend()
              }}
              label={sendLabel}
              title="Send (Enter)"
              size={THREAD_CHAT_SEND_SIZE}
              testId={sendTestId}
            />
          </ThreadComposeBox>
        </div>
      </div>
    </div>
  )
}

/** A line above the box: a refusal (`error`) or a quiet note. */
export function ThreadChatNote({ children, error = false, testId }: { children: ReactNode; error?: boolean; testId?: string }): JSX.Element {
  return (
    <div
      data-testid={testId}
      data-thread-chat-note={error ? 'error' : ''}
      className="selectable-copy"
      style={{
        margin: '0 6px 6px',
        fontSize: '0.8em',
        color: error ? 'var(--zen-danger)' : 'var(--zen-text-muted)',
      }}
    >
      {children}
    </div>
  )
}

/** A soft pill above the box (a ticket's quick answer). `on`: the accent
 *  fill (the answer the ticket took). */
export function ThreadChatChip({
  children,
  on = false,
  disabled,
  title,
  onClick,
  testId,
}: {
  children: ReactNode
  on?: boolean
  disabled?: boolean
  title?: string
  onClick: () => void
  testId?: string
}): JSX.Element {
  const fill = on ? 'var(--zen-accent)' : 'var(--zen-surface-raised)'
  return (
    <button
      type="button"
      data-testid={testId}
      data-thread-chat-chip={on ? 'on' : ''}
      data-zen-soft-button={on ? undefined : ''}
      {...ZEN_SOLID_PROPS}
      data-chamfer=""
      aria-pressed={on}
      disabled={disabled}
      title={title}
      onClick={onClick}
      className="max-w-full truncate cursor-pointer disabled:cursor-default"
      style={{
        padding: '4px 12px',
        borderRadius: 999,
        fontSize: '0.85em',
        border: `1px solid ${on ? 'var(--zen-accent)' : 'var(--zen-border)'}`,
        background: fill,
        color: on ? 'var(--zen-accent-text)' : 'var(--zen-text)',
        ...chamferVars({ cut: chamferCutFor(26), fill, edge: on ? 'var(--zen-accent)' : 'var(--zen-border)', border: 1 }),
      }}
    >
      {children}
    </button>
  )
}

/** The pills' row (above the box, under any notes). */
export function ThreadChatChips({ children, testId }: { children: ReactNode; testId?: string }): JSX.Element {
  return (
    <div data-testid={testId} className="flex flex-wrap" style={{ gap: 6, marginBottom: 8 }}>
      {children}
    </div>
  )
}

/** The centred muted line of an empty chat (the Thread's empty state). */
export function ThreadChatEmpty({ children, testId }: { children: ReactNode; testId?: string }): JSX.Element {
  return (
    <div
      data-testid={testId}
      data-zen-thread-empty=""
      className="h-full flex flex-col items-center justify-center px-6 text-center"
      style={{ color: 'var(--zen-text-muted)' }}
    >
      <div className="max-w-[40ch]">{children}</div>
    </div>
  )
}
