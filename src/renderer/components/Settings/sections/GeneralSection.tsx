import { OwnerTimezoneRow } from './OwnerTimezoneRow'
import React from 'react'
import { useCallback, useEffect, useRef, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import { useConnectHostStore } from '@/stores/connect-host'
import { useServerSupports, featureMinVersion } from '@/lib/server-capabilities'
import { useConfirmDialogStore } from '@/stores/confirm-dialog'
import { useToastStore } from '@/stores/toast'
import { ProgressiveHeaderBlur, Toggle, Tag } from '@/components/ui'
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
import { showContextMenu } from '@/lib/context-menu'
import {
  autoUpdateChecksEnabled,
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
import { GitToolsRow } from '@/components/GitTools/GitToolsNotice'

export const GENERAL_MANIFEST: SettingEntry[] = [
  { id: 'general.app-version', section: 'general', group: 'General', label: 'App version', description: 'K2 version and updates. Right-click the version to turn automatic checks on or off.', keywords: ['update', 'version', 'check', 'release', 'automatic', 'disable'] },
  { id: 'general.cli-version', section: 'general', group: 'General', label: 'CLI version', description: 'The installed k2 command-line tool, with install and update', keywords: ['k2so', 'cli', 'terminal', 'install', 'update', 'path'] },
  { id: 'general.git-tools', section: 'general', group: 'General', label: 'Git', description: "Whether git works on this server. On a Mac without Apple's command line tools, install them in one click (no Xcode needed)", keywords: ['git', 'xcode', 'xcode-select', 'command line tools', 'clt', 'developer tools', 'install', 'apple', 'homebrew', 'apt'] },
  { id: 'general.start-page', section: 'general', group: 'General', label: 'Start page', description: 'The page K2 opens on: Home or Agents (this computer only)', keywords: ['start', 'startup', 'launch', 'open', 'default page', 'home', 'agents', 'workspaces', 'landing'] },
  { id: 'general.font-family', section: 'general', group: 'General', label: 'Font family', description: 'The app font; same setting as Editors / Fonts → Code', keywords: ['font', 'typeface', 'family', 'text', 'monospace'] },
  { id: 'general.font-size', section: 'general', group: 'General', label: 'Font size', description: 'Text size for code, the message box, chats and tickets; same setting as Editors / Fonts → Code', keywords: ['font', 'size', 'text size', 'zoom', 'bigger', 'smaller'] },
  { id: 'general.link-copy-button', section: 'general', group: 'General', label: 'Show a copy button beside links on hover', description: 'A copy icon beside links in chats, Threads and tickets; off by default; this computer only. Right-click a link to copy it either way', keywords: ['copy', 'link', 'url', 'hover', 'button', 'icon', 'chat', 'thread', 'clipboard'] },
  { id: 'general.devtools', section: 'general', group: 'General', label: 'Developer tools', description: 'Open the web inspector for this window', keywords: ['devtools', 'developer', 'console', 'inspect', 'debug', 'chromium', 'network', 'logs'] },
  { id: 'general.your-account', section: 'general', group: 'General', label: 'Your account', description: 'The login on your phone or laptop that counts as you', keywords: ['account', 'login', 'owner', 'you', 'me', 'this is me', 'linked', 'phone', 'laptop', 'connect', 'identity'] },
  { id: 'general.config-location', section: 'general', group: 'General', label: 'Config location', description: '~/.k2/settings.json', keywords: ['settings', 'config', 'location', 'path'] },
  { id: 'general.reset-all', section: 'general', group: 'General', label: 'Reset all settings', description: 'Reset every setting to its default', keywords: ['reset', 'defaults', 'factory'] },
  { id: 'general.daemon', section: 'general', group: 'Server', label: 'K2 server', description: 'Background service that keeps agents running when the app is closed', keywords: ['server', 'daemon', 'background', 'launchd', 'persistent', 'lid', 'sleep', 'wake', 'agent'] },
  { id: 'general.keep-daemon-on-quit', section: 'general', group: 'Server', label: 'Keep server running when the window is closed', description: 'On: the red close button hides the window and keeps agents and K2 Companion running. Off: it stops everything. Cmd+Q always closes everything.', keywords: ['daemon', 'server', 'agent', 'companion', 'close', 'red button', 'window', 'hide', 'background', 'persistent'] },
  { id: 'general.agents-survive-restarts', section: 'general', group: 'Server', label: 'Agents keep running through updates and restarts', description: 'On by default: agent sessions keep running when K2 updates, restarts or crashes; quitting the server still stops them. macOS and Linux.', keywords: ['agents', 'survive', 'restart', 'update', 'crash', 'keep', 'running', 'ptyd', 'session', 'terminal'] },
  { id: 'general.clone-personal-computers', section: 'general', group: 'Server', label: "Allow cloning agents to people's personal computers", description: 'When off, Admins and Members cannot clone agents from this server to their own computers. Owner only.', keywords: ['clone', 'copy', 'agent', 'workspace', 'personal', 'computer', 'pull', 'permission', 'migrate'] },
  { id: 'general.restart-host', section: 'general', group: 'Server', label: 'Restart connected server', description: 'Restart the server you are connected to, not this computer', keywords: ['restart', 'reboot', 'remote', 'host', 'connect', 'server', 'daemon', 'bounce'] },
  { id: 'general.update-host', section: 'general', group: 'Server', label: 'Update connected server', description: 'Update the server you are connected to, not this computer', keywords: ['update', 'upgrade', 'remote', 'host', 'connect', 'server', 'daemon', 'version'] },
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

export function GeneralSection(): React.JSX.Element {
  const resetAllSettings = useSettingsStore((s) => s.resetAllSettings)
  const generalSubTab = useSettingsStore((s) => s.generalSubTab)
  // B2: when connected to a REMOTE host, the left "App Version" row is THIS
  // Mac's local Tauri app version — NOT the host's. Label it "This Mac" so
  // it can't be mistaken for the host version (the host version lives in the
  // right-pane UpdateHostRow). `activeHost === 'local'` ⇒ no badge.
  const activeHost = useConnectHostStore((s) => s.activeHost)
  const isRemote = activeHost !== 'local'
  const [tab, setTab] = useState<GeneralSubTab>('general')
  const [confirming, setConfirming] = useState(false)

  // Deep-link / search: a one-shot sub-tab (e.g. a Server row's result).
  useEffect(() => {
    if (!generalSubTab) return
    setTab(generalSubTab)
    useSettingsStore.setState({ generalSubTab: null })
  }, [generalSubTab])
  const [currentVersion, setCurrentVersion] = useState<string>('')
  const updateStatus = useUpdateStore((s) => s.status)
  const updateVersion = useUpdateStore((s) => s.version)
  const updateProgress = useUpdateStore((s) => s.progress)
  const updateError = useUpdateStore((s) => s.error)
  const updatePackageUrl = useUpdateStore((s) => s.archDownloadUrl)
  const linuxCanInstall = useUpdateStore((s) => s.linuxCanInstall)
  const linuxInstalling = useUpdateStore((s) => s.linuxInstalling)
  const linuxManualCommand = useUpdateStore((s) => s.linuxManualCommand)

  // Load current version on mount
  useEffect(() => {
    invoke<string>('get_current_version').then(setCurrentVersion).catch((e) => console.warn('[settings]', e))
  }, [])

  const handleCheckUpdate = useCallback(async () => {
    await checkForUpdate(true)
  }, [])

  const handleVersionContextMenu = useCallback((e: React.MouseEvent) => {
    e.preventDefault()
    e.stopPropagation()
    const enabled = autoUpdateChecksEnabled()
    void showContextMenu([
      {
        id: 'auto-update-checks',
        label: 'Automatic update checks',
        checked: enabled,
      },
    ]).then((id) => {
      if (id === 'auto-update-checks') {
        setAutoUpdateChecksEnabled(!enabled)
      }
    })
  }, [])

  // Auto-check for updates when navigated here from the update toast
  useEffect(() => {
    if (useSettingsStore.getState().pendingUpdateCheck) {
      useSettingsStore.setState({ pendingUpdateCheck: false })
      handleCheckUpdate()
    }
  }, [handleCheckUpdate])

  // Chrome (title + tabs) spans the full General pane; the body is
  // half-width forms. Remote hosts: this pane is the left half;
  // Restart/Update live on the right (see Settings.tsx + GeneralRemoteHostPanel).

  return (
    <div className="w-full h-full flex flex-col min-h-0">
      {/* Full-width chrome */}
      <div className="flex-shrink-0 px-6 pt-6">
        <h2 className="type-title text-[var(--color-text-primary)] mb-3">General</h2>

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
                className={`px-3 py-2 type-control transition-colors no-drag cursor-pointer border-b-2 -mb-px inline-flex items-center gap-1.5 ${
                  active
                    ? 'border-[var(--color-accent)] text-[var(--color-text-primary)]'
                    : 'border-transparent text-[var(--color-text-muted)] hover:text-[var(--color-text-secondary)]'
                }`}
              >
                {t.label}
              </button>
            )
          })}
        </div>
      </div>

      {/* Tab body: half-width forms */}
      <div className="relative flex-1 min-h-0 w-full max-w-xl">
      <ProgressiveHeaderBlur />
      <div className="h-full overflow-y-auto px-6 pt-4 pb-8 [scrollbar-gutter:stable] w-full">
      {tab === 'general' && (
      <div className="space-y-4">
        {/* Version & Update */}
        <div className="flex items-center justify-between py-2 border-b border-[var(--color-border)]" data-settings-id="general.app-version">
          <span className="flex items-center gap-2 min-w-0">
            <span className="type-row text-[var(--color-text-secondary)]">K2 by Alakazam Labs</span>
            {isRemote && (
              <Tag
                tone="warn"
                title="The K2 version on this computer. The connected server's version is under Connected server."
              >
                This computer
              </Tag>
            )}
          </span>
          <div className="flex items-center gap-3">
            <div className="flex items-center gap-1.5">
              <span
                className="w-1.5 h-1.5 flex-shrink-0"
                style={{ backgroundColor: updateStatus === 'available' ? 'var(--color-status-warn-soft)' : 'var(--color-status-ok-soft)' }}
              />
              <span
                className="type-row text-[var(--color-text-muted)] no-drag cursor-default"
                data-testid="app-version-label"
                title="Right-click to change automatic update checks"
                onContextMenu={handleVersionContextMenu}
              >
                v{currentVersion || '...'}
              </span>
            </div>
            {/* Hosted web: edge delivers versioned SPA — no in-app Tauri updater. */}
            {webFeatures.appUpdater && updateStatus === 'idle' && (
              <button
                onClick={handleCheckUpdate}
                className="px-2 py-0.5 type-control text-[var(--color-text-muted)] border border-[var(--color-border)] hover:text-[var(--color-text-primary)] hover:border-[var(--color-text-muted)] transition-colors no-drag cursor-pointer"
              >
                Check for updates
              </button>
            )}
            {webFeatures.appUpdater && updateStatus === 'checking' && (
              <span className="type-hint text-[var(--color-text-muted)]">Checking...</span>
            )}
          </div>
        </div>

        {/* Update available — desktop Tauri updater only */}
        {webFeatures.appUpdater && updateStatus === 'available' && updateVersion && (
          <div className="flex items-center justify-between p-3 bg-[var(--color-accent)]/10 border border-[var(--color-accent)]/30">
            <div>
              <p className="type-row text-[var(--color-text-primary)]">K2 v{updateVersion} is available</p>
              <p className="type-hint text-[var(--color-text-muted)] mt-0.5">You&apos;re on v{currentVersion}</p>
              {updatePackageUrl && linuxCanInstall && (
                <p className="type-hint text-[var(--color-text-muted)] mt-0.5" data-testid="linux-package-update-hint">
                  K2 downloads the package, verifies it, and asks for your password to install it. K2 then restarts.
                </p>
              )}
              {updatePackageUrl && !linuxCanInstall && (
                <p className="type-hint text-[var(--color-text-muted)] mt-0.5" data-testid="linux-package-update-hint">
                  Updates on Linux come from your package (deb, rpm or Arch). Download opens the new version on GitHub releases.
                </p>
              )}
            </div>
            <button
              className="px-3 py-1 type-control bg-[var(--color-accent)] text-[var(--color-on-accent)] hover:bg-[var(--color-accent)]/90 transition-colors no-drag cursor-pointer"
              onClick={() => void useUpdateStore.getState().openAvailableDownload()}
            >
              {updatePackageUrl && linuxCanInstall ? 'Install & relaunch' : 'Download'}
            </button>
          </div>
        )}

        {/* Downloading */}
        {webFeatures.appUpdater && updateStatus === 'downloading' && linuxInstalling && (
          <div className="p-3 border border-[var(--color-border)]" data-testid="linux-package-installing">
            <p className="type-row text-[var(--color-text-primary)]">Installing v{updateVersion}...</p>
            <p className="type-hint text-[var(--color-text-muted)] mt-0.5">
              Downloading and checking the package. K2 will ask for your password, then restart.
            </p>
          </div>
        )}
        {webFeatures.appUpdater && updateStatus === 'downloading' && !linuxInstalling && (
          <div className="p-3 border border-[var(--color-border)]">
            <div className="flex items-center justify-between mb-2">
              <span className="type-row text-[var(--color-text-primary)]">Downloading v{updateVersion}...</span>
              <span className="type-hint tabular-nums text-[var(--color-text-muted)]">{updateProgress}%</span>
            </div>
            <div className="h-1.5 bg-[var(--color-border)] overflow-hidden">
              <div
                className="h-full bg-[var(--color-accent)] transition-all duration-300"
                style={{ width: `${updateProgress}%` }}
              />
            </div>
          </div>
        )}

        {/* Ready to install */}
        {webFeatures.appUpdater && updateStatus === 'ready' && (
          <div className="flex items-center justify-between p-3 bg-[color-mix(in_srgb,var(--color-status-ok)_10%,transparent)] border border-[color-mix(in_srgb,var(--color-status-ok)_30%,transparent)]">
            <div>
              <p className="type-row text-[var(--color-text-primary)]">v{updateVersion} is ready to install</p>
              <p className="type-hint text-[var(--color-text-muted)] mt-0.5">The app will restart after installation</p>
            </div>
            <button
              className="px-3 py-1 type-control bg-[var(--color-status-ok)] text-[var(--color-on-accent)] hover:bg-[var(--color-status-ok-hover)] transition-colors no-drag cursor-pointer"
              onClick={() => useUpdateStore.getState().installAndRelaunch()}
            >
              Install & relaunch
            </button>
          </div>
        )}

        {/* Error */}
        {webFeatures.appUpdater && updateStatus === 'error' && (
          <div className="p-3 border border-[color-mix(in_srgb,var(--color-status-error)_30%,transparent)] bg-[color-mix(in_srgb,var(--color-status-error)_5%,transparent)]">
            <p className="type-hint text-[var(--color-status-error-soft)]">{updateError}</p>
            {linuxManualCommand && (
              <div className="mt-2 flex items-start gap-2" data-testid="linux-package-manual-command">
                <code className="flex-1 min-w-0 break-all select-text type-hint p-1.5 bg-[var(--color-bg-elevated)] border border-[var(--color-border)] text-[var(--color-text-primary)]">
                  {linuxManualCommand}
                </code>
                <button
                  className="px-2 py-0.5 type-control text-[var(--color-text-muted)] border border-[var(--color-border)] hover:text-[var(--color-text-primary)] transition-colors no-drag cursor-pointer"
                  onClick={() => void navigator.clipboard?.writeText(linuxManualCommand)}
                >
                  Copy
                </button>
              </div>
            )}
            <div className="flex items-center gap-2 mt-2">
              <button
                className="px-2 py-0.5 type-control text-[var(--color-text-muted)] border border-[var(--color-border)] hover:text-[var(--color-text-primary)] transition-colors no-drag cursor-pointer"
                onClick={handleCheckUpdate}
              >
                Retry
              </button>
              <button
                className="px-2 py-0.5 type-control text-[var(--color-accent)] border border-[var(--color-accent)]/30 hover:bg-[var(--color-accent)]/10 transition-colors no-drag cursor-pointer"
                onClick={() => {
                  const tag = updateVersion ? `v${updateVersion}` : 'latest'
                  invoke('plugin:opener|open_url', { url: `https://github.com/Alakazam-211/K2/releases/tag/${tag}` }).catch(() => {
                    window.open(`https://github.com/Alakazam-211/K2/releases/tag/${tag}`)
                  })
                }}
              >
                Download
              </button>
            </div>
          </div>
        )}

        {/* CLI Version — right under App Version so it feels like part of the app */}
        <CLIVersionRow />

        {/* Git (0.46.4): the server's git, and on a Mac without Apple's
            command line tools the one-click install. Daemon-owned state. */}
        <GitToolsRow />

        <StartPageRow />

        {/* App font (0.45.7): the same setting as Editors / Fonts → Code.
            Sits below the app and CLI versions (Rosson 2026-10-16). */}
        <div data-settings-card="" className="border border-[var(--color-border)]" data-settings-id="general.font">
          <AppFontRows where="general" lastHasBorder={false} />
        </div>

        {/* Hover copy icon beside chat links (Rosson 2026-11-04): next to
            the font rows, off by default, this computer only. */}
        <ChatLinkCopyRow />

        {/* Desktop only: open Chromium DevTools (prod + debug) for diagnosing
            remote freezes / network. Pref is thin-client localStorage. */}
        {webFeatures.appDevTools && <DevToolsRow />}

        {/* Re-open the "What's new" popup for the current version. Pairs
            with the auto-show on first launch after an update (0.38.7). */}
        <WhatsNewRow />

        {/* You (0.46.0, Rosson 2026-10-17): your Connect account (it was the
            "This is me (owner)" checkbox on Admin Access). The separate
            "Your name" is gone: the owner's name is the picked account's
            first and last name (profile modal, Admin Access). */}
        <div data-settings-group="you" data-testid="general-you-group">
          <div className="pb-2">
            <OwnerTimezoneRow />
          </div>
          <h3 className="type-section uppercase tracking-wider text-[var(--color-text-muted)] m-0 mb-1">You</h3>
          <YourAccountRow />
        </div>

        <div className="flex items-center justify-between py-2 border-b border-[var(--color-border)]" data-settings-id="general.config-location">
          <span className="type-row text-[var(--color-text-secondary)]">Config location</span>
          <span className="type-row text-[var(--color-text-muted)]">~/.k2/settings.json</span>
        </div>

        <div className="pt-4" data-settings-id="general.reset-all">
          {confirming ? (
            <div className="flex items-center gap-2">
              <span className="type-row text-[var(--color-status-error-soft)]">Reset all settings to defaults?</span>
              <button
                onClick={() => {
                  resetAllSettings()
                  setConfirming(false)
                }}
                className="px-3 py-1 type-control bg-[color-mix(in_srgb,var(--color-status-error)_20%,transparent)] text-[var(--color-status-error-soft)] border border-[color-mix(in_srgb,var(--color-status-error)_40%,transparent)] hover:bg-[color-mix(in_srgb,var(--color-status-error)_30%,transparent)] no-drag cursor-pointer"
              >
                Reset
              </button>
              <button
                onClick={() => setConfirming(false)}
                className="px-3 py-1 type-control text-[var(--color-text-muted)] border border-[var(--color-border)] hover:text-[var(--color-text-primary)] no-drag cursor-pointer"
              >
                Cancel
              </button>
            </div>
          ) : (
            <button
              onClick={() => setConfirming(true)}
              className="px-3 py-1 type-control text-[var(--color-text-secondary)] border border-[var(--color-border)] hover:text-[var(--color-text-primary)] hover:border-[var(--color-text-muted)] no-drag cursor-pointer"
            >
              Reset all settings
            </button>
          )}
        </div>
      </div>
      )}

      {tab === 'server' && (
        <div className="space-y-4">
          <p className="type-hint text-[var(--color-text-muted)] leading-relaxed -mt-1">
            The K2 server on this computer.
          </p>
          <DaemonRow />
          <KeepDaemonOnQuitRow />
          <AgentsSurviveRestartsRow />
          <ClonePersonalComputersRow scope={primaryScope()} />
        </div>
      )}

      </div>
      </div>
    </div>
  )
}

// Right-pane companion to the General section, rendered by the Settings shell
// ONLY when connected to a remote host (half/half split, full-height divider —
// see Settings.tsx). Holds the host-only controls: Restart (#661) + Update
// (P4 / Shape A). Never mistakable for "this Mac" — local update lives in the
// App Version row, local restart in the K2 Server row, both in the left pane.
export function GeneralRemoteHostPanel(): React.JSX.Element {
  return (
    <div className="w-full">
      <h2 className="type-title text-[var(--color-text-primary)] mb-4">Connected host</h2>
      <div className="space-y-4">
        <RestartHostRow />
        <UpdateHostRow />
      </div>
    </div>
  )
}

// ── Composer 1c (D4) — "Allow remote instruct" ─────────────────────────
// MOVED to Settings → K2 Connect (shared/AllowRemoteInstructRow), beneath
// the "Enable federation" master. Since 2026-10-16 it is "Let agents on
// other servers message every agent" and covers federated agents only;
// people who log in can always message agents.

function CLIVersionRow(): React.JSX.Element {
  const [status, setStatus] = useState<{
    installed: boolean
    installedVersion: string | null
    bundledVersion: string | null
    updateAvailable: boolean
    packageManaged?: boolean
  } | null>(null)
  const [loading, setLoading] = useState(false)
  const [checking, setChecking] = useState(false)

  const checkStatus = useCallback(async () => {
    try {
      const result = await invoke<{
        installed: boolean
        installedVersion: string | null
        bundledVersion: string | null
        updateAvailable: boolean
        packageManaged?: boolean
      }>('cli_install_status')
      setStatus(result)
    } catch {
      // silently fail
    }
  }, [])

  useEffect(() => { checkStatus() }, [checkStatus])

  const handleInstallOrUpdate = useCallback(async () => {
    setLoading(true)
    try {
      await invoke('cli_install')
      await checkStatus()
    } catch (err) {
      console.error('[cli]', err)
    } finally {
      setLoading(false)
    }
  }, [checkStatus])

  const handleCheckForUpdates = useCallback(async () => {
    setChecking(true)
    try {
      await checkStatus()
    } finally {
      setChecking(false)
    }
  }, [checkStatus])

  // Compare versions properly — only show update if bundled is actually newer
  const compareVersions = (a: string, b: string): number => {
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
  const updateAvailable = !status?.packageManaged && status?.installed && status.bundledVersion && status.installedVersion
    && compareVersions(status.bundledVersion, status.installedVersion) > 0

  return (
    <div className="flex items-center justify-between py-2 border-b border-[var(--color-border)]">
      <span className="type-row text-[var(--color-text-secondary)]">CLI Version</span>
      <div className="flex items-center gap-3">
        {status?.installed ? (
          <>
            <div className="flex items-center gap-1.5">
              <span
                className="w-1.5 h-1.5 flex-shrink-0"
                style={{ backgroundColor: updateAvailable ? 'var(--color-status-warn-soft)' : 'var(--color-status-ok-soft)' }}
              />
              <span className="type-row text-[var(--color-text-muted)]">
                v{status.installedVersion || '?'}
              </span>
            </div>
            {updateAvailable ? (
              <button
                onClick={handleInstallOrUpdate}
                disabled={loading}
                className="px-2 py-0.5 type-control bg-[var(--color-accent)] text-[var(--color-on-accent)] hover:opacity-90 transition-opacity no-drag cursor-pointer disabled:opacity-50"
              >
                {loading ? 'Updating...' : `Update to v${status.bundledVersion}`}
              </button>
            ) : (
              <button
                onClick={handleCheckForUpdates}
                disabled={checking}
                className="px-2 py-0.5 type-control text-[var(--color-text-muted)] border border-[var(--color-border)] hover:text-[var(--color-text-primary)] hover:border-[var(--color-text-muted)] transition-colors no-drag cursor-pointer disabled:opacity-50"
              >
                {checking ? 'Checking...' : 'Check for updates'}
              </button>
            )}
          </>
        ) : (
          <>
            <span className="type-row text-[var(--color-text-muted)]">Not installed</span>
            <button
              onClick={handleInstallOrUpdate}
              disabled={loading}
              className="px-2 py-0.5 type-control bg-[var(--color-accent)] text-[var(--color-on-accent)] hover:opacity-90 transition-opacity no-drag cursor-pointer disabled:opacity-50"
            >
              {loading ? 'Installing...' : 'Install'}
            </button>
          </>
        )}
      </div>
    </div>
  )
}

// ── Developer tools (desktop) ──────────────────────────────────────────
// Thin-client only: preference lives in localStorage (`k2:enable-devtools`),
// NOT daemon settings.json. Toggle remembers intent; "Open DevTools" invokes
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
        .addToast(e instanceof Error ? e.message : String(e), 'error')
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
    <div
      className="flex items-center justify-between py-2 border-b border-[var(--color-border)]"
      data-settings-id="general.devtools"
    >
      <div className="flex-1 min-w-0 mr-3">
        <span className="type-row text-[var(--color-text-secondary)]">
          Developer tools
        </span>
        <p className="type-hint text-[var(--color-text-muted)] mt-0.5">
          Opens the web inspector for this window, for troubleshooting. On this computer only.
        </p>
      </div>
      <div className="flex items-center gap-2 flex-shrink-0">
        {enabled && (
          <button
            type="button"
            onClick={() => void openDevTools()}
            disabled={opening}
            className="px-2 py-0.5 type-control text-[var(--color-text-muted)] border border-[var(--color-border)] hover:text-[var(--color-text-primary)] hover:border-[var(--color-text-muted)] transition-colors no-drag cursor-pointer disabled:opacity-50"
          >
            {opening ? 'Opening…' : 'Open inspector'}
          </button>
        )}
        <Toggle
          checked={enabled}
          onChange={toggle}
          aria-label="Enable developer tools"
        />
      </div>
    </div>
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
    <div className="flex items-center justify-between py-2 border-b border-[var(--color-border)]">
      <span className="type-row text-[var(--color-text-secondary)]">Release notes</span>
      <button
        type="button"
        onClick={handleClick}
        className="px-2 py-0.5 type-control text-[var(--color-text-muted)] border border-[var(--color-border)] hover:text-[var(--color-text-primary)] hover:border-[var(--color-text-muted)] transition-colors no-drag cursor-pointer"
      >
        Read what's new
      </button>
    </div>
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

  const refresh = useCallback(async () => {
    try {
      const result = await invoke<DaemonStatusState>('daemon_status')
      setStatus(result)
      setError(null)
    } catch (e) {
      setError(String(e))
    }
  }, [])

  // Refresh on mount + every 4s while the row is visible. Cheap —
  // daemon_status is just a file read + an HTTP ping on localhost.
  useEffect(() => {
    refresh()
    const id = window.setInterval(refresh, 4000)
    return () => window.clearInterval(id)
  }, [refresh])

  const handleInstall = useCallback(async () => {
    setBusy('install')
    setError(null)
    try {
      await invoke<string>('daemon_install')
      await refresh()
    } catch (e) {
      setError(String(e))
    } finally {
      setBusy(null)
    }
  }, [refresh])

  const handleRestart = useCallback(async () => {
    setBusy('restart')
    setError(null)
    try {
      await invoke('daemon_restart')
      // Give launchd a moment to respawn before we query again.
      await new Promise((r) => setTimeout(r, 1200))
      await refresh()
    } catch (e) {
      setError(String(e))
    } finally {
      setBusy(null)
    }
  }, [refresh])

  const handleViewLog = useCallback(async () => {
    try {
      const text = await invoke<string>('daemon_log_tail', { lines: 300 })
      setLogText(text)
      setShowingLog(true)
    } catch (e) {
      setError(String(e))
    }
  }, [])

  const dotColor = (() => {
    if (!status) return 'var(--color-neutral)'                          // loading
    if (status.state === 'running') return 'var(--color-status-ok-soft)'       // green
    if (status.state === 'not_installed') return 'var(--color-neutral)' // neutral
    return 'var(--color-status-warn-soft)'                                        // yellow (unreachable)
  })()

  const statusText = (() => {
    if (!status) return 'Loading...'
    if (status.state === 'running') return 'Running'
    if (status.state === 'not_installed') return 'Not installed'
    return 'Installed but unreachable'
  })()

  const runtimeText =
    status?.state === 'running'
      ? `PID ${status.pid}, up ${formatUptime(status.uptime_secs)}`
      : null

  return (
    <div className="py-2 border-b border-[var(--color-border)]">
      <div className="flex items-center justify-between gap-4">
        <div className="flex flex-col min-w-0 flex-1">
          <span className="type-row text-[var(--color-text-secondary)]">K2 server</span>
          <span className="type-hint text-[var(--color-text-muted)] mt-0.5">
            Keeps agents, terminals, heartbeats and K2 Companion running when the app is closed
          </span>
        </div>
        <div className="flex items-center gap-3 flex-shrink-0">
          <div className="flex items-center gap-1.5">
            <span className="w-1.5 h-1.5 flex-shrink-0" style={{ backgroundColor: dotColor }} />
            <span className="type-row text-[var(--color-text-muted)]">{statusText}</span>
          </div>
          {status?.state === 'not_installed' && (
            <button
              onClick={handleInstall}
              disabled={busy !== null}
              className="px-2 py-0.5 type-control bg-[var(--color-accent)] text-[var(--color-on-accent)] hover:opacity-90 transition-opacity no-drag cursor-pointer disabled:opacity-50"
            >
              {busy === 'install' ? 'Installing...' : 'Install'}
            </button>
          )}
          {status?.state === 'running' && (
            <button
              onClick={handleRestart}
              disabled={busy !== null}
              className="px-2 py-0.5 type-control text-[var(--color-text-muted)] border border-[var(--color-border)] hover:text-[var(--color-text-primary)] hover:border-[var(--color-text-muted)] transition-colors no-drag cursor-pointer disabled:opacity-50"
            >
              {busy === 'restart' ? 'Restarting...' : 'Restart'}
            </button>
          )}
          {status?.state === 'unreachable' && (
            <button
              onClick={handleRestart}
              disabled={busy !== null}
              className="px-2 py-0.5 type-control bg-[color-mix(in_srgb,var(--color-status-warn-soft)_20%,transparent)] text-[var(--color-status-warn-text)] border border-[color-mix(in_srgb,var(--color-status-warn-soft)_30%,transparent)] hover:bg-[color-mix(in_srgb,var(--color-status-warn-soft)_30%,transparent)] transition-colors no-drag cursor-pointer disabled:opacity-50"
            >
              {busy === 'restart' ? 'Restarting...' : 'Restart'}
            </button>
          )}
        </div>
      </div>

      {/* Secondary row — View log + runtime details when installed */}
      {status && status.state !== 'not_installed' && (
        <div className="flex items-center gap-3 mt-1.5 pl-0">
          <button
            onClick={handleViewLog}
            className="type-hint text-[var(--color-text-muted)] underline hover:text-[var(--color-text-primary)] transition-colors no-drag cursor-pointer"
          >
            View log
          </button>
          {runtimeText && (
            <span className="type-hint text-[var(--color-text-muted)]">{runtimeText}</span>
          )}
        </div>
      )}

      {/* Inline log viewer — appears when "View log" is clicked */}
      {showingLog && (
        <div className="mt-2 p-2 bg-black/30 border border-[var(--color-border)] max-h-60 overflow-auto">
          <div className="flex items-center justify-between mb-1">
            <span className="type-hint text-[var(--color-text-muted)]">~/.k2/daemon.stdout.log (last 300 lines)</span>
            <button
              onClick={() => setShowingLog(false)}
              className="type-hint text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] no-drag cursor-pointer"
            >
              Close
            </button>
          </div>
          <pre className="type-hint text-[var(--color-text-muted)] whitespace-pre-wrap font-mono leading-tight">
            {logText || '(log file empty or missing)'}
          </pre>
        </div>
      )}

      {/* Error surface — visible until the next action clears it */}
      {error && (
        <div className="mt-2 p-2 bg-[color-mix(in_srgb,var(--color-status-error)_5%,transparent)] border border-[color-mix(in_srgb,var(--color-status-error)_30%,transparent)]">
          <p className="type-hint text-[var(--color-status-error-soft)] break-all">{error}</p>
        </div>
      )}
    </div>
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

function AgentsSurviveRestartsRow(): React.JSX.Element {
  const [on, setOn] = useState<boolean>(true)

  useEffect(() => {
    settingsGet()
      .then((s) => setOn(s.terminal?.ptyd !== 'off'))
      .catch((e) => console.warn('[agents-survive-restarts]', e))
  }, [])

  const toggle = async (): Promise<void> => {
    const next = !on
    setOn(next) // optimistic
    try {
      // The daemon deep-merges `terminal`, so the font settings stay.
      await settingsUpdate({ terminal: { ptyd: next ? 'on' : 'off' } })
    } catch (e) {
      console.error('[agents-survive-restarts]', e)
      setOn(!next) // revert
    }
  }

  return (
    <div className="flex items-center justify-between py-2 border-b border-[var(--color-border)]">
      <div className="flex-1 min-w-0 mr-3">
        <span className="type-row text-[var(--color-text-secondary)]">
          Agents keep running through updates and restarts
        </span>
        <p className="type-hint text-[var(--color-text-muted)] mt-0.5">
          {on
            ? 'New agent sessions keep running when K2 updates, restarts or crashes, and pick up where they were. Quitting the server still stops them. Sessions already running before you turned this on stop at the next restart. macOS and Linux.'
            : 'Off: updating or restarting K2 stops running agents; they resume their conversation afterwards.'}
        </p>
      </div>
      <Toggle
        checked={on}
        onChange={() => void toggle()}
        aria-label="Agents keep running through updates and restarts"
      />
    </div>
  )
}

// ── Keep daemon on quit ───────────────────────────────────────────────
// Default is ON — persistent agents keep running past Cmd+Q. User can
// flip OFF to get "normal app" quit behavior (daemon stops with the
// window). Honored by:
//   - RunEvent::ExitRequested handler in src-tauri/src/lib.rs
//   - Menubar → Quit K2
// Both check the same setting via `get_keep_daemon_on_quit`.

function KeepDaemonOnQuitRow(): React.JSX.Element {
  // Default `true` so the UI renders immediately without a null gate;
  // the real value arrives in the next tick from invoke() and either
  // matches or flips.
  const [keep, setKeep] = useState<boolean>(true)

  useEffect(() => {
    // The old `get_keep_daemon_on_quit` command read the daemon's full
    // settings snapshot and pulled `keepDaemonOnQuit` (default true if
    // absent). Mirror that read here.
    settingsGet()
      .then((s) => setKeep(s.keepDaemonOnQuit ?? true))
      .catch((e) => console.warn('[keep-daemon-on-quit]', e))
  }, [])

  const toggle = async (): Promise<void> => {
    const next = !keep
    setKeep(next) // optimistic
    try {
      // Partial settings update — the daemon deep-merges `keepDaemonOnQuit`.
      // The old `set_keep_daemon_on_quit` command emitted NO cross-window
      // sync event, so we mirror that (no `sync:settings` emit here).
      await settingsUpdate({ keepDaemonOnQuit: next })
    } catch (e) {
      console.error('[keep-daemon-on-quit]', e)
      setKeep(!next) // revert
    }
  }

  // <Toggle> gives every switch in Settings the same physical footprint
  // (36x20 track, 16x16 thumb).
  return (
    <div className="flex items-center justify-between py-2 border-b border-[var(--color-border)]">
      <div className="flex-1 min-w-0 mr-3">
        <span className="type-row text-[var(--color-text-secondary)]">
          Keep server running when the window is closed
        </span>
        <p className="type-hint text-[var(--color-text-muted)] mt-0.5">
          {keep
            ? 'The red close button hides the window. Agents keep working and K2 Companion stays connected. The menu bar shows status. Cmd+Q closes everything.'
            : 'The red close button stops everything, same as Cmd+Q. Agents pause and K2 Companion disconnects until you reopen K2.'}
        </p>
      </div>
      <Toggle
        checked={keep}
        onChange={() => void toggle()}
        aria-label="Keep server running when the window is closed"
      />
    </div>
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
    <div
      className="py-2.5 px-3 border border-[color-mix(in_srgb,var(--color-status-warn)_40%,transparent)] bg-[color-mix(in_srgb,var(--color-status-warn)_5%,transparent)]"
      data-settings-id="general.restart-host"
    >
      <div className="flex items-center justify-between gap-4">
        <div className="flex flex-col min-w-0 flex-1">
          <span className="flex items-center gap-2 min-w-0">
            <Tag tone="warn">
              Connected server
            </Tag>
            <span className="type-name text-[var(--color-text-primary)] truncate">
              {hostLabel}
            </span>
            <span className="type-hint text-[var(--color-text-muted)] font-mono truncate">
              {hostname}
            </span>
          </span>
          <span className="type-hint text-[var(--color-text-muted)] mt-1 leading-relaxed">
            Restart the server you&apos;re connected to, <strong className="text-[var(--color-status-warn-amber-soft)]">not this computer</strong>.
            Sessions on it disconnect briefly, then reconnect automatically.
          </span>
        </div>
        <div className="flex-shrink-0">
          {restarting ? (
            <span className="type-hint text-[var(--color-status-warn-amber-soft)]">Restarting {hostLabel}…</span>
          ) : (
            <button
              onClick={() => void handleRestart()}
              disabled={!canRestart}
              title={gate.title}
              className="px-3 py-1 type-control text-[var(--color-status-warn-amber-bright)] bg-[color-mix(in_srgb,var(--color-status-warn)_15%,transparent)] border border-[color-mix(in_srgb,var(--color-status-warn)_40%,transparent)] hover:bg-[color-mix(in_srgb,var(--color-status-warn)_25%,transparent)] transition-colors no-drag cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed"
            >
              Restart {hostLabel}
            </button>
          )}
        </div>
      </div>
      {gate.note && (
        <div className="mt-2 type-hint text-[var(--color-text-muted)]">{gate.note}</div>
      )}
    </div>
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

  return (
    <div
      className="py-2.5 px-3 border border-[color-mix(in_srgb,var(--color-status-warn)_40%,transparent)] bg-[color-mix(in_srgb,var(--color-status-warn)_5%,transparent)]"
      data-settings-id="general.update-host"
    >
      <div className="flex items-center justify-between gap-4">
        <div className="flex flex-col min-w-0 flex-1">
          <span className="flex items-center gap-2 min-w-0">
            <Tag tone="warn">
              Connected server
            </Tag>
            <span className="type-name text-[var(--color-text-primary)] truncate">
              {hostLabel}
            </span>
            <span className="type-hint text-[var(--color-text-muted)] font-mono truncate">
              {hostname}
            </span>
          </span>
          <span className="type-hint text-[var(--color-text-muted)] mt-1 leading-relaxed">
            Update the server you&apos;re connected to, <strong className="text-[var(--color-status-warn-amber-soft)]">not this computer</strong>.
            It disconnects briefly to install, then reconnects automatically.
          </span>
        </div>
        <div className="flex-shrink-0">
          {/* Idle (no job, not staged): Check / Download / Install button */}
          {!inProgress && !staged && (
            check?.available ? (
              <button
                onClick={() => void handleDownload()}
                disabled={!canUpdate}
                title={gate.title}
                className="px-3 py-1 type-control text-[var(--color-status-warn-amber-bright)] bg-[color-mix(in_srgb,var(--color-status-warn)_15%,transparent)] border border-[color-mix(in_srgb,var(--color-status-warn)_40%,transparent)] hover:bg-[color-mix(in_srgb,var(--color-status-warn)_25%,transparent)] transition-colors no-drag cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed"
              >
                {/* Bundled-app hosts (Shape A) update in one shot — the Tauri
                    updater downloads + installs + relaunches, there's no
                    separate staged "Install & restart" step — so the button is
                    "Update Host", not "Download". Standalone (Shape B) hosts
                    keep the two-step Download → Install & restart flow. */}
                {check?.installKind === 'bundled-app' ? 'Update server' : 'Download'}
              </button>
            ) : (
              <button
                onClick={() => void handleCheck()}
                disabled={!canUpdate || checking}
                title={gate.title}
                className="px-3 py-1 type-control text-[var(--color-status-warn-amber-bright)] bg-[color-mix(in_srgb,var(--color-status-warn)_15%,transparent)] border border-[color-mix(in_srgb,var(--color-status-warn)_40%,transparent)] hover:bg-[color-mix(in_srgb,var(--color-status-warn)_25%,transparent)] transition-colors no-drag cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed"
              >
                {checking ? 'Checking…' : 'Check for updates'}
              </button>
            )
          )}
          {/* Staged: ready to install & restart the host */}
          {staged && (
            applying ? (
              <span className="type-hint text-[var(--color-status-warn-amber-soft)]">Installing & restarting {hostLabel}…</span>
            ) : (
              <button
                onClick={() => void handleApply()}
                disabled={!canUpdate}
                title={gate.title}
                className="px-3 py-1 type-control text-[var(--color-status-warn-amber-bright)] bg-[color-mix(in_srgb,var(--color-status-warn)_15%,transparent)] border border-[color-mix(in_srgb,var(--color-status-warn)_40%,transparent)] hover:bg-[color-mix(in_srgb,var(--color-status-warn)_25%,transparent)] transition-colors no-drag cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed"
              >
                Install & restart {hostLabel}
              </button>
            )
          )}
        </div>
      </div>

      {/* Why the buttons are off (a Member, an expired sign-in) or that the
          role couldn't be read yet — never "owner or admin" for an unread
          role (Rosson 2026-11-05). */}
      {gate.note && (
        <div className="mt-2 type-hint text-[var(--color-text-muted)]">{gate.note}</div>
      )}

      {/* B1: persistent host-version line — shown after ANY successful check,
          for BOTH up-to-date and update-available states, so the remote
          host's CURRENT version is always visible (not just in a toast or
          the update banner). Hidden once a job is downloading/staging (the
          phase line takes over) so the row doesn't double up. */}
      {check && !inProgress && !staged && (
        <div className="mt-2 type-hint text-[var(--color-text-muted)]">
          {hostVersionCopy(hostLabel, check.current)}
        </div>
      )}

      {/* Update-available banner once a check reports one (pre-download) */}
      {check?.available && !inProgress && !staged && (
        <div className="mt-1 type-hint text-[var(--color-status-warn-amber-bright)]">
          {updateAvailableCopy(hostLabel, check.current, check.latest)}
        </div>
      )}

      {/* B4: a newer version exists but there's no build for this host's
          platform — DISTINCT from "up to date" (available is false but the
          host is behind). Only when not already in/after a job. */}
      {check?.newerNoArtifact && !inProgress && !staged && (
        <div className="mt-1 type-hint text-[color-mix(in_srgb,var(--color-status-warn-amber-bright)_90%,transparent)]">
          {newerNoArtifactCopy(hostLabel, check.latest, check.platform)}
        </div>
      )}
      {check?.newerViaPackage && !inProgress && !staged && (
        <div className="mt-1 type-hint text-[color-mix(in_srgb,var(--color-status-warn-amber-bright)_90%,transparent)]">
          {newerViaPackageCopy(hostLabel, check.latest)}
        </div>
      )}

      {/* Comeback verdict (0.46.3): green only when the host reported the
          new version (or its job said done); red with the host's reason on
          a real failure; amber "couldn't confirm" when there was no verdict
          in time — never a false failure. */}
      {comeback?.kind === 'success' && !inProgress && !staged && (
        <div className="mt-2 type-hint text-[var(--color-status-ok-soft)]">
          {updateSucceededCopy(hostLabel, comeback.version)}
        </div>
      )}
      {comeback?.kind === 'unconfirmed' && !inProgress && !staged && (
        <div className="mt-2 type-hint text-[var(--color-status-warn-amber-bright)]">
          {updateUnconfirmedCopy(hostLabel, comeback.reason)}
        </div>
      )}
      {comeback?.kind === 'failed' && !inProgress && !staged && (
        <div className="mt-2 p-2 bg-[color-mix(in_srgb,var(--color-status-error)_5%,transparent)] border border-[color-mix(in_srgb,var(--color-status-error)_30%,transparent)]">
          <p className="type-hint text-[var(--color-status-error-soft)]">
            {updateFailedCopy(hostLabel, comeback.reason)}
          </p>
        </div>
      )}

      {/* In-flight phase + download progress line */}
      {(inProgress || staged) && (phase || hostGoneInstalling) && (
        <div className="mt-2">
          <div className="type-hint text-[var(--color-status-warn-amber-bright)]">
            {hostGoneInstalling || !phase
              ? watchLine
              : updatePhaseCopy(phase, hostLabel, {
                  // The daemon reports 0.0–1.0; the copy wants a percent.
                  progress: typeof status?.progress === 'number' ? status.progress * 100 : undefined,
                  current: check?.current,
                })}
          </div>
          {!hostGoneInstalling && phase === 'downloading' && typeof status?.progress === 'number' && (
            <div className="h-1.5 mt-1.5 bg-[color-mix(in_srgb,var(--color-status-warn)_20%,transparent)] overflow-hidden">
              <div
                className="h-full bg-[var(--color-status-warn-amber)] transition-all duration-300"
                style={{ width: `${Math.max(0, Math.min(100, status.progress * 100))}%` }}
              />
            </div>
          )}
        </div>
      )}

      {/* The job can't be shown as a phase: an unreadable status reply, or
          the host no longer has the downloaded update. */}
      {jobNote && !hostGoneInstalling && (
        <div className="mt-2 type-hint text-[var(--color-status-error-soft)] break-words">
          {jobNote}
        </div>
      )}

      {/* Failure surface — host-named, host stays on its current version */}
      {!hostGoneInstalling && isFailurePhase(phase) && phase && (
        <div className="mt-2 p-2 bg-[color-mix(in_srgb,var(--color-status-error)_5%,transparent)] border border-[color-mix(in_srgb,var(--color-status-error)_30%,transparent)]">
          <p className="type-hint text-[var(--color-status-error-soft)]">
            {updatePhaseCopy(phase, hostLabel, { current: check?.current })}
          </p>
          {status?.error && (
            <p className="mt-1 type-hint text-[var(--color-status-error-soft)] break-all">
              {status.error}
            </p>
          )}
        </div>
      )}
    </div>
  )
}
