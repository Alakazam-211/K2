// Settings → Accessibility: what K2 can reach on this Mac.
//
// Five macOS permissions, one card row each: the name, what K2 uses it for,
// its status (Allowed / Not allowed / Can't tell) and one action on the
// right. The page asks macOS every 2 s while it is open, so a permission
// turned on in System Settings flips to Allowed without a reload. macOS only
// lets an app check three of them (Microphone, Full Disk Access,
// Accessibility); Automation and Local Network always read Can't tell, and the
// page says why once, under the card.

import { useCallback, useEffect, useId, useRef, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import type { SettingEntry } from '../searchManifest'
import { Button } from '@/components/ui/Button'
import { Callout } from '@/components/ui/Callout'
import {
  SETTING_ROW_HINT,
  SettingRow,
  SettingsGroup,
  SettingsLoadState,
} from '../controls/SettingControls'
import { HealthWord, StatusDot, type HealthTone } from './ConnectionsParts'

interface PermissionStatus {
  fullDiskAccess: boolean
  accessibility: boolean
  microphone: boolean
}

interface MicrophoneRequestResult {
  granted: boolean
  openedSettings: boolean
}

type StatusKind = 'allowed' | 'not-allowed' | 'unknown'

type ReadState =
  | { kind: 'loading' }
  | { kind: 'ready'; status: PermissionStatus }
  | { kind: 'failed'; detail: string | null }

/** What pressing a row's button last did. */
type RowNote =
  | { kind: 'opened' }
  | { kind: 'asking' }
  | { kind: 'failed'; detail: string | null }

interface Permission {
  id: string
  label: string
  /** What K2 uses it for, in one plain line. */
  purpose: string
  /** The field macOS reports, or null when macOS gives apps no way to check. */
  field: keyof PermissionStatus | null
  command: string
}

const PERMISSIONS: Permission[] = [
  {
    id: 'permissions-microphone',
    label: 'Microphone',
    purpose: 'So macOS Dictation works in terminal panes. Press the Globe key twice to start.',
    field: 'microphone',
    command: 'permissions_request_microphone',
  },
  {
    id: 'permissions-full-disk-access',
    label: 'Full Disk Access',
    purpose: 'So K2 can open workspaces in folders macOS guards, like Documents, Downloads, Desktop and iCloud Drive.',
    field: 'fullDiskAccess',
    command: 'permissions_request_full_disk_access',
  },
  {
    id: 'permissions-accessibility',
    label: 'Accessibility',
    purpose: 'So K2 can type keystrokes for you and work with automation tools.',
    field: 'accessibility',
    command: 'permissions_request_accessibility',
  },
  {
    id: 'permissions-apple-events',
    label: 'Automation',
    purpose: 'So K2 can ask other Mac apps to do things, like opening a file in your editor.',
    field: null,
    command: 'permissions_request_apple_events',
  },
  {
    id: 'permissions-local-network',
    label: 'Local Network',
    purpose: 'So K2 can find servers on your network and the K2 Companion app on your phone.',
    field: null,
    command: 'permissions_request_local_network',
  },
]

const STATUS: Record<StatusKind, { tone: HealthTone; word: string }> = {
  allowed: { tone: 'ok', word: 'Allowed' },
  'not-allowed': { tone: 'warn', word: 'Not allowed' },
  unknown: { tone: 'neutral', word: 'Can’t tell' },
}

function isStatus(v: unknown): v is PermissionStatus {
  if (!v || typeof v !== 'object') return false
  const s = v as Record<string, unknown>
  return typeof s.fullDiskAccess === 'boolean' && typeof s.accessibility === 'boolean' && typeof s.microphone === 'boolean'
}

function reasonOf(err: unknown): string | null {
  const raw = err instanceof Error ? err.message : typeof err === 'string' ? err : ''
  return raw.trim().replace(/[.\s]+$/, '') || null
}

function StatusLine({ kind }: { kind: StatusKind }): React.JSX.Element {
  const { tone, word } = STATUS[kind]
  return (
    <span className="inline-flex items-center gap-2 type-row" data-permission-status={kind}>
      <StatusDot tone={tone} />
      <HealthWord tone={tone}>{word}</HealthWord>
    </span>
  )
}

function Note({ perm, kind, note }: { perm: Permission; kind: StatusKind; note: RowNote | undefined }): React.JSX.Element | null {
  if (!note) return null
  if (note.kind === 'failed') {
    return (
      <div role="alert" className="mt-1">
        <p className="type-hint text-[var(--color-status-error-soft)] leading-relaxed max-w-[75ch]">
          Couldn&rsquo;t open System Settings. Open it yourself, then go to Privacy &amp; Security &rarr; {perm.label}.
        </p>
        {note.detail && <p className={SETTING_ROW_HINT}>Reason: {note.detail}.</p>}
      </div>
    )
  }
  if (kind === 'allowed') {
    return (
      <p role="status" className="mt-1 flex items-center gap-1 type-hint text-[var(--color-status-ok-soft)]">
        <svg aria-hidden="true" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth={2} strokeLinecap="round" strokeLinejoin="round" className="w-3 h-3">
          <path d="M3.5 8.5 6.5 11.5 12.5 4.5" />
        </svg>
        {perm.label} is on. Nothing else to do.
      </p>
    )
  }
  const text =
    note.kind === 'asking'
      ? 'macOS is asking you now. Choose Allow in its window, and this row updates by itself.'
      : perm.field
        ? `System Settings is open at ${perm.label}. Turn on K2 there, and this row updates by itself.`
        : `System Settings is open at ${perm.label}. Turn on K2 there. macOS doesn’t tell apps about this one, so this row stays at Can’t tell.`
  return (
    <p role="status" className={`${SETTING_ROW_HINT} mt-1`}>
      {text}
    </p>
  )
}

function PermissionRow({
  perm,
  kind,
  note,
  busy,
  onRequest,
}: {
  perm: Permission
  kind: StatusKind
  note: RowNote | undefined
  busy: boolean
  onRequest: () => void
}): React.JSX.Element {
  const labelId = useId()
  const action =
    kind === 'allowed' ? null : (
      // The one to act on (Not allowed) gets the filled button; a row K2
      // can't read keeps the quieter outline, so the next step stands out.
      <Button
        variant={kind === 'not-allowed' ? 'default' : 'quiet'}
        className="flex-shrink-0"
        aria-describedby={labelId}
        disabled={busy}
        onClick={onRequest}
      >
        {perm.field === 'microphone' && kind === 'not-allowed' ? 'Allow…' : 'Open System Settings'}
      </Button>
    )
  return (
    <SettingRow
      settingId={perm.id}
      label={<span id={labelId}>{perm.label}</span>}
      hint={perm.purpose}
      below={
        <div>
          <StatusLine kind={kind} />
          <Note perm={perm} kind={kind} note={note} />
        </div>
      }
    >
      {action}
    </SettingRow>
  )
}

export function PermissionsSection(): React.JSX.Element {
  const [read, setRead] = useState<ReadState>({ kind: 'loading' })
  const [notes, setNotes] = useState<Record<string, RowNote>>({})
  const [busy, setBusy] = useState<string | null>(null)
  const [retrying, setRetrying] = useState(false)
  const alive = useRef(true)

  // Ask macOS. A failed read after a good one keeps the last answer (a poll
  // can blip); only a page that never got an answer shows Can't tell.
  const fetchStatus = useCallback(async (): Promise<void> => {
    try {
      const next = await invoke<unknown>('permissions_get_status')
      if (!alive.current) return
      if (isStatus(next)) setRead({ kind: 'ready', status: next })
      else setRead((r) => (r.kind === 'ready' ? r : { kind: 'failed', detail: null }))
    } catch (err) {
      console.warn('[permissions]', err)
      if (!alive.current) return
      setRead((r) => (r.kind === 'ready' ? r : { kind: 'failed', detail: reasonOf(err) }))
    }
  }, [])

  // Poll every 2 s while the page is open, so a permission turned on in
  // System Settings shows here without a reload.
  useEffect(() => {
    alive.current = true
    void fetchStatus()
    const interval = setInterval(() => void fetchStatus(), 2000)
    return () => {
      alive.current = false
      clearInterval(interval)
    }
  }, [fetchStatus])

  const retry = async (): Promise<void> => {
    setRetrying(true)
    await fetchStatus()
    if (alive.current) setRetrying(false)
  }

  const request = async (perm: Permission): Promise<void> => {
    setBusy(perm.id)
    try {
      // Microphone: the first time, macOS shows its own Allow window; after
      // that it only changes in System Settings, which this opens instead.
      const res = await invoke<MicrophoneRequestResult | null>(perm.command)
      if (!alive.current) return
      const asking = perm.field === 'microphone' && !!res && !res.granted && !res.openedSettings
      setNotes((n) => ({ ...n, [perm.id]: { kind: asking ? 'asking' : 'opened' } }))
      void fetchStatus()
    } catch (err) {
      console.warn('[permissions]', err)
      if (!alive.current) return
      setNotes((n) => ({ ...n, [perm.id]: { kind: 'failed', detail: reasonOf(err) } }))
    } finally {
      if (alive.current) setBusy(null)
    }
  }

  const kindOf = (perm: Permission): StatusKind => {
    if (read.kind !== 'ready' || !perm.field) return 'unknown'
    return read.status[perm.field] ? 'allowed' : 'not-allowed'
  }

  return (
    <div className="w-full max-w-[52rem] space-y-6 pb-6">
      <header className="min-w-0">
        <h2 className="type-title text-[var(--color-text-primary)]">Accessibility</h2>
        <p className="type-hint text-[var(--color-text-secondary)] mt-1 leading-relaxed max-w-[75ch]">
          What K2 can reach on this Mac. You turn each one on in System Settings, under Privacy &amp; Security, and this
          page updates by itself.
        </p>
      </header>

      {read.kind === 'failed' && (
        <Callout tone="warn" role="alert" className="type-hint" data-testid="permissions-read-failed">
          <div className="flex items-center justify-between gap-4">
            <div className="min-w-0 space-y-0.5">
              <p className="font-medium">K2 couldn&rsquo;t read these permissions from macOS.</p>
              <p className="text-[var(--color-text-secondary)]">
                Each one shows Can&rsquo;t tell until it can. You can still check them in System Settings.
              </p>
              {read.detail && <p className="text-[var(--color-text-secondary)]">Reason: {read.detail}.</p>}
            </div>
            <Button className="flex-shrink-0" disabled={retrying} onClick={() => void retry()}>
              {retrying ? 'Checking…' : 'Try again'}
            </Button>
          </div>
        </Callout>
      )}

      <div className="space-y-2">
        <SettingsGroup
          title="Permissions"
          card
          testId="permissions"
          state={
            read.kind === 'loading' ? (
              <SettingsLoadState kind="loading" title="Checking permissions…" testId="permissions-loading">
                Asking macOS what K2 is allowed to use.
              </SettingsLoadState>
            ) : undefined
          }
        >
          {PERMISSIONS.map((perm) => (
            <PermissionRow
              key={perm.id}
              perm={perm}
              kind={kindOf(perm)}
              note={notes[perm.id]}
              busy={busy === perm.id}
              onRequest={() => void request(perm)}
            />
          ))}
        </SettingsGroup>

        {read.kind === 'ready' && (
          <p className={SETTING_ROW_HINT} data-testid="permissions-cant-tell">
            Can&rsquo;t tell: macOS doesn&rsquo;t let apps check Automation or Local Network. Open System Settings to
            see whether K2 has them.
          </p>
        )}
      </div>
    </div>
  )
}

// Search manifest entries for the global Settings search.
export const PERMISSIONS_MANIFEST: SettingEntry[] = [
  {
    id: 'permissions-microphone',
    section: 'permissions',
    label: 'Microphone',
    description: 'Lets you dictate into terminal panes.',
    keywords: ['microphone', 'mic', 'dictation', 'voice', 'audio', 'permission'],
  },
  {
    id: 'permissions-full-disk-access',
    section: 'permissions',
    label: 'Full Disk Access',
    description: 'Lets K2 open workspaces in Documents, Downloads, Desktop and iCloud Drive.',
    keywords: ['full disk access', 'fda', 'tcc', 'documents', 'downloads', 'icloud', 'permission'],
  },
  {
    id: 'permissions-accessibility',
    section: 'permissions',
    label: 'Accessibility',
    description: 'Lets K2 type keystrokes for you and work with automation tools.',
    keywords: ['accessibility', 'a11y', 'keystrokes', 'automation', 'permission'],
  },
  {
    id: 'permissions-apple-events',
    section: 'permissions',
    label: 'Automation',
    description: 'Lets K2 ask other Mac apps to do things.',
    keywords: ['apple events', 'automation', 'applescript', 'permission'],
  },
  {
    id: 'permissions-local-network',
    section: 'permissions',
    label: 'Local Network',
    description: 'Lets K2 find servers and the K2 Companion app on your network.',
    keywords: ['local network', 'lan', 'companion', 'bonjour', 'permission'],
  },
]
