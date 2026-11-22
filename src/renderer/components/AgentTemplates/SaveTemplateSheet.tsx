// Save as template (prd-agent-templates-v1 §7; the T5–T7 contract's T6 and
// T7): Settings → Workspaces → an agent → Save as template.
//
//   1. Details: the template's id (a slug), name and summary, and which wiki
//      notes go with it (none unless picked).
//   2. Review: the daemon's dry run (`POST /cli/agent-templates/save` with
//      `dryRun: true`): its files, context, suggested permissions, tool,
//      heartbeats and what it leaves out.
//   3. Save: the same request for real, then what was saved.
//
// A refusal is said by its code (an id that's built in or someone else's, a
// secret found in a file — named by file and line, never shown — too large,
// an agent saving another workspace). A server from before this route
// answers 404: the sheet says it can't save templates yet, and the row
// stops offering the button for that server.

import { useEffect, useId, useState } from 'react'
import { Button, Callout, Checkbox, Input, Spinner, Surface, Textarea } from '@/components/ui'
import { BookmarkIcon } from '@/components/icons/K2Icons'
import { DIALOG_FIELD, DIALOG_LABEL, DIALOG_MUTED } from '@/components/Settings/sections/AccessDialogs'
import { SettingRow } from '@/components/Settings/controls/SettingControls'
import { fetchWikiIndex, isWikiArticleNode, type WikiNode } from '@/components/Wiki/wiki-api'
import { asArray } from '@/lib/as-array'
import { primaryScope } from '@/kessel/server-scope'
import {
  SAVE_UNSUPPORTED_TEXT,
  authorLabel,
  listTemplates,
  saveErrorView,
  saveTemplate,
  type SaveErrorView,
  type SaveTemplateRequest,
  type SaveTemplateResult,
} from '@/lib/agent-templates-api'
import { slugifyAddressToken } from '@/lib/federation'
import { openCreateAgentSheet, useAgentTemplatesStore } from '@/stores/agent-templates'
import { ReviewCard, ReviewList, ReviewRow, ReviewSection, TemplateSheetFrame, type ReviewItem } from './TemplateSheetFrame'
import { contextWords, fileWords, humanize, permissionWords, toolWords } from './review-words'
import { PRODUCT_NAME } from '@/lib/brand'

export const SAVE_TEMPLATE_COPY = {
  button: 'Save as template',
  rowLabel: 'Make more agents like this one',
  rowHint: 'Keeps this agent’s setup so you can make more agents like it.',
  details: 'Save as template',
  detailsIntro: 'A template keeps this agent’s charter, context, skills and heartbeats. Never its chats or secrets.',
  wiki: 'Wiki notes to include',
  wikiHint: 'None go unless you pick them.',
  review: 'Review',
  reviewIntro: 'This is everything the template will hold.',
  save: 'Save template',
  saving: 'Saving…',
  done: 'Template saved',
  createFrom: 'Create an agent from it',
  tryAgain: 'Try again',
} as const

/** After step 1 on a server without the save: what happened and what next. */
export const SAVE_UNSUPPORTED_NEXT = `Nothing was saved. Update ${PRODUCT_NAME} on this server to save templates.`
/** The id's rule, under its field. */
export const SAVE_ID_HINT = 'Lowercase letters, numbers and single hyphens. Saving again with the same id makes a new version.'

export const SAVE_TEMPLATE_CLOSE_HELD = 'Can’t close while the template saves'

const SKIP_WHY: Record<string, string> = {
  k2_managed: `${PRODUCT_NAME} provides it`,
}

export function idProblem(id: string): string | null {
  const v = id.trim()
  if (!v) return 'Give the template an id.'
  return slugifyAddressToken(v) === v ? null : 'Use lowercase letters, numbers and single hyphens.'
}

export function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`
  return `${(n / 1024).toFixed(n < 10 * 1024 ? 1 : 0)} KB`
}

/** The row in Settings → Workspaces → an agent: the button, or why not. */
export function SaveTemplateRow({ project }: { project: { path: string; name: string } }): React.JSX.Element {
  const scope = primaryScope()
  const hostKey = scope.hostKey
  const unsupported = useAgentTemplatesStore((s) => !!s.saveUnsupported[hostKey])
  const markUnsupported = useAgentTemplatesStore((s) => s.markSaveUnsupported)
  const [open, setOpen] = useState(false)
  // The list says up front whether this server can save (`canSave`). A
  // server from before the flag leaves it out: the button stays, and a 404
  // from the save marks it then.
  useEffect(() => {
    if (unsupported) return
    let cancelled = false
    listTemplates(scope).then(
      (r) => {
        if (!cancelled && r?.canSave === false) markUnsupported(hostKey)
      },
      () => {
        /* unknown: keep the button; the save itself says why it can't */
      },
    )
    return () => {
      cancelled = true
    }
  }, [scope, hostKey, unsupported, markUnsupported])
  return (
    <SettingRow label={SAVE_TEMPLATE_COPY.rowLabel} hint={unsupported ? SAVE_UNSUPPORTED_TEXT : SAVE_TEMPLATE_COPY.rowHint} testId="save-template-row">
      {unsupported ? null : (
        <Button variant="quiet" onClick={() => setOpen(true)} data-testid="save-template-open">
          {SAVE_TEMPLATE_COPY.button}
        </Button>
      )}
      {open ? <SaveTemplateSheet project={project} onClose={() => setOpen(false)} /> : null}
    </SettingRow>
  )
}

type Phase = 'details' | 'review' | 'done'

export function SaveTemplateSheet({ project, onClose }: { project: { path: string; name: string }; onClose: () => void }): React.JSX.Element {
  const scope = primaryScope()
  const markUnsupported = useAgentTemplatesStore((s) => s.markSaveUnsupported)
  const ids = { id: useId(), idHint: useId(), name: useId(), summary: useId(), wiki: useId() }

  const [phase, setPhase] = useState<Phase>('details')
  const [id, setId] = useState(() => slugifyAddressToken(project.name) ?? '')
  const [name, setName] = useState(project.name)
  const [summary, setSummary] = useState('')
  const [wiki, setWiki] = useState<string[]>([])
  const [notes, setNotes] = useState<string[] | null>(null)
  const [notesError, setNotesError] = useState(false)
  const [showProblems, setShowProblems] = useState(false)

  const [busy, setBusy] = useState(false)
  const [plan, setPlan] = useState<SaveTemplateResult | null>(null)
  const [saved, setSaved] = useState<SaveTemplateResult | null>(null)
  const [error, setError] = useState<SaveErrorView | null>(null)

  useEffect(() => {
    let cancelled = false
    fetchWikiIndex(project.path).then(
      (idx) => {
        if (cancelled) return
        setNotes(asArray<WikiNode>(idx?.nodes).filter(isWikiArticleNode).map((n) => n.id).sort((a, b) => a.localeCompare(b)))
      },
      () => {
        if (!cancelled) {
          setNotes([])
          setNotesError(true)
        }
      },
    )
    return () => {
      cancelled = true
    }
  }, [project.path])

  const request = (dryRun: boolean): SaveTemplateRequest => ({
    workspace: project.path,
    id: id.trim(),
    ...(name.trim() ? { name: name.trim() } : {}),
    ...(summary.trim() ? { summary: summary.trim() } : {}),
    wiki,
    dryRun,
  })

  const fail = (e: unknown): void => {
    const v = saveErrorView(e)
    if (v.code === 'unsupported') markUnsupported(scope.hostKey)
    setError(v)
    if (v.fieldsProblem) setPhase('details')
  }

  const review = async (): Promise<void> => {
    if (idProblem(id)) return setShowProblems(true)
    setBusy(true)
    setError(null)
    try {
      setPlan(await saveTemplate(scope, request(true)))
      setPhase('review')
    } catch (e) {
      fail(e)
    } finally {
      setBusy(false)
    }
  }

  const save = async (): Promise<void> => {
    setBusy(true)
    setError(null)
    try {
      setSaved(await saveTemplate(scope, request(false)))
      setPhase('done')
    } catch (e) {
      fail(e)
    } finally {
      setBusy(false)
    }
  }

  const frame = { testId: 'save-template-sheet', busy, heldReason: SAVE_TEMPLATE_CLOSE_HELD, onCancel: onClose }
  const status = busy ? (
    <p className="flex items-center gap-2 type-hint text-[var(--color-text-secondary)]" role="status" data-testid="save-template-busy">
      <Spinner />
      <span>{phase === 'review' ? SAVE_TEMPLATE_COPY.saving : 'Checking…'}</span>
    </p>
  ) : null
  const errorNote = error ? (
    <Callout tone="error" role="alert" className="type-hint space-y-0.5" data-testid="save-template-error" data-code={error.code ?? undefined}>
      <p>{error.text}</p>
      {error.detail ? <p>The server said: {error.detail.trim().replace(/[.\s]+$/, '')}.</p> : null}
    </Callout>
  ) : null
  const unsupported = error?.code === 'unsupported'

  if (phase === 'done' && saved) {
    return (
      <TemplateSheetFrame
        {...frame}
        title={SAVE_TEMPLATE_COPY.done}
        focusKey="done"
        footer={
          <>
            <Button variant="ghost" size="md" onClick={onClose} data-testid="save-template-close">
              Close
            </Button>
            <Button
              variant="accent"
              size="md"
              onClick={() => {
                onClose()
                openCreateAgentSheet(saved.id)
              }}
              data-testid="save-template-create-from"
            >
              {SAVE_TEMPLATE_COPY.createFrom}
            </Button>
          </>
        }
      >
        <div className="space-y-3" data-testid="save-template-done">
          <Surface chamfer material="tile" className="flex items-start gap-3 px-4 py-3">
            <BookmarkIcon size={16} className="mt-px flex-shrink-0 text-[var(--color-accent-text)]" />
            <div className="min-w-0 space-y-0.5">
              <p className="type-row text-[var(--color-text-primary)]">
                {saved.id}
                <span className="text-[var(--color-text-secondary)]"> (version {saved.version})</span>
              </p>
              <p className="type-hint text-[var(--color-text-secondary)] select-text [overflow-wrap:anywhere]">{saved.path}</p>
            </div>
          </Surface>
          <p className={DIALOG_MUTED}>It’s under “Yours” when you create an agent.</p>
          <SaveHolds plan={saved} />
        </div>
      </TemplateSheetFrame>
    )
  }

  if (phase === 'review' && plan) {
    return (
      <TemplateSheetFrame
        {...frame}
        title={SAVE_TEMPLATE_COPY.review}
        intro={SAVE_TEMPLATE_COPY.reviewIntro}
        step={{ at: 2, of: 2 }}
        focusKey="review"
        onSubmit={() => void save()}
        status={status}
        footer={
          <>
            <Button variant="ghost" size="md" disabled={busy} onClick={() => setPhase('details')} data-testid="save-template-back">
              Back
            </Button>
            {unsupported ? null : (
              <Button type="submit" variant="accent" size="md" disabled={busy} data-testid="save-template-save">
                {busy ? SAVE_TEMPLATE_COPY.saving : error ? SAVE_TEMPLATE_COPY.tryAgain : SAVE_TEMPLATE_COPY.save}
              </Button>
            )}
          </>
        }
      >
        <SaveReview plan={plan} />
        {errorNote}
      </TemplateSheetFrame>
    )
  }

  const problem = showProblems ? idProblem(id) : null
  return (
    <TemplateSheetFrame
      {...frame}
      title={SAVE_TEMPLATE_COPY.details}
      intro={SAVE_TEMPLATE_COPY.detailsIntro}
      step={{ at: 1, of: 2 }}
      focusKey="details"
      onSubmit={() => (unsupported ? onClose() : void review())}
      status={status}
      footer={
        <>
          <Button variant="ghost" size="md" disabled={busy} onClick={onClose} data-testid="save-template-cancel">
            {unsupported ? 'Close' : 'Cancel'}
          </Button>
          {unsupported ? null : (
            <Button type="submit" variant="accent" size="md" disabled={busy || !!problem} data-testid="save-template-next">
              Next
            </Button>
          )}
        </>
      }
    >
      {unsupported ? (
        <div className="space-y-1">
          <p className={DIALOG_MUTED} data-testid="save-template-unsupported">
            {SAVE_UNSUPPORTED_TEXT}
          </p>
          <p className={DIALOG_MUTED} data-testid="save-template-unsupported-next">
            {SAVE_UNSUPPORTED_NEXT}
          </p>
        </div>
      ) : (
        <>
          <div>
            <label htmlFor={ids.id} className={DIALOG_LABEL}>
              Template id
            </label>
            <Input
              id={ids.id}
              className={DIALOG_FIELD}
              value={id}
              autoComplete="off"
              spellCheck={false}
              aria-invalid={problem || error?.fieldsProblem ? true : undefined}
              aria-describedby={ids.idHint}
              onChange={(e) => setId(e.target.value)}
              data-testid="save-template-id"
            />
            <div id={ids.idHint}>
              {problem ? (
                <p className="type-hint text-[var(--color-status-error-soft)] mt-1" data-testid="save-template-id-problem">
                  {problem}
                </p>
              ) : (
                <p className={`${DIALOG_MUTED} mt-1`}>{SAVE_ID_HINT}</p>
              )}
            </div>
          </div>
          <div>
            <label htmlFor={ids.name} className={DIALOG_LABEL}>
              Name
            </label>
            <Input id={ids.name} className={DIALOG_FIELD} value={name} autoComplete="off" onChange={(e) => setName(e.target.value)} data-testid="save-template-name" />
          </div>
          <div>
            <label htmlFor={ids.summary} className={DIALOG_LABEL}>
              Summary <span className="text-[var(--color-text-secondary)]">(optional)</span>
            </label>
            <Textarea id={ids.summary} className={DIALOG_FIELD} rows={2} value={summary} onChange={(e) => setSummary(e.target.value)} data-testid="save-template-summary" />
          </div>
          <fieldset className="p-0 border-0 min-w-0 space-y-2" aria-describedby={ids.wiki} data-testid="save-template-wiki">
            <legend className={`${DIALOG_LABEL} p-0`}>{SAVE_TEMPLATE_COPY.wiki}</legend>
            <p id={ids.wiki} className={DIALOG_MUTED}>
              {SAVE_TEMPLATE_COPY.wikiHint}
            </p>
            {notes === null ? (
              <div className="flex items-center gap-2" role="status">
                <Spinner />
                <span className={DIALOG_MUTED}>Loading…</span>
              </div>
            ) : notes.length === 0 ? (
              <p className={DIALOG_MUTED} data-testid="save-template-no-notes">
                {notesError ? `${PRODUCT_NAME} couldn’t read this agent’s wiki.` : 'This agent has no wiki notes.'}
              </p>
            ) : (
              <Surface chamfer material="tile" className="divide-y divide-[var(--color-border)]" data-testid="save-template-notes">
                {notes.map((n) => (
                  <label key={n} className="flex items-start gap-2.5 px-3.5 py-2 cursor-pointer">
                    <Checkbox
                      className="mt-0.5"
                      checked={wiki.includes(n)}
                      onChange={(e) => setWiki((w) => (e.target.checked ? [...w, n] : w.filter((x) => x !== n)))}
                      data-testid={`save-template-note-${n}`}
                    />
                    <span className="type-hint text-[var(--color-text-primary)] [overflow-wrap:anywhere]">{n}</span>
                  </label>
                ))}
              </Surface>
            )}
          </fieldset>
          {errorNote}
        </>
      )}
    </TemplateSheetFrame>
  )
}

/** The dry run, drawn: the template on one card, then what it holds. */
function SaveReview({ plan }: { plan: SaveTemplateResult }): React.JSX.Element {
  return (
    <>
      <ReviewCard testId="save-template-review-head">
        <ReviewRow label="Template">
          {plan.id}
          <span className="text-[var(--color-text-secondary)]"> (version {plan.version})</span>
        </ReviewRow>
        <ReviewRow label="By">{authorLabel(plan.author)}</ReviewRow>
        <ReviewRow label="Saved in">
          <span className="select-text">{plan.path}</span>
        </ReviewRow>
      </ReviewCard>
      <SaveHolds plan={plan} />
    </>
  )
}

/** Everything a template holds and leaves out, in plain names with the raw
 *  path or id under each, every list the server returned (an empty one said
 *  so). The review shows it before saving; the saved state shows it again. */
function SaveHolds({ plan: raw }: { plan: SaveTemplateResult }): React.JSX.Element {
  // Every list through asArray: an older or odd body never breaks the map.
  const plan = {
    ...raw,
    files: asArray<SaveTemplateResult['files'][number]>(raw.files),
    catalog: asArray<string>(raw.catalog),
    suggest: asArray<string>(raw.suggest),
    heartbeats: asArray<string>(raw.heartbeats),
    skipped: asArray<SaveTemplateResult['skipped'][number]>(raw.skipped),
  }
  const none = <span className="text-[var(--color-text-secondary)]">None</span>
  const list = (items: ReviewItem[], testId: string): React.JSX.Element => (items.length ? <ReviewList testId={testId} items={items} /> : none)
  return (
    <>
      <ReviewSection title="What it holds">
        <ReviewCard>
          <ReviewRow label="Files" testId="save-template-files">
            {list(
              plan.files.map((f) => ({ key: f.path, ...fileWords(f.path), note: formatBytes(f.bytes), testId: 'save-template-file' })),
              'save-template-files-list',
            )}
          </ReviewRow>
          <ReviewRow label="Context" testId="save-template-catalog">
            {list(plan.catalog.map((x) => ({ key: x, what: contextWords(x), raw: x })), 'save-template-catalog-list')}
          </ReviewRow>
          <ReviewRow label="Permissions" testId="save-template-suggest">
            {list(plan.suggest.map((x) => ({ key: x, what: permissionWords(x), raw: x, note: 'Suggested' })), 'save-template-suggest-list')}
          </ReviewRow>
          <ReviewRow label="Tool" testId="save-template-tool">
            {plan.tool ? toolWords(plan.tool) : <span className="text-[var(--color-text-secondary)]">The server default</span>}
          </ReviewRow>
          <ReviewRow label="Heartbeats" testId="save-template-heartbeats">
            {list(plan.heartbeats.map((x) => ({ key: x, what: humanize(x), raw: x })), 'save-template-heartbeats-list')}
          </ReviewRow>
        </ReviewCard>
      </ReviewSection>
      <ReviewSection title="Left out" hint="Chats and secrets are never included. Nothing else is, unless it’s listed here." testId="save-template-skipped">
        {plan.skipped.length ? (
          <ReviewCard>
            {plan.skipped.map((x) => (
              <ReviewRow key={`${x.what}-${x.name}`} label={x.what ? humanize(x.what) : x.what} testId="save-template-skip">
                <ReviewList items={[{ key: x.name, what: `${humanize(x.name)} (${x.what})`, raw: x.name, note: SKIP_WHY[x.why] ?? x.why }]} />
              </ReviewRow>
            ))}
          </ReviewCard>
        ) : null}
      </ReviewSection>
    </>
  )
}
