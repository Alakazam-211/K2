import React from 'react'
import { useSettingsStore } from '@/stores/settings'
import type { TerminalSettings } from '@/stores/settings'
import { useTerminalSettingsStore } from '@/stores/terminal-settings'
import type { LinkClickMode, TerminalRenderer } from '@/stores/terminal-settings'
import { SettingRow } from '../controls/SettingControls'
import { SettingDropdown } from '../controls/SettingControls'
import type { SettingEntry } from '../searchManifest'

export const TERMINAL_MANIFEST: SettingEntry[] = [
  { id: 'terminal.font-family', section: 'terminal', label: 'Font Family', description: 'Typeface for terminal text', keywords: ['font', 'typeface'] },
  { id: 'terminal.font-size', section: 'terminal', label: 'Font Size', description: 'Text size in pixels', keywords: ['font', 'size', 'zoom'] },
  { id: 'terminal.cursor-style', section: 'terminal', label: 'Cursor Style', description: 'Bar, block, or underline', keywords: ['cursor', 'caret'] },
  { id: 'terminal.scrollback', section: 'terminal', label: 'Scrollback Buffer', description: 'Number of scrollback lines retained', keywords: ['history', 'buffer', 'scroll'] },
  { id: 'terminal.natural-text-editing', section: 'terminal', label: 'Natural Text Editing', description: 'Opt+Arrow word motion, Cmd+Arrow line motion', keywords: ['keyboard', 'edit', 'opt', 'alt'] },
  { id: 'terminal.link-click-mode', section: 'terminal', label: 'Link Click Mode', description: 'Click vs Cmd+Click to activate links', keywords: ['link', 'url', 'click'] },
  { id: 'terminal.open-links-in-split', section: 'terminal', label: 'Open Links in Split Pane', description: 'Open file links in a sibling pane when splits are active', keywords: ['link', 'split', 'pane'] },
  { id: 'terminal.renderer', section: 'terminal', label: 'Terminal Renderer', description: 'Alacritty (default)', keywords: ['renderer', 'engine', 'alacritty', 'v2', 'session stream', 'legacy'] },
]

export function TerminalSection(): React.JSX.Element {
  const terminal = useSettingsStore((s) => s.terminal)
  const updateTerminalSettings = useSettingsStore((s) => s.updateTerminalSettings)
  const linkClickMode = useTerminalSettingsStore((s) => s.linkClickMode)
  const setLinkClickMode = useTerminalSettingsStore((s) => s.setLinkClickMode)
  const openLinksInSplitPane = useTerminalSettingsStore((s) => s.openLinksInSplitPane)
  const setOpenLinksInSplitPane = useTerminalSettingsStore((s) => s.setOpenLinksInSplitPane)
  const renderer = useTerminalSettingsStore((s) => s.renderer)
  const setRenderer = useTerminalSettingsStore((s) => s.setRenderer)

  return (
    <div className="max-w-xl">
      <h2 className="text-sm font-medium text-[var(--color-text-primary)] mb-4">Terminal</h2>

      <div className="space-y-4">
        {/* Font Family */}
        <SettingRow settingId="terminal.font-family" label="Font Family">
          <input
            type="text"
            value={terminal.fontFamily}
            onChange={(e) => updateTerminalSettings({ fontFamily: e.target.value })}
            className="w-64 px-2 py-1 text-xs bg-[var(--color-bg-surface)] border border-[var(--color-border)] text-[var(--color-text-primary)] outline-none focus:border-[var(--color-accent)] no-drag"
          />
        </SettingRow>

        {/* Font Size */}
        <SettingRow settingId="terminal.font-size" label="Font Size">
          <div className="flex items-center gap-3">
            <input
              type="range"
              min={10}
              max={24}
              step={1}
              value={terminal.fontSize}
              onChange={(e) => updateTerminalSettings({ fontSize: parseInt(e.target.value, 10) })}
              className="w-40 no-drag k2so-slider"
            />
            <input
              type="number"
              min={10}
              max={24}
              value={terminal.fontSize}
              onChange={(e) => {
                const v = parseInt(e.target.value, 10)
                if (v >= 10 && v <= 24) updateTerminalSettings({ fontSize: v })
              }}
              className="w-14 px-2 py-1 text-xs bg-[var(--color-bg-surface)] border border-[var(--color-border)] text-[var(--color-text-primary)] outline-none focus:border-[var(--color-accent)] no-drag text-center"
            />
          </div>
        </SettingRow>

        {/* Cursor Style */}
        <SettingRow settingId="terminal.cursor-style" label="Cursor Style">
          <SettingDropdown
            value={terminal.cursorStyle}
            options={[
              { value: 'bar', label: 'Bar' },
              { value: 'block', label: 'Block' },
              { value: 'underline', label: 'Underline' },
            ]}
            onChange={(v) => updateTerminalSettings({ cursorStyle: v as TerminalSettings['cursorStyle'] })}
          />
        </SettingRow>

        {/* Scrollback */}
        <SettingRow settingId="terminal.scrollback" label="Scrollback Buffer">
          <input
            type="number"
            min={500}
            max={100000}
            step={500}
            value={terminal.scrollback}
            onChange={(e) => {
              const v = parseInt(e.target.value, 10)
              if (v >= 500 && v <= 100000) updateTerminalSettings({ scrollback: v })
            }}
            className="w-28 px-2 py-1 text-xs bg-[var(--color-bg-surface)] border border-[var(--color-border)] text-[var(--color-text-primary)] outline-none focus:border-[var(--color-accent)] no-drag text-center"
          />
        </SettingRow>

        {/* Natural Text Editing */}
        <SettingRow settingId="terminal.natural-text-editing" label={
          <span title="Opt+Arrow to move by word, Cmd+Arrow for line start/end, Opt+Backspace to delete word">
            Natural Text Editing
          </span>
        }>
          <button
            onClick={() => updateTerminalSettings({ naturalTextEditing: !terminal.naturalTextEditing })}
            className={`w-7 h-3.5 flex items-center transition-colors no-drag cursor-pointer flex-shrink-0 ${
              terminal.naturalTextEditing ? 'bg-[var(--color-accent)]' : 'bg-[var(--color-border)]'
            }`}
          >
            <span
              className={`w-2.5 h-2.5 bg-white block transition-transform ${
                terminal.naturalTextEditing ? 'translate-x-3.5' : 'translate-x-0.5'
              }`}
            />
          </button>
        </SettingRow>

        {/* Link Click Mode */}
        <SettingRow settingId="terminal.link-click-mode" label={
          <span title="How to activate clickable links (URLs and file paths) in terminal output">
            Link Click Mode
          </span>
        }>
          <SettingDropdown
            value={linkClickMode}
            options={[
              { value: 'click', label: 'Click' },
              { value: 'cmd-click', label: '\u2318 + Click' },
            ]}
            onChange={(v) => setLinkClickMode(v as LinkClickMode)}
          />
        </SettingRow>

        {/* Terminal Renderer.
         *
         *  0.39.0: Only one renderer remains — daemon-hosted Alacritty
         *  (v2). The Kessel JSON-stream beta was retired; any persisted
         *  'alacritty' (Legacy) or 'kessel' value is coerced to
         *  'alacritty-v2' by the store's setter and persist migration.
         *  The dropdown is kept as a single-option control for
         *  discoverability + future renderer additions.
         */}
        <SettingRow settingId="terminal.renderer" label={
          <span title="Alacritty runs on the daemon, survives Tauri quit, and supports heartbeats. Changing this only affects NEW terminals; existing tabs keep their current renderer.">
            Terminal Renderer
          </span>
        }>
          <SettingDropdown
            value={renderer === 'alacritty' ? 'alacritty-v2' : renderer}
            options={[
              { value: 'alacritty-v2', label: 'Alacritty' },
            ]}
            onChange={(v) => setRenderer(v as TerminalRenderer)}
          />
        </SettingRow>

        {/* Open Links in Split Pane */}
        <SettingRow settingId="terminal.open-links-in-split" label={
          <span title="When split panes are active, open file links in the sibling pane instead of a new tab">
            Open Links in Split Pane
          </span>
        }>
          <button
            onClick={() => setOpenLinksInSplitPane(!openLinksInSplitPane)}
            className={`w-7 h-3.5 flex items-center transition-colors no-drag cursor-pointer flex-shrink-0 ${
              openLinksInSplitPane ? 'bg-[var(--color-accent)]' : 'bg-[var(--color-border)]'
            }`}
          >
            <span
              className={`w-2.5 h-2.5 bg-white block transition-transform ${
                openLinksInSplitPane ? 'translate-x-3.5' : 'translate-x-0.5'
              }`}
            />
          </button>
        </SettingRow>

      </div>
    </div>
  )
}
