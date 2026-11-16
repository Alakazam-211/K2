// "Say something to X": the field (K2's own), a chip naming what the message
// points at (it goes as an address; the chip keeps one width so the field's
// edges never move), and Send standing apart. The last line of the
// conversation shows above it; a failure keeps the words and offers Send
// again (the main Send steps aside); while not connected the field is off
// ("Back online to send"). Words and states: lib/live-view/shared/say.ts.

import { Button, cx, Input } from '@/components/ui'
import { sayView, type SayState } from '@/lib/live-view/shared/say'
import { AtGlyph, CloseGlyph, SendGlyph, WarnGlyph } from './glyphs'

export function SayBar({
  toName,
  address,
  state,
  offline,
  paused,
  onDraft,
  onSend,
  onSendAgain,
  onClose,
}: {
  toName: string
  /** What the message points at: the chip's words and its tooltip. */
  address: { label: string; title: string } | null
  state: SayState
  offline: boolean
  paused: boolean
  onDraft: (text: string) => void
  onSend: () => void
  onSendAgain: () => void
  /** The row was opened to ask someone (not by following): it can close. */
  onClose?: () => void
}): React.JSX.Element {
  const v = sayView(state, { toName, offline, paused })
  const label = `Say something to ${toName}${address ? ` about ${address.label}` : ''}`
  return (
    <div data-testid="lv-say" className="lv-say flex flex-none flex-col gap-1 px-3 py-2">
      {v.line ? (
        v.line.tone === 'bad' ? (
          <div role="alert" className="type-hint flex items-center gap-1.5 text-[var(--color-text-primary)]">
            <span aria-hidden="true" className="lv-bad-ink inline-flex">
              <WarnGlyph size={14} />
            </span>
            <span>{v.line.text}</span>
            <Button size="sm" onClick={onSendAgain} disabled={offline} className="text-[var(--color-text-primary)]">
              {v.line.action}
            </Button>
          </div>
        ) : (
          <div role="status" data-testid="lv-say-last" className="type-hint truncate text-[var(--color-text-primary)]">
            <b className="font-semibold">{v.line.who}:</b> {v.line.text}
            {v.line.note ? <span className="text-[var(--color-text-secondary)]"> {v.line.note}</span> : null}
          </div>
        )
      ) : null}
      <div className="flex items-center gap-2">
        <Input
          type="text"
          aria-label={label}
          placeholder={v.placeholder}
          disabled={v.disabled}
          value={state.draft}
          onChange={(e) => onDraft(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === 'Enter' && !e.nativeEvent.isComposing) {
              e.preventDefault()
              if (v.canSend) onSend()
            }
          }}
          className="min-w-0 flex-1"
          data-testid="lv-say-field"
        />
        {address ? (
          <span
            data-testid="lv-say-chip"
            data-k2-chamfer="chip"
            title={`What your message points at: ${address.title}`}
            className="lv-say-chip type-hint inline-flex h-[24px] w-[160px] flex-none items-center gap-1 overflow-hidden rounded-[var(--radius-pill)] border border-[var(--color-control-edge)] bg-[var(--color-bg-elevated)] px-2 text-[var(--color-text-primary)]"
          >
            <span aria-hidden="true" className="inline-flex flex-none text-[var(--color-text-secondary)]">
              <AtGlyph size={11} />
            </span>
            <span className="truncate">{address.label}</span>
          </span>
        ) : null}
        <Button
          size="md"
          variant={v.sendPrimary ? 'accent' : 'default'}
          data-variant={v.sendPrimary ? 'accent' : 'default'}
          disabled={!v.canSend}
          onClick={onSend}
          title={`Send to ${toName}`}
          className={cx('ml-1 inline-flex flex-none items-center gap-1.5', v.sendPrimary ? '' : 'text-[var(--color-text-primary)]')}
        >
          <SendGlyph size={14} />
          Send
        </Button>
        {onClose ? (
          <Button size="md" aria-label="Close" title="Close" onClick={onClose} className="inline-flex flex-none items-center text-[var(--color-text-primary)]">
            <CloseGlyph size={14} />
          </Button>
        ) : null}
      </div>
    </div>
  )
}
