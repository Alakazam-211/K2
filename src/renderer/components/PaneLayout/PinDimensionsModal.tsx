// Pin Dimensions modal — the pin-to-size entry point since the
// right-click rework (replaces v0.40.27's pushpin ContextMenu).
// Opened from a tab's context menu ("Pin Dimensions…"); LEFT is a
// scrollable preset list, RIGHT is two labeled fields the presets
// populate — the user can also just type fully custom dimensions.
// Apply POSTs the pin via applyPinSize and closes on success;
// daemon rejections surface inline and keep the modal open.
//
// Overlay/escape/close conventions mirror PresenceModal (backdrop
// mousedown closes, Escape closes via a capture-phase window
// listener, fixed-centered surface on the same z-band).
//
// All non-trivial logic (preset rows, populate/diverge form state,
// bounds validation, POST payload) lives in pinSizeMenu.ts as pure
// helpers — unit-tested there without rendering.

import { useEffect, useMemo, useState } from 'react'
import { usePinnedSizeStore } from '@/stores/pinned-size'
import {
  applyPinSize,
  buildPresetRows,
  PIN_BOUNDS,
  pinFormFieldEdited,
  pinFormFromPin,
  pinFormPresetClicked,
  validatePinDims,
  type PinFormState,
} from './pinSizeMenu'

interface PinDimensionsModalProps {
  sessionId: string
  onClose: () => void
}

export default function PinDimensionsModal({
  sessionId,
  onClose,
}: PinDimensionsModalProps): React.JSX.Element {
  // "Match my window now" uses the dims THIS window last measured for
  // the pane (recorded by TerminalPane's sendResize even while pinned).
  const dims = usePinnedSizeStore((s) => s.dims[sessionId] ?? null)
  const presetRows = useMemo(() => buildPresetRows(dims), [dims])

  // Re-pin path: when the session is already pinned, start from the
  // current pin so "tweak the numbers" is one edit away.
  const [form, setForm] = useState<PinFormState>(() =>
    pinFormFromPin(usePinnedSizeStore.getState().pins[sessionId] ?? null),
  )
  const [submitting, setSubmitting] = useState(false)
  const [submitError, setSubmitError] = useState<string | null>(null)

  const validation = validatePinDims(form.cols, form.rows)
  // Don't nag about the untouched empty form — the hint appears once
  // the user has typed (or preset-populated) something invalid.
  const showHint = !validation.ok && (form.cols.trim() !== '' || form.rows.trim() !== '')

  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent): void => {
      if (e.key === 'Escape') {
        e.preventDefault()
        e.stopPropagation()
        onClose()
      }
    }
    window.addEventListener('keydown', handleKeyDown, true)
    return () => window.removeEventListener('keydown', handleKeyDown, true)
  }, [onClose])

  const handleApply = (): void => {
    if (!validation.ok || submitting) return
    setSubmitting(true)
    setSubmitError(null)
    applyPinSize(sessionId, { cols: validation.cols, rows: validation.rows })
      .then(() => onClose())
      .catch((err) => {
        // Surface the daemon's message inline; the modal stays open so
        // the user can adjust and retry.
        setSubmitError(`Pin failed: ${err instanceof Error ? err.message : String(err)}`)
        setSubmitting(false)
      })
  }

  const handleField = (field: 'cols' | 'rows', value: string): void => {
    setForm((s) => pinFormFieldEdited(s, field, value))
    setSubmitError(null)
  }

  const fieldStyle: React.CSSProperties = {
    width: '100%',
    padding: '4px 8px',
    fontSize: '12px',
    background: 'var(--color-bg-primary)',
    border: '1px solid var(--color-border)',
    color: 'var(--color-text-primary)',
    outline: 'none',
  }

  return (
    <>
      {/* Semi-transparent backdrop */}
      <div
        style={{
          position: 'fixed',
          inset: 0,
          zIndex: 99998,
          background: 'rgba(0, 0, 0, 0.5)',
        }}
        onMouseDown={(e) => {
          e.stopPropagation()
          onClose()
        }}
      />

      {/* Dialog */}
      <div
        className="no-drag"
        data-pin-dimensions-modal=""
        style={{
          position: 'fixed',
          top: '50%',
          left: '50%',
          transform: 'translate(-50%, -50%)',
          zIndex: 99999,
          width: 420,
          maxWidth: 'calc(100vw - 48px)',
          maxHeight: 'calc(100vh - 96px)',
          display: 'flex',
          flexDirection: 'column',
          background: 'var(--color-bg-surface)',
          border: '1px solid var(--color-border)',
          boxShadow: '0 8px 32px rgba(0, 0, 0, 0.6), 0 2px 8px rgba(0, 0, 0, 0.4)',
        }}
      >
        {/* Header */}
        <div className="flex items-center justify-between px-4 py-3 border-b border-[var(--color-border)]">
          <div className="text-sm font-semibold text-[var(--color-text-primary)]">
            Pin Dimensions
          </div>
          <button
            onClick={onClose}
            className="flex h-6 w-6 items-center justify-center text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-elevated)] hover:text-[var(--color-text-primary)] transition-colors"
            title="Close (Esc)"
          >
            <svg width="12" height="12" viewBox="0 0 12 12" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round">
              <line x1="2" y1="2" x2="10" y2="10" />
              <line x1="10" y1="2" x2="2" y2="10" />
            </svg>
          </button>
        </div>

        {/* Body: preset list (left) + fields (right) */}
        <div className="flex gap-4 px-4 py-3" style={{ minHeight: 0 }}>
          {/* Preset list */}
          <div
            className="flex flex-col overflow-y-auto border border-[var(--color-border)]"
            style={{ width: 150, maxHeight: 224, flexShrink: 0 }}
            data-pin-preset-list=""
          >
            {presetRows.map((row) => {
              const selected = form.selectedPresetId === row.id
              return (
                <button
                  key={row.id}
                  className={`px-2.5 py-1.5 text-left text-xs transition-colors flex-shrink-0 ${
                    selected
                      ? 'bg-white/[0.10] text-[var(--color-accent)]'
                      : 'text-[var(--color-text-secondary)] hover:bg-white/[0.05] hover:text-[var(--color-text-primary)]'
                  }`}
                  onClick={() => {
                    setForm(pinFormPresetClicked(row))
                    setSubmitError(null)
                  }}
                  data-pin-preset-row={row.id}
                >
                  {row.label}
                </button>
              )
            })}
          </div>

          {/* Fields + validation + actions */}
          <div className="flex flex-1 flex-col min-w-0">
            <label className="text-[10px] uppercase tracking-wide text-[var(--color-text-muted)]">
              Columns (width)
            </label>
            <input
              type="text"
              inputMode="numeric"
              className="mt-1"
              style={fieldStyle}
              value={form.cols}
              placeholder={`${PIN_BOUNDS.minCols}–${PIN_BOUNDS.maxCols}`}
              onChange={(e) => handleField('cols', e.target.value)}
              onKeyDown={(e) => {
                if (e.key === 'Enter') handleApply()
              }}
              data-pin-cols-input=""
            />

            <label className="mt-3 text-[10px] uppercase tracking-wide text-[var(--color-text-muted)]">
              Rows (height)
            </label>
            <input
              type="text"
              inputMode="numeric"
              className="mt-1"
              style={fieldStyle}
              value={form.rows}
              placeholder={`${PIN_BOUNDS.minRows}–${PIN_BOUNDS.maxRows}`}
              onChange={(e) => handleField('rows', e.target.value)}
              onKeyDown={(e) => {
                if (e.key === 'Enter') handleApply()
              }}
              data-pin-rows-input=""
            />

            {/* Validation / submit error hint (reserved height — no
                layout jump when it appears) */}
            <div
              className="mt-2 text-[10px]"
              style={{ minHeight: 28, color: '#f66' }}
              data-pin-dimensions-hint=""
            >
              {submitError ?? (showHint && !validation.ok ? validation.error : '')}
            </div>

            <div className="mt-auto flex items-center justify-end gap-2 pt-2">
              <button
                className="px-3 py-1 text-xs text-[var(--color-text-secondary)] border border-[var(--color-border)] hover:bg-white/[0.05] hover:text-[var(--color-text-primary)] transition-colors"
                onClick={onClose}
              >
                Cancel
              </button>
              <button
                className="px-3 py-1 text-xs font-medium transition-colors disabled:opacity-40 disabled:cursor-not-allowed"
                style={{
                  background: 'var(--color-accent)',
                  color: 'var(--color-bg-primary)',
                }}
                disabled={!validation.ok || submitting}
                onClick={handleApply}
                data-pin-apply=""
              >
                {submitting ? 'Applying…' : 'Apply'}
              </button>
            </div>
          </div>
        </div>
      </div>
    </>
  )
}
