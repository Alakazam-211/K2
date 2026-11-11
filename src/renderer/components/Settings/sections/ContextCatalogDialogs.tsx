// Settings → Context Catalog dialogs: New pack (name it, then write it) and
// Delete. Both use the Settings dialog frame shared with Admin Access
// (AccessDialogs.tsx): one width, top edge in one place, labelled
// full-width fields, the server's reason when a write fails.

import React, { useId, useState } from 'react'
import { Button, Input } from '@/components/ui'
import {
  createContextCatalogPack,
  contextErrorMessage,
  deleteContextCatalogPack,
  type ContextCatalogEntry,
  type CreatedCatalogPack,
} from '@/lib/context-stack'
import {
  AccessSheet as Sheet,
  DIALOG_FIELD as FIELD,
  DIALOG_MUTED as MUTED,
  FailedNote,
  Field,
} from './AccessDialogs'
import {
  createFailureNext,
  PACK_NAME_MAX,
  packNameProblem,
  SHORT_NAME_MAX,
  shortNameFrom,
  shortNameProblem,
} from './context-catalog-copy'

/** New pack: name, short name (made from the name until edited), tags.
 *  Done: it's in the catalog and empty; write it now or later. */
export function NewPackDialog({
  taken,
  onClose,
  onCreated,
  onWrite,
}: {
  /** Ids already in the catalog. */
  taken: readonly string[]
  /** Closed without writing it (Cancel, or Later after it was made). */
  onClose: (created: CreatedCatalogPack | null) => void
  /** The catalog changed (refresh it). */
  onCreated: (created: CreatedCatalogPack) => void
  /** Open the new pack to write it. */
  onWrite: (created: CreatedCatalogPack) => void
}): React.JSX.Element {
  const [name, setName] = useState('')
  const [slug, setSlug] = useState('')
  const [slugEdited, setSlugEdited] = useState(false)
  const [tags, setTags] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [created, setCreated] = useState<CreatedCatalogPack | null>(null)
  const ids = { name: useId(), slug: useId(), tags: useId(), form: useId() }

  const shortName = slugEdited ? slug.trim() : shortNameFrom(name)
  const nameProblem = packNameProblem(name)
  const slugProblem = shortNameProblem(shortName, taken)
  const ready = !!name.trim() && !nameProblem && !!shortName && !slugProblem
  const label = name.trim()

  const submit = async (): Promise<void> => {
    if (!ready || busy) return
    setBusy(true)
    setError(null)
    try {
      const made = await createContextCatalogPack({
        id: `user:${shortName}`,
        label,
        tags: tags
          .split(',')
          .map((t) => t.trim())
          .filter(Boolean),
      })
      setCreated(made)
      onCreated(made)
    } catch (e) {
      setError(contextErrorMessage(e, 'The server didn’t answer'))
    } finally {
      setBusy(false)
    }
  }

  if (created) {
    const shown = created.entry.label || label
    return (
      <Sheet
        title={`${shown} is in the catalog`}
        onClose={() => onClose(created)}
        testId="pack-created-dialog"
        footer={
          <>
            <Button variant="ghost" size="md" onClick={() => onClose(created)}>
              Later
            </Button>
            <Button variant="accent" size="md" onClick={() => onWrite(created)} autoFocus>
              Write it now
            </Button>
          </>
        }
      >
        <p className={MUTED}>
          It’s empty for now. Next, write what it asks of agents: an agent opens the pack with you, asks what it should say, and
          writes it down.
        </p>
        <p className={MUTED}>
          No workspace uses it yet. When it’s ready, add it to a workspace in Workspaces / Agents → the workspace → Context.
        </p>
      </Sheet>
    )
  }

  return (
    <Sheet
      title="New context pack"
      intro="Name it here. Next you write what it asks of agents, with an agent’s help. It isn’t added to any workspace until you add it."
      onClose={() => onClose(null)}
      busy={busy}
      testId="new-pack-dialog"
      footer={
        <>
          <Button variant="ghost" size="md" onClick={() => onClose(null)} disabled={busy}>
            Cancel
          </Button>
          <Button type="submit" form={ids.form} variant="accent" size="md" disabled={!ready || busy}>
            {busy ? 'Creating…' : 'Create pack'}
          </Button>
        </>
      }
    >
      <form
        id={ids.form}
        className="space-y-4"
        onSubmit={(e) => {
          e.preventDefault()
          void submit()
        }}
      >
        <Field
          label="Name"
          htmlFor={ids.name}
          hint="What people see in the catalog and in a workspace’s Context tab."
          problem={nameProblem}
        >
          <Input
            id={ids.name}
            autoFocus
            className={FIELD}
            maxLength={PACK_NAME_MAX}
            aria-invalid={nameProblem ? true : undefined}
            value={name}
            onChange={(e) => setName(e.target.value)}
          />
        </Field>
        <Field
          label="Short name"
          htmlFor={ids.slug}
          hint={
            shortName ? (
              <>
                Agents add it by this name: <span className="font-mono">user:{shortName}</span>. It can’t be changed later.
              </>
            ) : (
              'Made from the name. Agents add the pack by it, and it can’t be changed later.'
            )
          }
          problem={slugProblem}
        >
          <Input
            id={ids.slug}
            className={`${FIELD} font-mono`}
            autoCapitalize="none"
            autoCorrect="off"
            spellCheck={false}
            maxLength={SHORT_NAME_MAX + 1}
            aria-invalid={slugProblem ? true : undefined}
            value={shortName}
            onChange={(e) => {
              setSlugEdited(true)
              setSlug(e.target.value)
            }}
          />
        </Field>
        <Field label="Tags" htmlFor={ids.tags} hint="Optional. Words to find it by, separated by commas.">
          <Input id={ids.tags} className={FIELD} value={tags} onChange={(e) => setTags(e.target.value)} />
        </Field>

        {error ? <FailedNote what={`Couldn’t create ${label || 'the pack'}.`} detail={error} next={createFailureNext(error)} /> : null}
      </form>
    </Sheet>
  )
}

/** Delete a pack someone wrote, after a confirm. Cancel has the focus; a
 *  failure stays in the dialog and says the pack is still there. */
export function DeletePackDialog({
  pack,
  onClose,
  onDeleted,
}: {
  pack: ContextCatalogEntry
  onClose: () => void
  onDeleted: (pack: ContextCatalogEntry) => void
}): React.JSX.Element {
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const run = async (): Promise<void> => {
    if (busy) return
    setBusy(true)
    setError(null)
    try {
      await deleteContextCatalogPack(pack.id)
      onDeleted(pack)
    } catch (e) {
      setError(contextErrorMessage(e, 'The server didn’t answer'))
      setBusy(false)
    }
  }

  return (
    <Sheet
      title={`Delete ${pack.label}?`}
      onClose={onClose}
      busy={busy}
      testId="delete-pack-dialog"
      footer={
        <>
          <Button variant="ghost" size="md" onClick={onClose} disabled={busy} autoFocus>
            Cancel
          </Button>
          <Button variant="danger-muted" size="md" onClick={() => void run()} disabled={busy}>
            {busy ? 'Deleting…' : 'Delete pack'}
          </Button>
        </>
      }
    >
      <p className={MUTED}>
        K2 removes it, and what it says, from the catalog. Workspaces that already use it keep their own copy; remove it there in
        Workspaces / Agents → the workspace → Context.
      </p>
      {error ? (
        <FailedNote what={`Couldn’t delete ${pack.label}.`} detail={error} next="It’s still in the catalog. Try again in a moment." />
      ) : null}
    </Sheet>
  )
}
