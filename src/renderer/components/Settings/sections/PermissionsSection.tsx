// 0.37.9 — macOS permissions settings page.
//
// Five permission rows. Each polls its OS-level state every 2s while
// the page is open (so a freshly-granted permission flips to "Granted"
// without the user reloading), and offers an Open-Settings or
// programmatic-prompt button to acquire it.

import { useCallback, useEffect, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import type { SettingEntry } from '../searchManifest'

interface PermissionStatus {
  fullDiskAccess: boolean
  accessibility: boolean
  microphone: boolean
}

interface MicrophoneRequestResult {
  granted: boolean
  openedSettings: boolean
}

type StatusKind = 'granted' | 'not-granted' | 'unknown'

function StatusBadge({ kind }: { kind: StatusKind }): React.JSX.Element {
  // Match the styling of similar badges around the Settings UI —
  // small caps + thin border for "not granted" / "unknown", filled
  // accent for "granted".
  if (kind === 'granted') {
    return (
      <span className="text-[9px] uppercase tracking-wider px-1.5 py-0.5 bg-[var(--color-accent)]/15 text-[var(--color-accent)] font-medium">
        Granted
      </span>
    )
  }
  if (kind === 'not-granted') {
    return (
      <span className="text-[9px] uppercase tracking-wider px-1.5 py-0.5 border border-[var(--color-border)] text-[var(--color-text-muted)] font-medium">
        Not granted
      </span>
    )
  }
  return (
    <span className="text-[9px] uppercase tracking-wider px-1.5 py-0.5 border border-[var(--color-border)] text-[var(--color-text-muted)] font-medium">
      Unknown
    </span>
  )
}

function PermissionRow({
  id,
  label,
  description,
  status,
  buttonLabel,
  onRequest,
}: {
  id: string
  label: string
  description: string
  status: StatusKind
  buttonLabel: string
  onRequest: () => void
}): React.JSX.Element {
  return (
    <div
      data-settings-id={id}
      className="flex items-start justify-between gap-4 py-3 border-b border-[var(--color-border)] last:border-b-0"
    >
      <div className="min-w-0 flex-1 space-y-0.5">
        <div className="text-xs font-medium text-[var(--color-text-primary)]">
          {label}
        </div>
        <p className="text-[10px] text-[var(--color-text-muted)]">
          {description}
        </p>
      </div>
      <div className="flex items-center gap-2 shrink-0 pt-0.5">
        <StatusBadge kind={status} />
        <button
          onClick={onRequest}
          className="px-2 py-1 text-[10px] bg-[var(--color-bg)] border border-[var(--color-border)] hover:border-[var(--color-text-muted)] text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)] transition-colors no-drag cursor-pointer"
        >
          {buttonLabel}
        </button>
      </div>
    </div>
  )
}

export function PermissionsSection(): React.JSX.Element {
  const [status, setStatus] = useState<PermissionStatus | null>(null)
  const [loading, setLoading] = useState(true)

  // Poll every 2s while the page is open so the user sees their
  // freshly-granted permission flip to "Granted" without reloading.
  useEffect(() => {
    let cancelled = false
    const fetchStatus = async (): Promise<void> => {
      try {
        const next = await invoke<PermissionStatus>('permissions_get_status')
        if (cancelled) return
        setStatus(next)
        setLoading(false)
      } catch (err) {
        console.warn('[permissions]', err)
        if (cancelled) return
        setLoading(false)
      }
    }
    void fetchStatus()
    const interval = setInterval(fetchStatus, 2000)
    return () => {
      cancelled = true
      clearInterval(interval)
    }
  }, [])

  const requestFda = useCallback(async (): Promise<void> => {
    try {
      await invoke('permissions_request_full_disk_access')
    } catch (err) {
      console.warn('[permissions]', err)
    }
  }, [])

  const requestAccessibility = useCallback(async (): Promise<void> => {
    try {
      await invoke('permissions_request_accessibility')
    } catch (err) {
      console.warn('[permissions]', err)
    }
  }, [])

  const requestMicrophone = useCallback(async (): Promise<void> => {
    try {
      // First call fires AVCaptureDevice.requestAccess directly,
      // which shows macOS's native permission sheet. After that
      // first answer, this command opens System Settings instead
      // (the OS won't re-prompt programmatically once decided).
      await invoke<MicrophoneRequestResult>('permissions_request_microphone')
    } catch (err) {
      console.warn('[permissions]', err)
    }
  }, [])

  const requestAppleEvents = useCallback(async (): Promise<void> => {
    try {
      await invoke('permissions_request_apple_events')
    } catch (err) {
      console.warn('[permissions]', err)
    }
  }, [])

  const requestLocalNetwork = useCallback(async (): Promise<void> => {
    try {
      await invoke('permissions_request_local_network')
    } catch (err) {
      console.warn('[permissions]', err)
    }
  }, [])

  const kindFromBool = (b: boolean | undefined): StatusKind => {
    if (b === undefined) return 'unknown'
    return b ? 'granted' : 'not-granted'
  }

  // Apple Events / Local Network don't have a programmatic check —
  // we always render them as "Unknown" with an Open Settings button.
  // The user opens System Settings, flips the toggle, and trusts
  // their own eyes; we don't claim a status we can't verify.

  return (
    <div className="max-w-2xl">
      <div className="mb-4">
        <h3 className="text-xs font-medium text-[var(--color-text-primary)]">
          Permissions
        </h3>
        <p className="text-[10px] text-[var(--color-text-muted)] mt-0.5">
          OS-level access K2 needs to function. Grants are managed by
          macOS in <span className="text-[var(--color-text-secondary)]">System Settings → Privacy &amp; Security</span>.
        </p>
      </div>

      <div className="border border-[var(--color-border)]">
        {loading ? (
          <div className="px-3 py-6 text-[10px] text-[var(--color-text-muted)] text-center">
            Loading permission status…
          </div>
        ) : (
          <div className="px-3">
            <PermissionRow
              id="permissions-microphone"
              label="Microphone"
              description="Required for Apple Dictation to work in terminal panes (press Fn-Fn / Globe to dictate)."
              status={kindFromBool(status?.microphone)}
              buttonLabel={status?.microphone ? 'Open settings' : 'Request access'}
              onRequest={() => { void requestMicrophone() }}
            />
            <PermissionRow
              id="permissions-full-disk-access"
              label="Full Disk Access"
              description="Read workspace folders outside ~/Documents (Documents, Downloads, Desktop, and iCloud Drive are TCC-protected by default)."
              status={kindFromBool(status?.fullDiskAccess)}
              buttonLabel="Open settings"
              onRequest={() => { void requestFda() }}
            />
            <PermissionRow
              id="permissions-accessibility"
              label="Accessibility"
              description="Programmatic keystroke replay and integration with automation tools."
              status={kindFromBool(status?.accessibility)}
              buttonLabel="Open settings"
              onRequest={() => { void requestAccessibility() }}
            />
            <PermissionRow
              id="permissions-apple-events"
              label="Automation (Apple Events)"
              description="Run AppleScript commands and interact with other Mac apps (e.g. open files in external editors)."
              status="unknown"
              buttonLabel="Open settings"
              onRequest={() => { void requestAppleEvents() }}
            />
            <PermissionRow
              id="permissions-local-network"
              label="Local Network"
              description="Discover and connect to development servers and the Mobile Companion device on your local network."
              status="unknown"
              buttonLabel="Open settings"
              onRequest={() => { void requestLocalNetwork() }}
            />
          </div>
        )}
      </div>

      <p className="mt-3 text-[10px] text-[var(--color-text-muted)]">
        Granting a permission opens System Settings — flip the toggle next to
        K2 and the status here updates automatically.
      </p>
    </div>
  )
}

// Search manifest entries for the global Settings search.
export const PERMISSIONS_MANIFEST: SettingEntry[] = [
  {
    id: 'permissions-microphone',
    section: 'permissions',
    label: 'Microphone',
    description: 'Required for Apple Dictation in terminal panes.',
    keywords: ['microphone', 'mic', 'dictation', 'voice', 'audio', 'permission'],
  },
  {
    id: 'permissions-full-disk-access',
    section: 'permissions',
    label: 'Full Disk Access',
    description: 'Read workspaces outside ~/Documents.',
    keywords: ['full disk access', 'fda', 'tcc', 'documents', 'downloads', 'icloud', 'permission'],
  },
  {
    id: 'permissions-accessibility',
    section: 'permissions',
    label: 'Accessibility',
    description: 'Programmatic keystroke replay and automation.',
    keywords: ['accessibility', 'a11y', 'keystrokes', 'automation', 'permission'],
  },
  {
    id: 'permissions-apple-events',
    section: 'permissions',
    label: 'Automation (Apple Events)',
    description: 'AppleScript and inter-app integration.',
    keywords: ['apple events', 'automation', 'applescript', 'permission'],
  },
  {
    id: 'permissions-local-network',
    section: 'permissions',
    label: 'Local Network',
    description: 'Mobile Companion device discovery on LAN.',
    keywords: ['local network', 'lan', 'companion', 'bonjour', 'permission'],
  },
]
