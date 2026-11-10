import React from 'react'
import { useCallback, useEffect, useRef, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import { useConnectHostStore } from '@/stores/connect-host'
import { useServerSupports, featureMinVersion } from '@/lib/server-capabilities'
import { useConfirmDialogStore } from '@/stores/confirm-dialog'
import { useToastStore } from '@/stores/toast'
import { Button, Callout, Toggle, Tag } from '@/components/ui'
import { restartHostConfirmCopy } from './restart-host'
import { hostActionGate, refusalCopy } from './host-role'
import { useActiveHostRole } from './useActiveHostRole'
import {
  updateHostConfirmCopy,
  updateAvailableCopy,
  hostVersionCopy,
  newerNoArtifactCopy,
  newerViaPackageCopy,
  updatePhaseCopy,
  updateSlowComebackCopy,
  updateReconnectingCopy,
  updateSucceededCopy,
  updateFailedCopy,
  updateUnconfirmedCopy,
  isAmbiguousApplyError,
  isJobStateRefusal,
  settleApplyRefusal,
  pollErrorMeansRestart,
  watchUpdateComeback,
  isForbiddenError,
  isStaged,
  isTerminalPhase,
  isFailurePhase,
  readUpdateCheck,
  readUpdateStatus,
  readUpdateStart,
  UpdateReplyError,
  keepUpdateJob,
  keptUpdateJob,
  forgetUpdateJob,
  updateJobGoneCopy,
  updateStatusUnreadableCopy,
  type UpdateCheckResult,
  type UpdateStatusResult,
  type UpdatePhase,
} from './update-host'
// Comeback watcher (0.40.48 remote-update ergonomics): after the update
// job reaches `restarting`, poll the host's /boot-status (host token) until it's
// back on the new version — same helper + pattern the Connections tiles'
// waitForHostReady uses (hostBootStatus's retry loop also evicts the dead
// pooled socket, so observing 'ready' implies a healthy pool).
import { hostBootStatus, remoteCreds } from '@/lib/host-ops'
import { jittered } from '@/lib/backoff'
// Plan B — the keep-daemon-on-quit flag lives in the daemon's settings
// store. The old `get/set_keep_daemon_on_quit` Tauri commands proxied
// `/cli/settings/{get,update}`; route them through the host-aware
// daemon-settings client instead so the toggle works against any daemon.
import { settingsGet, settingsUpdate } from '@/lib/daemon-settings'
import { useSettingsStore } from '@/stores/settings'
import { useUpdateStore } from '@/stores/update'
import { checkForUpdate } from '@/hooks/useUpdateChecker'
import {
  AUTO_UPDATE_CHECKS_EVENT,
  autoUpdateChecksEnabled,
  readLastUpdateCheck,
  setAutoUpdateChecksEnabled,
} from '@/lib/auto-update-checks'
import type { SettingEntry } from '../searchManifest'
import { webFeatures } from '@/web/features'
import type { GeneralSubTab } from '@/stores/settings'
import { StartPageRow } from './StartPageRow'
import { primaryScope } from '@/kessel/server-scope'
import { AppFontRows } from '../shared/AppFontRows'
import { ChatLinkCopyRow } from './ChatLinkCopyRow'
import { ClonePersonalComputersRow } from './ClonePersonalComputersRow'
import { YourAccountRow } from './YourAccountRow'
import { OwnerTimezoneRow } from './OwnerTimezoneRow'
import { GitToolsRow } from '@/components/GitTools/GitToolsNotice'
import {
  SETTING_ROW_HINT,
  SETTING_ROW_LABEL,
  SettingRow,
  SettingSwitchRow,
  SettingsGroup,
} from '../controls/SettingControls'
import { HealthWord, StatusDot, type HealthTone } from './ConnectionsParts'
import { SectionErrorBoundary } from '../SectionErrorBoundary'

export const GENERAL_MANIFEST: SettingEntry[] = [
  { id: 'general.app-version', section: 'general', group: 'General', label: 'K2 app', description: 'The K2 version on this computer: check for, download and install updates', keywords: ['update', 'version', 'check', 'release', 'download', 'install', 'app version'] },
  { id: 'general.auto-update-checks', section: 'general', group: 'General', label: 'Check for updates automatically', description: 'Look for a new version every few hours and say when one is ready. This computer only.', keywords: ['update', 'automatic', 'auto', 'check', 'disable', 'background'] },
  { id: 'general.cli-version', section: 'general', group: 'General', label: 'k2 command', description: 'The k2 command-line tool for terminals and agents: its version, and installing or updating it', keywords: ['k2so', 'cli', 'command', 'terminal', 'install', 'update', 'path', 'cli version'] },
  { id: 'general.git-tools', section: 'general', group: 'General', label: 'Git', description: "Whether git works on this server. On a Mac without Apple's command line tools, install them in one click (no Xcode needed)", keywords: ['git', 'xcode', 'xcode-select', 'command line tools', 'clt', 'developer tools', 'install', 'apple', 'homebrew', 'apt'] },
  { id: 'general.release-notes', section: 'general', group: 'General', label: 'Release notes', description: "What's new in this version and the ones before it", keywords: ['release notes', 'whats new', 'changelog', 'changes', 'new'] },
  { id: 'general.start-page', section: 'general', group: 'General', label: 'Start page', description: 'The page K2 opens on: Home or Agents. This computer only.', keywords: ['start', 'startup', 'launch', 'open', 'default page', 'home', 'agents', 'workspaces', 'landing'] },
  { id: 'general.font-family', section: 'general', group: 'General', label: 'Font family', description: 'The font K2 uses for code and text. Same setting as Editors / Fonts → Code', keywords: ['font', 'typeface', 'family', 'text', 'monospace'] },
  { id: 'general.font-size', section: 'general', group: 'General', label: 'Font size', description: 'Text size for code, the message box, chats and tickets. Same setting as Editors / Fonts → Code', keywords: ['font', 'size', 'text size', 'zoom', 'bigger', 'smaller'] },
  { id: 'general.link-copy-button', section: 'general', group: 'General', label: 'Show a copy button beside links on hover', description: 'A copy icon beside links in chats, Threads and tickets. Off by default; this computer only. Right-click a link to copy it either way', keywords: ['copy', 'link', 'url', 'hover', 'button', 'icon', 'chat', 'thread', 'clipboard'] },
  { id: 'general.devtools', section: 'general', group: 'General', label: 'Developer tools', description: 'Opens the web inspector for this window, for troubleshooting. This computer only.', keywords: ['devtools', 'developer', 'console', 'inspect', 'inspector', 'debug', 'network', 'logs'] },
  { id: 'general.owner-timezone', section: 'general', group: 'General', label: 'Your timezone', description: "What a reminder's \"13:00\" and a mail check window mean on this server. Empty: the server's own clock", keywords: ['timezone', 'time zone', 'zone', 'clock', 'reminder', 'heartbeat', 'schedule', 'mail', 'check window', 'utc'] },
  { id: 'general.your-account', section: 'general', group: 'General', label: 'Your account', description: 'The login on your phone or laptop that counts as you', keywords: ['account', 'login', 'owner', 'you', 'me', 'this is me', 'linked', 'phone', 'laptop', 'connect', 'identity'] },
  { id: 'general.config-location', section: 'general', group: 'General', label: 'Settings file', description: '~/.k2/settings.json, where this server keeps its settings', keywords: ['settings', 'config', 'location', 'path', 'file', 'json'] },
  { id: 'general.reset-all', section: 'general', group: 'General', label: 'Reset all settings', description: "Put this server's settings back to their defaults. Workspaces, agents, chats and files stay. Owner only.", keywords: ['reset', 'defaults', 'factory', 'restore'] },
  { id: 'general.daemon', section: 'general', group: 'Server', label: 'K2 server', description: 'The background service that runs agents, terminals and heartbeats: its status, restart and log', keywords: ['server', 'daemon', 'background', 'launchd', 'persistent', 'lid', 'sleep', 'wake', 'agent', 'log', 'restart'] },
  { id: 'general.keep-daemon-on-quit', section: 'general', group: 'Server', label: 'Keep server running when the window is closed', description: 'On: the red close button hides the window and agents keep running. Off: it quits everything. Cmd+Q always quits.', keywords: ['daemon', 'server', 'agent', 'companion', 'close', 'red button', 'window', 'hide', 'background', 'persistent'] },
  { id: 'general.agents-survive-restarts', section: 'general', group: 'Server', label: 'Agents keep running through updates and restarts', description: 'Agent sessions keep running when K2 updates, restarts or crashes. On by default; macOS and Linux.', keywords: ['agents', 'survive', 'restart', 'update', 'crash', 'keep', 'running', 'ptyd', 'session', 'terminal'] },
  { id: 'general.clone-personal-computers', section: 'general', group: 'Server', label: "Allow cloning agents to people's personal computers", description: "When off, Admins and Members can't clone agents from this server to their own computers. Owner only.", keywords: ['clone', 'copy', 'agent', 'workspace', 'personal', 'computer', 'pull', 'permission', 'migrate'] },
  { id: 'general.restart-host', section: 'general', group: 'Server', label: 'Restart connected server', description: "Restart the server you're connected to, not this computer", keywords: ['restart', 'reboot', 'remote', 'host', 'connect', 'server', 'daemon', 'bounce'] },
  { id: 'general.update-host', section: 'general', group: 'Server', label: 'Update connected server', description: "Update the server you're connected to, not this computer", keywords: ['update', 'upgrade', 'remote', 'host', 'connect', 'server', 'daemon', 'version'] },
]

const GENERAL_TABS: Array<{ id: GeneralSubTab; label: string }> = [
  { id: 'general', label: 'General' },
  { id: 'server', label: 'Server' },
  // The Experimental tab held only "Open other servers' agents in Home";
  // that row graduated to Workspaces in 0.46.3, so the tab is gone.
  // The Workspaces tab moved out in 0.46.5 (Rosson 2026-10-27): its rows and
  // the Canonical Agent Flow guide are Settings → Workspaces / Agents →
  // Workspace settings (WorkspaceSettingsPage.tsx).
]

/** "just now", "5 min ago", "2 hours ago", "3 days ago". */
export function checkedAgo(at: number, now: number = Date.now()): string {
  const mins = Math.floor((now - at) / 60_000)
  if (mins < 1) return 'just now'
  if (mins < 60) return `${mins} min ago`
  const hours = Math.floor(mins / 60)
  if (hours < 24) return `${hours} hour${hours === 1 ? '' : 's'} ago`
  const days = Math.floor(hours / 24)
  return days === 1 ? 'yesterday' : `${days} days ago`
}

/** A thin determinate bar for a download. The track uses the control edge
 *  colour so it reads against a card or a callout. */
function ProgressBar({ value, label }: { value: number; label: string }): React.JSX.Element {
  const pct = Math.max(0, Math.min(100, value))
  return (
    <div
      role="progressbar"
      aria-label={label}
      aria-valuemin={0}
      aria-valuemax={100}
      aria-valuenow={Math.round(pct)}
      className="h-1.5 w-full overflow-hidden rounded-[var(--radius-field)] bg-[var(--color-control-edge)]"
    >
      <div className="h-full bg-[var(--color-accent-fill)] transition-[width] duration-300" style={{ width: `${pct}%` }} />
    </div>
  )
}

/** Primary and secondary lines inside a callout, in the page's text roles. */
function CalloutLines({ title, detail }: { title: React.ReactNode; detail?: React.ReactNode }): React.JSX.Element {
  return (
    <div className="min-w-0">
      <p className="type-row text-[var(--color-text-primary)]">{title}</p>
      {detail && <p className="type-hint text-[var(--color-text-secondary)] mt-0.5 break-words">{detail}</p>}
    </div>
  )
}

// The page follows Settings → Workspaces / Agents: a title with one line
// under it, the tab strip, then card groups of rows at one content width.
// Connected to another server, the Connected server card sits in a second
// column beside the tab's cards, under the same title and tabs.
const CONTENT_WIDTH = 'max-w-[52rem]'

export function GeneralSection(): React.JSX.Element {
  const generalSubTab = useSettingsStore((s) => s.generalSubTab)
  const activeHost = useConnectHostStore((s) => s.activeHost)
  const isRemote = activeHost !== 'local'
  const [tab, setTab] = useState<GeneralSubTab>('general')

  // Deep-link / search: a one-shot sub-tab (e.g. a Server row's result).
  useEffect(() => {
    if (!generalSubTab) return
    setTab(generalSubTab)
    useSettingsStore.setState({ generalSubTab: null })
  }, [generalSubTab])

  const width = isRemote ? 'max-w-[106rem]' : CONTENT_WIDTH

  return (
    <div className="w-full h-full min-h-0 flex flex-col p-6">
      <div className={`flex-shrink-0 w-full space-y-4 ${width}`}>
        <div className="min-w-0">
          <h2 className="type-title text-[var(--color-text-primary)]">General</h2>
          <p className="type-hint text-[var(--color-text-secondary)] mt-0.5">
            Updates and preferences for K2 on this computer, and the server it runs.
          </p>
        </div>

        <div
          role="tablist"
          aria-label="General settings"
          className="flex flex-wrap gap-0.5 border-b border-[var(--color-border)] w-full"
        >
          {GENERAL_TABS.map((t) => {
            const active = tab === t.id
            return (
              <button
                key={t.id}
                type="button"
                role="tab"
                aria-selected={active}
                onClick={() => setTab(t.id)}
                className={`px-3 py-2 type-control transition-colors no-drag cursor-pointer border-b-2 -mb-px outline-none focus-visible:outline-2 focus-visible:outline-solid focus-visible:-outline-offset-2 focus-visible:outline-[var(--color-accent)] ${
                  active
                    ? 'border-[var(--color-accent)] text-[var(--color-text-primary)]'
                    : 'border-transparent text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)]'
                }`}
              >
                {t.label}
              </button>
            )
          })}
        </div>
      </div>

      <div className="flex-1 overflow-y-auto min-h-0 pt-5 pb-8 w-full [scrollbar-gutter:stable] @container">
        <div className={`w-full ${width} flex flex-col gap-6 @4xl:flex-row @4xl:items-start`}>
          <div className={`flex-1 min-w-0 w-full ${CONTENT_WIDTH} space-y-6`}>
            {tab === 'general' && <GeneralTab isRemote={isRemote} />}
            {tab === 'server' && <ServerTab />}
          </div>
          {isRemote && (
            <div className={`flex-1 min-w-0 w-full ${CONTENT_WIDTH}`}>
              <SectionErrorBoundary>
                <GeneralRemoteHostPanel />
              </SectionErrorBoundary>
            </div>
          )}
        </div>
      </div>
    </div>
  )
}

function GeneralTab({ isRemote }: { isRemote: boolean }): React.JSX.Element {
  // An update on offer (or ready) is the page's one primary action; the
  // k2 command's Install steps back to a plain button meanwhile.
  const updateStatus = useUpdateStore((s) => s.status)
  const updateOffered = webFeatures.appUpdater && (updateStatus === 'available' || updateStatus === 'ready')
  return (
    <>
      <SettingsGroup title="K2" card>
        <AppVersionRow isRemote={isRemote} />
        {webFeatures.appUpdater && <AutoUpdateChecksRow />}
        <CLIVersionRow primary={!updateOffered} />
        {/* Git (0.46.4): the server's git, and on a Mac without Apple's
            command line tools the one-click install. Daemon-owned state. */}
        <GitToolsRow primary={!updateOffered} />
        {/* Re-open the "What's new" popup for the current version. Pairs
            with the auto-show on first launch after an update (0.38.7). */}
        <WhatsNewRow />
      </SettingsGroup>

      {/* App font (0.45.7): the same setting as Editors / Fonts → Code,
          below the app and CLI versions (Rosson 2026-10-16). It lives in the
          server's settings (a reset puts it back), so it has its own card
          rather than sitting under "This computer". */}
      <SettingsGroup title="Text" hint="Same setting as Editors / Fonts → Code, saved on the server." card>
        <AppFontRows where="general" />
      </SettingsGroup>

      <SettingsGroup title="This computer" hint="These apply on this computer only." card>
        {/* Hover copy icon beside chat links (Rosson 2026-11-04): next to
            the font card, off by default, this computer only. */}
        <ChatLinkCopyRow />
        <StartPageRow />
        {/* Desktop only: open the web inspector (prod + debug) for
            diagnosing remote freezes / network. Pref is thin-client
            localStorage. */}
        {webFeatures.appDevTools && <DevToolsRow />}
      </SettingsGroup>

      {/* You: your timezone (daemon-owned; what a reminder's "13:00" and a
          mail check window mean), then your Connect account (0.46.0, Rosson
          2026-10-17; it was the "This is me (owner)" checkbox on Admin
          Access). The timezone row is always there, so a Member, or a server
          without owner-linked accounts, still sees a card, never a bare
          heading. */}
      <SettingsGroup title="You" card>
        <OwnerTimezoneRow />
        <YourAccountRow />
      </SettingsGroup>

      <SettingsGroup title="Saved settings" card>
        <ConfigLocationRow />
        <ResetAllRow />
      </SettingsGroup>
    </>
  )
}

function ServerTab(): React.JSX.Element {
  return (
    <>
      <SettingsGroup
        title="K2 server"
        hint="The background service that runs agents, terminals, heartbeats and K2 Companion."
        card
      >
        <DaemonRow />
        <KeepDaemonOnQuitRow />
        <AgentsSurviveRestartsRow />
      </SettingsGroup>
      <SettingsGroup title="Permissions" card>
        <ClonePersonalComputersRow scope={primaryScope()} />
      </SettingsGroup>
    </>
  )
}

// ── K2 app version and updates ─────────────────────────────────────────
// The desktop app's own updater (useUpdateStore). Each state is designed:
// checking is a quiet line; an update on offer or ready is the page's one
// primary button; an error names what failed and offers Try again and the
// release page. Hosted web has no in-app updater (the edge serves it).
function AppVersionRow({ isRemote }: { isRemote: boolean }): React.JSX.Element {
  const [currentVersion, setCurrentVersion] = useState<string>('')
  // Set once this page's own check came back with nothing newer.
  const [upToDate, setUpToDate] = useState(false)
  const updateStatus = useUpdateStore((s) => s.status)
  const updateVersion = useUpdateStore((s) => s.version)
  const updateProgress = useUpdateStore((s) => s.progress)
  const updateError = useUpdateStore((s) => s.error)
  const updatePackageUrl = useUpdateStore((s) => s.archDownloadUrl)
  const linuxCanInstall = useUpdateStore((s) => s.linuxCanInstall)
  const linuxInstalling = useUpdateStore((s) => s.linuxInstalling)
  const linuxManualCommand = useUpdateStore((s) => s.linuxManualCommand)
  const [copied, setCopied] = useState(false)

  useEffect(() => {
    invoke<string>('get_current_version').then(setCurrentVersion).catch((e) => console.warn('[settings]', e))
  }, [])

  const handleCheckUpdate = useCallback(async () => {
    setUpToDate(false)
    await checkForUpdate(true)
    setUpToDate(useUpdateStore.getState().status === 'idle')
  }, [])

  // Auto-check for updates when navigated here from the update toast
  useEffect(() => {
    if (useSettingsStore.getState().pendingUpdateCheck) {
      useSettingsStore.setState({ pendingUpdateCheck: false })
      void handleCheckUpdate()
    }
  }, [handleCheckUpdate])

  const openReleasePage = (): void => {
    const tag = updateVersion ? `v${updateVersion}` : 'latest'
    const url = `https://github.com/Alakazam-211/K2/releases/tag/${tag}`
    invoke('plugin:opener|open_url', { url }).catch(() => {
      window.open(url)
    })
  }

  const copyManualCommand = (): void => {
    if (!linuxManualCommand) return
    navigator.clipboard?.writeText(linuxManualCommand).then(
      () => {
        setCopied(true)
        window.setTimeout(() => setCopied(false), 2000)
      },
      (e: unknown) => useToastStore.getState().addToast(`Couldn't copy: ${String(e)}`, 'error'),
    )
  }

  const version = currentVersion ? `Version ${currentVersion}.` : 'Reading the version…'
  const last = updateStatus === 'idle' && !upToDate ? readLastUpdateCheck() : null
  const status = !webFeatures.appUpdater
    ? 'Updates come with the web page.'
    : updateStatus === 'idle'
      ? upToDate
        ? 'Up to date.'
        : last
          ? `Last checked ${checkedAgo(last.at)}.`
          : ''
      : ''
  const checking = updateStatus === 'checking'
  const canCheck = updateStatus === 'idle' || checking

  let below: React.ReactNode = null
  if (webFeatures.appUpdater) {
    if (checking) {
      below = (
        <p role="status" className="type-hint text-[var(--color-text-secondary)]">
          Looking for a newer version…
        </p>
      )
    } else if (updateStatus === 'available' && updateVersion) {
      const linuxInstall = !!updatePackageUrl && linuxCanInstall
      below = (
        <Callout tone="info" className="flex items-start justify-between gap-4">
          <CalloutLines
            title={`K2 ${updateVersion} is available`}
            detail={
              updatePackageUrl ? (
                <span data-testid="linux-package-update-hint">
                  {linuxCanInstall
                    ? 'K2 downloads the package, checks it, and asks for your password to install it. Then K2 restarts.'
                    : 'On Linux, updates come from your package (deb, rpm or Arch). Download opens the new version on GitHub.'}
                </span>
              ) : (
                `You're on ${currentVersion || 'an older version'}. Download it, then install and restart.`
              )
            }
          />
          <Button
            variant="accent"
            className="flex-shrink-0"
            onClick={() => void useUpdateStore.getState().openAvailableDownload()}
          >
            {linuxInstall ? 'Install and restart' : 'Download'}
          </Button>
        </Callout>
      )
    } else if (updateStatus === 'downloading' && linuxInstalling) {
      below = (
        <Callout tone="info" data-testid="linux-package-installing" role="status">
          <CalloutLines
            title={`Installing K2 ${updateVersion ?? ''}…`}
            detail="Downloading and checking the package. K2 asks for your password, then restarts."
          />
        </Callout>
      )
    } else if (updateStatus === 'downloading') {
      below = (
        <Callout tone="info" role="status" className="space-y-2">
          <div className="flex items-center justify-between gap-4">
            <CalloutLines title={`Downloading K2 ${updateVersion ?? ''}…`} />
            <span className="type-hint tabular-nums text-[var(--color-text-secondary)]">{updateProgress}%</span>
          </div>
          <ProgressBar value={updateProgress} label={`Downloading K2 ${updateVersion ?? ''}`} />
        </Callout>
      )
    } else if (updateStatus === 'ready') {
      below = (
        <Callout tone="ok" className="flex items-start justify-between gap-4">
          <CalloutLines
            title={`K2 ${updateVersion ?? ''} is ready to install`}
            detail="K2 closes and opens again to finish."
          />
          <Button
            variant="accent"
            className="flex-shrink-0"
            onClick={() => void useUpdateStore.getState().installAndRelaunch()}
          >
            Install and restart
          </Button>
        </Callout>
      )
    } else if (updateStatus === 'error') {
      below = (
        <Callout tone="error" role="alert" className="space-y-2">
          <CalloutLines title="K2 couldn't finish the update" detail={updateError || 'No reason was given.'} />
          {linuxManualCommand && (
            <div className="flex items-start gap-2" data-testid="linux-package-manual-command">
              <code className="flex-1 min-w-0 break-all select-text type-hint font-mono px-2 py-1.5 rounded-[var(--radius-field)] bg-[var(--color-bg)] border border-[var(--color-border)] text-[var(--color-text-primary)]">
                {linuxManualCommand}
              </code>
              <Button variant="quiet" className="flex-shrink-0" onClick={copyManualCommand}>
                {copied ? 'Copied' : 'Copy'}
              </Button>
            </div>
          )}
          <div className="flex items-center gap-2">
            <Button onClick={() => void handleCheckUpdate()}>Try again</Button>
            <Button variant="quiet" onClick={openReleasePage}>
              Open release page
            </Button>
          </div>
        </Callout>
      )
    }
  }

  return (
    <SettingRow
      settingId="general.app-version"
      label={
        <span className="flex items-center gap-2 min-w-0">
          <span>K2 app</span>
          {isRemote && (
            <Tag tone="neutral" title="The K2 version on this computer. The connected server's version is under Connected server.">
              This computer
            </Tag>
          )}
        </span>
      }
      hint={
        <span data-testid="app-version-label">
          {version}
          {status ? ` ${status}` : ''}
        </span>
      }
      below={below ?? undefined}
    >
      {webFeatures.appUpdater && canCheck && (
        <Button className="flex-shrink-0" onClick={() => void handleCheckUpdate()} disabled={checking}>
          {checking ? 'Checking…' : 'Check for updates'}
        </Button>
      )}
    </SettingRow>
  )
}

/** The automatic check (every few hours) as a visible switch. It was only
 *  on the version's right-click menu before. Thin-client localStorage. */
function AutoUpdateChecksRow(): React.JSX.Element {
  const [on, setOn] = useState(autoUpdateChecksEnabled)
  useEffect(() => {
    const sync = (): void => setOn(autoUpdateChecksEnabled())
    window.addEventListener(AUTO_UPDATE_CHECKS_EVENT, sync)
    return () => window.removeEventListener(AUTO_UPDATE_CHECKS_EVENT, sync)
  }, [])
  return (
    <SettingSwitchRow
      settingId="general.auto-update-checks"
      control={
        <Toggle
          checked={on}
          onChange={(next) => {
            setAutoUpdateChecksEnabled(next)
            setOn(next)
          }}
          aria-label="Check for updates automatically"
        />
      }
    >
      <div className={SETTING_ROW_LABEL}>Check for updates automatically</div>
      <div className={SETTING_ROW_HINT}>Every few hours. K2 tells you when a new version is ready.</div>
    </SettingSwitchRow>
  )
}

// ── Composer 1c (D4) — "Allow remote instruct" ─────────────────────────
// MOVED to Settings → K2 Connect (shared/AllowRemoteInstructRow), beneath
// the "Enable federation" master. Since 2026-10-16 it is "Let agents on
// other servers message every agent" and covers federated agents only;
// people who log in can always message agents.

interface CliStatus {
  installed: boolean
  installedVersion: string | null
  bundledVersion: string | null
  updateAvailable: boolean
  packageManaged?: boolean
}

// Compare versions properly — only show update if bundled is actually newer
function compareVersions(a: string, b: string): number {
  const pa = a.split('.').map(Number)
  const pb = b.split('.').map(Number)
  for (let i = 0; i < Math.max(pa.length, pb.length); i++) {
    const va = pa[i] || 0
    const vb = pb[i] || 0
    if (va > vb) return 1
    if (va < vb) return -1
  }
  return 0
}

function CLIVersionRow({ primary }: { primary: boolean }): React.JSX.Element {
  const [status, setStatus] = useState<CliStatus | null>(null)
  const [readError, setReadError] = useState<string | null>(null)
  const [installError, setInstallError] = useState<string | null>(null)
  const [loading, setLoading] = useState(false)

  const checkStatus = useCallback(async (): Promise<CliStatus | null> => {
    try {
      const result = await invoke<CliStatus>('cli_install_status')
      setStatus(result)
      setReadError(null)
      return result
    } catch (e) {
      setReadError(e instanceof Error ? e.message : String(e))
      return null
    }
  }, [])

  useEffect(() => {
    void checkStatus()
  }, [checkStatus])

  const updateAvailable =
    !status?.packageManaged &&
    !!status?.installed &&
    !!status.bundledVersion &&
    !!status.installedVersion &&
    compareVersions(status.bundledVersion, status.installedVersion) > 0

  const handleInstallOrUpdate = useCallback(async () => {
    const wasInstalled = !!status?.installed
    setLoading(true)
    setInstallError(null)
    try {
      await invoke('cli_install')
      const next = await checkStatus()
      const v = next?.installedVersion ? ` (${next.installedVersion})` : ''
      useToastStore
        .getState()
        .addToast(wasInstalled ? `Updated the k2 command${v}.` : `Installed the k2 command${v}.`, 'success', 4000)
    } catch (err) {
      console.error('[cli]', err)
      setInstallError(err instanceof Error ? err.message : String(err))
    } finally {
      setLoading(false)
    }
  }, [checkStatus, status?.installed])

  const hint = (() => {
    const lead = 'Runs K2 from a terminal; agents use it too.'
    if (readError) return `${lead} Couldn't tell whether it's installed.`
    if (!status) return `${lead} Checking…`
    if (!status.installed) return `${lead} Not installed.`
    const v = `Version ${status.installedVersion || 'unknown'} installed.`
    if (status.packageManaged) return `${lead} ${v} It updates with K2's package.`
    if (updateAvailable) return `${lead} ${v} ${status.bundledVersion} is ready to install.`
    return `${lead} ${v}`
  })()

  const below = readError ? (
    <Callout tone="error" role="alert" className="flex items-start justify-between gap-4">
      <p className="type-hint min-w-0 break-words">Couldn&apos;t read the k2 command&apos;s status: {readError}</p>
      <Button className="flex-shrink-0" onClick={() => void checkStatus()}>
        Try again
      </Button>
    </Callout>
  ) : installError ? (
    <Callout tone="error" role="alert">
      <p className="type-hint break-words">
        Couldn&apos;t {status?.installed ? 'update' : 'install'} the k2 command: {installError}
      </p>
    </Callout>
  ) : undefined

  return (
    <SettingRow label="k2 command" hint={hint} settingId="general.cli-version" below={below}>
      {status && !status.installed && (
        <Button
          variant={primary ? 'accent' : 'default'}
          className="flex-shrink-0"
          onClick={() => void handleInstallOrUpdate()}
          disabled={loading}
        >
          {loading ? 'Installing…' : 'Install'}
        </Button>
      )}
      {updateAvailable && (
        <Button className="flex-shrink-0" onClick={() => void handleInstallOrUpdate()} disabled={loading}>
          {loading ? 'Updating…' : `Update to ${status?.bundledVersion}`}
        </Button>
      )}
    </SettingRow>
  )
}

// ── Developer tools (desktop) ──────────────────────────────────────────
// Thin-client only: preference lives in localStorage (`k2:enable-devtools`),
// NOT daemon settings.json. Toggle remembers intent; "Open inspector" invokes
// the Tauri `open_app_devtools` command (works in release via tauri devtools
// feature). Hidden on hosted web (`webFeatures.appDevTools`).
const DEVTOOLS_PREF_KEY = 'k2:enable-devtools'

function readDevToolsPref(): boolean {
  try {
    return localStorage.getItem(DEVTOOLS_PREF_KEY) === '1'
  } catch {
    return false
  }
}

function writeDevToolsPref(on: boolean): void {
  try {
    if (on) localStorage.setItem(DEVTOOLS_PREF_KEY, '1')
    else localStorage.removeItem(DEVTOOLS_PREF_KEY)
  } catch {
    /* private mode / quota */
  }
}

function DevToolsRow(): React.JSX.Element {
  const [enabled, setEnabled] = useState(readDevToolsPref)
  const [opening, setOpening] = useState(false)

  const openDevTools = useCallback(async () => {
    setOpening(true)
    try {
      await invoke('open_app_devtools')
    } catch (e) {
      // eslint-disable-next-line no-console
      console.warn('[devtools] open_app_devtools failed:', e)
      useToastStore
        .getState()
        .addToast(`Couldn't open the web inspector: ${e instanceof Error ? e.message : String(e)}`, 'error')
    } finally {
      setOpening(false)
    }
  }, [])

  const toggle = useCallback(
    (next: boolean) => {
      setEnabled(next)
      writeDevToolsPref(next)
      // Optionally open immediately when enabled so the toggle is useful
      // without a second click.
      if (next) void openDevTools()
    },
    [openDevTools],
  )

  return (
    <SettingSwitchRow
      settingId="general.devtools"
      control={
        <>
          {enabled && (
            <Button variant="quiet" onClick={() => void openDevTools()} disabled={opening}>
              {opening ? 'Opening…' : 'Open inspector'}
            </Button>
          )}
          <Toggle checked={enabled} onChange={toggle} aria-label="Developer tools" />
        </>
      }
    >
      <div className={SETTING_ROW_LABEL}>Developer tools</div>
      <div className={SETTING_ROW_HINT}>Opens the web inspector for this window, for troubleshooting.</div>
    </SettingSwitchRow>
  )
}

// ── What's New — open the latest notes ──────────────────────────────────
// The button does not clear the seen marker. The popup opens on the
// newest note. Arrows walk back through 0.41.0 and later. Dismiss
// still stamps the current version.
function WhatsNewRow(): React.JSX.Element {
  const handleClick = useCallback(() => {
    window.dispatchEvent(new CustomEvent('k2so:show-whats-new'))
  }, [])

  return (
    <SettingRow
      label="Release notes"
      hint="What changed in this version and the ones before it."
      settingId="general.release-notes"
    >
      <Button className="flex-shrink-0" onClick={handleClick}>
        What&apos;s new
      </Button>
    </SettingRow>
  )
}

// ── Settings file and Reset ────────────────────────────────────────────
const SETTINGS_FILE = '~/.k2/settings.json'

function ConfigLocationRow(): React.JSX.Element {
  const [copied, setCopied] = useState(false)
  const copy = (): void => {
    const done = (): void => {
      setCopied(true)
      window.setTimeout(() => setCopied(false), 2000)
    }
    const fail = (e: unknown): void => useToastStore.getState().addToast(`Couldn't copy the path: ${String(e)}`, 'error')
    if (!navigator.clipboard) return fail('the clipboard is not available here')
    navigator.clipboard.writeText(SETTINGS_FILE).then(done, fail)
  }
  return (
    <SettingRow
      label="Settings file"
      settingId="general.config-location"
      hint={
        <>
          Where this server keeps its settings: <span className="font-mono text-[var(--color-text-primary)]">{SETTINGS_FILE}</span>
        </>
      }
    >
      <Button variant="quiet" className="flex-shrink-0" onClick={copy} aria-label="Copy the settings file path">
        {copied ? 'Copied' : 'Copy path'}
      </Button>
    </SettingRow>
  )
}

/** What a reset does, from `k2_core::app_settings::reset` (every key in
 *  settings.json back to its default, and the K2 Companion password
 *  cleared) and the store's resetAllSettings (this computer's look stays). */
export const RESET_ALL_HINT =
  "Shortcuts, fonts, workspace options, sharing and previews go back to their defaults. Workspaces, agents, chats and files stay. Owner only."
export const RESET_ALL_CONFIRM =
  "Every setting on this server goes back to its default: shortcuts, fonts, workspace options, sharing and preview switches. The K2 Companion password is cleared, so phones sign in again. Workspaces, agents, chats, files and this computer's look stay. This can't be undone."

function ResetAllRow(): React.JSX.Element {
  const resetAllSettings = useSettingsStore((s) => s.resetAllSettings)
  const confirm = useConfirmDialogStore((s) => s.confirm)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const reset = async (): Promise<void> => {
    const ok = await confirm({
      title: 'Reset all settings?',
      message: RESET_ALL_CONFIRM,
      confirmLabel: 'Reset all settings',
      destructive: true,
    })
    if (!ok) return
    setBusy(true)
    setError(null)
    try {
      await resetAllSettings()
      useToastStore.getState().addToast('Settings are back to their defaults.', 'success', 5000)
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e)
      setError(/\b403\b/.test(msg) ? "Only this server's owner can reset its settings. Nothing was changed." : `Couldn't reset settings: ${msg}`)
    } finally {
      setBusy(false)
    }
  }

  return (
    <SettingRow
      label="Reset all settings"
      hint={RESET_ALL_HINT}
      settingId="general.reset-all"
      below={
        error ? (
          <Callout tone="error" role="alert">
            <p className="type-hint">{error}</p>
          </Callout>
        ) : undefined
      }
    >
      <Button variant="danger-muted" className="flex-shrink-0" onClick={() => void reset()} disabled={busy}>
        {busy ? 'Resetting…' : 'Reset…'}
      </Button>
    </SettingRow>
  )
}

// ── K2 Daemon ────────────────────────────────────────────────────────
// Backs the persistent-agents feature: a launchd-managed background
// process that keeps agents running while the Tauri window is closed
// and (optionally) wakes the machine from sleep on a schedule. This
// row is how a user knows it's running, installs it, or turns it off.
//
// The shape returned by `daemon_status` is a tagged union — we dispatch
// on `state` to decide which action buttons to show. Every button is a
// thin wrapper over a Tauri command; the command handlers wrap
// k2so_core::wake / launchctl. The frontend never touches launchctl
// directly.

type DaemonStatusState =
  | { state: 'running'; version: string; uptime_secs: number; pid: number; port: number }
  | { state: 'not_installed'; reason: string }
  | { state: 'unreachable'; reason: string }

function formatUptime(secs: number): string {
  if (secs < 60) return `${secs}s`
  if (secs < 3600) return `${Math.floor(secs / 60)}m`
  if (secs < 86400) return `${Math.floor(secs / 3600)}h ${Math.floor((secs % 3600) / 60)}m`
  return `${Math.floor(secs / 86400)}d ${Math.floor((secs % 86400) / 3600)}h`
}

function DaemonRow(): React.JSX.Element {
  const [status, setStatus] = useState<DaemonStatusState | null>(null)
  const [busy, setBusy] = useState<null | 'install' | 'restart'>(null)
  const [error, setError] = useState<string | null>(null)
  const [showingLog, setShowingLog] = useState(false)
  const [logText, setLogText] = useState<string>('')
  const confirm = useConfirmDialogStore((s) => s.confirm)

  const refresh = useCallback(async () => {
    try {
      const result = await invoke<DaemonStatusState>('daemon_status')
      setStatus(result)
      setError(null)
    } catch (e) {
      setError(`Couldn't read the K2 server's status: ${String(e)}`)
    }
  }, [])

  // Refresh on mount + every 4s while the row is visible. Cheap —
  // daemon_status is just a file read + an HTTP ping on localhost.
  useEffect(() => {
    void refresh()
    const id = window.setInterval(() => void refresh(), 4000)
    return () => window.clearInterval(id)
  }, [refresh])

  const handleInstall = useCallback(async () => {
    setBusy('install')
    setError(null)
    try {
      await invoke<string>('daemon_install')
      await refresh()
      useToastStore.getState().addToast('The K2 server is installed and running.', 'success', 4000)
    } catch (e) {
      setError(`Couldn't install the K2 server: ${String(e)}`)
    } finally {
      setBusy(null)
    }
  }, [refresh])

  const handleRestart = useCallback(
    async (ask: boolean) => {
      if (ask) {
        const ok = await confirm({
          title: 'Restart the K2 server?',
          message:
            'Agents, terminals and K2 Companion disconnect for a few seconds, then reconnect. With "Agents keep running through updates and restarts" on, agent sessions carry on.',
          confirmLabel: 'Restart',
        })
        if (!ok) return
      }
      setBusy('restart')
      setError(null)
      try {
        await invoke('daemon_restart')
        // Give launchd a moment to respawn before we query again.
        await new Promise((r) => setTimeout(r, 1200))
        await refresh()
        useToastStore.getState().addToast('The K2 server restarted.', 'success', 4000)
      } catch (e) {
        setError(`Couldn't restart the K2 server: ${String(e)}`)
      } finally {
        setBusy(null)
      }
    },
    [refresh, confirm],
  )

  const handleViewLog = useCallback(async () => {
    try {
      const text = await invoke<string>('daemon_log_tail', { lines: 300 })
      setLogText(text)
      setShowingLog(true)
    } catch (e) {
      setError(`Couldn't open the server log: ${String(e)}`)
    }
  }, [])

  const tone: HealthTone = !status
    ? 'neutral'
    : status.state === 'running'
      ? 'ok'
      : status.state === 'not_installed'
        ? 'neutral'
        : 'error'
  const word = !status
    ? 'Checking…'
    : status.state === 'running'
      ? 'Running'
      : status.state === 'not_installed'
        ? 'Not installed'
        : 'Not answering'
  const detail =
    status?.state === 'running'
      ? ` · version ${status.version} · up ${formatUptime(status.uptime_secs)}`
      : status?.state === 'not_installed'
        ? '. Without it, agents stop when you close K2.'
        : ''

  return (
    <SettingRow
      label="K2 server"
      settingId="general.daemon"
      hint={
        <span title={status?.state === 'running' ? `PID ${status.pid}, port ${status.port}` : undefined}>
          <span className="inline-flex items-center gap-1.5 align-middle">
            <StatusDot tone={tone} />
            <HealthWord tone={tone}>{word}</HealthWord>
          </span>
          {detail}
        </span>
      }
      below={
        status?.state === 'unreachable' || error || showingLog ? (
          <div className="space-y-2">
            {status?.state === 'unreachable' && !error && (
              <Callout tone="error" role="alert">
                <p className="type-hint break-words">
                  The K2 server is installed but isn&apos;t answering{status.reason ? ` (${status.reason})` : ''}. Restart
                  it; if it still doesn&apos;t answer, the log says why.
                </p>
              </Callout>
            )}
            {error && (
              <Callout tone="error" role="alert">
                <p className="type-hint break-words">{error}</p>
              </Callout>
            )}
            {showingLog && (
              <div
                data-k2-chamfer="surface"
                className="rounded-[var(--radius-box)] border border-[var(--color-border)] bg-[var(--color-bg)]"
              >
                <div className="flex items-center justify-between gap-3 px-3 py-2 border-b border-[var(--color-border)]">
                  <span className="type-hint text-[var(--color-text-secondary)] min-w-0 truncate">
                    Last 300 lines of <span className="font-mono">~/.k2/daemon.stdout.log</span>
                  </span>
                  <Button variant="quiet" className="flex-shrink-0" onClick={() => setShowingLog(false)}>
                    Close log
                  </Button>
                </div>
                <pre className="max-h-60 overflow-auto px-3 py-2 type-hint text-[var(--color-text-secondary)] whitespace-pre-wrap font-mono leading-tight">
                  {logText || 'The log is empty.'}
                </pre>
              </div>
            )}
          </div>
        ) : undefined
      }
    >
      {status && status.state !== 'not_installed' && !showingLog && (
        <Button variant="quiet" className="flex-shrink-0" onClick={() => void handleViewLog()}>
          View log
        </Button>
      )}
      {status?.state === 'not_installed' && (
        <Button variant="accent" className="flex-shrink-0" onClick={() => void handleInstall()} disabled={busy !== null}>
          {busy === 'install' ? 'Installing…' : 'Install'}
        </Button>
      )}
      {(status?.state === 'running' || status?.state === 'unreachable') && (
        <Button
          className="flex-shrink-0"
          onClick={() => void handleRestart(status.state === 'running')}
          disabled={busy !== null}
        >
          {busy === 'restart' ? 'Restarting…' : 'Restart'}
        </Button>
      )}
    </SettingRow>
  )
}

/** A daemon-settings switch row: reads its value once, saves on flip with
 *  the switch held while the write is out, and says so when either fails
 *  (the switch goes back on a refused write). */
function useDaemonSwitch(
  read: (s: Awaited<ReturnType<typeof settingsGet>>) => boolean,
  write: (next: boolean) => Parameters<typeof settingsUpdate>[0],
  tag: string,
): { on: boolean; saving: boolean; error: string | null; flip: () => void } {
  const [on, setOn] = useState(true)
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const readRef = useRef(read)
  readRef.current = read

  useEffect(() => {
    settingsGet()
      .then((s) => setOn(readRef.current(s)))
      .catch((e) => {
        console.warn(`[${tag}]`, e)
        setError(`Couldn't read this setting: ${e instanceof Error ? e.message : String(e)}`)
      })
  }, [tag])

  const flip = (): void => {
    const next = !on
    setOn(next) // optimistic
    setSaving(true)
    setError(null)
    settingsUpdate(write(next))
      .catch((e: unknown) => {
        console.error(`[${tag}]`, e)
        setOn(!next) // revert
        setError(`Couldn't save the change: ${e instanceof Error ? e.message : String(e)}`)
      })
      .finally(() => setSaving(false))
  }

  return { on, saving, error, flip }
}

function SwitchError({ error }: { error: string | null }): React.JSX.Element | null {
  if (!error) return null
  return (
    <p role="alert" className="type-hint text-[var(--color-status-error-soft)] mt-1">
      {error}
    </p>
  )
}

// ── Agents keep running through updates and restarts ──────────────────
// k2-ptyd (prd-k2-ptyd-v1 P67; on by default since 0.46.5): the daemon's
// own setting `terminal.ptyd` (`on` | `off` | `auto`; unset = on, only
// `off` turns it off). With it on, NEW agent
// sessions run under k2-ptyd and survive a daemon update, restart or
// crash; quitting the daemon still ends them. Daemon-first: the switch
// lives in the daemon's settings, so headless servers honour it too
// (`k2 settings agents-survive-restarts on`).

const SURVIVE_LABEL = 'Agents keep running through updates and restarts'

function AgentsSurviveRestartsRow(): React.JSX.Element {
  const { on, saving, error, flip } = useDaemonSwitch(
    (s) => s.terminal?.ptyd !== 'off',
    // The daemon deep-merges `terminal`, so the font settings stay.
    (next) => ({ terminal: { ptyd: next ? 'on' : 'off' } }),
    'agents-survive-restarts',
  )

  return (
    <SettingSwitchRow
      settingId="general.agents-survive-restarts"
      control={<Toggle checked={on} onChange={flip} disabled={saving} aria-label={SURVIVE_LABEL} />}
    >
      <div className={SETTING_ROW_LABEL}>{SURVIVE_LABEL}</div>
      <div className={SETTING_ROW_HINT}>
        {on
          ? 'New agent sessions keep running when K2 updates, restarts or crashes. Sessions started before you turned this on stop at the next restart. macOS and Linux.'
          : 'Updating or restarting K2 stops running agents; they pick up their conversation afterwards.'}
      </div>
      <SwitchError error={error} />
    </SettingSwitchRow>
  )
}

// ── Keep daemon on quit ───────────────────────────────────────────────
// Default is ON — persistent agents keep running past Cmd+Q. User can
// flip OFF to get "normal app" quit behavior (daemon stops with the
// window). Honored by:
//   - RunEvent::ExitRequested handler in src-tauri/src/lib.rs
//   - Menubar → Quit K2
// Both check the same setting via `get_keep_daemon_on_quit`.
// The old `set_keep_daemon_on_quit` command emitted NO cross-window sync
// event, so this mirrors that (no `sync:settings` emit here).

const KEEP_LABEL = 'Keep server running when the window is closed'

function KeepDaemonOnQuitRow(): React.JSX.Element {
  // Default `true` so the row renders at once; the daemon's value (default
  // true if absent) arrives next tick.
  const { on, saving, error, flip } = useDaemonSwitch(
    (s) => s.keepDaemonOnQuit ?? true,
    (next) => ({ keepDaemonOnQuit: next }),
    'keep-daemon-on-quit',
  )

  return (
    <SettingSwitchRow
      settingId="general.keep-daemon-on-quit"
      control={<Toggle checked={on} onChange={flip} disabled={saving} aria-label={KEEP_LABEL} />}
    >
      <div className={SETTING_ROW_LABEL}>{KEEP_LABEL}</div>
      <div className={SETTING_ROW_HINT}>
        {on
          ? 'The red close button hides the window; agents and K2 Companion keep going. Cmd+Q quits everything.'
          : 'The red close button quits everything, like Cmd+Q. Agents pause and K2 Companion disconnects.'}
      </div>
      <SwitchError error={error} />
    </SettingSwitchRow>
  )
}

// ── Connected server ──────────────────────────────────────────────────
// The second column of General, ONLY when connected to a remote host. It
// holds the host-only controls: Restart (#661) + Update (P4 / Shape A),
// in a card titled with the host so it is never mistaken for this
// computer (local update is the K2 app row, local restart the K2 server
// row on the Server tab). An older server without either route gets a
// row saying so instead of an empty card.
export function GeneralRemoteHostPanel(): React.JSX.Element | null {
  const activeHost = useConnectHostStore((s) => s.activeHost)
  const supportsRestart = useServerSupports('daemon-restart')
  const supportsUpdate = useServerSupports('remote-update')
  if (activeHost === 'local') return null
  const hostLabel = activeHost.label?.trim() || activeHost.hostname
  const needs = featureMinVersion('daemon-restart')
  return (
    <SettingsGroup
      title="Connected server"
      hint={
        <>
          <span className="text-[var(--color-text-primary)]">{hostLabel}</span>
          {hostLabel !== activeHost.hostname && <span className="font-mono"> ({activeHost.hostname})</span>}. These act
          on that server, not this computer.
        </>
      }
      card
    >
      {supportsRestart || supportsUpdate ? (
        <>
          <RestartHostRow />
          <UpdateHostRow />
        </>
      ) : (
        <SettingRow
          label="Restart and update"
          hint={`${hostLabel} runs a K2 too old to restart or update from here${needs ? ` (needs ${needs} or later)` : ''}. Update it on that computer and these controls appear.`}
          testId="remote-host-unsupported"
        >
          <span className="type-hint text-[var(--color-text-secondary)]">Not available</span>
        </SettingRow>
      )}
    </SettingsGroup>
  )
}

// ── Restart connected host (#661) ──────────────────────────────────────
// Lets a user restart the REMOTE machine they're connected to over K2
// Connect — NOT their local Mac. The #1 design goal is that it is
// UNMISTAKABLE which machine this acts on (the original update bug was
// "it restarted my laptop instead of the remote"):
//
//   * The row renders ONLY when the active host is a REMOTE ConnectHost.
//     For the local Mac it renders NOTHING — the local-Mac restart lives
//     in the K2 Server (DaemonRow) above. No ambiguity, no shared button.
//   * A prominent REMOTE badge + the host's display name + hostname sit on
//     the row, and the confirm dialog NAMES the host explicitly:
//     "This will restart <host> (the machine you're connected to)…".
//   * It posts host-aware `daemonCliPost('daemon/restart', {})`, which
//     getDaemonWs() routes to the ACTIVE host — never the local daemon.
//
// Reconnect is NOT our job: the ConnectionGate's soft-reconnect already
// covers the gap and returns when the host is back. We just fire the
// POST and show "Restarting <host>…".
//
// Gating:
//   * serverSupports('daemon-restart') (min 0.39.32) — an OLDER remote
//     that lacks the route hides the control instead of dead-ending on a
//     404.
//   * Owner/Admin on the active host, resolved from the host-aware
//     `auth/whoami` role. The route itself is owner-token-gated, so a
//     non-owner session 403s; we surface that 403 as a clear toast rather
//     than a silent no-op.
function RestartHostRow(): React.JSX.Element | null {
  const activeHost = useConnectHostStore((s) => s.activeHost)
  const supportsRestart = useServerSupports('daemon-restart')
  const confirm = useConfirmDialogStore((s) => s.confirm)
  const addToast = useToastStore((s) => s.addToast)

  // The viewer's role there, read again until known (host-role.ts): an
  // unread role is "checking", never "owner or admin only".
  const { role, recheck } = useActiveHostRole()
  const [restarting, setRestarting] = useState(false)

  const isRemote = activeHost !== 'local'
  // A stable label for copy: the user-facing name, falling back to the
  // hostname so the dialog is never blank.
  const hostLabel = isRemote ? (activeHost.label?.trim() || activeHost.hostname) : ''
  const hostname = isRemote ? activeHost.hostname : ''

  // UNMISTAKABLE: never render for the local Mac (DaemonRow owns that),
  // hide for an older remote without the route. A Member sees why it is
  // off; a role not read yet is "checking". Pure decision: hostActionGate.
  const gate = hostActionGate({ isRemote, supported: supportsRestart, role, hostLabel, action: 'restart' })
  const canRestart = gate.enabled
  if (!gate.show) return null

  const handleRestart = async (): Promise<void> => {
    const copy = restartHostConfirmCopy(hostLabel, hostname)
    const ok = await confirm({
      title: copy.title,
      message: copy.message,
      confirmLabel: copy.confirmLabel,
      destructive: true,
    })
    if (!ok) return
    setRestarting(true)
    try {
      await daemonCliPost(primaryScope(), 'daemon/restart', {})
      addToast(`Restarting ${hostLabel}… it'll reconnect automatically.`, 'info', 8000)
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e)
      // The route's 403 body is the same for a Member and for a login the
      // host no longer knows: ask whoami which it was, and say that.
      if (isForbiddenError(msg)) {
        addToast(refusalCopy(hostLabel, 'restart', msg, await recheck()), 'error', 8000)
      } else {
        addToast(`Couldn't restart ${hostLabel}: ${msg}`, 'error', 8000)
      }
      setRestarting(false)
    }
    // On success we deliberately leave `restarting` true: the host is
    // going down and the ConnectionGate's soft-reconnect takes over.
  }

  return (
    <SettingRow
      label="Restart"
      hint={`Sessions on ${hostLabel} disconnect for a moment, then reconnect by themselves.`}
      settingId="general.restart-host"
      below={
        restarting || gate.note ? (
          <div className="space-y-1">
            {restarting && (
              <p role="status" className="type-hint text-[var(--color-text-secondary)]">
                Restarting {hostLabel}… it reconnects by itself.
              </p>
            )}
            {gate.note && <p className="type-hint text-[var(--color-text-secondary)]">{gate.note}</p>}
          </div>
        ) : undefined
      }
    >
      <Button
        className="flex-shrink-0"
        onClick={() => void handleRestart()}
        disabled={!canRestart || restarting}
        title={gate.title}
      >
        {restarting ? 'Restarting…' : 'Restart…'}
      </Button>
    </SettingRow>
  )
}

// ── Update connected host (P4) ─────────────────────────────────────────
// Lets a user UPDATE the REMOTE machine they're connected to over K2
// Connect — NOT their local Mac. Directly analogous to RestartHostRow,
// with the same #1 design goal: it is UNMISTAKABLE which machine this
// acts on.
//
//   * The row renders ONLY when the active host is a REMOTE ConnectHost.
//     For the local Mac it renders NOTHING — the local-Mac update lives in
//     the "App Version" Tauri auto-updater area at the TOP of this section.
//   * A prominent REMOTE badge + the host's display name + hostname sit on
//     the row, and the confirm dialog NAMES the host explicitly.
//
// Flow (against the P3 daemon update routes, host-aware):
//   1. "Check for updates" → POST daemon/update/check → {current, latest,
//      available, …}. If available, show "Update available — c → l".
//   2. "Download" → POST daemon/update/start {} → {job_id}, then poll
//      GET daemon/update/status?job_id every ~1.5s, surfacing phase +
//      download progress.
//   3. phase==staged → "Install & restart <host>" → confirm (NAMES host) →
//      POST daemon/update/apply {job_id}. The host then restarts on the new
//      version.
//   4. Comeback (0.46.3): "Updating <host>… reconnecting…" while it is
//      away, then a verdict from what the host reports once back — its
//      /boot-status version + instanceId and the job (kept across the
//      restart by 0.46.3 daemons): "Updated <host> to vX", "Update failed
//      on <host>: <reason>", or "Couldn't confirm…" on timeout. A dropped
//      connection during apply/restart is never a failure by itself.
//   * failed / rolled-back are surfaced as a host-named message
//     ("Update rolled back — <host> is still on <current>").
//
// Gating (identical to RestartHostRow):
//   * serverSupports('remote-update') (min 0.39.33) — an OLDER remote that
//     lacks the routes hides the control instead of 404ing.
//   * Owner/Admin on the active host (host-aware auth/whoami role). The
//     routes are owner-token-gated, so a 403 surfaces as a clear toast.
export function UpdateHostRow(): React.JSX.Element | null {
  const activeHost = useConnectHostStore((s) => s.activeHost)
  const supportsUpdate = useServerSupports('remote-update')
  const confirm = useConfirmDialogStore((s) => s.confirm)
  const addToast = useToastStore((s) => s.addToast)

  // The viewer's role there, read again until known (host-role.ts).
  const { role, recheck } = useActiveHostRole()
  const [checking, setChecking] = useState(false)
  const [check, setCheck] = useState<UpdateCheckResult | null>(null)
  const [jobId, setJobId] = useState<string | null>(null)
  const [status, setStatus] = useState<UpdateStatusResult | null>(null)
  // A line about the job the status poll can't show as a phase: replies it
  // can't read, or the host no longer having the job (z3.k2.dev, 2026-11-05).
  const [jobNote, setJobNote] = useState<string | null>(null)
  const [applying, setApplying] = useState(false)
  // Comeback watcher (0.40.48; verdicts 0.46.3): once the host goes away to
  // install, watch it come back and decide from what it REPORTS — its
  // /boot-status version + instanceId and the update job (which a 0.46.3
  // daemon keeps across the restart). A dropped connection on the way is
  // the update working, never a failure by itself (Rosson 2026-10-19).
  const [watching, setWatching] = useState(false)
  const [comeback, setComeback] = useState<
    | null
    | { kind: 'watching' }
    | { kind: 'slow' }
    | { kind: 'success'; version?: string }
    | { kind: 'failed'; reason: string; version?: string }
    | { kind: 'unconfirmed'; reason: string; version?: string }
  >(null)
  // Standalone hosts may only go away after `apply` was sent; bundled-app
  // hosts any time after `start` (see pollErrorMeansRestart).
  const applySentRef = useRef(false)
  // /boot-status instanceId read just before the restart was triggered —
  // a different id afterwards proves the daemon restarted.
  const startInstanceRef = useRef<string | undefined>(undefined)

  const isRemote = activeHost !== 'local'
  const hostLabel = isRemote ? (activeHost.label?.trim() || activeHost.hostname) : ''
  const hostname = isRemote ? activeHost.hostname : ''
  const hostId = isRemote ? activeHost.id : ''

  // A host switch resets any in-flight check/job state so it never shows a
  // stale update — except a job this window started on THIS host (kept
  // across leaving the page): the row picks it up, so a downloaded update
  // still reaches Install & restart (z3.k2.dev, 2026-11-05). The role is
  // useActiveHostRole's.
  useEffect(() => {
    const kept = isRemote ? keptUpdateJob(hostId) : null
    setCheck(kept?.check ?? null)
    setJobId(kept?.jobId ?? null)
    setStatus(null)
    setJobNote(null)
    setApplying(false)
    setWatching(false)
    setComeback(null)
    applySentRef.current = kept?.applySent ?? false
    startInstanceRef.current = undefined
  }, [isRemote, hostId])

  const phase: UpdatePhase | null = status?.phase ?? null
  // The host is going away to install: the job said so, or a dropped
  // connection after the point where that is expected.
  const hostGoneInstalling = watching || phase === 'restarting' || phase === 'done'
  const installKind = check?.installKind

  // Poll the update job status every ~1.5s while a job is running and not
  // yet terminal. Once the host is going away the comeback watcher takes
  // over (it reads the job too, after the restart).
  useEffect(() => {
    if (!jobId) return
    if (isTerminalPhase(phase) || hostGoneInstalling) return
    let cancelled = false
    const tick = async (): Promise<void> => {
      try {
        const s = readUpdateStatus(
          hostLabel,
          await daemonCliGet<unknown>(primaryScope(), 'daemon/update/status', { job_id: jobId }),
        )
        if (cancelled) return
        setStatus(s)
        setJobNote(null)
        if (isFailurePhase(s.phase)) forgetUpdateJob(hostId)
      } catch (e) {
        if (cancelled) return
        const msg = e instanceof Error ? e.message : String(e)
        // A reply that isn't a status: say so on the row and keep the job
        // (the next readable answer brings its phase back). Never drop to
        // idle silently — that hid the Install & restart button. Checked
        // first: the message quotes the reply, which may say "Forbidden".
        if (e instanceof UpdateReplyError) {
          setJobNote(updateStatusUnreadableCopy(msg))
          return
        }
        // The host's token-gate 403 is the same for a Member and for a
        // login it no longer knows (a stale token mid-revival). Only a
        // Member loses the job; otherwise it is kept (the downloaded update
        // still reaches Install & restart once the login is back) and the
        // row says why it can't read it right now.
        if (isForbiddenError(msg)) {
          const now = await recheck()
          if (cancelled) return
          if (now.kind === 'known' && now.role === 'member') {
            addToast(refusalCopy(hostLabel, 'update', msg, now), 'error', 8000)
            forgetUpdateJob(hostId)
            setJobId(null)
            return
          }
          setJobNote(refusalCopy(hostLabel, 'update', msg, now))
          return
        }
        // The host forgot the job before Install & restart (it restarted):
        // nothing was installed. Say so and offer a fresh check.
        if (!applySentRef.current && (e as { status?: unknown } | null)?.status === 404) {
          forgetUpdateJob(hostId)
          setJobId(null)
          setStatus(null)
          setCheck(null) // its answer is stale now: check again
          setJobNote(updateJobGoneCopy(hostLabel))
          return
        }
        // The host dropped (or forgot the job across a restart) at a point
        // where that means it is installing: watch it come back. Earlier
        // blips are ignored and the poll carries on.
        if (pollErrorMeansRestart({ err: e, installKind, applySent: applySentRef.current })) {
          setWatching(true)
        }
      }
    }
    const id = window.setInterval(() => void tick(), 1500)
    // Fire one immediately so the row updates without a 1.5s lag.
    void tick()
    return () => {
      cancelled = true
      window.clearInterval(id)
    }
  }, [jobId, phase, hostGoneInstalling, installKind, hostLabel, hostId, addToast, recheck])

  // Surface a failure phase once, as a host-named toast with the reason.
  useEffect(() => {
    if (isFailurePhase(phase) && phase) {
      const base = updatePhaseCopy(phase, hostLabel, { current: check?.current })
      addToast(status?.error ? `${base} ${status.error}` : base, 'error', 10000)
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps -- once per failure phase
  }, [phase, hostLabel, check?.current, addToast])

  // Comeback watcher: poll the host's /boot-status (with the host's own
  // token, so a host that hides its details from strangers still reports
  // its version — PRD boot-status-ghost; hostBootStatus's retry loop
  // evicts dead pooled sockets) plus the job
  // until there is a verdict — success (back on the expected version, or the
  // job says done), failed (the job's real reason, or back on another
  // version), or unconfirmed after ~10 min (never reported as a failure).
  // The copy turns "taking longer than expected" at ~4 min.
  useEffect(() => {
    // `!isRemote` already narrows activeHost to a ConnectHost below.
    if (!hostGoneInstalling || !isRemote) return
    let alive = true
    const expected = check?.latest
    const from = check?.current
    const job = jobId
    const creds = remoteCreds(activeHost)
    setComeback({ kind: 'watching' })
    void (async () => {
      const outcome = await watchUpdateComeback(
        {
          bootStatus: () => hostBootStatus(creds),
          jobStatus: async () => {
            if (!job) return null
            try {
              return readUpdateStatus(
                hostLabel,
                await daemonCliGet<unknown>(primaryScope(), 'daemon/update/status', { job_id: job }),
              )
            } catch {
              return null // older daemon (404 after restart) / still settling / unreadable
            }
          },
          // Jittered so many clients watching the same rebooting host don't
          // poll in lockstep (same rationale as the recovery poll).
          sleep: (ms) => new Promise((r) => setTimeout(r, jittered(ms))),
          now: () => Date.now(),
          alive: () => alive,
          onSlow: () => setComeback((k) => (k?.kind === 'watching' ? { kind: 'slow' } : k)),
        },
        { expected, from, startInstanceId: startInstanceRef.current },
      )
      if (!alive || !outcome) return
      // Resolve the job UI back to idle: the outcome line takes over, and the
      // version line shows what the host ACTUALLY reports (never assumed).
      forgetUpdateJob(hostId)
      setComeback(outcome)
      setJobId(null)
      setStatus(null)
      setApplying(false)
      setWatching(false)
      applySentRef.current = false
      setCheck((c) =>
        c
          ? {
              ...c,
              current: outcome.version ?? c.current,
              available: outcome.kind === 'success' ? false : c.available,
            }
          : c,
      )
      if (outcome.kind === 'failed') {
        addToast(updateFailedCopy(hostLabel, outcome.reason), 'error', 10000)
      }
    })()
    return () => {
      alive = false
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps -- keyed on the
    // watch trigger + host identity; check.latest/current and the job id are
    // captured at start by design (they must not drift mid-watch).
  }, [hostGoneInstalling, isRemote, hostId])

  // Only a role the host REPORTED below Admin refuses; a role not read yet
  // is "checking" (host-role.ts), never "owner or admin only".
  const gate = hostActionGate({ isRemote, supported: supportsUpdate, role, hostLabel, action: 'update' })
  const canUpdate = gate.enabled
  if (!gate.show) return null

  /** The line for a failed update call. The routes' 403 body is the same
   *  for a Member and for a login the host no longer knows, so whoami is
   *  read again to say which. */
  const errorLine = async (e: unknown, lead: string): Promise<string> => {
    const msg = e instanceof Error ? e.message : String(e)
    if (!(e instanceof UpdateReplyError) && isForbiddenError(msg)) {
      return refusalCopy(hostLabel, 'update', msg, await recheck())
    }
    return `${lead}: ${msg}`
  }
  const reportError = async (e: unknown): Promise<void> => {
    addToast(await errorLine(e, `Couldn't update ${hostLabel}`), 'error', 8000)
  }

  // Read the host's instanceId right before triggering a restart, so the
  // watcher can tell a real restart from a network blip.
  const captureInstanceId = async (): Promise<void> => {
    if (!isRemote) return
    const s = await hostBootStatus(remoteCreds(activeHost))
    startInstanceRef.current = s?.instanceId
  }

  const handleCheck = async (): Promise<void> => {
    setChecking(true)
    setStatus(null)
    setJobId(null)
    setWatching(false)
    // A fresh check supersedes any resolved comeback verdict — its result
    // is newer truth than a line computed during the restart window (the
    // Baden screenshot: a stale "rolled back to v0.40.44" line sitting
    // under a correct "is on v0.40.47" check result).
    setComeback(null)
    setJobNote(null)
    forgetUpdateJob(hostId)
    try {
      // A reply that isn't an update check throws a named error instead of
      // rendering "<host> is on vundefined" (z3.k2.dev, 2026-11-05).
      const result = readUpdateCheck(
        hostLabel,
        await daemonCliPost<unknown>(primaryScope(), 'daemon/update/check', {}),
      )
      setCheck(result)
      if (!result.available) {
        // B4: a newer version exists but there's no build for this host's
        // platform — DON'T claim "up to date"; surface the distinct state.
        if (result.newerViaPackage) {
          addToast(newerViaPackageCopy(hostLabel, result.latest), 'info', 8000)
        } else if (result.newerNoArtifact) {
          addToast(
            newerNoArtifactCopy(hostLabel, result.latest, result.platform),
            'info',
            8000,
          )
        } else {
          addToast(`${hostLabel} is up to date (${result.current}).`, 'info', 6000)
        }
      }
    } catch (e) {
      addToast(await errorLine(e, `Couldn't check ${hostLabel} for updates`), 'error', 8000)
    } finally {
      setChecking(false)
    }
  }

  const handleDownload = async (): Promise<void> => {
    setComeback(null)
    setWatching(false)
    setJobNote(null)
    applySentRef.current = false
    // Bundled-app hosts restart on their own once the app has the update.
    if (check?.installKind === 'bundled-app') await captureInstanceId()
    try {
      const { job_id } = readUpdateStart(
        hostLabel,
        await daemonCliPost<unknown>(primaryScope(), 'daemon/update/start', {}),
      )
      // Kept for this window: leaving the page no longer loses the job.
      if (check) keepUpdateJob(hostId, { jobId: job_id, check, applySent: false })
      setStatus({ phase: 'downloading' })
      setJobId(job_id)
    } catch (e) {
      await reportError(e)
    }
  }

  const handleApply = async (): Promise<void> => {
    if (!jobId) return
    const copy = updateHostConfirmCopy(hostLabel, hostname, check?.latest ?? 'the new version')
    const ok = await confirm({
      title: copy.title,
      message: copy.message,
      confirmLabel: copy.confirmLabel,
      destructive: true,
    })
    if (!ok) return
    setApplying(true)
    await captureInstanceId()
    applySentRef.current = true
    if (check) keepUpdateJob(hostId, { jobId, check, applySent: true })
    setJobNote(null)
    const notApplied = (line: string): void => {
      applySentRef.current = false
      if (check) keepUpdateJob(hostId, { jobId, check, applySent: false })
      // On the row as well as a toast: the person must see why nothing
      // happened (Rosson 2026-11-05).
      setJobNote(line)
      addToast(line, 'error', 10000)
      setApplying(false)
    }
    try {
      await daemonCliPost(primaryScope(), 'daemon/update/apply', { job_id: jobId })
      // The host now installs + restarts; the watcher confirms the version.
      setStatus({ phase: 'restarting' })
    } catch (e) {
      // "unknown job_id" / "not 'staged'": went ahead only if the host
      // restarted or the job moved on; the same daemon with the job still
      // staged (or gone) means nothing was installed — say so now instead
      // of waiting ten minutes for a restart that never comes.
      if (isJobStateRefusal(e)) {
        const verdict = await settleApplyRefusal({
          hostLabel,
          err: e,
          startInstanceId: startInstanceRef.current,
          bootStatus: () => (isRemote ? hostBootStatus(remoteCreds(activeHost)) : Promise.resolve(null)),
          jobStatus: async () => {
            try {
              return readUpdateStatus(
                hostLabel,
                await daemonCliGet<unknown>(primaryScope(), 'daemon/update/status', { job_id: jobId }),
              )
            } catch (err) {
              return (err as { status?: unknown } | null)?.status === 404 ? 'unknown' : null
            }
          },
        })
        if (verdict.kind === 'went-ahead') {
          setStatus({ phase: 'restarting' })
          return
        }
        notApplied(verdict.message)
        if (verdict.jobGone) {
          // The job is gone on the host: start over from a fresh check.
          forgetUpdateJob(hostId)
          setJobId(null)
          setStatus(null)
          setCheck(null)
        }
        return
      }
      // The daemon restarting (or its tunnel going) mid-answer is the update
      // going ahead — not a failure. Watch the host come back and decide
      // from its version.
      if (isAmbiguousApplyError(e)) {
        setStatus({ phase: 'restarting' })
        return
      }
      notApplied(await errorLine(e, `Couldn't install the update on ${hostLabel}`))
    }
  }

  // Decide the action-area content from the current state. A job whose
  // phase isn't known yet (picked up after leaving the page, or replies
  // that can't be read) is in progress: never the idle Download button,
  // which would start a second download.
  const inProgress =
    hostGoneInstalling ||
    (jobId !== null && !isStaged(phase) && !isFailurePhase(phase))
  const staged = !hostGoneInstalling && isStaged(phase)
  const watchLine =
    comeback?.kind === 'slow' ? updateSlowComebackCopy(hostLabel) : updateReconnectingCopy(hostLabel)

  const idle = !inProgress && !staged
  const progressPct = typeof status?.progress === 'number' ? status.progress * 100 : undefined

  // The one control on the right: Check → Download (or Update server) →
  // Install & restart. A job in flight shows its phase below instead.
  let control: React.ReactNode = null
  if (idle) {
    control = check?.available ? (
      <Button
        variant="accent"
        className="flex-shrink-0"
        onClick={() => void handleDownload()}
        disabled={!canUpdate}
        title={gate.title}
      >
        {/* Bundled-app hosts (Shape A) update in one shot — the Tauri
            updater downloads + installs + relaunches, there's no separate
            staged "Install & restart" step — so the button is "Update
            server", not "Download". Standalone (Shape B) hosts keep the
            two-step Download → Install & restart flow. */}
        {check?.installKind === 'bundled-app' ? 'Update server' : 'Download'}
      </Button>
    ) : (
      <Button
        className="flex-shrink-0"
        onClick={() => void handleCheck()}
        disabled={!canUpdate || checking}
        title={gate.title}
      >
        {checking ? 'Checking…' : 'Check for updates'}
      </Button>
    )
  } else if (staged) {
    control = (
      <Button
        variant="accent"
        className="flex-shrink-0"
        onClick={() => void handleApply()}
        disabled={!canUpdate || applying}
        title={gate.title}
      >
        {applying ? 'Installing…' : `Install & restart ${hostLabel}`}
      </Button>
    )
  }

  const below: React.ReactNode[] = []
  // Why the buttons are off (a Member, an expired sign-in) or that the role
  // couldn't be read yet — never "owner or admin" for an unread role
  // (Rosson 2026-11-05).
  if (gate.note) below.push(<p key="gate" className="type-hint text-[var(--color-text-secondary)]">{gate.note}</p>)
  // Update-available banner once a check reports one (pre-download).
  if (check?.available && idle) {
    below.push(
      <Callout key="available" tone="info">
        <p className="type-hint text-[var(--color-text-primary)]">{updateAvailableCopy(hostLabel, check.current, check.latest)}</p>
      </Callout>,
    )
  }
  // B4: a newer version exists but there's no build for this host's
  // platform — DISTINCT from "up to date" (available is false but the host
  // is behind). Only when not already in/after a job.
  if (check?.newerNoArtifact && idle) {
    below.push(
      <Callout key="no-artifact" tone="warn">
        <p className="type-hint">{newerNoArtifactCopy(hostLabel, check.latest, check.platform)}</p>
      </Callout>,
    )
  }
  if (check?.newerViaPackage && idle) {
    below.push(
      <Callout key="via-package" tone="warn">
        <p className="type-hint">{newerViaPackageCopy(hostLabel, check.latest)}</p>
      </Callout>,
    )
  }
  // Comeback verdict (0.46.3): ok only when the host reported the new
  // version (or its job said done); error with the host's reason on a real
  // failure; warn "couldn't confirm" when there was no verdict in time —
  // never a false failure.
  if (comeback?.kind === 'success' && idle) {
    below.push(
      <Callout key="success" tone="ok" role="status">
        <p className="type-hint">{updateSucceededCopy(hostLabel, comeback.version)}</p>
      </Callout>,
    )
  }
  if (comeback?.kind === 'unconfirmed' && idle) {
    below.push(
      <Callout key="unconfirmed" tone="warn" role="status">
        <p className="type-hint">{updateUnconfirmedCopy(hostLabel, comeback.reason)}</p>
      </Callout>,
    )
  }
  if (comeback?.kind === 'failed' && idle) {
    below.push(
      <Callout key="failed" tone="error" role="alert">
        <p className="type-hint">{updateFailedCopy(hostLabel, comeback.reason)}</p>
      </Callout>,
    )
  }
  // In-flight phase + download progress.
  if ((inProgress || staged) && (phase || hostGoneInstalling)) {
    below.push(
      <div key="phase" role="status" className="space-y-1.5">
        <p className="type-hint text-[var(--color-text-secondary)]">
          {hostGoneInstalling || !phase
            ? watchLine
            : updatePhaseCopy(phase, hostLabel, {
                // The daemon reports 0.0–1.0; the copy wants a percent.
                progress: progressPct,
                current: check?.current,
              })}
        </p>
        {!hostGoneInstalling && phase === 'downloading' && progressPct !== undefined && (
          <ProgressBar value={progressPct} label={`Downloading the update for ${hostLabel}`} />
        )}
      </div>,
    )
  }
  // The job can't be shown as a phase: an unreadable status reply, or the
  // host no longer has the downloaded update.
  if (jobNote && !hostGoneInstalling) {
    below.push(
      <Callout key="job-note" tone="error" role="alert">
        <p className="type-hint break-words">{jobNote}</p>
      </Callout>,
    )
  }
  // Failure surface — host-named, host stays on its current version.
  if (!hostGoneInstalling && isFailurePhase(phase) && phase) {
    below.push(
      <Callout key="failure" tone="error" role="alert">
        <p className="type-hint">{updatePhaseCopy(phase, hostLabel, { current: check?.current })}</p>
        {status?.error && <p className="mt-1 type-hint break-all">{status.error}</p>}
      </Callout>,
    )
  }

  return (
    <SettingRow
      label="Update"
      settingId="general.update-host"
      hint={
        // B1: the host's CURRENT version after ANY successful check, for
        // both up-to-date and update-available states. Hidden once a job is
        // downloading/staging (the phase line below takes over).
        check && idle ? hostVersionCopy(hostLabel, check.current) : `Check whether a newer K2 is ready for ${hostLabel}.`
      }
      below={below.length > 0 ? <div className="space-y-2">{below}</div> : undefined}
    >
      {control}
    </SettingRow>
  )
}
