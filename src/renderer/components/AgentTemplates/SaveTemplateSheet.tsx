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
import { Button, Callout, Checkbox, Input, Spinner, Textarea } from '@/components/ui'
import { DIALOG_FIELD, DIALOG_LABEL, DIALOG_MUTED } from '@/components/Settings/sections/AccessDialogs'
import { SettingRow } from '@/components/Settings/controls/SettingControls'
import { fetchWikiIndex, isWikiArticleNode, type WikiNode } from '@/components/Wiki/wiki-api'
import { asArray } from '@/lib/as-array'
import { primaryScope } from '@/kessel/server-scope'
import {
  SAVE_UNSUPPORTED_TEXT,
  authorLabel,
  saveErrorView,
  saveTemplate,
  type SaveErrorView,
  type SaveTemplateRequest,
  type SaveTemplateResult,
} from '@/lib/agent-templates-api'
import { slugifyAddressToken } from '@/lib/federation'
import { useAgentTemplatesStore } from '@/stores/agent-templates'
import { ReviewGroup, ReviewLine, TemplateSheetFrame } from './TemplateSheetFrame'

export const SAVE_TEMPLATE_COPY = {
  button: 'Save as template',
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
  tryAgain: 'Try again',
} as const

export const SAVE_TEMPLATE_CLOSE_HELD = 'Can’t close while the template saves'

const SKIP_WHY: Record<string, string> = {
  k2_managed: 'K2 provides it',
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
  const hostKey = primaryScope().hostKey
  const unsupported = useAgentTemplatesStore((s) => !!s.saveUnsupported[hostKey])
  const [open, setOpen] = useState(false)
  return (
    <SettingRow label={SAVE_TEMPLATE_COPY.button} hint={unsupported ? SAVE_UNSUPPORTED_TEXT : SAVE_TEMPLATE_COPY.rowHint}>
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
  const ids = { id: useId(), name: useId(), summary: useId(), wiki: useId() }

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
          <Button variant="accent" size="md" onClick={onClose} data-testid="save-template-close">
            Close
          </Button>
        }
      >
        <div className="space-y-0.5" data-testid="save-template-done">
          <p className="type-row text-[var(--color-text-primary)]">
            {saved.id}, version {saved.version}
          </p>
          <p className={`${DIALOG_MUTED} font-mono [overflow-wrap:anywhere]`}>{saved.path}</p>
          <p className={DIALOG_MUTED}>It’s under “Yours” when you create an agent.</p>
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
            <Button type="submit" variant="accent" size="md" disabled={busy} data-testid="save-template-next">
              Next
            </Button>
          )}
        </>
      }
    >
      {unsupported ? (
        <p className={DIALOG_MUTED} data-testid="save-template-unsupported">
          {SAVE_UNSUPPORTED_TEXT}
        </p>
      ) : (
        <>
          <div>
            <label htmlFor={ids.id} className={DIALOG_LABEL}>
              Id
            </label>
            <Input
              id={ids.id}
              className={`${DIALOG_FIELD} font-mono`}
              value={id}
              autoComplete="off"
              spellCheck={false}
              aria-invalid={problem || error?.fieldsProblem ? true : undefined}
              onChange={(e) => setId(e.target.value)}
              data-testid="save-template-id"
            />
            {problem ? (
              <p className="type-hint text-[var(--color-status-error-soft)] mt-1" data-testid="save-template-id-problem">
                {problem}
              </p>
            ) : (
              <p className={`${DIALOG_MUTED} mt-1`}>Saving again with the same id makes a new version.</p>
            )}
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
          <fieldset className="space-y-1.5" aria-describedby={ids.wiki} data-testid="save-template-wiki">
            <legend className={DIALOG_LABEL}>{SAVE_TEMPLATE_COPY.wiki}</legend>
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
                {notesError ? 'K2 couldn’t read this agent’s wiki.' : 'This agent has no wiki notes.'}
              </p>
            ) : (
              notes.map((n) => (
                <label key={n} className="flex items-start gap-2 cursor-pointer">
                  <Checkbox
                    className="mt-0.5"
                    checked={wiki.includes(n)}
                    onChange={(e) => setWiki((w) => (e.target.checked ? [...w, n] : w.filter((x) => x !== n)))}
                    data-testid={`save-template-note-${n}`}
                  />
                  <span className="type-hint text-[var(--color-text-primary)] [overflow-wrap:anywhere]">{n}</span>
                </label>
              ))
            )}
          </fieldset>
          {errorNote}
        </>
      )}
    </TemplateSheetFrame>
  )
}

/** The dry run, drawn: every list it returned, empty ones said so. */
function SaveReview({ plan: raw }: { plan: SaveTemplateResult }): React.JSX.Element {
  // Every list through asArray: an older or odd body never breaks the map.
  const plan = {
    ...raw,
    files: asArray<SaveTemplateResult['files'][number]>(raw.files),
    catalog: asArray<string>(raw.catalog),
    suggest: asArray<string>(raw.suggest),
    heartbeats: asArray<string>(raw.heartbeats),
    skipped: asArray<SaveTemplateResult['skipped'][number]>(raw.skipped),
  }
  const none = <p className={DIALOG_MUTED}>None</p>
  const list = (items: string[], testId: string): React.JSX.Element =>
    items.length ? (
      <ul className="m-0 p-0 list-none" data-testid={testId}>
        {items.map((x) => (
          <ReviewLine key={x} what={x} />
        ))}
      </ul>
    ) : (
      none
    )
  return (
    <>
      <div className="space-y-0.5" data-testid="save-template-review-head">
        <p className="type-row text-[var(--color-text-primary)]">
          {plan.id}, version {plan.version}
        </p>
        <p className={DIALOG_MUTED}>By {authorLabel(plan.author)}</p>
        <p className={`${DIALOG_MUTED} font-mono [overflow-wrap:anywhere]`}>{plan.path}</p>
      </div>
      <ReviewGroup title="Files" testId="save-template-files">
        {plan.files.length ? (
          <ul className="m-0 p-0 list-none">
            {plan.files.map((f) => (
              <ReviewLine key={f.path} what={f.path} state={formatBytes(f.bytes)} testId="save-template-file" />
            ))}
          </ul>
        ) : (
          none
        )}
      </ReviewGroup>
      <ReviewGroup title="Context" testId="save-template-catalog">
        {list(plan.catalog, 'save-template-catalog-list')}
      </ReviewGroup>
      <ReviewGroup title="Suggested permissions" testId="save-template-suggest">
        {list(plan.suggest, 'save-template-suggest-list')}
      </ReviewGroup>
      <ReviewGroup title="Tool" testId="save-template-tool">
        {plan.tool ? <p className="type-hint text-[var(--color-text-primary)] font-mono">{plan.tool}</p> : <p className={DIALOG_MUTED}>The server default</p>}
      </ReviewGroup>
      <ReviewGroup title="Heartbeats" testId="save-template-heartbeats">
        {list(plan.heartbeats, 'save-template-heartbeats-list')}
      </ReviewGroup>
      <ReviewGroup title="Left out" testId="save-template-skipped">
        {plan.skipped.length ? (
          <ul className="m-0 p-0 list-none">
            {plan.skipped.map((s) => (
              <ReviewLine key={`${s.what}-${s.name}`} what={`${s.what}: ${s.name}`} state={SKIP_WHY[s.why] ?? s.why} testId="save-template-skip" />
            ))}
          </ul>
        ) : (
          none
        )}
      </ReviewGroup>
    </>
  )
}
