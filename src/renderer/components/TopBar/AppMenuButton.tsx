import { useEffect, useRef, useState } from 'react'
import {
  APP_MENU_ACTION_IDS,
  handleAppMenuAction,
  type AppMenuActionId,
} from '@/lib/app-menu-actions'
import { APP_MENU_BUTTON_MIN_WIDTH_PX } from '@/lib/desktop-chrome'

type MenuEntry =
  | { kind: 'item'; id: AppMenuActionId; label: string }
  | { kind: 'sep' }

const MENU_SECTIONS: { title: string; items: MenuEntry[] }[] = [
  {
    title: 'K2',
    items: [
      { kind: 'item', id: 'settings', label: 'Settings…' },
      { kind: 'item', id: 'check-for-updates', label: 'Check for Updates…' },
      { kind: 'sep' },
      { kind: 'item', id: 'quit', label: 'Quit' },
    ],
  },
  {
    title: 'File',
    items: [
      { kind: 'item', id: 'new-document', label: 'New Document' },
      { kind: 'item', id: 'new-tab', label: 'New Tab' },
      { kind: 'item', id: 'launch-agent', label: 'Launch Default Agent' },
      { kind: 'sep' },
      { kind: 'item', id: 'split-pane', label: 'Split Pane' },
      { kind: 'sep' },
      { kind: 'item', id: 'open-workspace', label: 'Open Workspace…' },
      { kind: 'sep' },
      { kind: 'item', id: 'close-tab', label: 'Close Tab' },
    ],
  },
  {
    title: 'View',
    items: [
      { kind: 'item', id: 'command-palette', label: 'Command Palette' },
      { kind: 'item', id: 'running-agents', label: 'Running Agents' },
      { kind: 'item', id: 'projects', label: 'Projects' },
      { kind: 'item', id: 'toggle-sidebar', label: 'Toggle Sidebar' },
      { kind: 'item', id: 'toggle-assistant', label: 'Toggle Assistant' },
      { kind: 'item', id: 'focus-window', label: 'Open in Focus Window' },
    ],
  },
  {
    title: 'Window',
    items: [
      { kind: 'item', id: 'new-window', label: 'New Window' },
      { kind: 'sep' },
      { kind: 'item', id: 'minimize', label: 'Minimize' },
      { kind: 'item', id: 'maximize', label: 'Maximize' },
      { kind: 'sep' },
      { kind: 'item', id: 'close-window', label: 'Close Window' },
    ],
  },
]

// Compile-time guard: every APP_MENU_ACTION_IDS entry appears in MENU_SECTIONS.
const _menuIds = new Set(
  MENU_SECTIONS.flatMap((s) =>
    s.items.filter((i): i is Extract<MenuEntry, { kind: 'item' }> => i.kind === 'item').map((i) => i.id),
  ),
)
void APP_MENU_ACTION_IDS.every((id) => _menuIds.has(id))

export default function AppMenuButton(): React.JSX.Element {
  const [open, setOpen] = useState(false)
  const rootRef = useRef<HTMLDivElement>(null)

  useEffect(() => {
    if (!open) return
    const onDown = (e: MouseEvent): void => {
      if (rootRef.current && !rootRef.current.contains(e.target as Node)) setOpen(false)
    }
    const onKey = (e: KeyboardEvent): void => {
      if (e.key === 'Escape') {
        e.stopPropagation()
        setOpen(false)
      }
    }
    document.addEventListener('mousedown', onDown)
    window.addEventListener('keydown', onKey, true)
    return () => {
      document.removeEventListener('mousedown', onDown)
      window.removeEventListener('keydown', onKey, true)
    }
  }, [open])

  const run = (id: AppMenuActionId): void => {
    setOpen(false)
    void handleAppMenuAction(id).catch(() => {})
  }

  return (
    <div ref={rootRef} className="relative flex-shrink-0 no-drag">
      <button
        type="button"
        onClick={() => setOpen((o) => !o)}
        className="flex h-6 items-center justify-center px-2 text-[11px] font-medium text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-elevated)] hover:text-[var(--color-text-primary)] transition-colors no-drag"
        style={{
          minWidth: APP_MENU_BUTTON_MIN_WIDTH_PX,
          // @ts-expect-error -- Electron-specific CSS property
          WebkitAppRegion: 'no-drag',
        }}
        aria-haspopup="menu"
        aria-expanded={open}
        title="Menu"
      >
        Menu
      </button>

      {open && (
        <div
          role="menu"
          className="absolute left-0 top-full z-[100] mt-0.5 min-w-[200px] max-h-[min(70vh,480px)] overflow-y-auto border border-[var(--color-border)] bg-[var(--color-bg-elevated)] py-1 shadow-lg"
        >
          {MENU_SECTIONS.map((section, si) => (
            <div key={section.title}>
              {si > 0 && <div className="my-1 border-t border-[var(--color-border)]" />}
              <div className="px-2.5 py-1 text-[10px] font-medium uppercase tracking-wider text-[var(--color-text-muted)]">
                {section.title}
              </div>
              {section.items.map((item, ii) =>
                item.kind === 'sep' ? (
                  <div key={`${section.title}-sep-${ii}`} className="my-1 border-t border-[var(--color-border)]" />
                ) : (
                  <button
                    key={item.id}
                    type="button"
                    role="menuitem"
                    onClick={() => run(item.id)}
                    className="flex w-full items-center px-2.5 py-1.5 text-left text-xs text-[var(--color-text-secondary)] hover:bg-white/[0.06] hover:text-[var(--color-text-primary)] transition-colors cursor-pointer"
                  >
                    {item.label}
                  </button>
                ),
              )}
            </div>
          ))}
        </div>
      )}
    </div>
  )
}
