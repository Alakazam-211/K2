// Settings → K2 Server → Local Grok Bots (0.45.7).
//
// A per-server integration, so it lives with the other server-owned
// switches (Vault, Data Sources): the daemon stores it, applies it for
// every client and headless, and only the owner or an admin can change it.
// Both switches are OFF by default:
//   - Show: detect Grok Bot on that server's computer, list its agents in
//     Agents (tagged "Grok Bot") and read their chats.
//   - Send: let people send messages to them from K2. Each message runs a
//     billed Grok turn and speaks as the person who sends it.
// Before Show is on, an admin sees what K2 found (the folder and the agent
// names) so nothing is read without knowing what it is.

import React, { useCallback, useEffect, useState } from 'react'
import type { SettingEntry } from '../searchManifest'
import { SettingsGroup } from '../controls/SettingControls'
import { Toggle } from '@/components/ui/Toggle'
import { primaryScope } from '@/kessel/server-scope'
import {
  detectGrokBots,
  fetchGrokSettings,
  saveGrokSettings,
  type GrokDetection,
  type GrokSettings,
} from '@/lib/grokbots'
import { useGrokBotsStore } from '@/stores/grokbots'

export const LOCAL_GROKBOTS_MANIFEST: SettingEntry[] = [
  {
    id: 'grokbots.show',
    section: 'local-grokbots',
    label: 'Show Grok Bots',
    description: 'Detect Grok Bot agents on this server and show their chats in Agents and Home',
    keywords: ['grok', 'grok bot', 'grokbot', 'sand', 'agents', 'external', 'detect', 'local'],
    group: 'Local Grok Bots',
  },
  {
    id: 'grokbots.send',
    section: 'local-grokbots',
    label: 'Send to Grok Bots',
    description: 'Let people send messages to Grok Bot agents from K2 (billed Grok turns)',
    keywords: ['grok', 'grok bot', 'grokbot', 'send', 'message', 'billed'],
    group: 'Local Grok Bots',
  },
]

function errorText(err: unknown): string {
  return err instanceof Error ? err.message : String(err)
}

function Row({
  settingId,
  title,
  hint,
  children,
}: {
  settingId: string
  title: string
  hint: React.ReactNode
  children: React.ReactNode
}): React.JSX.Element {
  return (
    <div
      className="flex items-center justify-between gap-4 py-2 border-b border-[var(--color-border)]"
      data-settings-id={settingId}
    >
      <div className="min-w-0">
        <div className="text-xs text-[var(--color-text-primary)]">{title}</div>
        <div className="text-[10px] text-[var(--color-text-muted)]">{hint}</div>
      </div>
      <div className="flex-shrink-0">{children}</div>
    </div>
  )
}

export function LocalGrokBotsSection(): React.JSX.Element {
  const [settings, setSettings] = useState<GrokSettings | null>(null)
  const [found, setFound] = useState<GrokDetection | null>(null)
  const [loadError, setLoadError] = useState<string | null>(null)
  const [saveError, setSaveError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)

  useEffect(() => {
    let cancelled = false
    const scope = primaryScope()
    fetchGrokSettings(scope).then(
      (s) => {
        if (cancelled) return
        setSettings(s)
        // The preview is an admin read (folder + names, no chat, no token).
        if (s.canEdit) {
          detectGrokBots(scope).then(
            (d) => !cancelled && setFound(d),
            () => {},
          )
        }
      },
      (e: unknown) => !cancelled && setLoadError(errorText(e)),
    )
    return () => {
      cancelled = true
    }
  }, [])

  const save = useCallback(async (change: { show?: boolean; send?: boolean }): Promise<void> => {
    setBusy(true)
    setSaveError(null)
    try {
      const scope = primaryScope()
      setSettings(await saveGrokSettings(scope, change))
      void useGrokBotsStore.getState().refresh(scope)
    } catch (e) {
      setSaveError(errorText(e))
    } finally {
      setBusy(false)
    }
  }, [])

  const canEdit = settings?.canEdit === true
  const locked = !canEdit || busy

  return (
    <div className="max-w-xl" data-testid="local-grokbots-settings">
      <h2 className="text-lg font-semibold text-[var(--color-text-primary)] mb-1">Local Grok Bots</h2>
      <p className="text-xs text-[var(--color-text-muted)] mb-6">
        Grok Bot agents running on this server's computer can show up in Agents and on your Homes, tagged
        "Grok Bot". K2 reads only Grok Bot's agent list and their chats, for the user K2 runs as. Off until you
        turn it on.
      </p>

      <div className="space-y-6">
        <SettingsGroup title="What K2 found">
          {found === null && canEdit && <p className="text-[11px] text-[var(--color-text-muted)] py-2">Looking…</p>}
          {found === null && settings && !canEdit && (
            <p className="text-[11px] text-[var(--color-text-muted)] py-2">Only the owner or an admin can see this.</p>
          )}
          {found && !found.installed && (
            <p data-testid="grokbots-not-found" className="text-[11px] text-[var(--color-text-muted)] py-2">
              No Grok Bot here (looked in <code>{found.dataPath}</code>).
            </p>
          )}
          {found && found.installed && (
            <div data-testid="grokbots-found" className="py-2 text-[11px] text-[var(--color-text-secondary)]">
              <div>
                Grok Bot data in <code>{found.dataPath}</code>
              </div>
              <div className="mt-1">
                {found.agents.length === 0
                  ? 'No agents yet.'
                  : `Agents: ${found.agents.map((a) => a.name).join(', ')}`}
              </div>
            </div>
          )}
        </SettingsGroup>

        <SettingsGroup title="Local Grok Bots">
          {loadError && (
            <p role="alert" className="text-[11px] text-[var(--color-status-error)] py-2">
              Could not load this setting: {loadError}
            </p>
          )}
          {!settings && !loadError && <p className="text-[11px] text-[var(--color-text-muted)] py-2">Loading…</p>}
          {settings && (
            <>
              {!canEdit && (
                <p data-testid="grokbots-settings-readonly" className="text-[10px] text-[var(--color-text-muted)] py-1">
                  Only the owner or an admin can change these.
                </p>
              )}
              <Row
                settingId="grokbots.show"
                title="Show"
                hint="Detect Grok Bot, list its agents and read their chats. Agents and App guests never see them."
              >
                <Toggle
                  aria-label="Show Grok Bots"
                  checked={settings.show}
                  disabled={locked}
                  onChange={(v) => void save(v ? { show: true } : { show: false, send: false })}
                />
              </Row>
              <Row
                settingId="grokbots.send"
                title="Send"
                hint="Let the owner and admins message Grok Bot agents from K2. Each message runs a billed Grok turn and is sent as the person typing."
              >
                <Toggle
                  aria-label="Send to Grok Bots"
                  checked={settings.send && settings.show}
                  disabled={locked || !settings.show}
                  onChange={(v) => void save({ send: v })}
                />
              </Row>
            </>
          )}
          {saveError && (
            <p role="alert" className="text-[11px] text-[var(--color-status-error)] py-2">
              Could not save: {saveError}
            </p>
          )}
        </SettingsGroup>
      </div>
    </div>
  )
}
