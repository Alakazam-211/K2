import { useEffect, useState, type JSX } from 'react'
import { ChatMessage } from '@/components/common/ChatMessage'
import { formatRelativeTime } from '@/lib/format-relative-time'
import { useSettingsStore } from '@/stores/settings'
import { useOverlayThread } from './useOverlayThread'
import {
  isVoidedHitl,
  type OverlayDoc,
  type OverlayThreadItem,
} from './overlayThread'

interface ThreadOverlayPaneProps {
  addr: string
  conversationId: string | null
}

/** Overlay log only — Message-the-agent compose stays on TerminalPane. */
export function ThreadOverlayPane({
  addr,
  conversationId,
}: ThreadOverlayPaneProps): JSX.Element {
  const { items, error, answer, voidCard } = useOverlayThread({
    addr,
    conversationId,
    enabled: true,
  })

  const visible = items.filter((it) => !isVoidedHitl(it.doc))
  const editorFontSize = useSettingsStore((s) => s.editor.fontSize) || 12
  const [nowSec, setNowSec] = useState(() => Math.floor(Date.now() / 1000))
  useEffect(() => {
    const id = setInterval(() => setNowSec(Math.floor(Date.now() / 1000)), 30_000)
    return () => clearInterval(id)
  }, [])

  return (
    <div
      className="h-full flex flex-col min-h-0 bg-[var(--color-bg)]"
      data-testid="thread-overlay-pane"
    >
      <div className="flex-1 min-h-0 overflow-y-auto px-2 py-2">
        {error && (
          <div className="text-[11px] text-[var(--color-text-muted)] px-2.5">{error}</div>
        )}
        {!error && visible.length === 0 && (
          <div className="text-[11px] text-[var(--color-text-muted)] px-2.5">
            No overlay posts yet. Message the agent below — Thread vs Terminal
            chooses where it is sent.
          </div>
        )}
        <div className="flex flex-col gap-2.5">
          {visible.map((it) => (
            <ThreadItemRow
              key={it.id}
              item={it}
              nowSec={nowSec}
              fontSize={editorFontSize}
              onAnswer={(payload) => void answer(it.id, payload)}
              onVoid={() => void voidCard(it.id)}
            />
          ))}
        </div>
      </div>
    </div>
  )
}

function isHumanPost(doc: OverlayDoc): boolean {
  return doc.via === 'compose' || doc.from === 'owner'
}

export function ThreadItemRow({
  item,
  nowSec,
  fontSize,
  onAnswer,
  onVoid,
}: {
  item: OverlayThreadItem
  nowSec?: number
  fontSize?: number
  onAnswer?: (payload: { answer?: string; secret?: string }) => void
  onVoid?: () => void
}): JSX.Element {
  const kind = item.doc.kind
  const owner = isHumanPost(item.doc)
  const author = owner ? 'You' : item.doc.from || 'unknown'
  const timeLabel = formatRelativeTime(
    item.doc.created_at,
    nowSec ?? Math.floor(Date.now() / 1000),
  )
  const prompt =
    kind === 'choice'
      ? item.doc.choice?.prompt || item.doc.body || ''
      : kind === 'secret'
        ? item.doc.secret?.prompt || item.doc.body || ''
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
              onPick={(label) => onAnswer?.({ answer: label })}
            />
          ) : kind === 'secret' && item.doc.secret ? (
            <SecretCard
              name={item.doc.secret.name}
              status={item.doc.secret.status}
              onSubmit={(value) => onAnswer?.({ secret: value })}
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

function ChoiceCard({
  options,
  allowCustom,
  status,
  answer,
  onPick,
}: {
  options: { label: string }[]
  allowCustom: boolean
  status: string
  answer?: string | null
  onPick: (label: string) => void
}): JSX.Element {
  const pending = status === 'pending'
  const [custom, setCustom] = useState('')
  return (
    <div
      data-testid="thread-choice-card"
      className="mt-1 w-full box-border flex flex-col gap-1.5 border border-[var(--color-border)] bg-[var(--color-bg)] px-2 py-1.5"
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
              className={`flex w-full items-center gap-2 box-border px-2 py-1.5 text-left text-[11px] font-medium border transition-colors ${
                selected
                  ? 'border-[var(--color-accent)] bg-[var(--color-accent)]/15 text-[var(--color-text-primary)]'
                  : pending
                    ? 'border-[var(--color-border)] text-[var(--color-text-secondary)] hover:border-[var(--color-accent)] hover:text-[var(--color-text-primary)] cursor-pointer'
                    : 'border-[var(--color-border)] text-[var(--color-text-muted)] opacity-50'
              } disabled:cursor-not-allowed`}
            >
              <span
                className={`w-4 shrink-0 text-[10px] font-semibold ${
                  selected
                    ? 'text-[var(--color-accent)]'
                    : 'text-[var(--color-text-muted)]'
                }`}
              >
                {letter}
              </span>
              <span className="min-w-0 flex-1">{opt.label}</span>
            </button>
          )
      })}
      {allowCustom && pending && (
        <div className="mt-0.5 flex gap-2">
          <input
            data-testid="thread-choice-custom"
            value={custom}
            onChange={(e) => setCustom(e.target.value)}
            placeholder="Custom…"
            className="flex-1 bg-transparent text-[12px] text-[var(--color-text-primary)] outline-none border border-[var(--color-border)] px-2 py-1"
          />
          <button
            type="button"
            data-testid="thread-choice-custom-submit"
            disabled={!custom.trim()}
            onClick={() => onPick(custom.trim())}
            className="px-2 py-1 text-[11px] border border-[var(--color-border)]"
          >
            Send
          </button>
        </div>
      )}
    </div>
  )
}

function SecretCard({
  name,
  status,
  onSubmit,
  onDismiss,
}: {
  name: string
  status: string
  onSubmit: (value: string) => void
  onDismiss: () => void
}): JSX.Element {
  const pending = status === 'pending'
  const [value, setValue] = useState('')
  return (
    <div data-testid="thread-secret-card">
      <div className="text-[10px] text-[var(--color-text-muted)] mb-2">{name}</div>
      {pending ? (
        <div className="flex gap-2 items-center">
          <input
            data-testid="thread-secret-field"
            type="password"
            autoComplete="off"
            value={value}
            onChange={(e) => setValue(e.target.value)}
            placeholder="Paste secret"
            className="flex-1 bg-transparent text-[12px] text-[var(--color-text-primary)] outline-none border border-[var(--color-border)] px-2 py-1"
          />
          <button
            type="button"
            data-testid="thread-secret-submit"
            disabled={!value}
            onClick={() => {
              const v = value
              setValue('')
              onSubmit(v)
            }}
            className="px-2 py-1 text-[11px] border border-[var(--color-border)]"
          >
            Set
          </button>
          <button
            type="button"
            data-testid="thread-secret-dismiss"
            onClick={onDismiss}
            className="px-2 py-1 text-[11px] text-[var(--color-text-muted)]"
          >
            Dismiss
          </button>
        </div>
      ) : (
        <div className="text-[11px] text-[var(--color-text-muted)]" data-testid="thread-secret-set">
          {name} {status}
        </div>
      )}
    </div>
  )
}
