import { useEffect, useCallback } from 'react'
import { useConfirmDialogStore } from '../../stores/confirm-dialog'

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

  // Keyboard handling
  useEffect(() => {
    if (!isOpen) return

    const handleKeyDown = (e: KeyboardEvent): void => {
      if (e.key === 'Escape') {
        e.preventDefault()
        e.stopPropagation()
        handleCancel()
      } else if (e.key === 'Enter') {
        e.preventDefault()
        e.stopPropagation()
        handleConfirm()
      }
    }

    window.addEventListener('keydown', handleKeyDown, true)
    return () => window.removeEventListener('keydown', handleKeyDown, true)
  }, [isOpen, handleCancel, handleConfirm])

  if (!isOpen) return null

  return (
    <>
      {/* Semi-transparent backdrop */}
      <div
        style={{
          position: 'fixed',
          inset: 0,
          zIndex: 99998,
          background: 'rgba(0, 0, 0, 0.5)'
        }}
        onMouseDown={(e) => {
          e.stopPropagation()
          handleCancel()
        }}
      />

      {/* Dialog */}
      <div
        className="no-drag"
        style={{
          position: 'fixed',
          top: '50%',
          left: '50%',
          transform: 'translate(-50%, -50%)',
          zIndex: 99999,
          minWidth: 340,
          maxWidth: 480,
          background: 'var(--color-bg-surface)',
          border: '1px solid var(--color-border)',
          boxShadow: '0 8px 32px rgba(0, 0, 0, 0.6), 0 2px 8px rgba(0, 0, 0, 0.4)',
          padding: '20px 24px',
          fontFamily:
            "'MesloLGM Nerd Font', Menlo, Monaco, 'Cascadia Code', 'Fira Code', 'SF Mono', Consolas, monospace"
        }}
      >
        {/* Title */}
        <div
          style={{
            fontSize: '14px',
            fontWeight: 600,
            color: 'var(--color-text-primary)',
            marginBottom: 8
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
          <button
            onClick={(e) => {
              e.stopPropagation()
              handleCancel()
            }}
            style={{
              padding: '6px 14px',
              fontSize: '12px',
              fontFamily: 'inherit',
              border: '1px solid var(--color-border)',
              background: 'transparent',
              color: 'var(--color-text-secondary)',
              cursor: 'pointer',
              lineHeight: '1.4'
            }}
          >
            Cancel
          </button>
          <button
            onClick={(e) => {
              e.stopPropagation()
              handleConfirm()
            }}
            style={{
              padding: '6px 14px',
              fontSize: '12px',
              fontFamily: 'inherit',
              border: '1px solid',
              borderColor: confirmDestructive ? '#c53030' : 'var(--color-border)',
              background: confirmDestructive ? '#c53030' : 'var(--color-bg-surface)',
              color: confirmDestructive ? '#fff' : 'var(--color-text-primary)',
              cursor: 'pointer',
              fontWeight: 500,
              lineHeight: '1.4'
            }}
          >
            {confirmLabel}
          </button>
        </div>
      </div>
    </>
  )
}
