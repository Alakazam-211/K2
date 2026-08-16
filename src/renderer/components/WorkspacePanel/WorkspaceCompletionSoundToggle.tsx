import { useCallback, useState } from 'react'
import { emit } from '@tauri-apps/api/event'
import { daemonCliPost } from '@/lib/daemon-cli'
import { useProjectsStore } from '@/stores/projects'
import { useSettingsStore } from '@/stores/settings'

type SoundProject = { id: string; path: string; completionSoundEnabled?: number }

async function writeWorkspaceCompletionSound(
  project: SoundProject,
  next: boolean,
): Promise<void> {
  await daemonCliPost('workspace/set', {
    project: project.path,
    fields: { completion_sound_enabled: next ? '1' : '0' },
  })
  useProjectsStore.setState((s) => ({
    projects: s.projects.map((p) =>
      p.id === project.id ? { ...p, completionSoundEnabled: next ? 1 : 0 } : p,
    ),
  }))
  void emit('sync:projects').catch(() => {})
}

/** Settings → workspace Agent tab: per-workspace completion chime. */
export function WorkspaceCompletionSoundToggle({
  project,
}: {
  project: SoundProject
}): React.JSX.Element {
  const [busy, setBusy] = useState(false)
  const enabled = (project.completionSoundEnabled ?? 1) !== 0
  const globalOn = useSettingsStore((s) => s.completionSoundEnabled)

  const toggle = useCallback(async (): Promise<void> => {
    if (busy) return
    const next = !enabled
    setBusy(true)
    try {
      await writeWorkspaceCompletionSound(project, next)
    } catch (err) {
      console.error('[completion-sound] write failed', err)
    } finally {
      setBusy(false)
    }
  }, [busy, enabled, project.id, project.path])

  return (
    <div className="border border-[var(--color-border)] p-3">
    <div className="flex items-start gap-3">
      <button
        type="button"
        onClick={() => void toggle()}
        role="switch"
        aria-checked={enabled}
        disabled={busy}
        data-settings-id="projects.completion-sound"
        className={`mt-0.5 w-7 h-3.5 flex items-center transition-colors no-drag cursor-pointer flex-shrink-0 disabled:opacity-50 ${
          enabled ? 'bg-[var(--color-accent)]' : 'bg-[var(--color-border)]'
        }`}
        title={
          enabled
            ? 'Chime when an agent in this workspace finishes unwatched'
            : 'Muted — this workspace will not chime'
        }
      >
        <span
          className={`w-2.5 h-2.5 bg-[var(--color-on-accent)] block transition-transform ${
            enabled ? 'translate-x-3.5' : 'translate-x-0.5'
          }`}
        />
      </button>
      <div className="flex-1 min-w-0">
        <div className="text-xs font-medium text-[var(--color-text-primary)]">
          Completion sound
        </div>
        <div className="text-[10px] text-[var(--color-text-muted)] mt-0.5 leading-relaxed">
          Chime when an agent in this workspace finishes unwatched.
        </div>
        {!globalOn && (
          <div className="text-[10px] text-[var(--color-text-muted)] mt-1 leading-relaxed">
            Turned off globally in Settings → General.
          </div>
        )}
      </div>
    </div>
    </div>
  )
}

/** Drawer header bell: accent = workspace on, muted grey + slash = off. */
export function WorkspaceCompletionSoundBell({
  project,
}: {
  project: SoundProject
}): React.JSX.Element {
  const [busy, setBusy] = useState(false)
  const enabled = (project.completionSoundEnabled ?? 1) !== 0

  const toggle = useCallback(async (): Promise<void> => {
    if (busy) return
    const next = !enabled
    setBusy(true)
    try {
      await writeWorkspaceCompletionSound(project, next)
    } catch (err) {
      console.error('[completion-sound] write failed', err)
    } finally {
      setBusy(false)
    }
  }, [busy, enabled, project.id, project.path])

  return (
    <button
      type="button"
      onClick={() => void toggle()}
      role="switch"
      aria-checked={enabled}
      disabled={busy}
      className={`flex-shrink-0 p-0.5 no-drag cursor-pointer disabled:opacity-50 ${
        enabled
          ? 'text-[var(--color-accent)]'
          : 'text-[var(--color-text-muted)]'
      }`}
      title={
        enabled
          ? 'Completion sound on — click to mute this workspace'
          : 'Completion sound muted for this workspace'
      }
      aria-label={
        enabled
          ? 'Mute completion sound for this workspace'
          : 'Unmute completion sound for this workspace'
      }
    >
      {enabled ? <BellIcon /> : <BellOffIcon />}
    </button>
  )
}

function BellIcon(): React.JSX.Element {
  return (
    <svg
      className="w-3.5 h-3.5"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.75"
      strokeLinecap="round"
      strokeLinejoin="round"
    >
      <path d="M6 8a6 6 0 0 1 12 0c0 7 3 9 3 9H3s3-2 3-9" />
      <path d="M10.3 21a1.94 1.94 0 0 0 3.4 0" />
    </svg>
  )
}

function BellOffIcon(): React.JSX.Element {
  return (
    <svg
      className="w-3.5 h-3.5"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.75"
      strokeLinecap="round"
      strokeLinejoin="round"
    >
      <path d="M6 8a6 6 0 0 1 12 0c0 7 3 9 3 9H3s3-2 3-9" />
      <path d="M10.3 21a1.94 1.94 0 0 0 3.4 0" />
      <path d="M2 2l20 20" />
    </svg>
  )
}
