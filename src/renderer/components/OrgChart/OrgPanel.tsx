// One org, opened from its bucket (Rosson 11-10): its purpose (a short
// paragraph, ≤ 400 characters), its bulletin (at most 5 lines, 160
// characters each, live: a head's or an Admin's edit lands at once), its
// head and its members. Whoever may manage the chart edits the name,
// purpose and head; an Owner or Admin person writes the bulletin; a Member
// reads.
// The 5-line and 160-character limits are the daemon's; the panel says the
// same sentence before it sends.

import { useEffect, useRef, useState } from 'react'
import { Button, Input, Surface } from '@/components/ui'
import type { Org, OrgAgent } from './org-api'
import { BULLETIN_LINE_MAX, BULLETIN_MAX_LINES, ORG_NAME_MAX, PURPOSE_MAX } from './org-api'

/** The daemon's refusal for a bulletin, or null when it may be sent. */
export function bulletinProblem(lines: string[]): string | null {
  if (!lines.length) return 'An org bulletin needs at least one line.'
  if (lines.length > BULLETIN_MAX_LINES) {
    const cut = lines.length - BULLETIN_MAX_LINES
    return `An org bulletin is at most ${BULLETIN_MAX_LINES} lines; this one has ${lines.length}. Cut ${cut} line${cut === 1 ? '' : 's'} and send it again.`
  }
  const long = lines.findIndex((l) => l.length > BULLETIN_LINE_MAX)
  if (long >= 0) return `Line ${long + 1} is ${lines[long]!.length} characters; the most is ${BULLETIN_LINE_MAX}.`
  return null
}

/** A textarea's text as bulletin lines (blank lines dropped, ends trimmed). */
export function bulletinLines(text: string): string[] {
  return text
    .split('\n')
    .map((l) => l.trim())
    .filter(Boolean)
}

/** A bulletin's `updatedAt` (unix seconds) as people read it. */
function when(seconds: number): string {
  const d = new Date(seconds * 1000)
  return Number.isFinite(d.getTime()) ? d.toLocaleString(undefined, { month: 'short', day: 'numeric', hour: 'numeric', minute: '2-digit' }) : ''
}

const muted = 'text-[var(--color-text-muted)]'
const field =
  'w-full resize-none border border-[var(--color-border)] bg-[var(--color-bg-inset)] px-2 py-1.5 type-row text-[var(--color-text-primary)] outline-none focus:border-[var(--color-accent)] rounded-[var(--radius-field)] disabled:opacity-70'

export function OrgPanel({
  org,
  agents,
  canManage,
  canEditBulletin,
  onClose,
  onRename,
  onPurpose,
  onBulletin,
  onHead,
  onDelete,
}: {
  org: Org
  agents: Map<string, OrgAgent>
  /** Name, purpose, head, delete (the chart's `canManage`). */
  canManage: boolean
  /** The bulletin: an Owner or Admin person (the chart's `canEdit`). */
  canEditBulletin: boolean
  onClose: () => void
  onRename: (name: string) => Promise<void>
  onPurpose: (purpose: string) => Promise<void>
  onBulletin: (lines: string[]) => Promise<void>
  onHead: (agent: string) => void
  onDelete: () => void
}): React.JSX.Element {
  const [name, setName] = useState(org.name)
  const [purpose, setPurpose] = useState(org.purpose)
  const [bulletin, setBulletin] = useState((org.bulletin?.lines ?? []).join('\n'))
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState<string | null>(null)

  // Another person's or the head's edit while this is open: show it in each
  // field this panel hasn't been typed in (unsent words are never replaced).
  const edited = useRef({ name: false, purpose: false, bulletin: false })
  useEffect(() => {
    if (!edited.current.name) setName(org.name)
    if (!edited.current.purpose) setPurpose(org.purpose)
    if (!edited.current.bulletin) setBulletin((org.bulletin?.lines ?? []).join('\n'))
  }, [org])

  const lines = bulletinLines(bulletin)
  const problem = bulletin.trim() ? bulletinProblem(lines) : null
  const bulletinDirty = lines.join('\n') !== (org.bulletin?.lines ?? []).join('\n')
  const purposeDirty = purpose.trim() !== org.purpose

  const run = async (what: 'name' | 'purpose' | 'bulletin', fn: () => Promise<void>): Promise<void> => {
    setBusy(what)
    setError(null)
    try {
      await fn()
      edited.current[what] = false
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setBusy(null)
    }
  }

  const head = org.headId ? agents.get(org.headId) ?? null : null
  const members = org.memberIds.map((id) => agents.get(id)).filter((a): a is OrgAgent => !!a)

  return (
    <Surface
      role2="surface"
      chamfer
      material="raised"
      elevation={2}
      data-testid="org-panel"
      data-org-panel={org.id}
      className="flex h-full w-[340px] flex-shrink-0 flex-col overflow-hidden"
      onPointerDown={(e) => e.stopPropagation()}
    >
      <div className="flex items-center gap-2 border-b border-[var(--color-border)] px-3 py-2">
        {canManage ? (
          <Input
            value={name}
            maxLength={ORG_NAME_MAX}
            aria-label="Org name"
            onChange={(e) => {
              edited.current.name = true
              setName(e.target.value)
            }}
            onBlur={() => {
              const next = name.trim()
              if (next && next !== org.name) void run('name', () => onRename(next))
              else {
                edited.current.name = false
                setName(org.name)
              }
            }}
            onKeyDown={(e) => {
              if (e.key === 'Enter') (e.target as HTMLInputElement).blur()
            }}
          />
        ) : (
          <h2 className="truncate type-heading text-[var(--color-text-primary)]">{org.name}</h2>
        )}
        <button
          type="button"
          aria-label="Close"
          title="Close (Esc)"
          onClick={onClose}
          className="ml-auto flex h-6 w-6 flex-shrink-0 items-center justify-center text-[var(--color-text-muted)] hover:bg-[var(--color-bg-hover)] hover:text-[var(--color-text-primary)]"
        >
          <svg width="12" height="12" viewBox="0 0 12 12" fill="none" stroke="currentColor" strokeWidth="1.5">
            <line x1="2" y1="2" x2="10" y2="10" />
            <line x1="10" y1="2" x2="2" y2="10" />
          </svg>
        </button>
      </div>

      <div className="flex-1 space-y-4 overflow-y-auto px-3 py-3">
        <section className="space-y-1">
          <div className={`type-section uppercase tracking-wider ${muted}`}>Purpose</div>
          {canManage ? (
            <>
              <textarea
                className={field}
                rows={4}
                maxLength={PURPOSE_MAX}
                value={purpose}
                aria-label="Purpose"
                placeholder="What this org does, and what to bring to it."
                onChange={(e) => {
                  edited.current.purpose = true
                  setPurpose(e.target.value)
                }}
              />
              <div className="flex items-center gap-2">
                <span className={`type-hint ${muted}`}>
                  {purpose.length} / {PURPOSE_MAX}
                </span>
                <Button
                  size="sm"
                  className="ml-auto"
                  disabled={!purposeDirty || busy !== null}
                  onClick={() => void run('purpose', () => onPurpose(purpose.trim()))}
                >
                  Save purpose
                </Button>
              </div>
            </>
          ) : (
            <p data-testid="org-panel-purpose" className="whitespace-pre-wrap type-row text-[var(--color-text-primary)]">
              {org.purpose || <span className={muted}>No purpose yet.</span>}
            </p>
          )}
        </section>

        <section className="space-y-1">
          <div className={`type-section uppercase tracking-wider ${muted}`}>Bulletin</div>
          {canEditBulletin ? (
            <>
              <textarea
                className={field + ' font-[var(--font-code)]'}
                rows={BULLETIN_MAX_LINES + 1}
                value={bulletin}
                aria-label="Bulletin"
                placeholder="One line per line: what this org is working on."
                onChange={(e) => {
                  edited.current.bulletin = true
                  setBulletin(e.target.value)
                }}
              />
              <div className="flex items-center gap-2">
                <span className={`type-hint ${problem ? 'text-[var(--color-status-error-text)]' : muted}`} data-testid="org-panel-bulletin-count">
                  {problem ?? `${lines.length} of ${BULLETIN_MAX_LINES} lines`}
                </span>
                <Button
                  size="sm"
                  className="ml-auto"
                  disabled={!bulletinDirty || !!problem || !lines.length || busy !== null}
                  onClick={() => void run('bulletin', () => onBulletin(lines))}
                >
                  Save bulletin
                </Button>
              </div>
            </>
          ) : org.bulletin?.lines.length ? (
            <ol data-testid="org-panel-bulletin" className="space-y-0.5 type-row text-[var(--color-text-primary)]">
              {org.bulletin.lines.map((l, i) => (
                <li key={i}>{l}</li>
              ))}
            </ol>
          ) : (
            <p className={`type-hint ${muted}`}>No bulletin yet. The head writes it.</p>
          )}
          {org.bulletin ? (
            <div className={`type-hint ${muted}`}>
              Updated {when(org.bulletin.updatedAt)} by {org.bulletin.updatedBy}
            </div>
          ) : null}
        </section>

        <section className="space-y-1">
          <div className={`type-section uppercase tracking-wider ${muted}`}>Head</div>
          <div className="type-row text-[var(--color-text-primary)]">{head ? head.name : <span className={muted}>No head yet</span>}</div>
        </section>

        <section className="space-y-1">
          <div className={`type-section uppercase tracking-wider ${muted}`}>Members ({members.length})</div>
          {members.length ? (
            <ul className="space-y-1">
              {members.map((m) => (
                <li key={m.id} data-org-panel-member={m.id} className="flex items-center gap-2 type-row text-[var(--color-text-primary)]">
                  <span className="min-w-0 flex-1">
                    <span className="block truncate">{m.name}</span>
                    {m.role ? <span className={`block truncate type-hint ${muted}`}>{m.role}</span> : null}
                  </span>
                  {m.id === org.headId ? (
                    <span className="type-badge text-[var(--color-accent)]">Head</span>
                  ) : canManage ? (
                    <Button size="sm" variant="ghost" onClick={() => onHead(m.id)}>
                      Make head
                    </Button>
                  ) : null}
                </li>
              ))}
            </ul>
          ) : (
            <p className={`type-hint ${muted}`}>{canManage ? 'Drag agents into the bucket to add them.' : 'No agents yet.'}</p>
          )}
        </section>

        {error ? (
          <div role="alert" className="type-hint text-[var(--color-status-error-text)]">
            {error}
          </div>
        ) : null}
      </div>

      {canManage ? (
        <div className="border-t border-[var(--color-border)] px-3 py-2">
          <Button size="sm" variant="danger" onClick={onDelete}>
            Delete org
          </Button>
          <span className={`ml-2 type-hint ${muted}`}>Its agents leave it; nothing else changes.</span>
        </div>
      ) : null}
    </Surface>
  )
}
