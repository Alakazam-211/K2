import { useEffect, useRef } from 'react'
import { useToastStore } from '@/stores/toast'
import { usePanelsStore } from '@/stores/panels'
import { useSettingsStore } from '@/stores/settings'
import type { Toast as ToastData } from '@/stores/toast'

const ACCENT_COLORS: Record<ToastData['type'], string> = {
  success: '#22c55e',
  error: '#ef4444',
  info: '#3b82f6'
}

const CORNER_SIZE = 8
const CORNER_WEIGHT = 2

/** Four L-shaped corner brackets */
function CornerBrackets({ color }: { color: string }): React.JSX.Element {
  const s = CORNER_SIZE
  const w = CORNER_WEIGHT
  return (
    <>
      {/* Top-left */}
      <span className="absolute top-0 left-0" style={{ width: s, height: w, background: color }} />
      <span className="absolute top-0 left-0" style={{ width: w, height: s, background: color }} />
      {/* Top-right */}
      <span className="absolute top-0 right-0" style={{ width: s, height: w, background: color }} />
      <span className="absolute top-0 right-0" style={{ width: w, height: s, background: color }} />
      {/* Bottom-left */}
      <span className="absolute bottom-0 left-0" style={{ width: s, height: w, background: color }} />
      <span className="absolute bottom-0 left-0" style={{ width: w, height: s, background: color }} />
      {/* Bottom-right */}
      <span className="absolute bottom-0 right-0" style={{ width: s, height: w, background: color }} />
      <span className="absolute bottom-0 right-0" style={{ width: w, height: s, background: color }} />
    </>
  )
}

function ToastItem({ toast }: { toast: ToastData }): React.JSX.Element {
  const removeToast = useToastStore((s) => s.removeToast)
  const progressRef = useRef<HTMLDivElement>(null)
  const color = ACCENT_COLORS[toast.type]

  useEffect(() => {
    const el = progressRef.current
    if (!el) return
    el.style.width = '100%'
    el.style.transition = `width ${toast.duration}ms linear`
    requestAnimationFrame(() => {
      el.style.width = '0%'
    })
  }, [toast.duration])

  return (
    <div className="relative bg-[var(--color-bg-elevated)] text-[var(--color-text-primary)] text-xs shadow-lg min-w-[240px] max-w-[360px] overflow-hidden">
      <CornerBrackets color={color} />
      <div className="px-3 py-2.5 flex flex-col gap-1.5">
        <div className="flex items-start gap-2">
          <span className="flex-1 leading-relaxed">{toast.message}</span>
          <button
            className="flex-shrink-0 w-4 h-4 flex items-center justify-center text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] transition-colors"
            onClick={() => removeToast(toast.id)}
          >
            <svg width="8" height="8" viewBox="0 0 8 8" fill="none" stroke="currentColor" strokeWidth="1.5">
              <line x1="1" y1="1" x2="7" y2="7" />
              <line x1="7" y1="1" x2="1" y2="7" />
            </svg>
          </button>
        </div>
        {toast.action && (
          <button
            className="self-start text-[10px] text-[var(--color-accent)] hover:text-[var(--color-accent)]/80 font-mono cursor-pointer transition-colors"
            onClick={() => {
              toast.action!.onClick()
              removeToast(toast.id)
            }}
          >
            {toast.action.label}
          </button>
        )}
      </div>
      {/* Progress bar along the bottom */}
      <div className="flex justify-end">
        <div
          ref={progressRef}
          className="h-[2px]"
          style={{ backgroundColor: color, opacity: 0.4 }}
        />
      </div>
    </div>
  )
}

export default function Toast(): React.JSX.Element | null {
  const toasts = useToastStore((s) => s.toasts)
  const rightPanelOpen = usePanelsStore((s) => s.rightPanelOpen)
  const rightPanelWidth = usePanelsStore((s) => s.rightPanelWidth)
  const settingsOpen = useSettingsStore((s) => s.settingsOpen)

  if (toasts.length === 0) return null

  // Settings page: centered. Otherwise: anchored to the left edge of the right sidebar
  // (or far right if sidebar is closed)
  const positionStyle: React.CSSProperties = settingsOpen
    ? { left: '50%', transform: 'translateX(-50%)' }
    : rightPanelOpen
      ? { right: rightPanelWidth + 12 }
      : { right: 12 }

  return (
    <div
      className="fixed bottom-4 z-[9999] flex flex-col items-end gap-2 pointer-events-auto"
      style={positionStyle}
    >
      {toasts.map((toast) => (
        <ToastItem key={toast.id} toast={toast} />
      ))}
    </div>
  )
}
