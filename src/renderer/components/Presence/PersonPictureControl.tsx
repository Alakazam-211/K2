// Set, replace or remove a person's profile picture (0.46.8, Rosson
// 2026-11-04). One control, two places: the profile modal (your own) and
// Settings → Admin Access (someone's, `person` given). The daemon decides
// who may change whose (Admin Access's rule: yours always; the owner's
// only by an Owner; anyone else's by an Admin+ who can act on their role)
// and re-encodes every picture; a refusal shows its reason here.
//
// The picture shown is the shared one (`usePersonPicture`), so a change
// made anywhere (another window, another device, an Admin) shows at once
// through `person_picture_changed`.

import { useRef, useState } from 'react'
import type { ServerScope } from '@/kessel/server-scope'
import {
  PICTURE_ACCEPT,
  clearPersonPicture,
  pictureFileProblem,
  readPictureFile,
  setPersonPicture,
  usePersonPicture,
} from '@/lib/person-pictures'

export interface PersonPictureControlProps {
  scope: ServerScope
  /** The login whose picture this is (`owner` or a username): what is
   *  shown. */
  login: string
  /** Send `person` (someone else's picture, Admin Access). Absent: the
   *  caller's own. */
  person?: string
  /** `buttons` (the profile modal) or `links` (an Admin Access row). */
  look?: 'buttons' | 'links'
  /** Test id prefix. */
  testId: string
}

export function PersonPictureControl({ scope, login, person, look = 'buttons', testId }: PersonPictureControlProps): React.JSX.Element {
  const picture = usePersonPicture(scope, login)
  const input = useRef<HTMLInputElement>(null)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const run = async (fn: () => Promise<unknown>): Promise<void> => {
    setBusy(true)
    setError(null)
    try {
      await fn()
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setBusy(false)
    }
  }

  const onPick = (file: File | undefined): void => {
    if (!file) return
    const problem = pictureFileProblem(file)
    if (problem) {
      setError(problem)
      return
    }
    void run(async () => setPersonPicture(scope, await readPictureFile(file), person))
  }

  const btn =
    look === 'links'
      ? 'text-[10px] text-[var(--color-text-muted)] hover:text-[var(--color-text-secondary)] hover:underline no-drag cursor-pointer disabled:opacity-60'
      : 'px-2 py-0.5 text-[11px] border border-[var(--color-border)] text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)] hover:bg-[var(--color-bg-hover)] no-drag cursor-pointer disabled:opacity-60'

  return (
    <span className="inline-flex flex-col gap-1 min-w-0" data-testid={testId}>
      <span className="inline-flex items-center gap-2">
        <input
          ref={input}
          type="file"
          accept={PICTURE_ACCEPT}
          className="hidden"
          aria-label={person ? `${person} picture file` : 'Picture file'}
          data-testid={`${testId}-file`}
          onChange={(e) => {
            onPick(e.target.files?.[0])
            e.target.value = ''
          }}
        />
        <button type="button" className={btn} disabled={busy} data-testid={`${testId}-choose`} onClick={() => input.current?.click()}>
          {busy ? 'Saving…' : picture ? 'Change picture' : look === 'links' ? 'Add picture' : 'Upload picture'}
        </button>
        {picture ? (
          <button
            type="button"
            className={btn}
            disabled={busy}
            data-testid={`${testId}-remove`}
            onClick={() => void run(() => clearPersonPicture(scope, person))}
          >
            Remove picture
          </button>
        ) : null}
      </span>
      {error ? (
        <span role="alert" className="text-[11px] text-[var(--color-status-error-soft)]" data-testid={`${testId}-error`}>
          {error}
        </span>
      ) : null}
    </span>
  )
}
