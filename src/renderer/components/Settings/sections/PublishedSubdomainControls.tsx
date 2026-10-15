import { useState } from 'react'
import { SettingDropdown } from '../controls/SettingControls'
import {
  isReservedNestedLabel,
  previewHost,
  validateLabel,
  validateTarget,
  type MutationResult,
  type PublishedSubdomainsApi,
} from '@/lib/published-subdomains'

// Settings → K2 Connect → URLs: the owner/admin controls for published
// (nested) subdomains — "Add published subdomain" plus per-row Change
// target / Remove. The calls live in lib/published-subdomains.ts; these
// components only collect input and show the outcome. The panel decides
// whether to render them at all (local server, owner or admin).

export const inputCls =
  'w-full px-2 py-1 text-xs bg-[var(--color-bg-surface)] border border-[var(--color-border)] text-[var(--color-text-primary)] outline-none focus:border-[var(--color-accent)] no-drag'

export const buttonCls =
  'px-3 py-1 text-[11px] border border-[var(--color-border)] text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-elevated)] no-drag cursor-pointer disabled:opacity-60 disabled:cursor-default'

const smallButtonCls =
  'px-1.5 py-0.5 text-[10px] border border-[var(--color-border)] text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-elevated)] no-drag cursor-pointer disabled:opacity-60 disabled:cursor-default'

const NO_WORKSPACE = ''

/** The note under a finished change. */
export function outcomeNote(verb: string, r: MutationResult): string {
  return r.refreshed ? `${verb}.` : `${verb}. The URL list updates within a minute.`
}

function errorText(e: unknown): string {
  return e instanceof Error ? e.message : String(e)
}

export function AddPublishedSubdomainForm({
  api,
  primary,
  publicUrl,
  workspaces,
}: {
  api: PublishedSubdomainsApi
  primary: string
  publicUrl: string | null
  workspaces: ReadonlyArray<{ id: string; name: string }>
}): React.JSX.Element {
  const [open, setOpen] = useState(false)
  const [label, setLabel] = useState('')
  const [target, setTarget] = useState('')
  const [owner, setOwner] = useState(NO_WORKSPACE)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [note, setNote] = useState<string | null>(null)
  const [warning, setWarning] = useState<string | null>(null)

  const labelCheck = validateLabel(label)
  const targetCheck = validateTarget(target)
  // Only nag about a field once something is typed in it.
  const labelError = label.trim() && !labelCheck.ok ? labelCheck.error : null
  const targetError = target.trim() && !targetCheck.ok ? targetCheck.error : null
  const canSubmit = labelCheck.ok && targetCheck.ok && !busy

  const ownerOptions = [
    { value: NO_WORKSPACE, label: 'None (this server)' },
    ...[...workspaces]
      .sort((a, b) => a.name.localeCompare(b.name))
      .map((w) => ({ value: w.id, label: w.name })),
  ]

  const reset = (): void => {
    setLabel('')
    setTarget('')
    setOwner(NO_WORKSPACE)
  }

  const submit = async (): Promise<void> => {
    if (!labelCheck.ok || !targetCheck.ok) return
    setBusy(true)
    setError(null)
    setNote(null)
    setWarning(null)
    try {
      const r = await api.create(labelCheck.value, targetCheck.value, {
        projectId: owner || null,
      })
      setNote(outcomeNote(`Added ${previewHost(labelCheck.value, primary, publicUrl)}`, r))
      setWarning(r.warning)
      reset()
      setOpen(false)
    } catch (e) {
      setError(errorText(e))
    } finally {
      setBusy(false)
    }
  }

  if (!open) {
    return (
      <div className="mt-2 space-y-1">
        <button type="button" className={buttonCls} onClick={() => setOpen(true)}>
          Add published subdomain
        </button>
        {note && <p className="text-[10px] text-[var(--color-status-ok-soft)]">{note}</p>}
        {warning && (
          <p className="text-[10px] text-[var(--color-status-warn-amber-soft)] leading-relaxed">{warning}</p>
        )}
      </div>
    )
  }

  return (
    <form
      className="mt-2 p-2 border border-[var(--color-border)] space-y-2"
      aria-label="Add published subdomain"
      onSubmit={(e) => {
        e.preventDefault()
        void submit()
      }}
    >
      <span className="text-[10px] font-medium text-[var(--color-text-muted)] uppercase tracking-wider">
        Add published subdomain
      </span>
      <label className="block">
        <span className="block text-[10px] text-[var(--color-text-muted)] mb-0.5">Label</span>
        <input
          className={inputCls}
          placeholder="staging"
          autoCapitalize="off"
          autoCorrect="off"
          spellCheck={false}
          value={label}
          onChange={(e) => setLabel(e.target.value)}
          aria-invalid={labelError ? true : undefined}
        />
      </label>
      <p className="text-[10px] font-mono text-[var(--color-text-secondary)]" data-testid="subdomain-preview">
        {previewHost(label, primary, publicUrl)}
      </p>
      {labelError && <p className="text-[10px] text-[var(--color-status-error-soft)]">{labelError}</p>}
      <label className="block">
        <span className="block text-[10px] text-[var(--color-text-muted)] mb-0.5">Target</span>
        <input
          className={inputCls}
          placeholder="localhost:<port>"
          autoCapitalize="off"
          autoCorrect="off"
          spellCheck={false}
          value={target}
          onChange={(e) => setTarget(e.target.value)}
          aria-invalid={targetError ? true : undefined}
        />
      </label>
      {targetError && <p className="text-[10px] text-[var(--color-status-error-soft)]">{targetError}</p>}
      <div>
        <span className="block text-[10px] text-[var(--color-text-muted)] mb-0.5">Workspace</span>
        <SettingDropdown
          value={owner}
          options={ownerOptions}
          onChange={(v) => setOwner(v)}
          menuAlign="left"
          ariaLabel="Workspace that owns this subdomain"
        />
      </div>
      {error && (
        <p role="alert" className="text-[10px] text-[var(--color-status-error-soft)] leading-relaxed">
          {error}
        </p>
      )}
      <div className="flex items-center gap-2">
        <button type="submit" className={buttonCls} disabled={!canSubmit}>
          {busy ? 'Adding…' : 'Add'}
        </button>
        <button
          type="button"
          className={buttonCls}
          disabled={busy}
          onClick={() => {
            reset()
            setError(null)
            setOpen(false)
          }}
        >
          Cancel
        </button>
      </div>
    </form>
  )
}

export function PublishedSubdomainRowActions({
  api,
  label,
  target,
  projectId,
  host,
  confirmRemove,
}: {
  api: PublishedSubdomainsApi
  label: string
  target: string
  projectId: string | null
  /** The full host shown in the confirm text. */
  host: string
  confirmRemove: (opts: {
    title: string
    message: string
    confirmLabel?: string
    destructive?: boolean
  }) => Promise<boolean>
}): React.JSX.Element {
  const [editing, setEditing] = useState(false)
  const [draft, setDraft] = useState(target)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [note, setNote] = useState<string | null>(null)

  const reserved = isReservedNestedLabel(label)
  const draftCheck = validateTarget(draft)
  const draftError = draft.trim() && !draftCheck.ok ? draftCheck.error : null

  const save = async (): Promise<void> => {
    if (!draftCheck.ok) return
    setBusy(true)
    setError(null)
    setNote(null)
    try {
      const r = await api.updateTarget(label, draftCheck.value, { projectId })
      setNote(r.warning ?? outcomeNote(`Now points to ${draftCheck.value}`, r))
      setEditing(false)
    } catch (e) {
      setError(errorText(e))
    } finally {
      setBusy(false)
    }
  }

  const remove = async (): Promise<void> => {
    const ok = await confirmRemove({
      title: 'Remove published subdomain?',
      message: `${host} will stop working for everyone. Your app on ${target} keeps running.`,
      confirmLabel: 'Remove',
      destructive: true,
    })
    if (!ok) return
    setBusy(true)
    setError(null)
    setNote(null)
    try {
      const r = await api.remove(label, { projectId })
      setNote(r.warning ?? outcomeNote('Removed', r))
    } catch (e) {
      setError(errorText(e))
    } finally {
      setBusy(false)
    }
  }

  return (
    <div className="col-span-full space-y-1">
      {editing ? (
        <form
          className="flex items-center gap-1.5"
          aria-label={`Change target for ${label}`}
          onSubmit={(e) => {
            e.preventDefault()
            void save()
          }}
        >
          <input
            className={inputCls}
            style={{ maxWidth: 200 }}
            placeholder="localhost:<port>"
            autoCapitalize="off"
            autoCorrect="off"
            spellCheck={false}
            value={draft}
            onChange={(e) => setDraft(e.target.value)}
            aria-label="New target"
          />
          <button type="submit" className={smallButtonCls} disabled={busy || !draftCheck.ok}>
            {busy ? 'Saving…' : 'Save'}
          </button>
          <button
            type="button"
            className={smallButtonCls}
            disabled={busy}
            onClick={() => {
              setDraft(target)
              setError(null)
              setEditing(false)
            }}
          >
            Cancel
          </button>
        </form>
      ) : (
        <div className="flex items-center gap-1.5">
          <button
            type="button"
            className={smallButtonCls}
            disabled={busy || reserved}
            title={reserved ? `'${label}' is reserved; it can be removed but not repointed.` : undefined}
            onClick={() => {
              setDraft(target)
              setNote(null)
              setEditing(true)
            }}
          >
            Change target
          </button>
          <button type="button" className={smallButtonCls} disabled={busy} onClick={() => void remove()}>
            {busy ? 'Removing…' : 'Remove'}
          </button>
        </div>
      )}
      {draftError && editing && (
        <p className="text-[10px] text-[var(--color-status-error-soft)]">{draftError}</p>
      )}
      {error && (
        <p role="alert" className="text-[10px] text-[var(--color-status-error-soft)] leading-relaxed">
          {error}
        </p>
      )}
      {note && <p className="text-[10px] text-[var(--color-text-muted)] leading-relaxed">{note}</p>}
    </div>
  )
}
