// The Mobile tab's notices (research-mobile-pane-polish-v1 §4 items 6, 9):
// toned toasts over the stage (an icon, the text, actions, ×) instead of
// plain strips that pushed the device down, and the hold pill at the top.
// A screenshot's toast carries its thumbnail and "Attach to Thread".

export type MobileNoticeTone = 'info' | 'success' | 'error'

export interface MobileNoticeAction {
  label: string
  onClick(): void
  testId: string
}

const TONE: Record<MobileNoticeTone, { icon: string; text: string }> = {
  info: { icon: 'text-[var(--color-accent)]', text: 'text-[var(--color-text-secondary)]' },
  success: { icon: 'text-[var(--color-status-ok)]', text: 'text-[var(--color-text-secondary)]' },
  error: { icon: 'text-[var(--color-status-error)]', text: 'text-[var(--color-status-error-text)]' },
}

function ToneIcon({ tone }: { tone: MobileNoticeTone }): React.JSX.Element {
  return (
    <svg className={`mt-px h-3.5 w-3.5 flex-shrink-0 ${TONE[tone].icon}`} fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2} aria-hidden>
      {tone === 'success' ? (
        <path strokeLinecap="round" strokeLinejoin="round" d="M5 13l4 4L19 7" />
      ) : tone === 'error' ? (
        <path strokeLinecap="round" strokeLinejoin="round" d="M12 8v5m0 3.5v.01M10.3 3.9 2.4 17.6A2 2 0 0 0 4.1 20.6h15.8a2 2 0 0 0 1.7-3L13.7 3.9a2 2 0 0 0-3.4 0Z" />
      ) : (
        <path strokeLinecap="round" strokeLinejoin="round" d="M12 16v-4m0-4h.01M21 12a9 9 0 1 1-18 0 9 9 0 0 1 18 0Z" />
      )}
    </svg>
  )
}

export function MobileNotice({
  tone,
  text,
  thumb,
  actions = [],
  onDismiss,
}: {
  tone: MobileNoticeTone
  text: string
  /** A screenshot's thumbnail (a data URL of the frame it captured). */
  thumb?: string | null
  actions?: MobileNoticeAction[]
  onDismiss(): void
}): React.JSX.Element {
  return (
    <div
      role={tone === 'error' ? 'alert' : 'status'}
      className="pointer-events-auto flex max-w-full items-start gap-2 border border-[var(--color-border)] bg-[var(--color-bg)] px-3 py-2 text-xs shadow-lg animate-[context-menu-fade-in_150ms_ease-out]"
      style={{ borderRadius: 'var(--radius-field, 0px)' }}
      data-mobile-notice-tone={tone}
    >
      {thumb ? (
        <img src={thumb} alt="" className="h-12 w-auto flex-shrink-0 border border-[var(--color-border)] object-contain" data-mobile-notice-thumb="" />
      ) : (
        <ToneIcon tone={tone} />
      )}
      <div className="flex min-w-0 flex-1 flex-col gap-1.5">
        <span className={`break-words ${TONE[tone].text}`} data-mobile-notice="">
          {text}
        </span>
        {actions.length > 0 ? (
          <div className="flex flex-wrap gap-1.5">
            {actions.map((a) => (
              <button
                key={a.testId}
                type="button"
                className="h-6 cursor-pointer border border-[var(--color-border)] px-2 text-xs text-[var(--color-text-primary)] hover:border-[var(--color-accent)]"
                style={{ borderRadius: 'var(--radius-field, 0px)' }}
                onClick={a.onClick}
                data-mobile-notice-action={a.testId}
              >
                {a.label}
              </button>
            ))}
          </div>
        ) : null}
      </div>
      <button
        type="button"
        aria-label="Dismiss"
        className="-mr-1 flex h-5 w-5 flex-shrink-0 cursor-pointer items-center justify-center text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)]"
        onClick={onDismiss}
        data-mobile-notice-dismiss=""
      >
        <svg className="h-3 w-3" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2.5} aria-hidden>
          <path strokeLinecap="round" strokeLinejoin="round" d="M6 6l12 12M18 6L6 18" />
        </svg>
      </button>
    </div>
  )
}

/** Who holds the device, as a pill at the top of the stage. */
export function MobileHoldPill({ text, kind }: { text: string; kind: string }): React.JSX.Element {
  return (
    <div
      className="flex items-center gap-1.5 border border-[var(--color-status-warn)] bg-[var(--color-status-warn-soft)] px-3 py-1 text-xs text-[var(--color-status-warn-text)] shadow-sm"
      style={{ borderRadius: 'calc(var(--radius-field, 0px) * 100)' }}
      data-mobile-held={kind}
    >
      <span className="h-1.5 w-1.5 flex-shrink-0 rounded-full bg-[var(--color-status-warn)]" aria-hidden />
      <span data-mobile-held-text="">{text}</span>
    </div>
  )
}
