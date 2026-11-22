// Settings → 3rd Party Bots → Grok Bot (0.45.7).
//
// A per-server integration (under K2 Server before 0.45.7; now the first
// leaf of the 3rd Party Bots group): the daemon stores it, applies it for
// every client and headless, and only the owner or an admin can change it.
// Both switches are OFF by default:
//   - Show: detect Grok Bot on that server's computer, list its agents in
//     Agents (tagged "Grok Bot") and read their chats.
//   - Send: let people send messages to them from K2. Each message runs a
//     billed Grok turn and speaks as the person who sends it. Turning it on
//     asks first.
// Before Show is on, an admin sees what K2 found (the folder and the agent
// names) so nothing is read without knowing what it is. A Member sees the
// switches as plain On / Off, with one line saying who can change them.

import React, { useCallback, useEffect, useState } from 'react'
import type { SettingEntry } from '../searchManifest'
import {
  SETTING_ROW_HINT,
  SETTING_ROW_LABEL,
  SettingSwitchRow,
  SettingsGroup,
  SettingsLoadState,
} from '../controls/SettingControls'
import { DENIED_LINE, RowSaveError, RowSaveNote, useRowSave } from './row-save'
import { Button, Tag, Toggle } from '@/components/ui'
import { primaryScope } from '@/kessel/server-scope'
import { daemonErrorText } from '@/lib/daemon-error'
import { useConfirmDialogStore } from '@/stores/confirm-dialog'
import {
  detectGrokBots,
  fetchGrokSettings,
  saveGrokSettings,
  type GrokDetection,
  type GrokSettings,
} from '@/lib/grokbots'
import { useGrokBotsStore } from '@/stores/grokbots'
import { StatusDot, type HealthTone } from './ConnectionsParts'
import { PRODUCT_NAME } from '@/lib/brand'

export const LOCAL_GROKBOTS_MANIFEST: SettingEntry[] = [
  {
    id: 'grokbots.show',
    section: 'local-grokbots',
    label: 'Show Grok Bots',
    description: 'Detect Grok Bot agents on this server and show their chats in Agents and Home',
    keywords: ['grok', 'grok bot', 'grokbot', 'sand', 'agents', 'external', 'detect', 'local'],
    group: `What ${PRODUCT_NAME} may do`,
  },
  {
    id: 'grokbots.send',
    section: 'local-grokbots',
    label: 'Send to Grok Bots',
    description: `Let people send messages to Grok Bot agents from ${PRODUCT_NAME} (billed Grok turns)`,
    keywords: ['grok', 'grok bot', 'grokbot', 'send', 'message', 'billed'],
    group: `What ${PRODUCT_NAME} may do`,
  },
]

export const GROK_READ_ONLY_LINE = DENIED_LINE
const SHOW_LABEL = 'Show Grok Bot agents'
const SEND_LABEL = 'Let people message them'

const PAGE = 'w-full max-w-[52rem] space-y-6 pb-6'

/** "Nova", "Nova and Ledger", "Nova, Ledger and Atlas". */
function andList(names: string[]): string {
  if (names.length <= 1) return names.join('')
  return `${names.slice(0, -1).join(', ')} and ${names[names.length - 1]}`
}

function Header({ readOnly }: { readOnly: boolean }): React.JSX.Element {
  return (
    <header className="min-w-0">
      <h2 className="type-title text-[var(--color-text-primary)] flex items-center gap-2">
        Grok Bot
        {readOnly && <Tag tone="muted">View only</Tag>}
      </h2>
      <p className={`${SETTING_ROW_HINT} mt-1`}>
        Show Grok Bot agents from this server’s computer in Agents and on Home, tagged Grok Bot. Off until you turn
        it on.
      </p>
      {readOnly && (
        <p className="type-hint text-[var(--color-text-primary)] mt-1" data-testid="grokbots-settings-readonly">
          {GROK_READ_ONLY_LINE}
        </p>
      )}
    </header>
  )
}

/** A status row: a dot, a short title, one line and an optional action. */
function StatusRow({
  tone,
  title,
  children,
  action,
  testId,
}: {
  tone: HealthTone
  title: string
  children: React.ReactNode
  action?: React.ReactNode
  testId?: string
}): React.JSX.Element {
  return (
    <div className="px-4 py-3" data-testid={testId} data-tone={tone}>
      <div className="flex items-start justify-between gap-6">
        <div className="min-w-0">
          <div className={`${SETTING_ROW_LABEL} flex items-center gap-2`}>
            <StatusDot tone={tone} />
            <span>{title}</span>
          </div>
          <div className={SETTING_ROW_HINT}>{children}</div>
        </div>
        {action && <div className="flex items-center gap-2 flex-shrink-0">{action}</div>}
      </div>
    </div>
  )
}

/** A switch a Member can see but not change: plain On / Off on its edge. */
function ReadOnlySwitch({ on, label }: { on: boolean; label: string }): React.JSX.Element {
  return (
    <span aria-label={label} data-readonly-value="" className="type-row text-[var(--color-text-primary)]">
      {on ? 'On' : 'Off'}
    </span>
  )
}

const PATH = 'font-mono break-words text-[var(--color-text-primary)]'

export function LocalGrokBotsSection(): React.JSX.Element {
  const [settings, setSettings] = useState<GrokSettings | null>(null)
  const [loadError, setLoadError] = useState<string | null>(null)
  const [found, setFound] = useState<GrokDetection | null>(null)
  const [findError, setFindError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const showSave = useRowSave()
  const sendSave = useRowSave()

  // The preview is an admin read (folder + names, no chat, no token).
  const look = useCallback((): void => {
    setFound(null)
    setFindError(null)
    detectGrokBots(primaryScope()).then(setFound, (e: unknown) => setFindError(daemonErrorText(e)))
  }, [])

  const load = useCallback((): void => {
    setSettings(null)
    setLoadError(null)
    fetchGrokSettings(primaryScope()).then(
      (s) => {
        setSettings(s)
        if (s.canEdit) look()
      },
      (e: unknown) => setLoadError(daemonErrorText(e)),
    )
  }, [look])

  useEffect(() => load(), [load])

  const save = useCallback(
    async (change: { show?: boolean; send?: boolean }, status: ReturnType<typeof useRowSave>): Promise<void> => {
      setBusy(true)
      status.saving()
      try {
        const scope = primaryScope()
        setSettings(await saveGrokSettings(scope, change))
        status.saved()
        void useGrokBotsStore.getState().refresh(scope)
      } catch (e) {
        status.failed(e)
      } finally {
        setBusy(false)
      }
    },
    [],
  )

  const flipSend = async (next: boolean): Promise<void> => {
    if (next) {
      const ok = await useConfirmDialogStore.getState().confirm({
        title: 'Let people message Grok Bot agents?',
        message:
          `The owner and admins can then send messages to Grok Bot agents from ${PRODUCT_NAME}. Each message runs a billed Grok turn and is sent as the person typing. You can turn this off at any time.`,
        confirmLabel: 'Turn on sending',
      })
      if (!ok) return
    }
    await save({ send: next }, sendSave)
  }

  if (!settings) {
    return (
      <div className={PAGE} data-testid="local-grokbots-settings">
        <Header readOnly={false} />
        {loadError ? (
          <SettingsLoadState
            kind="error"
            title="Couldn’t load the Grok Bot settings"
            detail={loadError}
            action={<Button onClick={load}>Try again</Button>}
            testId="grokbots-load-error"
          >
            Nothing changed on this server; this page just couldn’t read it.
          </SettingsLoadState>
        ) : (
          <SettingsLoadState kind="loading" title="Loading the Grok Bot settings…" testId="grokbots-loading" />
        )}
      </div>
    )
  }

  const canEdit = settings.canEdit
  const locked = !canEdit || busy
  // Show can't be turned on until K2 has found Grok Bot here (it can always
  // be turned off).
  const showWaits =
    !canEdit || settings.show || found?.installed
      ? null
      : findError
        ? `You can turn this on once ${PRODUCT_NAME} can check this computer for Grok Bot.`
        : found
          ? 'You can turn this on once Grok Bot is installed on this computer.'
          : `You can turn this on once ${PRODUCT_NAME} has finished looking for Grok Bot.`

  const foundState = !canEdit ? undefined : findError ? (
    <SettingsLoadState
      kind="error"
      title="Couldn’t check for Grok Bot"
      detail={findError}
      action={<Button onClick={look}>Try again</Button>}
      testId="grokbots-find-error"
    >
      Nothing was read or changed; {PRODUCT_NAME} just couldn’t look on this computer.
    </SettingsLoadState>
  ) : found === null ? (
    <SettingsLoadState kind="loading" title="Looking for Grok Bot on this computer…" testId="grokbots-looking" />
  ) : undefined

  return (
    <div className={PAGE} data-testid="local-grokbots-settings">
      <Header readOnly={!canEdit} />

      <SettingsGroup title={`What ${PRODUCT_NAME} found`} card state={foundState} testId="grokbots-found-group">
        {!canEdit ? (
          <StatusRow
            tone={settings.show ? 'ok' : 'neutral'}
            title={settings.show ? 'Grok Bot agents show on this server' : 'Grok Bot is off on this server'}
            testId="grokbots-member-status"
          >
            {settings.show
              ? 'Its agents and their chats appear in Agents and on Home, tagged Grok Bot.'
              : `Its agents and chats stay out of ${PRODUCT_NAME}.`}
          </StatusRow>
        ) : found && found.installed ? (
          <StatusRow tone="ok" title="Grok Bot is on this computer" testId="grokbots-found">
            {found.agents.length === 0
              ? 'No agents yet.'
              : `${found.agents.length === 1 ? '1 agent' : `${found.agents.length} agents`}: ${andList(found.agents.map((a) => a.name))}.`}
            <span className="block">
              Its data is in <code className={PATH}>{found.dataPath}</code>.
            </span>
          </StatusRow>
        ) : found ? (
          <StatusRow
            tone="neutral"
            title="Grok Bot isn’t on this computer"
            testId="grokbots-not-found"
            action={<Button onClick={look}>Check again</Button>}
          >
            {PRODUCT_NAME} looked in <code className={PATH}>{found.dataPath}</code> and found nothing. Install Grok Bot for the user {PRODUCT_NAME}
            runs as, then check again.
          </StatusRow>
        ) : null}
      </SettingsGroup>

      <SettingsGroup title={`What ${PRODUCT_NAME} may do`} card testId="grokbots-switches">
        <SettingSwitchRow
          settingId="grokbots.show"
          control={
            canEdit ? (
              <>
                <RowSaveNote state={showSave.state} />
                <Toggle
                  aria-label={SHOW_LABEL}
                  checked={settings.show}
                  disabled={locked || showWaits !== null}
                  onChange={(v) => void save(v ? { show: true } : { show: false, send: false }, showSave)}
                />
              </>
            ) : (
              <ReadOnlySwitch on={settings.show} label={SHOW_LABEL} />
            )
          }
        >
          <div className={SETTING_ROW_LABEL}>{SHOW_LABEL}</div>
          <div className={SETTING_ROW_HINT}>
            List its agents in Agents and on Home and read their chats, for the user {PRODUCT_NAME} runs as. Agents and App guests
            never see them.
          </div>
          {showWaits && (
            <div className={SETTING_ROW_HINT} data-testid="grokbots-show-waits">
              {showWaits}
            </div>
          )}
          <RowSaveError state={showSave.state} testId="grokbots-show-error" />
        </SettingSwitchRow>
        <SettingSwitchRow
          settingId="grokbots.send"
          control={
            canEdit ? (
              <>
                <RowSaveNote state={sendSave.state} />
                <Toggle
                  aria-label={SEND_LABEL}
                  checked={settings.send && settings.show}
                  disabled={locked || !settings.show}
                  onChange={(v) => void flipSend(v)}
                />
              </>
            ) : (
              <ReadOnlySwitch on={settings.send && settings.show} label={SEND_LABEL} />
            )
          }
        >
          <div className={SETTING_ROW_LABEL}>{SEND_LABEL}</div>
          <div className={SETTING_ROW_HINT}>
            The owner and admins can message Grok Bot agents from {PRODUCT_NAME}. Each message runs a billed Grok turn and is sent as the
            person typing.
            {canEdit && !settings.show && (
              <span className="block mt-1" data-testid="grokbots-send-needs-show">
                Turn on {SHOW_LABEL} first.
              </span>
            )}
          </div>
          <RowSaveError state={sendSave.state} testId="grokbots-send-error" />
        </SettingSwitchRow>
      </SettingsGroup>
    </div>
  )
}
