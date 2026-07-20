import React from 'react'
import { useCallback, useEffect, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import { useConnectHostStore } from '@/stores/connect-host'
import { useServerSupports, featureMinVersion } from '@/lib/server-capabilities'
import { useConfirmDialogStore } from '@/stores/confirm-dialog'
import { useToastStore } from '@/stores/toast'
import { Toggle } from '@/components/ui'
import { restartHostVisibility, restartHostConfirmCopy, type RestartRole } from './restart-host'
import {
  updateHostVisibility,
  updateHostConfirmCopy,
  updateAvailableCopy,
  hostVersionCopy,
  newerNoArtifactCopy,
  updatePhaseCopy,
  updateCompleteCopy,
  updateSlowComebackCopy,
  shouldResolveComeback,
  updateForbiddenCopy,
  isForbiddenError,
  isStaged,
  isTerminalPhase,
  isFailurePhase,
  type UpdateRole,
  type UpdateCheckResult,
  type UpdateStatusResult,
  type UpdatePhase,
} from './update-host'
// Comeback watcher (0.40.48 remote-update ergonomics): after the update
// job reaches `restarting`, poll the host's PUBLIC /boot-status until it's
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
import { useSettingsStore, sanitizeOwnerDisplayName, OWNER_DISPLAY_NAME_MAX } from '@/stores/settings'
import { useUpdateStore } from '@/stores/update'
import { checkForUpdate } from '@/hooks/useUpdateChecker'
import { AgenticSystemsToggle } from '../shared/AgenticSystemsToggle'
import { ClaudeAuthRefreshRow } from '../shared/ClaudeAuthRefreshRow'
import { LocalLLMSettings } from '../shared/LocalLLMSettings'
import type { SettingEntry } from '../searchManifest'
import { webFeatures } from '@/web/features'

export const GENERAL_MANIFEST: SettingEntry[] = [
  { id: 'general.app-version', section: 'general', label: 'App Version', description: 'K2 version and auto-updater', keywords: ['update', 'version', 'check', 'release'] },
  { id: 'general.cli-version', section: 'general', label: 'CLI Version', description: 'Installed k2so CLI version + install/update button', keywords: ['k2so', 'cli', 'terminal', 'install', 'update', 'path'] },
  { id: 'general.agentic-systems', section: 'general', label: 'Agentic Systems', description: 'Enable AI agent orchestration, workspace manager, heartbeat, review queue', keywords: ['ai', 'agent', 'agentic', 'heartbeat', 'manager', 'review', 'beta'] },
  { id: 'general.claude-auth-refresh', section: 'general', label: 'Auto-refresh Claude credentials', description: 'Background scheduler that keeps your Claude session alive', keywords: ['claude', 'auth', 'token', 'login', 'credentials', 'scheduler'] },
  { id: 'general.daemon', section: 'general', label: 'K2 Server', description: 'Background service that keeps agents running when the app is closed', keywords: ['server', 'daemon', 'background', 'launchd', 'persistent', 'lid', 'sleep', 'wake', 'agent'] },
  { id: 'general.keep-daemon-on-quit', section: 'general', label: 'Keep server running when the window is closed', description: 'When on, clicking the red close button hides the window and keeps the Agent & Companion server running. When off, the red button stops everything. Cmd+Q always closes everything.', keywords: ['daemon', 'server', 'agent', 'companion', 'close', 'red button', 'window', 'hide', 'background', 'persistent'] },
  { id: 'general.restart-host', section: 'general', label: 'Restart connected host', description: 'Restart the REMOTE machine you are connected to over K2 Connect', keywords: ['restart', 'reboot', 'remote', 'host', 'connect', 'server', 'daemon', 'bounce'] },
  { id: 'general.update-host', section: 'general', label: 'Update connected host', description: 'Update the REMOTE machine you are connected to over K2 Connect', keywords: ['update', 'upgrade', 'remote', 'host', 'connect', 'server', 'daemon', 'version'] },
  { id: 'general.active-window-hours', section: 'general', label: 'Active Bar window', description: 'How long workspaces stay Active after activity', keywords: ['active', 'bar', 'window', 'hours', 'tenure', 'workspace', 'recent', 'sidebar'] },
  { id: 'general.completion-sound', section: 'general', label: 'Completion sound', description: 'Play a sound when an agent finishes unwatched', keywords: ['sound', 'chime', 'audio', 'notification', 'agent', 'done', 'finished', 'complete', 'unseen', 'orange', 'amber', 'dot'] },
  { id: 'general.owner-display-name', section: 'general', label: 'Your name', description: 'The name K2 agents call you when you message them', keywords: ['name', 'display', 'owner', 'you', 'from', 'identity', 'agents', 'call', 'message', 'sender'] },
  { id: 'general.ai-assistant', section: 'general', label: 'AI Workspace Assistant', description: 'Local LLM for natural-language workspace operations (⌘L)', keywords: ['ai', 'assistant', 'llm', 'cmd+l', 'qwen', 'model', 'local', 'gguf'] },
  { id: 'general.model-status', section: 'general', label: 'Model Status', description: 'Current local LLM load state', keywords: ['model', 'llm', 'loaded', 'download'] },
  { id: 'general.download-model', section: 'general', label: 'Download Default Model', description: 'Fetch Qwen2.5-1.5B locally (~1.1GB)', keywords: ['download', 'model', 'qwen', 'local llm'] },
  { id: 'general.custom-model', section: 'general', label: 'Custom Model', description: 'Point at any GGUF model file', keywords: ['model', 'gguf', 'custom', 'load'] },
  { id: 'general.config-location', section: 'general', label: 'Config Location', description: '~/.k2/settings.json', keywords: ['settings', 'config', 'location', 'path'] },
  { id: 'general.reset-all', section: 'general', label: 'Reset All Settings', description: 'Revert every setting to its default', keywords: ['reset', 'defaults', 'factory'] },
]

export function GeneralSection(): React.JSX.Element {
  const resetAllSettings = useSettingsStore((s) => s.resetAllSettings)
  // B2: when connected to a REMOTE host, the left "App Version" row is THIS
  // Mac's local Tauri app version — NOT the host's. Label it "This Mac" so
  // it can't be mistaken for the host version (the host version lives in the
  // right-pane UpdateHostRow). `activeHost === 'local'` ⇒ no badge.
  const activeHost = useConnectHostStore((s) => s.activeHost)
  const isRemote = activeHost !== 'local'
  const [confirming, setConfirming] = useState(false)
  const [currentVersion, setCurrentVersion] = useState<string>('')
  const updateStatus = useUpdateStore((s) => s.status)
  const updateVersion = useUpdateStore((s) => s.version)
  const updateProgress = useUpdateStore((s) => s.progress)
  const updateError = useUpdateStore((s) => s.error)

  // Load current version on mount
  useEffect(() => {
    invoke<string>('get_current_version').then(setCurrentVersion).catch((e) => console.warn('[settings]', e))
  }, [])

  const handleCheckUpdate = useCallback(async () => {
    await checkForUpdate(true)
  }, [])

  // Auto-check for updates when navigated here from the update toast
  useEffect(() => {
    if (useSettingsStore.getState().pendingUpdateCheck) {
      useSettingsStore.setState({ pendingUpdateCheck: false })
      handleCheckUpdate()
    }
  }, [handleCheckUpdate])

  // The Settings shell ALWAYS renders General in the LEFT pane of a half/half
  // split, so it fills its half-width pane (w-full). The RIGHT pane shows the
  // host-only Restart + Update controls (<GeneralRemoteHostPanel/>) with a
  // full-height divider ONLY when connected to a remote host; when local the
  // right pane is empty and the divider is hidden — General just stays at
  // half-width. (See Settings.tsx.)
  return (
    <div className="w-full">
      <h2 className="text-sm font-medium text-[var(--color-text-primary)] mb-4">General</h2>

      <div className="space-y-4">
        {/* Version & Update */}
        <div className="flex items-center justify-between py-2 border-b border-[var(--color-border)]">
          <span className="flex items-center gap-2 min-w-0">
            <span className="text-xs text-[var(--color-text-secondary)]">K2 by Alakazam Labs</span>
            {isRemote && (
              <span
                className="text-[8px] uppercase tracking-wider font-semibold px-1.5 py-0.5 bg-[color-mix(in_oklab,var(--color-status-warn)_20%,transparent)] text-[var(--color-status-warn-amber-soft)] flex-shrink-0"
                title="This is your local Mac's app version — not the connected host's. The host version is shown under the remote-host controls."
              >
                This Mac
              </span>
            )}
          </span>
          <div className="flex items-center gap-3">
            <div className="flex items-center gap-1.5">
              <span
                className="w-1.5 h-1.5 flex-shrink-0"
                style={{ backgroundColor: updateStatus === 'available' ? 'var(--color-status-warn-soft)' : 'var(--color-status-ok-soft)' }}
              />
              <span className="text-xs text-[var(--color-text-muted)]">
                v{currentVersion || '...'}
              </span>
            </div>
            {/* Hosted web: edge delivers versioned SPA — no in-app Tauri updater. */}
            {webFeatures.appUpdater && updateStatus === 'idle' && (
              <button
                onClick={handleCheckUpdate}
                className="px-2 py-0.5 text-[10px] text-[var(--color-text-muted)] border border-[var(--color-border)] hover:text-[var(--color-text-primary)] hover:border-[var(--color-text-muted)] transition-colors no-drag cursor-pointer"
              >
                Check for Updates
              </button>
            )}
            {webFeatures.appUpdater && updateStatus === 'checking' && (
              <span className="text-[10px] text-[var(--color-text-muted)]">Checking...</span>
            )}
          </div>
        </div>

        {/* Update available — desktop Tauri updater only */}
        {webFeatures.appUpdater && updateStatus === 'available' && updateVersion && (
          <div className="flex items-center justify-between p-3 bg-[var(--color-accent)]/10 border border-[var(--color-accent)]/30">
            <div>
              <p className="text-xs text-[var(--color-text-primary)]">K2 v{updateVersion} is available</p>
              <p className="text-[10px] text-[var(--color-text-muted)] mt-0.5">You&apos;re on v{currentVersion}</p>
            </div>
            <button
              className="px-3 py-1 text-xs font-medium bg-[var(--color-accent)] text-[var(--color-on-accent)] hover:bg-[var(--color-accent)]/90 transition-colors no-drag cursor-pointer"
              onClick={() => useUpdateStore.getState().startDownload()}
            >
              Download
            </button>
          </div>
        )}

        {/* Downloading */}
        {webFeatures.appUpdater && updateStatus === 'downloading' && (
          <div className="p-3 border border-[var(--color-border)]">
            <div className="flex items-center justify-between mb-2">
              <span className="text-xs text-[var(--color-text-primary)]">Downloading v{updateVersion}...</span>
              <span className="text-[10px] tabular-nums text-[var(--color-text-muted)]">{updateProgress}%</span>
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
              <p className="text-xs text-[var(--color-text-primary)]">v{updateVersion} is ready to install</p>
              <p className="text-[10px] text-[var(--color-text-muted)] mt-0.5">The app will restart after installation</p>
            </div>
            <button
              className="px-3 py-1 text-xs font-medium bg-[var(--color-status-ok)] text-[var(--color-on-accent)] hover:bg-[var(--color-status-ok-hover)] transition-colors no-drag cursor-pointer"
              onClick={() => useUpdateStore.getState().installAndRelaunch()}
            >
              Install & Relaunch
            </button>
          </div>
        )}

        {/* Error */}
        {webFeatures.appUpdater && updateStatus === 'error' && (
          <div className="p-3 border border-[color-mix(in_srgb,var(--color-status-error)_30%,transparent)] bg-[color-mix(in_srgb,var(--color-status-error)_5%,transparent)]">
            <p className="text-[10px] text-[var(--color-status-error-soft)]">{updateError}</p>
            <div className="flex items-center gap-2 mt-2">
              <button
                className="px-2 py-0.5 text-[10px] text-[var(--color-text-muted)] border border-[var(--color-border)] hover:text-[var(--color-text-primary)] transition-colors no-drag cursor-pointer"
                onClick={handleCheckUpdate}
              >
                Retry
              </button>
              <button
                className="px-2 py-0.5 text-[10px] text-[var(--color-accent)] border border-[var(--color-accent)]/30 hover:bg-[var(--color-accent)]/10 transition-colors no-drag cursor-pointer"
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

        {/* Re-open the "What's new" popup for the current version. Pairs
            with the auto-show on first launch after an update (0.38.7). */}
        <WhatsNewRow />

        {/* Agentic Systems master switch */}
        <AgenticSystemsToggle />

        {/* Claude Auth Auto-Refresh */}
        <ClaudeAuthRefreshRow />

        {/* P1.C — configurable Active-Bar tenure window */}
        <ActiveWindowHoursRow />

        {/* F4 — completion chime for unseen agent completions */}
        <CompletionSoundRow />

        {/* "Your display name" — the `from` attribution agents see when
            you message them via the composer (resolved server-side). */}
        <OwnerDisplayNameRow />

        {/* K2 Daemon — persistent-agents service */}
        <DaemonRow />

        {/* Keep-daemon-on-quit preference: honored by Cmd+Q and by the
            menubar's Quit K2 item. Default ON pairs with the menubar
            icon so users always have visibility into what's running. */}
        <KeepDaemonOnQuitRow />

        {/* AI Workspace Assistant (Cmd+L) — core feature, belongs in General */}
        <LocalLLMSettings />

        <div className="flex items-center justify-between py-2 border-b border-[var(--color-border)]">
          <span className="text-xs text-[var(--color-text-secondary)]">Config Location</span>
          <span className="text-xs text-[var(--color-text-muted)]">~/.k2/settings.json</span>
        </div>

        <div className="pt-4">
          {confirming ? (
            <div className="flex items-center gap-2">
              <span className="text-xs text-[var(--color-status-error-soft)]">Reset all settings to defaults?</span>
              <button
                onClick={() => {
                  resetAllSettings()
                  setConfirming(false)
                }}
                className="px-3 py-1 text-xs bg-[color-mix(in_srgb,var(--color-status-error)_20%,transparent)] text-[var(--color-status-error-soft)] border border-[color-mix(in_srgb,var(--color-status-error)_40%,transparent)] hover:bg-[color-mix(in_srgb,var(--color-status-error)_30%,transparent)] no-drag cursor-pointer"
              >
                Confirm
              </button>
              <button
                onClick={() => setConfirming(false)}
                className="px-3 py-1 text-xs text-[var(--color-text-muted)] border border-[var(--color-border)] hover:text-[var(--color-text-primary)] no-drag cursor-pointer"
              >
                Cancel
              </button>
            </div>
          ) : (
            <button
              onClick={() => setConfirming(true)}
              className="px-3 py-1 text-xs text-[var(--color-text-secondary)] border border-[var(--color-border)] hover:text-[var(--color-text-primary)] hover:border-[var(--color-text-muted)] no-drag cursor-pointer"
            >
              Reset All Settings
            </button>
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
      <h2 className="text-sm font-medium text-[var(--color-text-primary)] mb-4">Connected host</h2>
      <div className="space-y-4">
        <RestartHostRow />
        <UpdateHostRow />
      </div>
    </div>
  )
}

// ── Active Bar window (P1.C) ───────────────────────────────────────────
// How long a workspace stays in the sidebar's Active section after the
// user last interacted with it (Active-Bar rule 2). Default 24h, min 1h.
// Backed by `settings.activeWindowHours` (persisted via the daemon's
// app_settings deep-merge); read by ActiveBar and a future reaper.
function ActiveWindowHoursRow(): React.JSX.Element {
  const activeWindowHours = useSettingsStore((s) => s.activeWindowHours)
  const setActiveWindowHours = useSettingsStore((s) => s.setActiveWindowHours)
  // Local draft so the user can clear/type freely; commit (clamped) on blur
  // or Enter. Mirrors the typed-input ergonomics of the terminal settings.
  const [draft, setDraft] = useState<string>(String(activeWindowHours))

  useEffect(() => {
    setDraft(String(activeWindowHours))
  }, [activeWindowHours])

  const commit = useCallback(() => {
    const parsed = parseInt(draft, 10)
    const next = Number.isFinite(parsed) ? Math.max(1, parsed) : activeWindowHours
    setDraft(String(next))
    if (next !== activeWindowHours) {
      void setActiveWindowHours(next)
    }
  }, [draft, activeWindowHours, setActiveWindowHours])

  return (
    <div
      className="flex items-center justify-between py-2 border-b border-[var(--color-border)]"
      data-settings-id="general.active-window-hours"
    >
      <div className="flex-1 min-w-0 mr-3">
        <span className="text-xs text-[var(--color-text-secondary)]">
          Keep workspaces Active for
        </span>
        <p className="text-[10px] text-[var(--color-text-muted)] mt-0.5">
          How long a workspace stays in the sidebar&apos;s Active section after
          you last interacted with it. Minimum 1 hour.
        </p>
      </div>
      <div className="flex items-center gap-2 flex-shrink-0">
        <input
          type="number"
          min={1}
          step={1}
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          onBlur={commit}
          onKeyDown={(e) => {
            if (e.key === 'Enter') {
              e.currentTarget.blur()
            }
          }}
          // colorScheme:dark renders the native up/down stepper carrots light
          // (visible) instead of the default dark-on-dark black — matches the
          // K2 Connect section's number stepper.
          style={{ colorScheme: 'dark' }}
          className="w-16 px-2 py-1 text-xs bg-[var(--color-bg-surface)] border border-[var(--color-border)] text-[var(--color-text-primary)] outline-none focus:border-[var(--color-accent)] no-drag text-center"
        />
        <span className="text-[10px] text-[var(--color-text-muted)]">hours</span>
      </div>
    </div>
  )
}

// ── Completion sound (F4) ──────────────────────────────────────────────
// Chimes ONLY for UNSEEN completions — an agent finished while its pane
// wasn't being watched (the same fire that lights the Active-bar amber
// dot). Watched panes never chime. Backed by
// `settings.completionSoundEnabled` (daemon settings.json deep-merge),
// default ON. See .k2/notes/orange-dot-done-sound.md.
function CompletionSoundRow(): React.JSX.Element {
  const enabled = useSettingsStore((s) => s.completionSoundEnabled)
  const setCompletionSoundEnabled = useSettingsStore((s) => s.setCompletionSoundEnabled)

  return (
    <div
      className="flex items-center justify-between py-2 border-b border-[var(--color-border)]"
      data-settings-id="general.completion-sound"
    >
      <div className="flex-1 min-w-0 mr-3">
        <span className="text-xs text-[var(--color-text-secondary)]">
          Play a sound when an agent finishes unwatched
        </span>
        <p className="text-[10px] text-[var(--color-text-muted)] mt-0.5">
          A soft chime when an agent completes while you&apos;re looking
          elsewhere — the audible twin of the amber dot in the Active bar.
          Agents you&apos;re watching never chime.
        </p>
      </div>
      <Toggle
        checked={enabled}
        onChange={(next) => void setCompletionSoundEnabled(next)}
        aria-label="Play a sound when an agent finishes unwatched"
      />
    </div>
  )
}

// ── "Your display name" ────────────────────────────────────────────────
// The name K2 agents recognize YOU (the owner) by. Used server-side as the
// composer `from` attribution when you message an agent — the daemon
// resolves it from app_settings (`ownerDisplayName`) and NEVER from the
// request body (D3), falling back to "owner" when blank. Backed by
// `settings.ownerDisplayName` (persisted via the daemon's app_settings
// deep-merge), mirroring the typed-input ergonomics of ActiveWindowHoursRow.
function OwnerDisplayNameRow(): React.JSX.Element {
  const ownerDisplayName = useSettingsStore((s) => s.ownerDisplayName)
  const setOwnerDisplayName = useSettingsStore((s) => s.setOwnerDisplayName)
  // Local draft so the user can type freely; commit (sanitized) on blur
  // or Enter. The store re-sanitizes and the daemon re-sanitizes again.
  const [draft, setDraft] = useState<string>(ownerDisplayName)

  useEffect(() => {
    setDraft(ownerDisplayName)
  }, [ownerDisplayName])

  const commit = useCallback(() => {
    const next = sanitizeOwnerDisplayName(draft)
    setDraft(next)
    if (next !== ownerDisplayName) {
      void setOwnerDisplayName(next)
    }
  }, [draft, ownerDisplayName, setOwnerDisplayName])

  return (
    <div
      className="flex items-center justify-between py-2 border-b border-[var(--color-border)]"
      data-settings-id="general.owner-display-name"
    >
      <div className="flex-1 min-w-0 mr-3">
        <span className="text-xs text-[var(--color-text-secondary)]">
          Your name
        </span>
        <p className="text-[10px] text-[var(--color-text-muted)] mt-0.5">
          What agents call you when you message them. Leave blank to use
          &ldquo;owner&rdquo;.
        </p>
      </div>
      <div className="flex items-center gap-2 flex-shrink-0">
        <input
          type="text"
          value={draft}
          placeholder="owner"
          maxLength={OWNER_DISPLAY_NAME_MAX}
          onChange={(e) => setDraft(e.target.value)}
          onBlur={commit}
          onKeyDown={(e) => {
            if (e.key === 'Enter') {
              e.currentTarget.blur()
            }
          }}
          className="w-40 px-2 py-1 text-xs bg-[var(--color-bg-surface)] border border-[var(--color-border)] text-[var(--color-text-primary)] outline-none focus:border-[var(--color-accent)] no-drag"
        />
      </div>
    </div>
  )
}

// ── Composer 1c (D4) — "Allow remote instruct" ─────────────────────────
// MOVED to Settings → K2 Connect (shared/AllowRemoteInstructRow), beneath
// the "Enable federation" master: the toggle is the delivery consent for
// BOTH remote audiences (K2 Connect connect-users via the composer AND
// paired federation servers' inbound messages), so it lives with the rest
// of the remote-access switches instead of scattered here. See
// .k2/notes/federation-toggle-topology.md.

function CLIVersionRow(): React.JSX.Element {
  const [status, setStatus] = useState<{
    installed: boolean
    installedVersion: string | null
    bundledVersion: string | null
    updateAvailable: boolean
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
  const updateAvailable = status?.installed && status.bundledVersion && status.installedVersion
    && compareVersions(status.bundledVersion, status.installedVersion) > 0

  return (
    <div className="flex items-center justify-between py-2 border-b border-[var(--color-border)]">
      <span className="text-xs text-[var(--color-text-secondary)]">CLI Version</span>
      <div className="flex items-center gap-3">
        {status?.installed ? (
          <>
            <div className="flex items-center gap-1.5">
              <span
                className="w-1.5 h-1.5 flex-shrink-0"
                style={{ backgroundColor: updateAvailable ? 'var(--color-status-warn-soft)' : 'var(--color-status-ok-soft)' }}
              />
              <span className="text-xs text-[var(--color-text-muted)]">
                v{status.installedVersion || '?'}
              </span>
            </div>
            {updateAvailable ? (
              <button
                onClick={handleInstallOrUpdate}
                disabled={loading}
                className="px-2 py-0.5 text-[10px] bg-[var(--color-accent)] text-[var(--color-on-accent)] hover:opacity-90 transition-opacity no-drag cursor-pointer disabled:opacity-50"
              >
                {loading ? 'Updating...' : `Update to v${status.bundledVersion}`}
              </button>
            ) : (
              <button
                onClick={handleCheckForUpdates}
                disabled={checking}
                className="px-2 py-0.5 text-[10px] text-[var(--color-text-muted)] border border-[var(--color-border)] hover:text-[var(--color-text-primary)] hover:border-[var(--color-text-muted)] transition-colors no-drag cursor-pointer disabled:opacity-50"
              >
                {checking ? 'Checking...' : 'Check for Updates'}
              </button>
            )}
          </>
        ) : (
          <>
            <span className="text-xs text-[var(--color-text-muted)]">Not installed</span>
            <button
              onClick={handleInstallOrUpdate}
              disabled={loading}
              className="px-2 py-0.5 text-[10px] bg-[var(--color-accent)] text-[var(--color-on-accent)] hover:opacity-90 transition-opacity no-drag cursor-pointer disabled:opacity-50"
            >
              {loading ? 'Installing...' : 'Install'}
            </button>
          </>
        )}
      </div>
    </div>
  )
}

// ── What's New — re-open the popup ──────────────────────────────────────
// 0.38.8: small Settings row that lets the user re-read the most recent
// version's changelog without waiting for the next update. Clicking the
// button resets the last-seen marker daemon-side, then dispatches a
// `k2so:show-whats-new` window event the WhatsNewModal listens for to
// force-open. After the user dismisses, the marker gets re-stamped to
// the current version so the popup doesn't auto-show on next launch
// (idempotent with the normal dismiss flow).
function WhatsNewRow(): React.JSX.Element {
  const [busy, setBusy] = useState(false)

  const handleClick = useCallback(async () => {
    if (busy) return
    setBusy(true)
    try {
      await daemonCliGet('whats_new/reset')
      window.dispatchEvent(new CustomEvent('k2so:show-whats-new'))
    } catch (e) {
      // eslint-disable-next-line no-console
      console.debug('[whats-new] reset failed:', e)
    }
    setBusy(false)
  }, [busy])

  return (
    <div className="flex items-center justify-between py-2 border-b border-[var(--color-border)]">
      <span className="text-xs text-[var(--color-text-secondary)]">Release notes</span>
      <button
        onClick={handleClick}
        disabled={busy}
        className="px-2 py-0.5 text-[10px] text-[var(--color-text-muted)] border border-[var(--color-border)] hover:text-[var(--color-text-primary)] hover:border-[var(--color-text-muted)] transition-colors no-drag cursor-pointer disabled:opacity-50"
      >
        {busy ? 'Opening…' : "Read what's new"}
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
          <span className="text-xs text-[var(--color-text-secondary)]">K2 Server</span>
          <span className="text-[10px] text-[var(--color-text-muted)] mt-0.5">
            Keeps agents, terminals, heartbeats, &amp; companion app service running when the app is closed
          </span>
        </div>
        <div className="flex items-center gap-3 flex-shrink-0">
          <div className="flex items-center gap-1.5">
            <span className="w-1.5 h-1.5 flex-shrink-0" style={{ backgroundColor: dotColor }} />
            <span className="text-xs text-[var(--color-text-muted)]">{statusText}</span>
          </div>
          {status?.state === 'not_installed' && (
            <button
              onClick={handleInstall}
              disabled={busy !== null}
              className="px-2 py-0.5 text-[10px] bg-[var(--color-accent)] text-[var(--color-on-accent)] hover:opacity-90 transition-opacity no-drag cursor-pointer disabled:opacity-50"
            >
              {busy === 'install' ? 'Installing...' : 'Install'}
            </button>
          )}
          {status?.state === 'running' && (
            <button
              onClick={handleRestart}
              disabled={busy !== null}
              className="px-2 py-0.5 text-[10px] text-[var(--color-text-muted)] border border-[var(--color-border)] hover:text-[var(--color-text-primary)] hover:border-[var(--color-text-muted)] transition-colors no-drag cursor-pointer disabled:opacity-50"
            >
              {busy === 'restart' ? 'Restarting...' : 'Restart'}
            </button>
          )}
          {status?.state === 'unreachable' && (
            <button
              onClick={handleRestart}
              disabled={busy !== null}
              className="px-2 py-0.5 text-[10px] bg-[color-mix(in_srgb,var(--color-status-warn-soft)_20%,transparent)] text-[var(--color-status-warn-text)] border border-[color-mix(in_srgb,var(--color-status-warn-soft)_30%,transparent)] hover:bg-[color-mix(in_srgb,var(--color-status-warn-soft)_30%,transparent)] transition-colors no-drag cursor-pointer disabled:opacity-50"
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
            className="text-[10px] text-[var(--color-text-muted)] underline hover:text-[var(--color-text-primary)] transition-colors no-drag cursor-pointer"
          >
            View log
          </button>
          {runtimeText && (
            <span className="text-[10px] text-[var(--color-text-muted)]">{runtimeText}</span>
          )}
        </div>
      )}

      {/* Inline log viewer — appears when "View log" is clicked */}
      {showingLog && (
        <div className="mt-2 p-2 bg-black/30 border border-[var(--color-border)] max-h-60 overflow-auto">
          <div className="flex items-center justify-between mb-1">
            <span className="text-[10px] text-[var(--color-text-muted)]">~/.k2/daemon.stdout.log (last 300 lines)</span>
            <button
              onClick={() => setShowingLog(false)}
              className="text-[10px] text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] no-drag cursor-pointer"
            >
              Close
            </button>
          </div>
          <pre className="text-[10px] text-[var(--color-text-muted)] whitespace-pre-wrap font-mono leading-tight">
            {logText || '(log file empty or missing)'}
          </pre>
        </div>
      )}

      {/* Error surface — visible until the next action clears it */}
      {error && (
        <div className="mt-2 p-2 bg-[color-mix(in_srgb,var(--color-status-error)_5%,transparent)] border border-[color-mix(in_srgb,var(--color-status-error)_30%,transparent)]">
          <p className="text-[10px] text-[var(--color-status-error-soft)] break-all">{error}</p>
        </div>
      )}
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
        <span className="text-xs text-[var(--color-text-secondary)]">
          Keep server running when the window is closed
        </span>
        <p className="text-[10px] text-[var(--color-text-muted)] mt-0.5">
          {keep
            ? 'Red close button hides the window; agents keep working and the mobile companion stays reachable. Menu bar shows status. Cmd+Q closes everything.'
            : 'Red close button stops everything, same as Cmd+Q. Agents pause and the mobile companion disconnects until you reopen K2.'}
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

  const [role, setRole] = useState<RestartRole | null>(null)
  const [restarting, setRestarting] = useState(false)

  const isRemote = activeHost !== 'local'
  // A stable label for copy: the user-facing name, falling back to the
  // hostname so the dialog is never blank.
  const hostLabel = isRemote ? (activeHost.label?.trim() || activeHost.hostname) : ''
  const hostname = isRemote ? activeHost.hostname : ''
  const hostId = isRemote ? activeHost.id : ''

  // Resolve the viewer's role on the ACTIVE remote host (host-aware
  // whoami). Owner/Admin may restart; a Member can't (the route 403s).
  // Re-runs whenever the active remote changes.
  useEffect(() => {
    if (!isRemote) {
      setRole(null)
      return
    }
    let cancelled = false
    void (async () => {
      try {
        const data = await daemonCliGet<{ role?: string; owner?: boolean }>('auth/whoami')
        if (cancelled) return
        const resolved: RestartRole | null =
          data.role === 'owner' || data.role === 'admin' || data.role === 'member'
            ? data.role
            : data.owner
              ? 'owner'
              : null
        setRole(resolved)
      } catch {
        if (!cancelled) setRole(null)
      }
    })()
    return () => {
      cancelled = true
    }
  }, [isRemote, hostId])

  // UNMISTAKABLE: never render for the local Mac (DaemonRow owns that),
  // hide for an older remote without the route, and hide for Members who
  // can't restart. Pure decision lives in restartHostVisibility() so it's
  // unit-tested without rendering.
  const { show, canRestart } = restartHostVisibility({ isRemote, supportsRestart, role })
  if (!show) return null

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
      await daemonCliPost('daemon/restart', {})
      addToast(`Restarting ${hostLabel}… it'll reconnect automatically.`, 'info', 8000)
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e)
      // The route is owner-token-gated: a non-owner session 403s. Surface
      // a clear, host-named error rather than a silent failure.
      if (/403|forbidden|invalid or missing token/i.test(msg)) {
        addToast(
          `You don't have permission to restart ${hostLabel}. Only the host owner can restart it.`,
          'error',
          8000,
        )
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
            <span className="text-[8px] uppercase tracking-wider font-semibold px-1.5 py-0.5 bg-[color-mix(in_oklab,var(--color-status-warn)_20%,transparent)] text-[var(--color-status-warn-amber-soft)] flex-shrink-0">
              Remote host
            </span>
            <span className="text-xs text-[var(--color-text-primary)] font-medium truncate">
              {hostLabel}
            </span>
            <span className="text-[10px] text-[var(--color-text-muted)] font-mono truncate">
              {hostname}
            </span>
          </span>
          <span className="text-[10px] text-[var(--color-text-muted)] mt-1 leading-relaxed">
            Restart the machine you&apos;re connected to — <strong className="text-[var(--color-status-warn-amber-soft)]">not this Mac</strong>.
            Active sessions briefly disconnect, then reconnect automatically.
          </span>
        </div>
        <div className="flex-shrink-0">
          {restarting ? (
            <span className="text-[11px] text-[var(--color-status-warn-amber-soft)]">Restarting {hostLabel}…</span>
          ) : (
            <button
              onClick={() => void handleRestart()}
              disabled={!canRestart}
              title={canRestart ? undefined : 'Only the host owner or an admin can restart this host'}
              className="px-3 py-1 text-[11px] font-medium text-[var(--color-status-warn-amber-bright)] bg-[color-mix(in_srgb,var(--color-status-warn)_15%,transparent)] border border-[color-mix(in_srgb,var(--color-status-warn)_40%,transparent)] hover:bg-[color-mix(in_srgb,var(--color-status-warn)_25%,transparent)] transition-colors no-drag cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed"
            >
              Restart {hostLabel}
            </button>
          )}
        </div>
      </div>
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
//      version; the ConnectionGate's soft-reconnect returns us when it's
//      back. We do NOT manage reconnect.
//   * failed / rolled-back are surfaced as a host-named message
//     ("Update rolled back — <host> is still on <current>").
//
// Gating (identical to RestartHostRow):
//   * serverSupports('remote-update') (min 0.39.33) — an OLDER remote that
//     lacks the routes hides the control instead of 404ing.
//   * Owner/Admin on the active host (host-aware auth/whoami role). The
//     routes are owner-token-gated, so a 403 surfaces as a clear toast.
function UpdateHostRow(): React.JSX.Element | null {
  const activeHost = useConnectHostStore((s) => s.activeHost)
  const supportsUpdate = useServerSupports('remote-update')
  const confirm = useConfirmDialogStore((s) => s.confirm)
  const addToast = useToastStore((s) => s.addToast)

  const [role, setRole] = useState<UpdateRole | null>(null)
  const [checking, setChecking] = useState(false)
  const [check, setCheck] = useState<UpdateCheckResult | null>(null)
  const [jobId, setJobId] = useState<string | null>(null)
  const [status, setStatus] = useState<UpdateStatusResult | null>(null)
  const [applying, setApplying] = useState(false)
  // Comeback watcher (0.40.48): tracks the host's return AFTER the job hits
  // `restarting`/`done` (both mean "the host is going away now") so the row
  // resolves to a verified success line instead of saying "Installing &
  // restarting…" forever. `expected` is check.latest captured at watch
  // start — comparing it against the returned /boot-status version is how
  // we verify the update actually took (vs. rolled back).
  const [comeback, setComeback] = useState<
    | null
    | { kind: 'watching' }
    | { kind: 'slow' }
    | { kind: 'back'; version?: string; expected?: string }
  >(null)

  const isRemote = activeHost !== 'local'
  const hostLabel = isRemote ? (activeHost.label?.trim() || activeHost.hostname) : ''
  const hostname = isRemote ? activeHost.hostname : ''
  const hostId = isRemote ? activeHost.id : ''

  // Resolve the viewer's role on the ACTIVE remote host (host-aware
  // whoami). Owner/Admin may update; a Member can't (the route 403s).
  // Re-runs whenever the active remote changes. Also resets any in-flight
  // check/job state so a host switch never shows a stale update.
  useEffect(() => {
    setCheck(null)
    setJobId(null)
    setStatus(null)
    setApplying(false)
    setComeback(null)
    if (!isRemote) {
      setRole(null)
      return
    }
    let cancelled = false
    void (async () => {
      try {
        const data = await daemonCliGet<{ role?: string; owner?: boolean }>('auth/whoami')
        if (cancelled) return
        const resolved: UpdateRole | null =
          data.role === 'owner' || data.role === 'admin' || data.role === 'member'
            ? data.role
            : data.owner
              ? 'owner'
              : null
        setRole(resolved)
      } catch {
        if (!cancelled) setRole(null)
      }
    })()
    return () => {
      cancelled = true
    }
  }, [isRemote, hostId])

  // Poll the update job status every ~1.5s while a job is running and not
  // yet terminal. The ConnectionGate's soft-reconnect handles the host
  // going away on restart, so we stop polling at any terminal phase.
  const phase: UpdatePhase | null = status?.phase ?? null
  useEffect(() => {
    if (!jobId) return
    if (isTerminalPhase(phase)) return
    let cancelled = false
    const tick = async (): Promise<void> => {
      try {
        const s = await daemonCliGet<UpdateStatusResult>('daemon/update/status', { job_id: jobId })
        if (!cancelled) setStatus(s)
      } catch (e) {
        if (cancelled) return
        const msg = e instanceof Error ? e.message : String(e)
        // A connection-level error here is expected once the host starts
        // restarting — let the ConnectionGate handle it; don't spam toasts.
        // Any other error we surface once, then stop polling.
        if (isForbiddenError(msg)) {
          addToast(updateForbiddenCopy(hostLabel), 'error', 8000)
          setJobId(null)
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
  }, [jobId, phase, hostLabel, addToast])

  // Surface a failure phase once, as a host-named toast.
  useEffect(() => {
    if (isFailurePhase(phase) && phase) {
      addToast(updatePhaseCopy(phase, hostLabel, { current: check?.current }), 'error', 10000)
    }
  }, [phase, hostLabel, check?.current, addToast])

  // Comeback watcher (0.40.48): once the job reaches `restarting`/`done`
  // the host drops off — the status route is gone with it, so the old row
  // froze on "Installing & restarting…" forever. Poll the host's PUBLIC
  // /boot-status (no token needed; hostBootStatus's retry loop evicts dead
  // pooled sockets) until `phase === 'ready'`, then resolve the row:
  // verified success line + refreshed version line + back to idle. Soft
  // deadline (~4 min, matching the Connections tile's waitForHostReady)
  // flips the copy to an honest "taking longer than expected" while the
  // watch continues; hard stop at ~10 min leaves that copy up.
  const hostGoneInstalling = phase === 'restarting' || phase === 'done'
  useEffect(() => {
    // `!isRemote` already narrows activeHost to a ConnectHost below.
    if (!hostGoneInstalling || !isRemote) return
    let alive = true
    const expected = check?.latest
    const creds = remoteCreds(activeHost)
    // Baden false-rollback fix: the OLD daemon answers `ready` until the
    // moment it's actually replaced, so a `ready` probe alone proves
    // nothing. Track whether we've seen the host DOWN; resolution requires
    // version===expected OR sawDown (shouldResolveComeback).
    let sawDown = false
    setComeback({ kind: 'watching' })
    void (async () => {
      const slowAt = Date.now() + 4 * 60_000
      const giveUpAt = Date.now() + 10 * 60_000
      let lastReady: { version?: string } | null = null
      while (alive && Date.now() < giveUpAt) {
        // Jittered so many clients watching the same rebooting host don't
        // poll in lockstep (same rationale as the recovery poll).
        await new Promise((r) => setTimeout(r, jittered(2500)))
        if (!alive) return
        const s = await hostBootStatus(creds)
        if (!alive) return
        if (s === null) sawDown = true
        if (s?.phase === 'ready') lastReady = { version: s.version }
        if (
          shouldResolveComeback({
            phase: s?.phase,
            version: s?.version,
            expected,
            sawDown,
          })
        ) {
          setComeback({ kind: 'back', version: s?.version, expected })
          // Resolve the job UI back to idle: the phase line goes away, the
          // success line below takes over, and the persistent version line
          // reflects what the host ACTUALLY reports (never assume latest).
          setJobId(null)
          setStatus(null)
          setApplying(false)
          setCheck((c) =>
            c ? { ...c, current: s?.version ?? c.current, available: false } : c,
          )
          return
        }
        if (Date.now() >= slowAt) {
          setComeback((k) => (k?.kind === 'watching' ? { kind: 'slow' } : k))
        }
      }
      // Hard stop with the host answering `ready` but never seen down and
      // never on the expected version (e.g. sub-poll-interval restart on a
      // daemon too old to report a version): resolve honestly with what it
      // reports rather than abandoning the row on "still watching…".
      if (alive && lastReady) {
        setComeback({ kind: 'back', version: lastReady.version, expected })
        setJobId(null)
        setStatus(null)
        setApplying(false)
      }
    })()
    return () => {
      alive = false
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps -- keyed on the
    // watch trigger + host identity; check.latest is captured at start by
    // design (the expected version must not drift mid-watch).
  }, [hostGoneInstalling, isRemote, hostId])

  const { show, canUpdate } = updateHostVisibility({ isRemote, supportsUpdate, role })
  if (!show) return null

  const reportError = (e: unknown): void => {
    const msg = e instanceof Error ? e.message : String(e)
    if (isForbiddenError(msg)) {
      addToast(updateForbiddenCopy(hostLabel), 'error', 8000)
    } else {
      addToast(`Couldn't update ${hostLabel}: ${msg}`, 'error', 8000)
    }
  }

  const handleCheck = async (): Promise<void> => {
    setChecking(true)
    setStatus(null)
    setJobId(null)
    // A fresh check supersedes any resolved comeback verdict — its result
    // is newer truth than a line computed during the restart window (the
    // Baden screenshot: a stale "rolled back to v0.40.44" line sitting
    // under a correct "is on v0.40.47" check result).
    setComeback(null)
    try {
      const result = await daemonCliPost<UpdateCheckResult>('daemon/update/check', {})
      setCheck(result)
      if (!result.available) {
        // B4: a newer version exists but there's no build for this host's
        // platform — DON'T claim "up to date"; surface the distinct state.
        if (result.newerNoArtifact) {
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
      reportError(e)
    } finally {
      setChecking(false)
    }
  }

  const handleDownload = async (): Promise<void> => {
    try {
      const { job_id } = await daemonCliPost<{ job_id: string }>('daemon/update/start', {})
      setStatus({ phase: 'downloading' })
      setJobId(job_id)
    } catch (e) {
      reportError(e)
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
    try {
      await daemonCliPost('daemon/update/apply', { job_id: jobId })
      // The host now installs + restarts; show the restarting line and let
      // the ConnectionGate's soft-reconnect bring us back on the new version.
      setStatus({ phase: 'restarting' })
    } catch (e) {
      reportError(e)
      setApplying(false)
    }
  }

  // Decide the action-area content from the current state.
  const inProgress = jobId !== null && phase !== null && !isStaged(phase) && !isFailurePhase(phase)
  const staged = isStaged(phase)

  return (
    <div
      className="py-2.5 px-3 border border-[color-mix(in_srgb,var(--color-status-warn)_40%,transparent)] bg-[color-mix(in_srgb,var(--color-status-warn)_5%,transparent)]"
      data-settings-id="general.update-host"
    >
      <div className="flex items-center justify-between gap-4">
        <div className="flex flex-col min-w-0 flex-1">
          <span className="flex items-center gap-2 min-w-0">
            <span className="text-[8px] uppercase tracking-wider font-semibold px-1.5 py-0.5 bg-[color-mix(in_oklab,var(--color-status-warn)_20%,transparent)] text-[var(--color-status-warn-amber-soft)] flex-shrink-0">
              Remote host
            </span>
            <span className="text-xs text-[var(--color-text-primary)] font-medium truncate">
              {hostLabel}
            </span>
            <span className="text-[10px] text-[var(--color-text-muted)] font-mono truncate">
              {hostname}
            </span>
          </span>
          <span className="text-[10px] text-[var(--color-text-muted)] mt-1 leading-relaxed">
            Update the machine you&apos;re connected to — <strong className="text-[var(--color-status-warn-amber-soft)]">not this Mac</strong>.
            It briefly disconnects to install, then reconnects automatically.
          </span>
        </div>
        <div className="flex-shrink-0">
          {/* Idle (no job, not staged): Check / Download / Install button */}
          {!inProgress && !staged && (
            check?.available ? (
              <button
                onClick={() => void handleDownload()}
                disabled={!canUpdate}
                title={canUpdate ? undefined : 'Only the host owner or an admin can update this host'}
                className="px-3 py-1 text-[11px] font-medium text-[var(--color-status-warn-amber-bright)] bg-[color-mix(in_srgb,var(--color-status-warn)_15%,transparent)] border border-[color-mix(in_srgb,var(--color-status-warn)_40%,transparent)] hover:bg-[color-mix(in_srgb,var(--color-status-warn)_25%,transparent)] transition-colors no-drag cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed"
              >
                {/* Bundled-app hosts (Shape A) update in one shot — the Tauri
                    updater downloads + installs + relaunches, there's no
                    separate staged "Install & restart" step — so the button is
                    "Update Host", not "Download". Standalone (Shape B) hosts
                    keep the two-step Download → Install & restart flow. */}
                {check?.installKind === 'bundled-app' ? 'Update Host' : 'Download'}
              </button>
            ) : (
              <button
                onClick={() => void handleCheck()}
                disabled={!canUpdate || checking}
                title={canUpdate ? undefined : 'Only the host owner or an admin can update this host'}
                className="px-3 py-1 text-[11px] font-medium text-[var(--color-status-warn-amber-bright)] bg-[color-mix(in_srgb,var(--color-status-warn)_15%,transparent)] border border-[color-mix(in_srgb,var(--color-status-warn)_40%,transparent)] hover:bg-[color-mix(in_srgb,var(--color-status-warn)_25%,transparent)] transition-colors no-drag cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed"
              >
                {checking ? 'Checking…' : 'Check for updates'}
              </button>
            )
          )}
          {/* Staged: ready to install & restart the host */}
          {staged && (
            applying ? (
              <span className="text-[11px] text-[var(--color-status-warn-amber-soft)]">Installing & restarting {hostLabel}…</span>
            ) : (
              <button
                onClick={() => void handleApply()}
                disabled={!canUpdate}
                className="px-3 py-1 text-[11px] font-medium text-[var(--color-status-warn-amber-bright)] bg-[color-mix(in_srgb,var(--color-status-warn)_15%,transparent)] border border-[color-mix(in_srgb,var(--color-status-warn)_40%,transparent)] hover:bg-[color-mix(in_srgb,var(--color-status-warn)_25%,transparent)] transition-colors no-drag cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed"
              >
                Install & restart {hostLabel}
              </button>
            )
          )}
        </div>
      </div>

      {/* B1: persistent host-version line — shown after ANY successful check,
          for BOTH up-to-date and update-available states, so the remote
          host's CURRENT version is always visible (not just in a toast or
          the update banner). Hidden once a job is downloading/staging (the
          phase line takes over) so the row doesn't double up. */}
      {check && !inProgress && !staged && (
        <div className="mt-2 text-[11px] text-[var(--color-text-muted)]">
          {hostVersionCopy(hostLabel, check.current)}
        </div>
      )}

      {/* Update-available banner once a check reports one (pre-download) */}
      {check?.available && !inProgress && !staged && (
        <div className="mt-1 text-[11px] text-[var(--color-status-warn-amber-bright)]">
          {updateAvailableCopy(hostLabel, check.current, check.latest)}
        </div>
      )}

      {/* B4: a newer version exists but there's no build for this host's
          platform — DISTINCT from "up to date" (available is false but the
          host is behind). Only when not already in/after a job. */}
      {check?.newerNoArtifact && !inProgress && !staged && (
        <div className="mt-1 text-[11px] text-[color-mix(in_srgb,var(--color-status-warn-amber-bright)_90%,transparent)]">
          {newerNoArtifactCopy(hostLabel, check.latest, check.platform)}
        </div>
      )}

      {/* Verified comeback line (0.40.48): the watcher saw the host's
          /boot-status go 'ready' after the install-restart — the row's
          job state is already resolved back to idle by then, so this
          renders alongside the refreshed version line. Green = confirmed
          on the wire, not assumed. */}
      {comeback?.kind === 'back' && !inProgress && !staged && (
        <div className="mt-2 text-[11px] text-[var(--color-status-ok-soft)]">
          {updateCompleteCopy(hostLabel, comeback.expected, comeback.version)}
        </div>
      )}

      {/* In-flight phase + download progress line */}
      {(inProgress || staged) && phase && (
        <div className="mt-2">
          <div className="text-[11px] text-[var(--color-status-warn-amber-bright)]">
            {/* Past the watcher's soft deadline the restarting copy would be
                a lie ("it'll reconnect automatically" — it hasn't yet); be
                honest that it's slow while the watch continues. */}
            {comeback?.kind === 'slow'
              ? updateSlowComebackCopy(hostLabel)
              : updatePhaseCopy(phase, hostLabel, {
                  progress: status?.progress,
                  current: check?.current,
                })}
          </div>
          {phase === 'downloading' && typeof status?.progress === 'number' && (
            <div className="h-1.5 mt-1.5 bg-[color-mix(in_srgb,var(--color-status-warn)_20%,transparent)] overflow-hidden">
              <div
                className="h-full bg-[var(--color-status-warn-amber)] transition-all duration-300"
                style={{ width: `${Math.max(0, Math.min(100, status.progress))}%` }}
              />
            </div>
          )}
        </div>
      )}

      {/* Failure surface — host-named, host stays on its current version */}
      {isFailurePhase(phase) && phase && (
        <div className="mt-2 p-2 bg-[color-mix(in_srgb,var(--color-status-error)_5%,transparent)] border border-[color-mix(in_srgb,var(--color-status-error)_30%,transparent)]">
          <p className="text-[11px] text-[var(--color-status-error-soft)]">
            {updatePhaseCopy(phase, hostLabel, { current: check?.current })}
          </p>
          {status?.error && (
            <p className="mt-1 text-[10px] text-[var(--color-status-error-soft)] break-all">
              {status.error}
            </p>
          )}
        </div>
      )}
    </div>
  )
}
