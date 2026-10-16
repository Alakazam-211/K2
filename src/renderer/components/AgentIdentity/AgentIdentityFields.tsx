// Agent identity controls: picture, color, name and handle.
//
// One set of controls, two homes: Settings → Workspaces → Agent → Identity
// (the full form) and the room's Agent drawer tab (`compact`, narrow). Both
// write through the same routes and the same store actions, so a change in
// either shows up in the other, in the nav, and on every other client (the
// daemon emits `agent_identity_changed` + SyncProjects for each write).
//
// These controls are for the WINDOW'S server: the picture and color paint
// the window's projects store optimistically (`useProjectsStore`). Callers
// pass the window's scope (Settings: `primaryScope()`; the drawer: the
// primary room's `room.scope`, the same object). A room on another server
// shows its agent read-only instead (`AgentPanel`).
//
// Routes: picture `projects/detect-icon`, `projects/update {iconUrl}`,
// `projects/clear-icon`; color `projects/update {color}` (store
// `setProjectColor`); name `workspace/set-agent-display-name`; handle
// `workspace/set-handle` (Member floor for Connect logins, like the rest).

import { useEffect, useRef, useState } from 'react'
import { emit } from '@tauri-apps/api/event'
import { daemonCliPost } from '@/lib/daemon-cli'
import type { ServerScope } from '@/kessel/server-scope'
import { agentDisplayName, agentHandle, setAgentDisplayName, setAgentHandle } from '@/lib/workspace-agent'
import { onAgentIdentityChanged } from '@/stores/session-events'
import { agentIdentityDisplayName, agentIdentityMatchesPath } from '@/lib/agent-identity'
import { useProjectsStore, type ProjectWithWorkspaces } from '@/stores/projects'
import { useSettingsStore } from '@/stores/settings'
import { useConfirmDialogStore } from '@/stores/confirm-dialog'
import { useTunnelUrls } from '@/hooks/useTunnelUrls'
import { pickIconImage } from '@/lib/pick-remote-image'
import IconCropDialog from '@/components/Settings/IconCropDialog'
import { GROUP_AVATAR_COLORS } from '@/components/Projects/ProjectGroupAvatar'
import { normalizeHexColor } from '@/components/Projects/projects-api'

/** Plan B cross-window sync: re-emit `sync:projects` after a host-aware
 *  write so other windows re-fetch (the old Tauri shims did this). */
function emitProjectsChanged(): void {
  void emit('sync:projects').catch((e) =>
    console.warn('[agent-identity] sync:projects emit failed:', e),
  )
}

const BUTTON =
  'px-2.5 py-1 text-xs text-[var(--color-text-secondary)] border border-[var(--color-border)] hover:bg-[var(--color-bg-elevated)] hover:text-[var(--color-text-primary)] no-drag cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed transition-colors'
const BUTTON_DANGER =
  'px-2.5 py-1 text-xs text-[var(--color-status-error-soft)] border border-[color-mix(in_srgb,var(--color-status-error)_30%,transparent)] hover:bg-[color-mix(in_srgb,var(--color-status-error)_10%,transparent)] no-drag cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed transition-colors'

// ── Picture ──────────────────────────────────────────────────────────────

/** The agent's picture tile: the image, or the first letter on its color. */
export function AgentPictureTile({
  project,
  size,
}: {
  project: Pick<ProjectWithWorkspaces, 'name' | 'color' | 'iconUrl'>
  size: number
}): React.JSX.Element {
  return (
    <div
      className="flex-shrink-0 flex items-center justify-center overflow-hidden"
      data-testid="agent-picture-tile"
      style={{
        width: size,
        height: size,
        backgroundColor: project.iconUrl ? 'transparent' : project.color,
        border: project.iconUrl ? `2px solid ${project.color}` : 'none',
      }}
    >
      {project.iconUrl ? (
        <img
          src={project.iconUrl}
          alt={project.name}
          style={{ width: '100%', height: '100%', objectFit: 'cover', objectPosition: 'center', display: 'block' }}
        />
      ) : (
        <span
          className="text-[var(--color-on-accent)] font-bold"
          style={{ fontSize: Math.round(size * 0.46), lineHeight: 1 }}
        >
          {project.name.charAt(0).toUpperCase()}
        </span>
      )}
    </div>
  )
}

/** Picture + Detect / Upload / Remove (crop dialog on upload). */
export function AgentPictureControls({
  scope,
  project,
  compact = false,
}: {
  scope: ServerScope
  project: ProjectWithWorkspaces
  compact?: boolean
}): React.JSX.Element {
  const [iconLoading, setIconLoading] = useState(false)
  const [cropImage, setCropImage] = useState<string | null>(null)
  const fileInputRef = useRef<HTMLInputElement>(null)

  const refresh = async (): Promise<void> => {
    emitProjectsChanged()
    await useProjectsStore.getState().fetchProjects()
  }

  const handleDetectIcon = async (): Promise<void> => {
    setIconLoading(true)
    try {
      await daemonCliPost(scope, 'projects/detect-icon', { projectId: project.id })
      await refresh()
    } catch (err) {
      console.error('Icon detection failed:', err)
    } finally {
      setIconLoading(false)
    }
  }

  const handleUploadClick = (): void => {
    // Host-aware: on a remote host the native input would browse THIS
    // machine's disk — use the remote file picker instead.
    void pickIconImage({
      clickNativeInput: () => fileInputRef.current?.click(),
      setCropImage,
    })
  }

  const handleFileSelected = (e: React.ChangeEvent<HTMLInputElement>): void => {
    const file = e.target.files?.[0]
    if (!file) return
    const reader = new FileReader()
    reader.onload = () => {
      setCropImage(reader.result as string)
    }
    reader.readAsDataURL(file)
    // Reset input so the same file can be re-selected
    e.target.value = ''
  }

  const handleCropConfirm = async (croppedDataUrl: string): Promise<void> => {
    setCropImage(null)
    setIconLoading(true)
    try {
      await daemonCliPost(scope, 'projects/update', { id: project.id, iconUrl: croppedDataUrl })
      await refresh()
    } catch (err) {
      console.error('Icon save failed:', err)
    } finally {
      setIconLoading(false)
    }
  }

  const handleClearIcon = async (): Promise<void> => {
    setIconLoading(true)
    try {
      await daemonCliPost(scope, 'projects/clear-icon', { projectId: project.id })
      await refresh()
    } catch (err) {
      console.error('Icon clear failed:', err)
    } finally {
      setIconLoading(false)
    }
  }

  return (
    <>
      {cropImage && (
        <IconCropDialog
          imageDataUrl={cropImage}
          onConfirm={handleCropConfirm}
          onCancel={() => setCropImage(null)}
        />
      )}
      <div className={compact ? 'flex items-center gap-3' : 'flex items-center gap-4 py-2'}>
        <AgentPictureTile project={project} size={compact ? 40 : 48} />
        <div className="flex items-center gap-1.5 flex-wrap min-w-0">
          <button onClick={handleDetectIcon} disabled={iconLoading} className={BUTTON}>
            {iconLoading ? 'Working...' : 'Detect'}
          </button>
          <button onClick={handleUploadClick} disabled={iconLoading} className={BUTTON}>
            Upload
          </button>
          <input
            ref={fileInputRef}
            type="file"
            accept="image/png,image/jpeg,image/svg+xml,image/x-icon"
            className="hidden"
            onChange={handleFileSelected}
          />
          {project.iconUrl && (
            <button onClick={handleClearIcon} disabled={iconLoading} className={BUTTON_DANGER}>
              Remove
            </button>
          )}
        </div>
      </div>
    </>
  )
}

// ── Color ────────────────────────────────────────────────────────────────

/** Palette + free hex/rgb (same contract as project-group color). Writes
 *  through the store's optimistic `setProjectColor`. */
export function AgentColorPicker({
  project,
  compact = false,
}: {
  project: ProjectWithWorkspaces
  compact?: boolean
}): React.JSX.Element {
  const [customHex, setCustomHex] = useState('')
  const [hexError, setHexError] = useState(false)

  useEffect(() => {
    setCustomHex('')
    setHexError(false)
  }, [project.id])

  const apply = (color: string): void => {
    void useProjectsStore.getState().setProjectColor(project.id, color)
  }

  const swatches = (
    <div className={`flex items-center gap-1.5 flex-wrap min-w-0 ${compact ? '' : 'justify-end'}`}>
      {GROUP_AVATAR_COLORS.map((color) => (
        <button
          key={color}
          type="button"
          title={color}
          aria-label={`Color ${color}`}
          onClick={() => apply(color)}
          className={`w-4 h-4 flex-shrink-0 no-drag cursor-pointer transition-transform ${
            project.color === color ? 'scale-125 ring-1 ring-[color-mix(in_srgb,var(--color-text-primary)_50%,transparent)]' : 'hover:scale-110'
          }`}
          style={{ backgroundColor: color }}
        />
      ))}
      <input
        type="text"
        value={customHex}
        onChange={(e) => {
          setCustomHex(e.target.value)
          setHexError(false)
        }}
        onKeyDown={(e) => {
          if (e.key === 'Enter') {
            const normalized = normalizeHexColor(customHex)
            if (normalized === null) {
              setHexError(customHex.trim().length > 0)
              return
            }
            setHexError(false)
            setCustomHex('')
            apply(normalized)
          } else if (e.key === 'Escape') {
            e.stopPropagation()
            setCustomHex('')
            setHexError(false)
          }
        }}
        onBlur={() => {
          if (!customHex.trim()) return
          const normalized = normalizeHexColor(customHex)
          if (normalized === null) {
            setHexError(true)
            return
          }
          setHexError(false)
          setCustomHex('')
          apply(normalized)
        }}
        placeholder={project.color || '#rrggbb'}
        spellCheck={false}
        aria-label="Custom color (hex or rgb)"
        className={`w-[7.5rem] px-1.5 py-0.5 text-[11px] font-mono bg-[var(--color-bg)] border text-[var(--color-text-primary)] placeholder:text-[var(--color-text-muted)] focus:outline-none no-drag ${
          hexError
            ? 'border-[color-mix(in_srgb,var(--color-status-error-soft)_60%,transparent)]'
            : 'border-[var(--color-border)] focus:border-[var(--color-accent)]'
        }`}
      />
    </div>
  )

  if (compact) return swatches

  return (
    <div className="flex items-center justify-between py-2 border-t border-[var(--color-border)] gap-3">
      <span className="text-xs text-[var(--color-text-secondary)]">Color</span>
      {swatches}
    </div>
  )
}

// ── Agent Display Name Input (0.37.4) ────────────────────────────────
//
// A small inline editor that reads the workspace's primary agent's
// display name (AGENT.md `display_name:` → `name:` → projects.name)
// and writes new values via `k2so_workspace_set_agent_display_name`.
//
// Why a separate field from the technical agent name? Because in
// 0.37.4 the technical name (the agent's AGENT.md `name:` /
// directory basename — historically the `__lead__` sentinel for
// manager workspaces, removed in 0.39.0f Phase 2.1) still keys
// infrastructure layers — v2_session_map,
// `workspace_sessions.terminal_id`, pending_live queue dirs. Editing
// the technical name would cascade through all of them and risk
// dropping the live PTY. The display name decouples the
// human-facing label from those keys; the user gets a friendly
// "what to call this agent" without the rename gymnastics.
//
// A future 0.38.0 ships the full `agent-display-name.md` PRD: drop
// the technical name from infrastructure, collapse `display_name:`
// and `name:` back into one field. Until then, this is the cheap
// stepping stone that fixes the user-visible "I renamed my agent
// and the inbox tab still says the wrong name" complaint.

export function AgentDisplayNameField({
  scope,
  projectPath,
  helpText,
  trailing,
}: {
  scope: ServerScope
  projectPath: string
  helpText?: string
  /** Optional element rendered to the right of the Save button —
   *  meant for siblings like a Manage Persona button so the row
   *  reads input | save | persona on one line. */
  trailing?: React.ReactNode
}): React.JSX.Element {
  const [ready, setReady] = useState(false)
  const [draft, setDraft] = useState('')
  const [saved, setSaved] = useState('')
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const [flash, setFlash] = useState(false)

  useEffect(() => {
    let cancelled = false
    agentDisplayName(scope, projectPath)
      .then((n) => { if (!cancelled) { setDraft(n); setSaved(n); setReady(true) } })
      .catch((e) => { if (!cancelled) { console.error('[display-name] read failed:', e); setReady(true) } })
    return () => { cancelled = true }
  }, [scope, projectPath])

  // Agent rename live update: someone else renamed this agent. Follow it,
  // but never clobber what this user is typing (an unsaved edit stays).
  const savedRef = useRef(saved)
  savedRef.current = saved
  useEffect(() => {
    return onAgentIdentityChanged(scope, (e) => {
      if (!agentIdentityMatchesPath(e, projectPath)) return
      const n = agentIdentityDisplayName(e)
      if (!n) return
      setDraft((d) => (d === savedRef.current ? n : d))
      setSaved(n)
    })
  }, [scope, projectPath])

  const dirty = ready && draft !== saved

  // Mirrors k2-core's `validate_display_name` (0.40.24 S3 loosened
  // contract): display names are human labels — spaces + mixed case
  // are allowed ("QA Bot", "K2 - Marketing Manager"). Still banned:
  // empty, >64 chars, control characters, leading/trailing
  // whitespace, and "/" (the name seeds retire's archive folder
  // label).
  const validate = (n: string): string | null => {
    if (n.length === 0) return 'Display name must not be empty.'
    if (n.length > 64) return 'Display name must be at most 64 characters.'
    if (n !== n.trim()) return 'Display name must not start or end with whitespace.'
    if (n.includes('/')) return "Display name must not contain '/'."
    if (n.includes(':')) return "Display name must not contain ':' (federated addresses use name::host)."
    // eslint-disable-next-line no-control-regex
    if (/[\u0000-\u001f\u007f-\u009f]/.test(n)) return 'Display name must not contain control characters.'
    return null
  }

  const handleSave = async (): Promise<void> => {
    // 0.40.24 S3: names are case-preserving now (mixed case is part
    // of the label), so the 0.37.9 save-time lowercasing is retired
    // along with the slug-shaped rule.
    const candidate = draft
    const err = validate(candidate)
    if (err) { setError(err); return }
    setError(null)
    setBusy(true)
    try {
      await setAgentDisplayName(scope, projectPath, candidate)
      setDraft(candidate)
      setSaved(candidate)
      setFlash(true)
      setTimeout(() => setFlash(false), 1200)
    } catch (e) {
      setError(String(e))
    } finally {
      setBusy(false)
    }
  }

  return (
    <div>
      <div className="flex items-center gap-2">
        <div className="flex-1 min-w-0">
          <label className="block text-[9px] uppercase tracking-wider text-[var(--color-text-muted)] mb-1">
            Agent Name
          </label>
          <input
            type="text"
            value={draft}
            // 0.37.9 — DON'T transform the value on every keystroke.
            // Pre-fix this read `e.target.value.toLowerCase()`, which
            // mutates the DOM input's value mid-composition during
            // Apple Dictation. Dictation manages the input value
            // internally during the composition phase; if we mutate
            // it underneath, dictation's state desyncs and the
            // engagement hangs/aborts. (0.40.24 S3: lowercase
            // enforcement is gone entirely — names are case-
            // preserving; `validate()` at save time carries the
            // remaining rules.)
            onChange={(e) => { setDraft(e.target.value); setError(null) }}
            disabled={!ready || busy}
            placeholder="agent"
            aria-label="Agent Name"
            spellCheck={false}
            autoComplete="off"
            autoCorrect="off"
            autoCapitalize="off"
            onKeyDown={(e) => { if (e.key === 'Enter' && dirty && !busy) handleSave() }}
            className="w-full px-2 py-1 text-xs bg-[var(--color-bg-elevated)] border border-[var(--color-border)] text-[var(--color-text-primary)] focus:outline-none focus:border-[var(--color-accent)] disabled:opacity-60"
          />
        </div>
        <button
          onClick={handleSave}
          disabled={!ready || busy || !dirty}
          className="px-3 py-1.5 text-[10px] font-medium text-[var(--color-accent)] bg-[var(--color-accent)]/10 hover:bg-[var(--color-accent)]/20 border border-[var(--color-accent)]/30 transition-colors no-drag cursor-pointer disabled:opacity-50 flex-shrink-0 self-end"
        >
          {busy ? 'Saving…' : flash ? 'Saved' : 'Save'}
        </button>
        {trailing}
      </div>
      {error && (
        <p className="text-[10px] text-[var(--color-status-error-soft)] mt-1">{error}</p>
      )}
      {!error && helpText && (
        <p className="text-[9px] text-[var(--color-text-muted)] mt-1">{helpText}</p>
      )}
    </div>
  )
}

export function federatedHostFromTunnel(status: {
  running?: boolean
  public_url?: string | null
  subdomain?: string | null
} | null): string | null {
  if (!status?.running) return null
  const url = status.public_url?.trim()
  if (url) {
    try {
      const host = new URL(url.includes('://') ? url : `https://${url}`).hostname
      if (host) return host
    } catch {
      /* fall through to subdomain */
    }
  }
  const sub = status.subdomain?.trim()
  if (sub) return sub.includes('.') ? sub : `${sub}.k2.dev`
  return null
}

/** The agent's handle (its address). A change asks first: the address
 *  changes, and federated connections break until the other side updates.
 *  The daemon keeps the old handle as an alias and never hands it out
 *  again. */
export function AgentHandleField({
  scope,
  projectPath,
  projectHandle,
  compact = false,
}: {
  scope: ServerScope
  projectPath: string
  projectHandle?: string
  /** Drawer: a short button label ("Change") so it fits the drawer. */
  compact?: boolean
}): React.JSX.Element {
  const [handle, setHandle] = useState(projectHandle ?? '')
  const [draft, setDraft] = useState(projectHandle ?? '')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const confirm = useConfirmDialogStore((s) => s.confirm)
  const { status } = useTunnelUrls(scope)
  const tunnelHost = federatedHostFromTunnel(status)

  useEffect(() => {
    let cancelled = false
    agentHandle(scope, projectPath)
      .then((h) => {
        if (!cancelled) {
          setHandle(h)
          setDraft(h)
        }
      })
      .catch(() => {})
    return () => { cancelled = true }
  }, [scope, projectPath, projectHandle])

  // A handle change anywhere (another client, the CLI) repaints this field,
  // keeping an unsaved edit.
  const handleRef = useRef(handle)
  handleRef.current = handle
  useEffect(() => {
    return onAgentIdentityChanged(scope, (e) => {
      if (!agentIdentityMatchesPath(e, projectPath) || !e.handle) return
      const next = e.handle
      setDraft((d) => (d === handleRef.current ? next : d))
      setHandle(next)
    })
  }, [scope, projectPath])

  const dirty = draft.trim() !== handle.trim() && draft.trim().length > 0

  const onChange = async (): Promise<void> => {
    const next = draft.trim()
    if (!next || next === handle) return
    const ok = await confirm({
      title: 'Change handle?',
      message:
        `Changing the handle changes this agent's address (${handle || 'old'}::${tunnelHost ?? 'host'} → ${next}::${tunnelHost ?? 'host'}). Existing federated connections will break until the other side reconnects or updates the handle.`,
      confirmLabel: 'Change handle',
      destructive: true,
    })
    if (!ok) return
    setBusy(true)
    setError(null)
    try {
      const stored = await setAgentHandle(scope, projectPath, next)
      setHandle(stored)
      setDraft(stored)
      void emit('sync:projects').catch(() => {})
    } catch (e) {
      setError(String(e))
    } finally {
      setBusy(false)
    }
  }

  return (
    <div className="mt-3">
      <label className="block text-[9px] uppercase tracking-wider text-[var(--color-text-muted)] mb-1">
        Handle
      </label>
      <div className="flex items-center gap-2">
        <input
          type="text"
          value={draft}
          onChange={(e) => { setDraft(e.target.value); setError(null) }}
          disabled={busy}
          aria-label="Handle"
          spellCheck={false}
          autoComplete="off"
          autoCorrect="off"
          autoCapitalize="off"
          className="flex-1 min-w-0 px-2 py-1 text-xs font-mono bg-[var(--color-bg-elevated)] border border-[var(--color-border)] text-[var(--color-text-primary)] focus:outline-none focus:border-[var(--color-accent)] disabled:opacity-60"
        />
        <button
          onClick={() => { void onChange() }}
          disabled={busy || !dirty}
          className="px-3 py-1.5 text-[10px] font-medium text-[var(--color-accent)] bg-[var(--color-accent)]/10 hover:bg-[var(--color-accent)]/20 border border-[var(--color-accent)]/30 transition-colors no-drag cursor-pointer disabled:opacity-50 flex-shrink-0"
        >
          {busy ? 'Saving…' : compact ? 'Change…' : 'Change handle…'}
        </button>
      </div>
      {handle && tunnelHost && (
        <p className="text-[9px] font-mono text-[var(--color-text-muted)] mt-1 break-all">{handle}::{tunnelHost}</p>
      )}
      {handle && !tunnelHost && (
        <p className="text-[9px] text-[var(--color-text-muted)] mt-1 leading-relaxed">
          Get a subdomain so this agent can talk to other people&apos;s agents on K2.{' '}
          <button
            type="button"
            className="text-[var(--color-accent)] hover:underline bg-transparent border-none p-0 cursor-pointer no-drag"
            onClick={() => useSettingsStore.getState().openSettings('k2-connect')}
          >
            Open K2 Connect
          </button>
        </p>
      )}
      {error && (
        <p className="text-[10px] text-[var(--color-status-error-soft)] mt-1">{error}</p>
      )}
    </div>
  )
}
