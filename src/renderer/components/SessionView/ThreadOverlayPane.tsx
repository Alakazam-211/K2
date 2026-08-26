import { useCallback, useState, type JSX, type KeyboardEvent } from 'react'
import { useOverlayThread } from './useOverlayThread'
import {
  isVoidedHitl,
  type OverlayThreadItem,
} from './overlayThread'

interface ThreadOverlayPaneProps {
  addr: string
  conversationId: string | null
}

export function ThreadOverlayPane({
  addr,
  conversationId,
}: ThreadOverlayPaneProps): JSX.Element {
  const { items, error, posting, post, answer, voidCard } = useOverlayThread({
    addr,
    conversationId,
    enabled: true,
  })
  const [draft, setDraft] = useState('')

  const send = useCallback(async () => {
    const text = draft
    if (!text.trim() || posting) return
    setDraft('')
    try {
      await post(text)
    } catch {
      setDraft(text)
    }
  }, [draft, posting, post])

  const onKeyDown = (e: KeyboardEvent<HTMLTextAreaElement>) => {
    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault()
      void send()
    }
  }

  const visible = items.filter((it) => !isVoidedHitl(it.doc))

  return (
    <div
      className="h-full flex flex-col min-h-0 bg-[var(--color-bg)]"
      data-testid="thread-overlay-pane"
    >
      <div className="flex-1 min-h-0 overflow-y-auto px-3 py-2 space-y-2">
        {error && (
          <div className="text-[11px] text-[var(--color-text-muted)]">{error}</div>
        )}
        {!error && visible.length === 0 && (
          <div className="text-[11px] text-[var(--color-text-muted)]">
            No overlay posts yet. Compose below — this is not PTY inject.
          </div>
        )}
        {visible.map((it) => (
          <ThreadItemRow
            key={it.id}
            item={it}
            onAnswer={(payload) => void answer(it.id, payload)}
            onVoid={() => void voidCard(it.id)}
          />
        ))}
      </div>
      <div
        className="flex-shrink-0 border-t border-[var(--color-border)] px-3 py-2"
        data-compose-bar=""
      >
        <textarea
          data-testid="thread-compose"
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          onKeyDown={onKeyDown}
          disabled={posting || !addr.trim()}
          placeholder="Message the thread (not the terminal)"
          rows={1}
          className="w-full resize-none bg-transparent text-[12px] text-[var(--color-text-primary)] placeholder:text-[var(--color-text-muted)] outline-none"
        />
      </div>
    </div>
  )
}

export function ThreadItemRow({
  item,
  onAnswer,
  onVoid,
}: {
  item: OverlayThreadItem
  onAnswer?: (payload: { answer?: string; secret?: string }) => void
  onVoid?: () => void
}): JSX.Element {
  const kind = item.doc.kind
  return (
    <div data-testid="thread-item" data-kind={kind} data-seq={item.seq} data-status={
      item.doc.choice?.status || item.doc.secret?.status || ''
    }>
      <div className="text-[10px] text-[var(--color-text-muted)]">
        {item.doc.from || 'unknown'} · seq {item.seq}
      </div>
      {kind === 'choice' && item.doc.choice ? (
        <ChoiceCard
          prompt={item.doc.choice.prompt || item.doc.body || ''}
          options={item.doc.choice.options}
          allowCustom={item.doc.choice.allow_custom}
          status={item.doc.choice.status}
          answer={item.doc.choice.answer}
          onPick={(label) => onAnswer?.({ answer: label })}
        />
      ) : kind === 'secret' && item.doc.secret ? (
        <SecretCard
          name={item.doc.secret.name}
          prompt={item.doc.secret.prompt || item.doc.body}
          status={item.doc.secret.status}
          onSubmit={(value) => onAnswer?.({ secret: value })}
          onDismiss={() => onVoid?.()}
        />
      ) : (
        <div className="text-[12px] text-[var(--color-text-primary)] whitespace-pre-wrap">
          {item.doc.body || ''}
        </div>
      )}
    </div>
  )
}

function ChoiceCard({
  prompt,
  options,
  allowCustom,
  status,
  answer,
  onPick,
}: {
  prompt: string
  options: { label: string }[]
  allowCustom: boolean
  status: string
  answer?: string | null
  onPick: (label: string) => void
}): JSX.Element {
  const pending = status === 'pending'
  const [custom, setCustom] = useState('')
  return (
    <div data-testid="thread-choice-card">
      <div className="text-[12px] text-[var(--color-text-primary)] whitespace-pre-wrap mb-2">
        {prompt}
      </div>
      <div className="flex flex-wrap gap-2">
        {options.map((opt, i) => {
          const selected = answer === opt.label
          return (
            <button
              key={`${opt.label}-${i}`}
              type="button"
              data-testid="thread-choice-chip"
              data-label={opt.label}
              data-primary={i === 0 ? 'true' : 'false'}
              disabled={!pending}
              onClick={() => onPick(opt.label)}
              className={`px-3 py-1.5 text-[11px] font-medium border transition-colors ${
                selected
                  ? 'border-[var(--color-accent)] bg-[var(--color-accent)]/15 text-[var(--color-text-primary)]'
                  : pending
                    ? 'border-[var(--color-border)] text-[var(--color-text-secondary)] hover:border-[var(--color-accent)] hover:text-[var(--color-text-primary)] cursor-pointer'
                    : 'border-[var(--color-border)] text-[var(--color-text-muted)] opacity-50'
              } disabled:cursor-not-allowed`}
            >
              {opt.label}
            </button>
          )
        })}
      </div>
      {allowCustom && pending && (
        <div className="mt-2 flex gap-2">
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
  prompt,
  status,
  onSubmit,
  onDismiss,
}: {
  name: string
  prompt?: string | null
  status: string
  onSubmit: (value: string) => void
  onDismiss: () => void
}): JSX.Element {
  const pending = status === 'pending'
  const [value, setValue] = useState('')
  return (
    <div data-testid="thread-secret-card">
      <div className="text-[12px] text-[var(--color-text-primary)] mb-1">
        {prompt || `Secret ${name}`}
      </div>
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
