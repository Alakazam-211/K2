import React from 'react'
import { useCallback, useEffect, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { useSettingsStore } from '@/stores/settings'
import { Button } from '@/components/ui'
import {
  SaveError,
  SaveNote,
  SETTING_ROW_HINT,
  SettingDropdown,
  SettingRow,
  SettingsGroup,
  SettingsLoadState,
  useSaveStatus,
} from '../controls/SettingControls'
import type { SettingEntry } from '../searchManifest'
import { StatusDot } from './ConnectionsParts'

export const EDITORS_MANIFEST: SettingEntry[] = [
  {
    id: 'editors.default-editor',
    section: 'editors',
    group: 'Open with',
    label: 'Editor',
    description: 'Listed first when you open a workspace in an editor',
    keywords: ['editor', 'default', 'cursor', 'vscode', 'zed', 'open in'],
  },
  {
    id: 'editors.default-terminal',
    section: 'editors',
    group: 'Open with',
    label: 'Terminal app',
    description: 'Opens a tab’s folder from the tab’s right-click menu',
    keywords: ['terminal', 'default', 'iterm', 'warp', 'ghostty', 'open in'],
  },
  {
    id: 'editors.detected-editors',
    section: 'editors',
    label: 'Apps on this Mac',
    description: 'The editors and terminal apps K2 found, with Rescan',
    keywords: ['detected', 'editors', 'terminal apps', 'scan', 'rescan', 'refresh', 'installed'],
  },
]

export interface EditorDetected {
  id: string
  label: string
  macApp: string
  cliCommand: string
  installed: boolean
  type: 'editor' | 'terminal'
}

/** macOS Terminal: always there, so always offered. */
const MAC_TERMINAL = 'Terminal'

/** The server's list, or null when the reply isn't one (an old server, a
 *  web client with no editor scan): the page then says it couldn't check. */
export function parseDetectedApps(raw: unknown): EditorDetected[] | null {
  if (!Array.isArray(raw)) return null
  return raw.filter(
    (e): e is EditorDetected =>
      !!e && typeof e === 'object' && typeof (e as EditorDetected).label === 'string' && typeof (e as EditorDetected).id === 'string',
  )
}

function reasonOf(err: unknown): string {
  if (err instanceof Error) return err.message
  if (typeof err === 'string') return err
  return ''
}

type Load =
  | { kind: 'loading' }
  | { kind: 'ready'; apps: EditorDetected[] }
  | { kind: 'error'; detail: string }

const plural = (n: number, one: string, many: string): string => `${n} ${n === 1 ? one : many}`

export function EditorsSection(): React.JSX.Element {
  const global = useSettingsStore((s) => s.projectSettings['__global__'] as { defaultEditor?: string; defaultTerminal?: string } | undefined)
  const updateProjectSetting = useSettingsStore((s) => s.updateProjectSetting)
  const [load, setLoad] = useState<Load>({ kind: 'loading' })
  const [scanning, setScanning] = useState(false)
  /** What the last Rescan did, said under the card title. */
  const [scanNote, setScanNote] = useState<{ ok: boolean; text: string } | null>(null)
  const editorSave = useSaveStatus()
  const terminalSave = useSaveStatus()

  const loadApps = useCallback(async () => {
    setLoad({ kind: 'loading' })
    setScanNote(null)
    try {
      const apps = parseDetectedApps(await invoke<unknown>('projects_get_all_editors'))
      setLoad(apps ? { kind: 'ready', apps } : { kind: 'error', detail: '' })
    } catch (err) {
      setLoad({ kind: 'error', detail: reasonOf(err) })
    }
  }, [])

  const rescan = useCallback(async () => {
    setScanning(true)
    setScanNote(null)
    try {
      const apps = parseDetectedApps(await invoke<unknown>('projects_refresh_editors'))
      if (!apps) throw new Error('')
      setLoad({ kind: 'ready', apps })
      const editors = apps.filter((a) => a.installed && a.type === 'editor').length
      const terminals = apps.filter((a) => a.installed && a.type === 'terminal').length
      setScanNote({
        ok: true,
        text: `Rescanned just now: ${plural(editors, 'editor', 'editors')} and ${plural(terminals, 'terminal app', 'terminal apps')} installed.`,
      })
    } catch (err) {
      const reason = reasonOf(err).trim().replace(/[.\s]+$/, '')
      setScanNote({
        ok: false,
        text: `Couldn’t rescan${reason ? `: ${reason}` : ''}. The list shows the last scan; try again.`,
      })
    } finally {
      setScanning(false)
    }
  }, [])

  useEffect(() => {
    void loadApps()
  }, [loadApps])

  const apps = load.kind === 'ready' ? load.apps : []
  const installedEditors = apps.filter((e) => e.type === 'editor' && e.installed)
  const installedTerminals = apps.filter((e) => e.type === 'terminal' && e.installed)

  const savedEditor = global?.defaultEditor
  const editorValue = savedEditor ?? installedEditors[0]?.label ?? ''
  const editorOptions = installedEditors.map((ed) => ({ value: ed.label, label: ed.label }))
  if (editorValue && !editorOptions.some((o) => o.value === editorValue)) {
    editorOptions.push({ value: editorValue, label: load.kind === 'ready' ? `${editorValue} (not installed)` : editorValue })
  }
  const terminalValue = global?.defaultTerminal ?? MAC_TERMINAL
  const terminalOptions = [
    { value: MAC_TERMINAL, label: MAC_TERMINAL },
    ...installedTerminals.map((t) => ({ value: t.label, label: t.label })),
  ]
  if (!terminalOptions.some((o) => o.value === terminalValue)) {
    terminalOptions.push({ value: terminalValue, label: load.kind === 'ready' ? `${terminalValue} (not installed)` : terminalValue })
  }

  const save = async (key: 'defaultEditor' | 'defaultTerminal', value: string, status: ReturnType<typeof useSaveStatus>): Promise<void> => {
    status.reset()
    await Promise.resolve(updateProjectSetting('__global__', key, value))
    const now = useSettingsStore.getState().projectSettings['__global__'] as Record<string, string> | undefined
    if (now?.[key] === value) status.saved()
    else status.failed('')
  }

  // Installed first, editors before terminal apps, in the server's order.
  const listed = [...apps].sort((a, b) => Number(b.installed) - Number(a.installed) || (a.type === b.type ? 0 : a.type === 'editor' ? -1 : 1))
  const anyInstalled = installedEditors.length + installedTerminals.length > 0

  const rescanButton = (
    <Button variant="quiet" onClick={() => void rescan()} disabled={scanning || load.kind === 'loading'} aria-busy={scanning || undefined}>
      {scanning ? 'Scanning…' : 'Rescan'}
    </Button>
  )

  const appsState =
    load.kind === 'loading' ? (
      <SettingsLoadState kind="loading" title="Looking for editors and terminal apps…" testId="editors-loading">
        K2 checks which apps are installed on this Mac.
      </SettingsLoadState>
    ) : load.kind === 'error' ? (
      <SettingsLoadState
        kind="error"
        title="K2 couldn’t check which apps are installed"
        detail={load.detail}
        action={<Button onClick={() => void loadApps()}>Try again</Button>}
        testId="editors-error"
      >
        Your choices above are kept. Try again in a moment.
      </SettingsLoadState>
    ) : !anyInstalled ? (
      <SettingsLoadState kind="empty" title="No editors or terminal apps found" action={rescanButton} testId="editors-empty">
        Install one, such as VS Code, Cursor or iTerm2, then rescan. macOS Terminal is always available.
      </SettingsLoadState>
    ) : undefined

  return (
    <div className="w-full max-w-[52rem] space-y-6 pb-6" data-testid="editors-page">
      <header className="min-w-0">
        <h2 className="type-title text-[var(--color-text-primary)]">Defaults</h2>
        <p className={SETTING_ROW_HINT}>The apps K2 opens for you outside K2: your code editor and your terminal app.</p>
      </header>

      <SettingsGroup card title="Open with" testId="editors-defaults">
        <SettingRow
          settingId="editors.default-editor"
          label="Editor"
          hint={
            <>
              Listed first under Open in when you right-click a workspace. <SaveNote state={editorSave.state} />
              <SaveError state={editorSave.state} />
            </>
          }
          field
        >
          <SettingDropdown
            fullWidth
            ariaLabel="Editor"
            value={editorValue}
            placeholder={load.kind === 'loading' ? 'Looking…' : load.kind === 'error' ? 'Couldn’t check' : 'No editor found'}
            options={editorOptions}
            disabled={editorOptions.length === 0}
            onChange={(v) => save('defaultEditor', v, editorSave)}
          />
        </SettingRow>
        <SettingRow
          settingId="editors.default-terminal"
          label="Terminal app"
          hint={
            <>
              Opens a tab’s folder when you right-click the tab and pick Open in. <SaveNote state={terminalSave.state} />
              <SaveError state={terminalSave.state} />
            </>
          }
          field
        >
          <SettingDropdown
            fullWidth
            ariaLabel="Terminal app"
            value={terminalValue}
            options={terminalOptions}
            onChange={(v) => save('defaultTerminal', v, terminalSave)}
          />
        </SettingRow>
      </SettingsGroup>

      <SettingsGroup
        card
        title="Apps on this Mac"
        hint={
          scanNote ? (
            <span
              role={scanNote.ok ? 'status' : 'alert'}
              data-testid="editors-scan-note"
              className={scanNote.ok ? undefined : 'text-[var(--color-status-error-soft)]'}
            >
              {scanNote.text}
            </span>
          ) : appsState ? (
            'The editors and terminal apps K2 finds on this Mac.'
          ) : (
            'The editors and terminal apps K2 found. Installed one just now? Rescan to add it.'
          )
        }
        action={appsState ? undefined : rescanButton}
        state={appsState}
        settingId="editors.detected-editors"
        testId="editors-apps"
      >
        {listed.map((app) => (
          <div key={app.id} className="px-4 py-3 flex items-center justify-between gap-6" data-testid={`editor-app-${app.id}`}>
            <div className="min-w-0">
              <div className="type-row text-[var(--color-text-primary)]">{app.label}</div>
              <div className={SETTING_ROW_HINT}>{app.type === 'editor' ? 'Code editor' : 'Terminal app'}</div>
            </div>
            <span className="inline-flex items-center gap-2 type-hint text-[var(--color-text-secondary)] flex-shrink-0">
              <StatusDot tone={app.installed ? 'ok' : 'neutral'} />
              {app.installed ? 'Installed' : 'Not installed'}
            </span>
          </div>
        ))}
      </SettingsGroup>
    </div>
  )
}
