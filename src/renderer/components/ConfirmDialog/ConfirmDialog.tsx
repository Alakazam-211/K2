import { useEffect, useCallback } from 'react'
import { CONFIRM_DIALOG_CLOSED, useConfirmDialogStore } from '../../stores/confirm-dialog'
import { Button, DialogFrame, DialogScrim } from '@/components/ui'

export default function ConfirmDialog(): React.JSX.Element | null {
  const isOpen = useConfirmDialogStore((s) => s.isOpen)
  const title = useConfirmDialogStore((s) => s.title)
  const message = useConfirmDialogStore((s) => s.message)
  const confirmLabel = useConfirmDialogStore((s) => s.confirmLabel)
  const confirmDestructive = useConfirmDialogStore((s) => s.confirmDestructive)
  const onResolve = useConfirmDialogStore((s) => s.onResolve)
  const cancelLabel = useConfirmDialogStore((s) => s.cancelLabel)
  const safeDefault = useConfirmDialogStore((s) => s.safeDefault)
  const choices = useConfirmDialogStore((s) => s.choices)
  const onChoose = useConfirmDialogStore((s) => s.onChoose)
  const titleTooltip = useConfirmDialogStore((s) => s.titleTooltip)
  // Answers with a line each stack as rows above Cancel.
  const choiceRows = choices?.some((c) => c.hint) ?? false
  const close = useConfirmDialogStore((s) => s.close)

  // A `choose` dialog: one of its answers (Cancel is none of them).
  const handleChoice = useCallback(
    (id: string) => {
      onChoose?.(id)
      useConfirmDialogStore.setState({ ...CONFIRM_DIALOG_CLOSED })
    },
    [onChoose],
  )

  const handleConfirm = useCallback(() => {
    if (onResolve) {
      onResolve(true)
    }
    useConfirmDialogStore.setState({ ...CONFIRM_DIALOG_CLOSED })
  }, [onResolve])

  const handleCancel = useCallback(() => {
    close()
  }, [close])

  // Enter confirms (a `choose` dialog: its primary answer, if it has one).
  // Esc, the × and a press outside cancel (DialogFrame onCancel).
  useEffect(() => {
    if (!isOpen) return

    const handleKeyDown = (e: KeyboardEvent): void => {
      if (e.key !== 'Enter') return
      // A safe default: Enter presses the focused button (the no-op, or
      // a `choose` dialog's safe answer, which opens focused).
      if (safeDefault) return
      if (choices?.some((c) => c.safeDefault)) return
      if (choices) {
        const primary = choices.find((c) => c.primary)
        if (!primary) return
        e.preventDefault()
        e.stopPropagation()
        handleChoice(primary.id)
        return
      }
      e.preventDefault()
      e.stopPropagation()
      handleConfirm()
    }

    window.addEventListener('keydown', handleKeyDown, true)
    return () => window.removeEventListener('keydown', handleKeyDown, true)
  }, [isOpen, handleConfirm, handleChoice, choices, safeDefault])

  if (!isOpen) return null

  return (
    <>
      {/* Semi-transparent backdrop */}
      <DialogScrim />

      {/* Dialog */}
      <DialogFrame
        onCancel={handleCancel}
        style={{
          minWidth: 340,
          maxWidth: 480,
          padding: '20px 24px',
          fontFamily: 'var(--font-ui)'
        }}
      >
        {/* Title */}
        <div
          title={titleTooltip || undefined}
          style={{
            fontSize: '14px',
            fontWeight: 600,
            color: 'var(--color-text-primary)',
            marginBottom: 8,
            // Room for the frame's ×.
            paddingRight: 16
          }}
        >
          {title}
        </div>

        {/* Message */}
        {message ? (
        <div
          style={{
            fontSize: '12px',
            color: 'var(--color-text-secondary)',
            lineHeight: '1.5',
            marginBottom: 20,
            whiteSpace: 'pre-line',
            // Long unbroken text (a pairing token) wraps inside the dialog.
            overflowWrap: 'anywhere',
            userSelect: 'text',
          }}
        >
          {message}
        </div>
        ) : null}

        {/* Answers with a line each: rows above Cancel. */}
        {choices && choiceRows ? (
          <div role="group" style={{ display: 'flex', flexDirection: 'column', gap: 8, marginBottom: 16 }}>
            {choices.map((c) => (
              <Button
                key={c.id}
                variant={c.primary ? 'accent' : 'default'}
                size="md"
                autoFocus={c.safeDefault}
                className="leading-[1.4] w-full text-left !whitespace-normal"
                data-confirm-choice={c.id}
                onClick={(e) => {
                  e.stopPropagation()
                  handleChoice(c.id)
                }}
              >
                <span style={{ display: 'flex', flexDirection: 'column', alignItems: 'flex-start', gap: 2, whiteSpace: 'normal' }}>
                  <span style={{ fontWeight: 500 }}>{c.label}</span>
                  {c.hint ? (
                    <span style={{ fontSize: '11px', fontWeight: 400, color: 'var(--color-text-secondary)' }}>{c.hint}</span>
                  ) : null}
                </span>
              </Button>
            ))}
          </div>
        ) : null}

        {/* Buttons */}
        <div
          style={{
            display: 'flex',
            justifyContent: 'flex-end',
            gap: 8
          }}
        >
          <Button
            variant="ghost"
            size="md"
            className="leading-[1.4]"
            autoFocus={safeDefault}
            data-confirm-cancel=""
            onClick={(e) => {
              e.stopPropagation()
              handleCancel()
            }}
          >
            {cancelLabel}
          </Button>
          {choices && choiceRows ? null : choices ? (
            choices.map((c) => (
              <Button
                key={c.id}
                variant={c.primary ? 'accent' : 'default'}
                size="md"
                className="leading-[1.4] font-medium"
                data-confirm-choice={c.id}
                onClick={(e) => {
                  e.stopPropagation()
                  handleChoice(c.id)
                }}
              >
                {c.label}
              </Button>
            ))
          ) : (
          <Button
            variant={confirmDestructive ? 'danger-muted' : 'ghost'}
            size="md"
            className="leading-[1.4] font-medium"
            style={
              confirmDestructive
                ? undefined
                : { background: 'var(--color-bg-surface)', color: 'var(--color-text-primary)' }
            }
            onClick={(e) => {
              e.stopPropagation()
              handleConfirm()
            }}
          >
            {confirmLabel}
          </Button>
          )}
        </div>
      </DialogFrame>
    </>
  )
}
