// The frame of K2's own sheets over a Garden widget's card (the G8 pattern:
// ZenLinkSheet, ZenDownloadSheet, ZenWorkSheet, ZenGuestRoomsSheet,
// ZenPermissionSheet, ZenSecretSheet). K2's DOM laid over the card, outside
// the sealed frame. The card is dimmed; the sheet sits in the middle.
//
// Every sheet cancels the same way (components/ui/dialog-cancel.tsx): the ×
// ("Close (Esc)"), Esc wherever focus is (focus is often still in the
// widget's frame), and a press outside the sheet (the dimmed card or
// anywhere else). `onCancel` is the sheet's no-op answer (Don't open,
// Don't save, Cancel, Close): it opens, grants and keeps nothing.
//
// Focus moves into the sheet as it opens unless something in it took focus:
// a key pressed in the widget's sealed frame never reaches K2's window, so
// Esc could not cancel while focus stayed there.

import { useEffect, useRef } from 'react'
import { DialogCloseButton, useDialogCancel } from '@/components/ui/dialog-cancel'

/** Room for the × beside a sheet's first line. */
export const ZEN_SHEET_TITLE_STYLE: React.CSSProperties = { paddingRight: 24 }

export interface ZenSheetProps {
  label: string
  testId: string
  /** The sheet's no-op answer; `undefined` holds it open (no ×). */
  onCancel: (() => void) | undefined
  maxWidth: number
  /** Long content scrolls inside the sheet. Every sheet now scrolls when
   *  it doesn't fit its card; kept so callers can still say so. */
  scroll?: boolean
  /** Extra style on the sheet itself (Thread card tokens). */
  panelStyle?: React.CSSProperties
  /** Extra attributes on the dialog element (`data-kind`). */
  dialogAttrs?: Record<string, string | undefined>
  children: React.ReactNode
}

export function ZenSheet({
  label,
  testId,
  onCancel,
  maxWidth,
  panelStyle,
  dialogAttrs,
  children,
}: ZenSheetProps): React.JSX.Element {
  const cancel = useDialogCancel(onCancel)
  const panel = useRef<HTMLDivElement | null>(null)
  useEffect(() => {
    if (panel.current && !panel.current.contains(document.activeElement)) panel.current.focus({ preventScroll: true })
  }, [])
  return (
    <div
      role="dialog"
      aria-modal="true"
      aria-label={label}
      data-testid={testId}
      {...dialogAttrs}
      className="absolute inset-0 z-20 flex items-center justify-center"
      style={{ padding: 12, background: 'rgba(0, 0, 0, 0.4)' }}
    >
      <div
        ref={panel}
        tabIndex={-1}
        className="w-full"
        data-zen-sheet=""
        onPointerDownCapture={cancel.onPointerDownCapture}
        style={{
          position: 'relative',
          maxWidth,
          // Never taller than the card: in a narrow or short widget the body
          // scrolls (the × stays pinned), it is never cut off.
          maxHeight: '100%',
          display: 'flex',
          flexDirection: 'column',
          fontSize: 12,
          color: 'var(--zen-text)',
          background: 'var(--zen-surface-raised)',
          border: '1px solid var(--zen-border)',
          outline: 'none',
          ...panelStyle,
        }}
      >
        {/* Pinned: the × sits on the sheet, outside the part that scrolls. */}
        {onCancel && <DialogCloseButton tone="zen" placement="corner" onClick={onCancel} />}
        <div
          data-zen-sheet-body=""
          style={{
            minHeight: 0,
            overflow: 'auto',
            padding: 12,
            display: 'flex',
            flexDirection: 'column',
            gap: 8,
          }}
        >
          {children}
        </div>
      </div>
    </div>
  )
}
