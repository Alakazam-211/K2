import React from 'react'
import { useCallback, useEffect, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { useSettingsStore } from '@/stores/settings'
import { SettingDropdown } from '../controls/SettingControls'
import type { SettingEntry } from '../searchManifest'

export const EDITORS_MANIFEST: SettingEntry[] = [
  {
    id: 'editors.default-editor',
    section: 'editors',
    group: 'Defaults',
    label: 'Default Editor',
    description: 'Opens files and projects with this editor',
    keywords: ['editor', 'default', 'cursor', 'vscode', 'zed'],
  },
  {
    id: 'editors.default-terminal',
    section: 'editors',
    group: 'Defaults',
    label: 'Default Terminal',
    description: 'Right-click a tab to open in this terminal',
    keywords: ['terminal', 'default', 'iterm', 'warp', 'ghostty'],
  },
  {
    id: 'editors.detected-editors',
    section: 'editors',
    label: 'Detected Editors',
    description: 'Editors discovered on your system',
    keywords: ['detected', 'editors', 'scan', 'refresh'],
  },
  {
    id: 'editors.terminal-apps',
    section: 'editors',
    label: 'Terminal Apps',
    description: 'Terminal emulators detected on your system',
    keywords: ['terminal', 'apps', 'detected'],
  },
]

interface EditorDetected {
  id: string
  label: string
  macApp: string
  cliCommand: string
  installed: boolean
  type: 'editor' | 'terminal'
}

export function EditorsSection(): React.JSX.Element {
  const projectSettings = useSettingsStore((s) => s.projectSettings)
  const updateProjectSetting = useSettingsStore((s) => s.updateProjectSetting)
  const [editors, setEditors] = useState<EditorDetected[]>([])
  const [editorsLoading, setEditorsLoading] = useState(false)

  const loadEditors = useCallback(async () => {
    setEditorsLoading(true)
    try {
      const result = await invoke<EditorDetected[]>('projects_get_all_editors')
      setEditors(result)
    } catch (err) {
      console.error('Failed to load editors:', err)
    } finally {
      setEditorsLoading(false)
    }
  }, [])

  const refreshEditors = useCallback(async () => {
    setEditorsLoading(true)
    try {
      const result = await invoke<EditorDetected[]>('projects_refresh_editors')
      setEditors(result)
    } catch (err) {
      console.error('Failed to refresh editors:', err)
    } finally {
      setEditorsLoading(false)
    }
  }, [])

  useEffect(() => {
    void loadEditors()
  }, [loadEditors])

  const editorApps = editors.filter((e) => e.type === 'editor')
  const terminalApps = editors.filter((e) => e.type === 'terminal')

  return (
    <div className="max-w-2xl space-y-8">
      <div>
        <h2 className="text-sm font-medium text-[var(--color-text-primary)] mb-3">Defaults</h2>
        <div className="border border-[var(--color-border)]">
          <div
            className="flex items-center justify-between px-3 py-2.5 border-b border-[var(--color-border)]"
            data-settings-id="editors.default-editor"
          >
            <div>
              <div className="text-xs text-[var(--color-text-secondary)]">Default Editor</div>
              <div className="text-[10px] text-[var(--color-text-muted)] mt-0.5">
                Opens files and projects with this editor
              </div>
            </div>
            <SettingDropdown
              value={
                (projectSettings['__global__'] as { defaultEditor?: string } | undefined)
                  ?.defaultEditor ??
                editorApps.find((e) => e.installed)?.label ??
                'Cursor'
              }
              options={editorApps
                .filter((e) => e.installed)
                .map((ed) => ({ value: ed.label, label: ed.label }))}
              onChange={(v) => updateProjectSetting('__global__', 'defaultEditor', v)}
            />
          </div>
          <div
            className="flex items-center justify-between px-3 py-2.5"
            data-settings-id="editors.default-terminal"
          >
            <div>
              <div className="text-xs text-[var(--color-text-secondary)]">Default Terminal</div>
              <div className="text-[10px] text-[var(--color-text-muted)] mt-0.5">
                Right-click a tab to open in this terminal
              </div>
            </div>
            <SettingDropdown
              value={
                (projectSettings['__global__'] as { defaultTerminal?: string } | undefined)
                  ?.defaultTerminal ?? 'Terminal'
              }
              options={[
                { value: 'Terminal', label: 'Terminal' },
                ...terminalApps
                  .filter((e) => e.installed)
                  .map((ed) => ({ value: ed.label, label: ed.label })),
              ]}
              onChange={(v) => updateProjectSetting('__global__', 'defaultTerminal', v)}
            />
          </div>
        </div>
      </div>

      <div data-settings-id="editors.detected-editors">
        <div className="flex items-center justify-between mb-3">
          <h2 className="text-sm font-medium text-[var(--color-text-primary)]">Detected Editors</h2>
          <button
            type="button"
            onClick={() => void refreshEditors()}
            disabled={editorsLoading}
            className="px-3 py-1 text-xs text-[var(--color-text-muted)] border border-[var(--color-border)] hover:text-[var(--color-text-primary)] hover:border-[var(--color-text-muted)] no-drag cursor-pointer disabled:opacity-40 disabled:cursor-default font-mono"
          >
            {editorsLoading ? 'Scanning...' : 'Refresh'}
          </button>
        </div>

        <div className="border border-[var(--color-border)]">
          {editorApps.map((editor, i) => (
            <div
              key={editor.id}
              className={`flex items-center gap-3 px-3 py-1.5 ${
                i < editorApps.length - 1 ? 'border-b border-[var(--color-border)]' : ''
              }`}
            >
              <span
                className={`w-1.5 h-1.5 flex-shrink-0 ${
                  editor.installed
                    ? 'bg-[var(--color-status-ok)]'
                    : 'bg-[color-mix(in_srgb,var(--color-status-error)_60%,transparent)]'
                }`}
              />
              <span className="text-xs text-[var(--color-text-primary)] font-mono flex-1">
                {editor.label}
              </span>
              <span className="text-[10px] text-[var(--color-text-muted)] font-mono">
                {editor.installed ? editor.cliCommand || editor.macApp : 'not found'}
              </span>
            </div>
          ))}
          {editorApps.length === 0 && !editorsLoading && (
            <div className="px-3 py-2 text-[10px] text-[var(--color-text-muted)]">
              No editors detected yet. Click Refresh to scan.
            </div>
          )}
        </div>

        {terminalApps.length > 0 && (
          <div className="mt-3" data-settings-id="editors.terminal-apps">
            <div className="text-[10px] font-semibold text-[var(--color-text-muted)] uppercase tracking-wider mb-1 px-1">
              Terminal Apps
            </div>
            <div className="border border-[var(--color-border)]">
              {terminalApps.map((app, i) => (
                <div
                  key={app.id}
                  className={`flex items-center gap-3 px-3 py-1.5 ${
                    i < terminalApps.length - 1 ? 'border-b border-[var(--color-border)]' : ''
                  }`}
                >
                  <span
                    className={`w-1.5 h-1.5 flex-shrink-0 ${
                      app.installed
                        ? 'bg-[var(--color-status-ok)]'
                        : 'bg-[color-mix(in_srgb,var(--color-status-error)_60%,transparent)]'
                    }`}
                  />
                  <span className="text-xs text-[var(--color-text-primary)] font-mono flex-1">
                    {app.label}
                  </span>
                  <span className="text-[10px] text-[var(--color-text-muted)] font-mono">
                    {app.installed ? 'installed' : 'not found'}
                  </span>
                </div>
              ))}
            </div>
          </div>
        )}
      </div>
    </div>
  )
}
