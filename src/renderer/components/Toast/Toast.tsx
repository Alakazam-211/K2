import { useEffect, useLayoutEffect, useRef, useState } from 'react'
import { floatMaterial } from '@/components/ui/float-material'
import { createPortal } from 'react-dom'
import { installingCliLabel, useCliInstallStore } from '@/lib/ensure-cli'
import { useToastStore } from '@/stores/toast'
import { useSettingsStore } from '@/stores/settings'
import { usePageViewStore, type AppPage } from '@/stores/page-view'
import { isEffectivelyHidden } from '@/lib/workspace-switch-focus'
import { useBrowserOverlay } from '@/lib/browser-overlay'
import type { Toast as ToastData } from '@/stores/toast'

const ACCENT_COLORS: Record<ToastData['type'], string> = {
  success: 'var(--color-status-ok)',
  error: 'var(--color-status-error)',
  info: 'var(--color-accent)',
  warning: 'var(--color-status-warn)'
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

  // A cut box in Terminal like K2's other floating surfaces (its wrapper
  // carries the drop shadow: a box-shadow can't follow a cut); the kind
  // shows as a dot before the words and in the time bar along the bottom.
  return (
    <div data-k2-chamfer-float="menu" data-toast-item={toast.type}>
      <div
        {...floatMaterial('toast')}
        data-k2-chamfer="surface"
        className="relative bg-[var(--color-bg-elevated)] text-[var(--color-text-primary)] type-control border border-[var(--color-border)] [box-shadow:var(--shadow-3)] min-w-[240px] max-w-[360px]"
      >
      {/* The clip lives inside, so the cut's edge (drawn just outside the
          padding box) isn't clipped with it. */}
      <div className="rounded-[inherit] overflow-hidden">
      <div className="px-3 py-2.5 flex flex-col gap-1.5">
        <div className="flex items-start gap-2">
          <span aria-hidden="true" className="mt-[0.4em] w-2 h-2 flex-shrink-0 rounded-[var(--radius-status)]" style={{ backgroundColor: color }} />
          <span className="flex-1 leading-relaxed">{toast.message}</span>
          <button
            type="button"
            aria-label="Dismiss"
            className="k2-focus flex-shrink-0 w-4 h-4 flex items-center justify-center text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] transition-colors"
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
      </div>
    </div>
  )
}

type ToastAnchor = HTMLElement | 'settings'

// Home is the Agents room shell (same panes + workspace host), so it is
// not listed: its toasts land where Agents' do.
const PAGE_HOST: Partial<Record<AppPage, string>> = {
  projects: 'projects',
  feedback: 'feedback',
  wiki: 'wiki',
  mail: 'mail',
}

function firstVisible(root: ParentNode, selector: string): HTMLElement | null {
  const nodes = root.querySelectorAll(selector)
  for (const el of nodes) {
    if (el instanceof HTMLElement && !isEffectivelyHidden(el)) return el
  }
  return null
}

/** Most specific visible anchor inside `root`. A display:none grid is skipped. */
function bestHostWithin(root: ParentNode): HTMLElement | null {
  if (root instanceof HTMLElement && root.matches('[data-terminal-container]') && !isEffectivelyHidden(root)) {
    return root
  }
  const terminal = firstVisible(root, '[data-terminal-container]')
  if (terminal) return terminal

  if (
    root instanceof HTMLElement &&
    root.getAttribute('data-testid') === 'agent-session-terminal' &&
    !isEffectivelyHidden(root)
  ) {
    return root
  }
  const session = firstVisible(root, '[data-testid="agent-session-terminal"]')
  if (session) return session

  if (
    root instanceof HTMLElement &&
    root.getAttribute('data-toast-host') === 'pane' &&
    !isEffectivelyHidden(root)
  ) {
    return root
  }
  const pane = firstVisible(root, '[data-toast-host="pane"]')
  if (pane) return pane

  if (
    root instanceof HTMLElement &&
    root.getAttribute('data-toast-host') === 'workspace' &&
    !isEffectivelyHidden(root)
  ) {
    return root
  }
  return firstVisible(root, '[data-toast-host="workspace"]')
}

function focusedScope(focus: Element): HTMLElement | null {
  const selectors = [
    '[data-pane-item-id]',
    '[data-terminal-container]',
    '[data-testid="agent-session-terminal"]',
    '[data-toast-host="pane"]',
    '[data-toast-host="workspace"]',
  ]
  for (const selector of selectors) {
    const el = focus.closest(selector)
    if (el instanceof HTMLElement && !isEffectivelyHidden(el)) return el
  }
  return null
}

/** One visible surface. Prefer the focused pane, else the on-screen grid. */
function findWorkspaceToastHost(): HTMLElement | null {
  const focus = document.activeElement
  if (focus instanceof Element && focus !== document.body && focus !== document.documentElement) {
    const scope = focusedScope(focus)
    if (scope) {
      const host = bestHostWithin(scope)
      if (host) return host
      if (scope.hasAttribute('data-pane-item-id')) {
        const pane = scope.closest('[data-toast-host="pane"]')
        if (pane instanceof HTMLElement && !isEffectivelyHidden(pane)) {
          const within = bestHostWithin(pane)
          if (within) return within
        }
      }
    }
  }
  return bestHostWithin(document)
}

function resolveToastAnchor(settingsOpen: boolean, page: AppPage): ToastAnchor | null {
  if (settingsOpen) return 'settings'
  // A modal sheet over the page (a project's settings) hosts the toasts
  // its own changes make: under its scrim they would be hidden.
  const sheet = firstVisible(document, '[data-toast-host="sheet"]')
  if (sheet) return sheet
  const pageHost = PAGE_HOST[page]
  if (pageHost) {
    const el = document.querySelector(`[data-toast-host="${pageHost}"]`)
    if (el instanceof HTMLElement && !isEffectivelyHidden(el)) return el
    // Page is the front surface. Do not fall through into a pane buried
    // under isolation:isolate.
    return null
  }
  return findWorkspaceToastHost()
}

const PANE_STACK_STYLE: React.CSSProperties = {
  position: 'absolute',
  top: 12,
  right: 12,
  zIndex: 30,
}

/** In a modal sheet, and on the Projects page: bottom centre, as over
 *  Settings, clear of the title-row actions at the top (the sheet's ×,
 *  the project bar's Layout and Chat). */
const BOTTOM_HOSTS = new Set(['sheet', 'projects'])
const SHEET_STACK_STYLE: React.CSSProperties = {
  position: 'absolute',
  bottom: 16,
  left: '50%',
  transform: 'translateX(-50%)',
  zIndex: 30,
}

export default function Toast(): React.JSX.Element | null {
  const toasts = useToastStore((s) => s.toasts)
  const installing = useCliInstallStore((s) => s.installing)
  const settingsOpen = useSettingsStore((s) => s.settingsOpen)
  const page = usePageViewStore((s) => s.page)
  const [anchor, setAnchor] = useState<ToastAnchor | null>(null)
  const installNotice = installing ? installingCliLabel(installing) : null
  const stackRef = useRef<HTMLDivElement>(null)
  const stackLive = settingsOpen ? 'settings' : anchor
  // The stack docks in the visible pane; over a Browser page it claims its
  // own box so the page freezes under it instead of hiding the toast.
  useBrowserOverlay({
    active: (toasts.length > 0 || installNotice !== null) && stackLive != null,
    ref: stackRef,
    kind: 'toast',
    watch: stackLive,
  })

  useLayoutEffect(() => {
    if (toasts.length === 0 && !installNotice) return
    const publish = (): void => {
      const next = resolveToastAnchor(settingsOpen, page)
      setAnchor((prev) => (prev === next ? prev : next))
    }
    publish()
    const id = window.setInterval(publish, 200)
    // View-menu clicks commit the grid's display:none before the event
    // reaches document, so the stack leaves a hidden grid in the same turn.
    document.addEventListener('click', publish)
    document.addEventListener('focusin', publish)
    return () => {
      window.clearInterval(id)
      document.removeEventListener('click', publish)
      document.removeEventListener('focusin', publish)
    }
  }, [toasts.length, installNotice, settingsOpen, page])

  if (toasts.length === 0 && !installNotice) return null

  const live = settingsOpen ? 'settings' : anchor
  if (live == null) return null

  const stack = (
    <div
      ref={stackRef}
      data-toast-stack=""
      className={
        live === 'settings'
          ? 'fixed bottom-4 z-[9999] flex flex-col items-end gap-2 pointer-events-auto'
          : 'flex flex-col items-end gap-2 pointer-events-auto'
      }
      style={
        live === 'settings'
          ? { left: '50%', transform: 'translateX(-50%)' }
          : BOTTOM_HOSTS.has(live.getAttribute('data-toast-host') ?? '')
            ? SHEET_STACK_STYLE
            : PANE_STACK_STYLE
      }
    >
      {installNotice && (
        <div
          {...floatMaterial('toast')} className="relative bg-[var(--color-bg-elevated)] text-[var(--color-text-primary)] text-xs shadow-lg min-w-[240px] max-w-[360px] px-3 py-2.5"
          data-cli-install-notice=""
        >
          {installNotice}
        </div>
      )}
      {toasts.map((toast) => (
        <ToastItem key={toast.id} toast={toast} />
      ))}
    </div>
  )

  if (live === 'settings') return stack
  if (live instanceof HTMLElement) return createPortal(stack, live)
  return null
}
