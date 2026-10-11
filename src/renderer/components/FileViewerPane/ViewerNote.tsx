/** Centred status line for a preview pane (loading, fallback, error). */
export function ViewerNote({
  title,
  detail,
  tone = 'muted',
  testId,
}: {
  title: string
  detail?: string | null
  tone?: 'muted' | 'error'
  testId?: string
}): React.JSX.Element {
  return (
    <div
      className="flex h-full w-full flex-col items-center justify-center gap-1.5 px-8 text-center"
      data-testid={testId}
    >
      <span
        className={`text-sm ${tone === 'error' ? 'text-[var(--color-status-error-soft)]' : 'text-[var(--color-text-primary)]'}`}
      >
        {title}
      </span>
      {detail ? <span className="max-w-full break-words text-xs text-[var(--color-text-muted)]">{detail}</span> : null}
    </div>
  )
}
