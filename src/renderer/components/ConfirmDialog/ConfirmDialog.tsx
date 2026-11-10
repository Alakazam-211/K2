import { useEffect, useCallback } from 'react'
import { useConfirmDialogStore } from '../../stores/confirm-dialog'
import { Button, DialogFrame, DialogScrim } from '@/components/ui'

export default function ConfirmDialog(): React.JSX.Element | null {
  const isOpen = useConfirmDialogStore((s) => s.isOpen)
  const title = useConfirmDialogStore((s) => s.title)
  const message = useConfirmDialogStore((s) => s.message)
  const confirmLabel = useConfirmDialogStore((s) => s.confirmLabel)
  const confirmDestructive = useConfirmDialogStore((s) => s.confirmDestructive)
  const onResolve = useConfirmDialogStore((s) => s.onResolve)
  const close = useConfirmDialogStore((s) => s.close)

  const handleConfirm = useCallback(() => {
    if (onResolve) {
      onResolve(true)
    }
    useConfirmDialogStore.setState({
      isOpen: false,
      title: '',
      message: '',
      confirmLabel: 'Confirm',
      confirmDestructive: false,
      onResolve: null
    })
  }, [onResolve])

  const handleCancel = useCallback(() => {
    close()
  }, [close])

  // Enter confirms. Esc, the × and a press outside cancel (DialogFrame onCancel).
  useEffect(() => {
    if (!isOpen) return

    const handleKeyDown = (e: KeyboardEvent): void => {
      if (e.key === 'Enter') {
        e.preventDefault()
        e.stopPropagation()
        handleConfirm()
      }
    }

    window.addEventListener('keydown', handleKeyDown, true)
    return () => window.removeEventListener('keydown', handleKeyDown, true)
  }, [isOpen, handleConfirm])

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
            onClick={(e) => {
              e.stopPropagation()
              handleCancel()
            }}
          >
            Cancel
          </Button>
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
        </div>
      </DialogFrame>
    </>
  )
}
