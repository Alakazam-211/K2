// K2's own overlay in Zen (New Garden, and any K2 dialog over a Garden):
// portalled into the Zen root so it keeps the Garden's `--zen-*` tokens,
// registered as a K2 overlay (FC18) so the required-controls check reads a
// control under it as covered by K2, closed by Escape or a backdrop click.

import { useCallback, useEffect, useRef, useState } from 'react'
import { createPortal } from 'react-dom'
import { registerZenK2Overlay } from '@/lib/zen/zen-controls'

/** Portal target: the Zen root (keeps Zen's tokens), else the body. */
function useZenLayer(anchor: React.RefObject<HTMLElement | null>): HTMLElement | null {
  const [layer, setLayer] = useState<HTMLElement | null>(null)
  useEffect(() => {
    const root = anchor.current?.closest<HTMLElement>('[data-zen-root]') ?? document.querySelector<HTMLElement>('[data-zen-root]')
    setLayer(root ?? document.body)
  }, [anchor])
  return layer
}

export function ZenOverlay({
  label,
  onClose,
  children,
  testId,
  vars,
  width = 480,
}: {
  label: string
  onClose(): void
  children: React.ReactNode
  testId: string
  /** Extra CSS variables on the overlay (Settings maps `--zen-*` here). */
  vars?: React.CSSProperties
  width?: number
}): React.JSX.Element {
  const anchor = useRef<HTMLSpanElement | null>(null)
  const layer = useZenLayer(anchor)
  const boxRef = useRef<HTMLDivElement | null>(null)
  // A K2 overlay (FC18): while it is open, the required-controls check
  // reads a control under it as covered by K2, not by the page.
  const offOverlay = useRef<(() => void) | null>(null)
  useEffect(() => () => offOverlay.current?.(), [])
  const backdropRef = useCallback((el: HTMLDivElement | null) => {
    offOverlay.current?.()
    offOverlay.current = el ? registerZenK2Overlay(el) : null
  }, [])
  useEffect(() => {
    const onKey = (e: KeyboardEvent): void => {
      if (e.key !== 'Escape') return
      e.preventDefault()
      e.stopPropagation()
      onClose()
    }
    window.addEventListener('keydown', onKey, true)
    return () => window.removeEventListener('keydown', onKey, true)
  }, [onClose])
  useEffect(() => {
    // A field inside that took focus as it mounted (New Garden's name) keeps it.
    if (boxRef.current && !boxRef.current.contains(document.activeElement)) boxRef.current.focus({ preventScroll: true })
  }, [layer])
  return (
    <>
      <span ref={anchor} hidden />
      {layer &&
        createPortal(
          <div
            ref={backdropRef}
            className="no-drag flex items-center justify-center"
            data-zen-overlay=""
            style={{ ...vars, position: 'fixed', inset: 0, zIndex: 50, background: 'rgba(0, 0, 0, 0.32)' }}
            onMouseDown={(e) => {
              if (e.target === e.currentTarget) onClose()
            }}
          >
            <div
              ref={boxRef}
              role="dialog"
              aria-modal="true"
              aria-label={label}
              tabIndex={-1}
              data-testid={testId}
              className="flex flex-col"
              style={{
                width: `min(${width}px, calc(100vw - 32px))`,
                maxHeight: 'calc(100vh - 48px)',
                overflowY: 'auto',
                gap: 14,
                padding: 20,
                color: 'var(--zen-text)',
                background: 'var(--zen-surface-raised)',
                border: '1px solid var(--zen-border)',
                borderRadius: 'var(--zen-radius)',
                boxShadow: '0 18px 50px rgba(0, 0, 0, 0.25)',
                outline: 'none',
              }}
            >
              {children}
            </div>
          </div>,
          layer,
        )}
    </>
  )
}

export const zenButtonStyle = (primary: boolean): React.CSSProperties => ({
  padding: '5px 14px',
  borderRadius: 'calc(var(--zen-radius) - 4px)',
  border: `1px solid ${primary ? 'var(--zen-accent)' : 'var(--zen-border)'}`,
  background: primary ? 'var(--zen-accent)' : 'transparent',
  color: primary ? 'var(--zen-accent-text, #fff)' : 'var(--zen-text)',
  fontWeight: primary ? 600 : 400,
})
