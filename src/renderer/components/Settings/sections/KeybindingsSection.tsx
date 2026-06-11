import React from 'react'
import { useEffect, useRef, useState } from 'react'
import { useSettingsStore, getEffectiveKeybinding } from '@/stores/settings'
import { useTerminalSettingsStore } from '@/stores/terminal-settings'
import {
  HOTKEYS,
  RESERVED_KEYS,
  formatKeyCombo,
  keyEventToCombo,
  isReservedKey,
} from '@shared/hotkeys'
import type { HotkeyDefinition } from '@shared/hotkeys'
import { KeyCombo } from '@/components/KeySymbol'
import type { SettingEntry } from '../searchManifest'

/**
 * Keybindings manifest: one entry per hotkey definition (dynamically
 * derived from HOTKEYS) plus the workspace-number-shortcuts layout
 * toggle. HOTKEYS is imported above, so changing it here once keeps
 * the search index in sync.
 */
export const KEYBINDINGS_MANIFEST: SettingEntry[] = [
  {
    id: 'keybindings.shortcut-layout',
    section: 'keybindings',
    label: 'Workspace Number Shortcuts',
    description: '⌘ 1-9 for Active vs Pinned (swap)',
    keywords: ['workspace', 'shortcut', 'layout', 'number', 'cmd', 'pinned', 'active'],
  },
  {
    id: 'keybindings.reset-all',
    section: 'keybindings',
    label: 'Reset All to Defaults',
    description: 'Revert every keybinding to its default',
    keywords: ['reset', 'defaults', 'keybindings'],
  },
  ...HOTKEYS.map((h) => ({
    id: `keybindings.${h.id}`,
    section: 'keybindings' as const,
    group: h.category,
    label: h.label,
    description: `Rebind the ${h.category} hotkey for ${h.label}`,
    keywords: [h.id, h.category.toLowerCase(), 'hotkey', 'shortcut', 'keybinding', 'rebind'],
  })),
]

export function KeybindingsSection(): React.JSX.Element {
  const keybindings = useSettingsStore((s) => s.keybindings)
  const updateKeybinding = useSettingsStore((s) => s.updateKeybinding)
  const resetKeybinding = useSettingsStore((s) => s.resetKeybinding)
  const resetAllKeybindings = useSettingsStore((s) => s.resetAllKeybindings)
  const shortcutLayout = useTerminalSettingsStore((s) => s.shortcutLayout)
  const setShortcutLayout = useTerminalSettingsStore((s) => s.setShortcutLayout)
  const [capturing, setCapturing] = useState<string | null>(null)

  // Build a map of combo -> ids to detect conflicts
  const comboToIds: Record<string, string[]> = {}
  for (const h of HOTKEYS) {
    const combo = getEffectiveKeybinding(keybindings, h.id)
    if (!comboToIds[combo]) comboToIds[combo] = []
    comboToIds[combo].push(h.id)
  }

  const hasConflict = (id: string): boolean => {
    const combo = getEffectiveKeybinding(keybindings, id)
    return (comboToIds[combo]?.length ?? 0) > 1
  }

  // Group hotkeys by category
  const categories = ['Terminal', 'Tabs', 'Navigation', 'App'] as const
  const grouped = categories.map((cat) => ({
    category: cat,
    items: HOTKEYS.filter((h) => h.category === cat)
  }))

  return (
    <div className="max-w-2xl">
      <div className="flex items-center justify-between mb-4">
        <h2 className="text-sm font-medium text-[var(--color-text-primary)]">Keybindings</h2>
        <button
          onClick={resetAllKeybindings}
          className="px-3 py-1 text-xs text-[var(--color-text-muted)] border border-[var(--color-border)] hover:text-[var(--color-text-primary)] no-drag cursor-pointer"
        >
          Reset All to Defaults
        </button>
      </div>

      <div className="text-xs text-[var(--color-text-muted)] mb-3">
        Click a binding to rebind. Press Escape to cancel. Reserved keys: {RESERVED_KEYS.join(', ')}
      </div>

      {/* Workspace number shortcut layout */}
      <div className="mb-4 border border-[var(--color-border)] px-3 py-2.5 flex items-center justify-between">
        <div>
          <div className="text-xs text-[var(--color-text-primary)]">Workspace Number Shortcuts</div>
          <div className="text-[10px] text-[var(--color-text-muted)] mt-0.5">
            {shortcutLayout === 'cmd-active-cmdshift-pinned'
              ? <><KeyCombo combo="⌘" /> 1-9 switches Active, <KeyCombo combo="⇧⌘" /> 1-9 switches Pinned</>
              : <><KeyCombo combo="⌘" /> 1-9 switches Pinned, <KeyCombo combo="⇧⌘" /> 1-9 switches Active</>
            }
          </div>
        </div>
        <button
          onClick={() => setShortcutLayout(
            shortcutLayout === 'cmd-active-cmdshift-pinned'
              ? 'cmd-pinned-cmdshift-active'
              : 'cmd-active-cmdshift-pinned'
          )}
          className="px-3 py-1 text-xs text-[var(--color-text-muted)] border border-[var(--color-border)] hover:text-[var(--color-text-primary)] hover:border-[var(--color-text-muted)] no-drag cursor-pointer font-mono flex-shrink-0"
        >
          Swap
        </button>
      </div>

      <div className="space-y-4">
        {grouped.map(({ category, items }) => (
          <div key={category}>
            <div className="text-xs font-medium text-[var(--color-text-muted)] uppercase tracking-wider mb-1 px-1">
              {category}
            </div>
            {category === 'Navigation' && (
              <div className="border border-[var(--color-border)] mb-px">
                <div className="flex items-center justify-between px-3 py-2 border-b border-[var(--color-border)]">
                  <div>
                    <span className="text-xs text-[var(--color-text-secondary)]">Active Workspaces</span>
                    <span className="text-[10px] text-[var(--color-text-muted)] ml-2">1-9</span>
                  </div>
                  <span className="text-xs font-mono text-[var(--color-text-muted)] bg-white/[0.06] px-2 py-0.5">
                    <KeyCombo combo={shortcutLayout === 'cmd-active-cmdshift-pinned' ? '⌘ 1-9' : '⌥⌘ 1-9'} />
                  </span>
                </div>
                <div className="flex items-center justify-between px-3 py-2">
                  <div>
                    <span className="text-xs text-[var(--color-text-secondary)]">Pinned Workspaces</span>
                    <span className="text-[10px] text-[var(--color-text-muted)] ml-2">1-9</span>
                  </div>
                  <span className="text-xs font-mono text-[var(--color-text-muted)] bg-white/[0.06] px-2 py-0.5">
                    <KeyCombo combo={shortcutLayout === 'cmd-active-cmdshift-pinned' ? '⌥⌘ 1-9' : '⌘ 1-9'} />
                  </span>
                </div>
              </div>
            )}
            <div className="border border-[var(--color-border)]">
              {items.map((hotkey, i) => (
                <KeybindingRow
                  key={hotkey.id}
                  hotkey={hotkey}
                  combo={getEffectiveKeybinding(keybindings, hotkey.id)}
                  isCustom={!!keybindings[hotkey.id]}
                  conflict={hasConflict(hotkey.id)}
                  capturing={capturing === hotkey.id}
                  onStartCapture={() => setCapturing(hotkey.id)}
                  onCapture={(combo) => {
                    updateKeybinding(hotkey.id, combo)
                    setCapturing(null)
                  }}
                  onCancelCapture={() => setCapturing(null)}
                  onReset={() => resetKeybinding(hotkey.id)}
                  hasBorder={i < items.length - 1}
                />
              ))}
            </div>
          </div>
        ))}
      </div>
    </div>
  )
}

interface KeybindingRowProps {
  hotkey: HotkeyDefinition
  combo: string
  isCustom: boolean
  conflict: boolean
  capturing: boolean
  onStartCapture: () => void
  onCapture: (combo: string) => void
  onCancelCapture: () => void
  onReset: () => void
  hasBorder: boolean
}

function KeybindingRow({
  hotkey,
  combo,
  isCustom,
  conflict,
  capturing,
  onStartCapture,
  onCapture,
  onCancelCapture,
  onReset,
  hasBorder
}: KeybindingRowProps): React.JSX.Element {
  const captureRef = useRef<HTMLButtonElement>(null)

  useEffect(() => {
    if (!capturing) return

    const handler = (e: KeyboardEvent): void => {
      e.preventDefault()
      e.stopPropagation()

      if (e.key === 'Escape') {
        onCancelCapture()
        return
      }

      const newCombo = keyEventToCombo(e)
      // Ignore bare modifier presses
      if (['Meta', 'Control', 'Alt', 'Shift'].includes(e.key)) return

      if (isReservedKey(newCombo)) return

      onCapture(newCombo)
    }

    window.addEventListener('keydown', handler, true)
    return () => window.removeEventListener('keydown', handler, true)
  }, [capturing, onCapture, onCancelCapture])

  useEffect(() => {
    if (capturing && captureRef.current) {
      captureRef.current.focus()
    }
  }, [capturing])

  return (
    <div
      className={`flex items-center justify-between px-3 py-1.5 ${
        hasBorder ? 'border-b border-[var(--color-border)]' : ''
      } ${conflict ? 'bg-red-500/10' : ''}`}
    >
      <span className="text-xs text-[var(--color-text-secondary)] flex-1">{hotkey.label}</span>

      <div className="flex items-center gap-2">
        {capturing ? (
          <button
            ref={captureRef}
            className="px-2 py-0.5 text-xs bg-[var(--color-accent)]/20 border border-[var(--color-accent)]/50 text-[var(--color-accent)] no-drag cursor-pointer animate-pulse"
          >
            Press a key...
          </button>
        ) : (
          <button
            onClick={onStartCapture}
            className={`px-2 py-0.5 text-xs border no-drag cursor-pointer ${
              conflict
                ? 'bg-red-500/20 border-red-500/40 text-red-400'
                : 'bg-[var(--color-bg-surface)] border-[var(--color-border)] text-[var(--color-text-primary)]'
            } hover:border-[var(--color-text-muted)]`}
          >
            <KeyCombo combo={formatKeyCombo(combo)} />
          </button>
        )}

        {isCustom && (
          <button
            onClick={onReset}
            className="text-xs text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] no-drag cursor-pointer"
            title="Reset to default"
          >
            x
          </button>
        )}
      </div>
    </div>
  )
}
