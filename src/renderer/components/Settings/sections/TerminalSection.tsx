import React from 'react'
import { useSettingsStore } from '@/stores/settings'
import type { TerminalSettings } from '@/stores/settings'
import {
  useTerminalSettingsStore,
  LINE_HEIGHT_MULT_DEFAULT,
  LINE_HEIGHT_MULT_MIN,
  LINE_HEIGHT_MULT_MAX,
  CHAR_TRACKING_DEFAULT,
  CHAR_TRACKING_MIN,
  CHAR_TRACKING_MAX,
} from '@/stores/terminal-settings'
import type { LinkClickMode, TerminalPainterKind, TerminalRenderer } from '@/stores/terminal-settings'
import { SettingRow } from '../controls/SettingControls'
import { SettingDropdown } from '../controls/SettingControls'
import type { SettingEntry } from '../searchManifest'

export const TERMINAL_MANIFEST: SettingEntry[] = [
  { id: 'terminal.font-family', section: 'terminal', label: 'Font Family', description: 'Typeface for terminal text', keywords: ['font', 'typeface'] },
  { id: 'terminal.font-size', section: 'terminal', label: 'Font Size', description: 'Text size in pixels', keywords: ['font', 'size', 'zoom'] },
  { id: 'terminal.line-height', section: 'terminal', label: 'Line height (WebGL)', description: 'WebGL only — row spacing as a multiple of font size', keywords: ['line height', 'leading', 'spacing', 'rows', 'dense', 'open', 'webgl'] },
  { id: 'terminal.char-tracking', section: 'terminal', label: 'Character spacing (WebGL)', description: 'WebGL only — horizontal spacing between characters (tracking)', keywords: ['tracking', 'letter spacing', 'kerning', 'width', 'cell', 'character', 'open', 'tight', 'webgl'] },
  { id: 'terminal.cursor-style', section: 'terminal', label: 'Cursor Style', description: 'Bar, block, or underline', keywords: ['cursor', 'caret'] },
  { id: 'terminal.scrollback', section: 'terminal', label: 'Scrollback Buffer', description: 'Number of scrollback lines retained', keywords: ['history', 'buffer', 'scroll'] },
  { id: 'terminal.natural-text-editing', section: 'terminal', label: 'Natural Text Editing', description: 'Opt+Arrow word motion, Cmd+Arrow line motion', keywords: ['keyboard', 'edit', 'opt', 'alt'] },
  { id: 'terminal.link-click-mode', section: 'terminal', label: 'Link Click Mode', description: 'Click vs Cmd+Click to activate links', keywords: ['link', 'url', 'click'] },
  { id: 'terminal.open-links-in-split', section: 'terminal', label: 'Open Links in Split Pane', description: 'Open file links in a sibling pane when splits are active', keywords: ['link', 'split', 'pane'] },
  { id: 'terminal.renderer', section: 'terminal', label: 'Terminal Renderer', description: 'Kessel (default)', keywords: ['renderer', 'engine', 'alacritty', 'kessel', 'v2', 'session stream', 'legacy'] },
  { id: 'terminal.painter', section: 'terminal', label: 'Terminal Painter', description: 'DOM (default) or WebGL (experimental)', keywords: ['painter', 'webgl', 'gpu', 'canvas', 'rendering', 'experimental'] },
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
  const painter = useTerminalSettingsStore((s) => s.painter)
  const setPainter = useTerminalSettingsStore((s) => s.setPainter)
  const lineHeightMultiplier = useTerminalSettingsStore((s) => s.lineHeightMultiplier)
  const setLineHeightMultiplier = useTerminalSettingsStore((s) => s.setLineHeightMultiplier)
  const charTracking = useTerminalSettingsStore((s) => s.charTracking)
  const setCharTracking = useTerminalSettingsStore((s) => s.setCharTracking)

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

        {/* Line height — WebGL only. DOM keeps the fixed Kessel default. */}
        <SettingRow settingId="terminal.line-height" label={
          <span title="WebGL only. Vertical space per row as a multiple of font size. Default 1.2. Higher = more open leading. DOM painter is unchanged.">
            Line height (WebGL)
          </span>
        }>
          <div className="flex items-center gap-3 flex-wrap">
            <input
              type="range"
              min={LINE_HEIGHT_MULT_MIN}
              max={LINE_HEIGHT_MULT_MAX}
              step={0.02}
              value={lineHeightMultiplier}
              onChange={(e) => setLineHeightMultiplier(parseFloat(e.target.value))}
              className="w-40 no-drag k2so-slider"
              disabled={painter !== 'webgl'}
            />
            <input
              type="number"
              min={LINE_HEIGHT_MULT_MIN}
              max={LINE_HEIGHT_MULT_MAX}
              step={0.02}
              value={Number(lineHeightMultiplier.toFixed(2))}
              onChange={(e) => {
                const v = parseFloat(e.target.value)
                if (Number.isFinite(v)) setLineHeightMultiplier(v)
              }}
              disabled={painter !== 'webgl'}
              className="w-16 px-2 py-1 text-xs bg-[var(--color-bg-surface)] border border-[var(--color-border)] text-[var(--color-text-primary)] outline-none focus:border-[var(--color-accent)] no-drag text-center disabled:opacity-50"
            />
            <button
              type="button"
              onClick={() => setLineHeightMultiplier(LINE_HEIGHT_MULT_DEFAULT)}
              disabled={painter !== 'webgl'}
              className="text-[10px] no-drag cursor-pointer text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] transition-colors disabled:opacity-50 disabled:cursor-not-allowed"
            >
              Reset
            </button>
          </div>
          {painter !== 'webgl' && (
            <div className="text-[10px] text-[var(--color-text-muted)] mt-1">
              Switch Terminal Painter to WebGL to adjust (DOM is unaffected).
            </div>
          )}
        </SettingRow>

        {/* Character tracking — WebGL only. */}
        <SettingRow settingId="terminal.char-tracking" label={
          <span title="WebGL only. Horizontal spacing between characters (tracking). 1.0 = measured monospace width; higher = more open. DOM painter is unchanged.">
            Character spacing (WebGL)
          </span>
        }>
          <div className="flex items-center gap-3 flex-wrap">
            <input
              type="range"
              min={CHAR_TRACKING_MIN}
              max={CHAR_TRACKING_MAX}
              step={0.01}
              value={charTracking}
              onChange={(e) => setCharTracking(parseFloat(e.target.value))}
              className="w-40 no-drag k2so-slider"
              disabled={painter !== 'webgl'}
            />
            <input
              type="number"
              min={CHAR_TRACKING_MIN}
              max={CHAR_TRACKING_MAX}
              step={0.01}
              value={Number(charTracking.toFixed(2))}
              onChange={(e) => {
                const v = parseFloat(e.target.value)
                if (Number.isFinite(v)) setCharTracking(v)
              }}
              disabled={painter !== 'webgl'}
              className="w-16 px-2 py-1 text-xs bg-[var(--color-bg-surface)] border border-[var(--color-border)] text-[var(--color-text-primary)] outline-none focus:border-[var(--color-accent)] no-drag text-center disabled:opacity-50"
            />
            <button
              type="button"
              onClick={() => setCharTracking(CHAR_TRACKING_DEFAULT)}
              disabled={painter !== 'webgl'}
              className="text-[10px] no-drag cursor-pointer text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] transition-colors disabled:opacity-50 disabled:cursor-not-allowed"
            >
              Reset
            </button>
          </div>
          {painter !== 'webgl' && (
            <div className="text-[10px] text-[var(--color-text-muted)] mt-1">
              Switch Terminal Painter to WebGL to adjust (DOM is unaffected).
            </div>
          )}
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
              className={`w-2.5 h-2.5 bg-[var(--color-on-accent)] block transition-transform ${
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
         *  0.39.0: Only one renderer remains — the daemon-hosted
         *  Kessel stack. Any persisted 'alacritty' (Legacy) or
         *  pre-rename 'alacritty-v2' value is coerced to 'kessel' by
         *  the store's setter and persist migration. The dropdown is
         *  kept as a single-option control for discoverability +
         *  future renderer additions.
         */}
        <SettingRow settingId="terminal.renderer" label={
          <span title="Kessel runs on the daemon, survives Tauri quit, and supports heartbeats. Changing this only affects NEW terminals; existing tabs keep their current renderer.">
            Terminal Renderer
          </span>
        }>
          <SettingDropdown
            value={renderer === 'kessel' ? renderer : 'kessel'}
            options={[
              { value: 'kessel', label: 'Kessel' },
            ]}
            onChange={(v) => setRenderer(v as TerminalRenderer)}
          />
        </SettingRow>

        {/* Terminal Painter.
         *
         *  How the v2 grid is drawn: the proven DOM row strip
         *  (default) or the experimental WebGL2 instanced painter.
         *  The WebGL painter falls back to DOM per-pane on context
         *  loss / missing WebGL2 support, so flipping this is safe —
         *  but it is explicitly experimental until it has soaked.
         */}
        <SettingRow settingId="terminal.painter" label={
          <span title="How terminal cells are drawn. WebGL is experimental (GPU-accelerated); DOM is the proven default. Changing this only affects NEW terminals.">
            Terminal Painter
          </span>
        }>
          <SettingDropdown
            value={painter}
            options={[
              { value: 'dom', label: 'DOM' },
              { value: 'webgl', label: 'WebGL' },
            ]}
            onChange={(v) => setPainter(v as TerminalPainterKind)}
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
              className={`w-2.5 h-2.5 bg-[var(--color-on-accent)] block transition-transform ${
                openLinksInSplitPane ? 'translate-x-3.5' : 'translate-x-0.5'
              }`}
            />
          </button>
        </SettingRow>

      </div>
    </div>
  )
}
