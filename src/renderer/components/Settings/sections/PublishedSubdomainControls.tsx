import { useState } from 'react'
import { SettingDropdown, SETTING_FIELD_WIDTH } from '../controls/SettingControls'
import { Button, Input } from '@/components/ui'
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

/** One labelled field row of the add form: label left, field on the
 *  Settings field width at the right. */
function FieldRow({ label, htmlFor, children }: { label: string; htmlFor?: string; children: React.ReactNode }): React.JSX.Element {
  return (
    <div className="flex items-start justify-between gap-6">
      <label htmlFor={htmlFor} className="type-row text-[var(--color-text-primary)] pt-1">
        {label}
      </label>
      <div className={`${SETTING_FIELD_WIDTH} flex-shrink-0 space-y-1`}>{children}</div>
    </div>
  )
}

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
      <div className="flex items-start justify-between gap-6">
        <div className="min-w-0">
          <p className="type-row text-[var(--color-text-primary)]">Add a site</p>
          <p className="type-hint text-[var(--color-text-secondary)] mt-0.5">Share a port on this computer at its own address.</p>
          {note && <p role="status" className="type-hint text-[var(--color-status-ok-soft)] mt-1">{note}</p>}
          {warning && (
            <p className="type-hint text-[var(--color-status-warn-text)] leading-relaxed mt-1">{warning}</p>
          )}
        </div>
        <Button className="flex-shrink-0" onClick={() => setOpen(true)}>
          Add a site
        </Button>
      </div>
    )
  }

  return (
    <form
      className="space-y-3"
      aria-label="Add a site"
      onSubmit={(e) => {
        e.preventDefault()
        void submit()
      }}
    >
      <p className="type-row text-[var(--color-text-primary)]">Add a site</p>
      <FieldRow label="Name" htmlFor="add-site-label">
        <Input
          id="add-site-label"
          className="w-full type-control"
          placeholder="staging"
          autoCapitalize="off"
          autoCorrect="off"
          spellCheck={false}
          value={label}
          onChange={(e) => setLabel(e.target.value)}
          aria-invalid={labelError ? true : undefined}
        />
        <p className="type-hint font-mono text-[var(--color-text-secondary)] break-all" data-testid="subdomain-preview">
          {previewHost(label, primary, publicUrl)}
        </p>
        {labelError && <p className="type-hint text-[var(--color-status-error-soft)]">{labelError}</p>}
      </FieldRow>
      <FieldRow label="Shows" htmlFor="add-site-target">
        <Input
          id="add-site-target"
          className="w-full type-control"
          placeholder="localhost:<port>"
          autoCapitalize="off"
          autoCorrect="off"
          spellCheck={false}
          value={target}
          onChange={(e) => setTarget(e.target.value)}
          aria-invalid={targetError ? true : undefined}
        />
        {targetError && <p className="type-hint text-[var(--color-status-error-soft)]">{targetError}</p>}
      </FieldRow>
      <FieldRow label="Workspace">
        <SettingDropdown
          value={owner}
          options={ownerOptions}
          onChange={(v) => setOwner(v)}
          fullWidth
          ariaLabel="Workspace that owns this site"
        />
      </FieldRow>
      {error && (
        <p role="alert" className="type-hint text-[var(--color-status-error-soft)] leading-relaxed">
          {error}
        </p>
      )}
      <div className="flex items-center justify-end gap-2">
        <Button
          variant="ghost"
          disabled={busy}
          onClick={() => {
            reset()
            setError(null)
            setOpen(false)
          }}
        >
          Cancel
        </Button>
        <Button type="submit" variant="accent" disabled={!canSubmit}>
          {busy ? 'Adding…' : 'Add'}
        </Button>
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
    <div className="flex flex-col items-end gap-1 max-w-[18rem]">
      {editing ? (
        <form
          className="flex items-center gap-1.5"
          aria-label={`Change target for ${label}`}
          onSubmit={(e) => {
            e.preventDefault()
            void save()
          }}
        >
          <Input
            className="w-40 type-control"
            placeholder="localhost:<port>"
            autoCapitalize="off"
            autoCorrect="off"
            spellCheck={false}
            value={draft}
            onChange={(e) => setDraft(e.target.value)}
            aria-label="New target"
          />
          <Button
            variant="ghost"
            disabled={busy}
            onClick={() => {
              setDraft(target)
              setError(null)
              setEditing(false)
            }}
          >
            Cancel
          </Button>
          <Button type="submit" variant="accent" disabled={busy || !draftCheck.ok}>
            {busy ? 'Saving…' : 'Save'}
          </Button>
        </form>
      ) : (
        <div className="flex items-center gap-1.5">
          <Button
            variant="quiet"
            disabled={busy || reserved}
            title={reserved ? `'${label}' is reserved; it can be removed but not repointed.` : undefined}
            onClick={() => {
              setDraft(target)
              setNote(null)
              setEditing(true)
            }}
          >
            Change target
          </Button>
          <Button variant="danger-quiet" disabled={busy} onClick={() => void remove()}>
            {busy ? 'Removing…' : 'Remove'}
          </Button>
        </div>
      )}
      {draftError && editing && (
        <p className="type-hint text-[var(--color-status-error-soft)]">{draftError}</p>
      )}
      {error && (
        <p role="alert" className="type-hint text-[var(--color-status-error-soft)] leading-relaxed text-right">
          {error}
        </p>
      )}
      {note && <p role="status" className="type-hint text-[var(--color-text-secondary)] leading-relaxed text-right">{note}</p>}
    </div>
  )
}
