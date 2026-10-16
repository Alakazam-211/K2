// Help: the CLI tool explorer (Rosson 2026-10-16). Left rail = the CLI
// tools K2 ships (`k2` with its noun families, `k2-node`) and the pages
// (`k2 study`, the daemon glossary). Right = the selected tool's nouns,
// their commands and examples, or a page. Top middle = one search over
// all of it (⌘K / Ctrl+K focuses it).

import { useCallback, useEffect, useId, useMemo, useRef, useState } from 'react'
import { DialogScrim, Input, Surface } from '@/components/ui'
import { daemonCliGet } from '@/lib/daemon-cli'
import type { ServerScope } from '@/kessel/server-scope'
import catalogJson from '@shared/cli-catalog.generated.json'
import {
  buildExplorerModel,
  fixK2Name,
  glossaryPages,
  searchExplorer,
  SEARCH_BADGES,
  type CliCatalog,
  type ExplorerModel,
  type ExplorerNoun,
  type ExplorerPage,
  type ExplorerTarget,
  type ExplorerTool,
  type GlossaryEntry,
  type SearchResult,
} from './cli-explorer'

const MONO = "'MesloLGM Nerd Font', Menlo, Monaco, monospace"
const SANS = "-apple-system, BlinkMacSystemFont, 'SF Pro Text', 'Inter', system-ui, sans-serif"

const MODEL: ExplorerModel = buildExplorerModel(catalogJson as CliCatalog)

type GlossaryState =
  | { status: 'loading' }
  | { status: 'ok'; pages: ExplorerPage[] }
  | { status: 'error'; message: string }

function CopyButton({ text }: { text: string }): React.JSX.Element {
  const [copied, setCopied] = useState(false)
  useEffect(() => {
    if (!copied) return
    const t = setTimeout(() => setCopied(false), 1200)
    return () => clearTimeout(t)
  }, [copied])
  return (
    <button
      type="button"
      data-testid="help-copy"
      aria-label={`Copy ${text}`}
      title="Copy"
      onClick={(e) => {
        e.stopPropagation()
        void navigator.clipboard?.writeText(text).then(() => setCopied(true))
      }}
      className="flex-shrink-0 px-1.5 text-[10.5px] text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] hover:bg-[var(--color-bg-elevated)] border border-[var(--color-border)] cursor-pointer"
    >
      {copied ? 'Copied' : 'Copy'}
    </button>
  )
}

function CodeLine({ text, testId }: { text: string; testId?: string }): React.JSX.Element {
  return (
    <div className="flex items-start gap-2 min-w-0" data-testid={testId}>
      <code
        className="flex-1 min-w-0 text-[11.5px] leading-[1.5] text-[var(--color-text-primary)] break-words"
        style={{ fontFamily: MONO, whiteSpace: 'pre-wrap' }}
      >
        {text}
      </code>
      <CopyButton text={text} />
    </div>
  )
}

function Badge({ children }: { children: React.ReactNode }): React.JSX.Element {
  return (
    <span className="ml-1.5 text-[9.5px] uppercase tracking-wider text-[var(--color-text-muted)] border border-[var(--color-border)] px-1 py-[1px] align-middle">
      {children}
    </span>
  )
}

function RailButton({
  selected,
  indent,
  onClick,
  testId,
  children,
  mono,
}: {
  selected: boolean
  indent?: boolean
  onClick: () => void
  testId: string
  children: React.ReactNode
  mono?: boolean
}): React.JSX.Element {
  return (
    <button
      type="button"
      data-testid={testId}
      aria-current={selected ? 'true' : undefined}
      onClick={onClick}
      className={`block w-full text-left truncate py-[3px] pr-2 text-[12px] cursor-pointer ${
        selected
          ? 'bg-[var(--color-bg-elevated)] text-[var(--color-text-primary)]'
          : 'text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)] hover:bg-[var(--color-bg-hover)]'
      }`}
      style={{ paddingLeft: indent ? 22 : 10, fontFamily: mono ? MONO : undefined }}
    >
      {children}
    </button>
  )
}

function RailHeading({ children }: { children: React.ReactNode }): React.JSX.Element {
  return (
    <h3 className="m-0 px-2.5 pt-3 pb-1 text-[10px] font-semibold uppercase tracking-wider text-[var(--color-text-muted)]">
      {children}
    </h3>
  )
}

function NounCard({
  tool,
  noun,
  expanded,
  onToggle,
  highlightVerb,
}: {
  tool: ExplorerTool
  noun: ExplorerNoun
  expanded: boolean
  onToggle: () => void
  highlightVerb?: string
}): React.JSX.Element {
  const label = `${tool.name} ${noun.noun}`
  const count = noun.verbs.length
  return (
    <li
      id={`help-noun-${noun.key}`}
      data-testid={`help-noun-${noun.key}`}
      data-expanded={expanded ? 'true' : 'false'}
      className="m-0 py-2 border-b border-[var(--color-border)] last:border-b-0"
    >
      <button
        type="button"
        onClick={onToggle}
        aria-expanded={expanded}
        className="block w-full text-left cursor-pointer bg-transparent p-0"
      >
        <span className="text-[12.5px] leading-[1.45]">
          <code
            className="text-[12px] text-[var(--color-text-primary)]"
            style={{ fontFamily: MONO, background: 'var(--color-overlay-soft-bg)', padding: '1px 5px' }}
          >
            {label}
          </code>
          {noun.note === 'sidecar' ? <Badge>sidecar</Badge> : null}
          {noun.note === 'linux-sidecar' ? <Badge>Linux sidecar</Badge> : null}
          <span className="ml-1.5 text-[10.5px] text-[var(--color-text-muted)]">
            {count ? `${count} command${count === 1 ? '' : 's'}` : noun.help ? 'help' : ''}
            {count || noun.help ? (expanded ? ' ▾' : ' ▸') : ''}
          </span>
        </span>
        <span className="block mt-0.5 text-[12px] leading-[1.45] text-[var(--color-text-secondary)]">{noun.blurb}</span>
      </button>
      {noun.also.length ? (
        <div className="mt-0.5 text-[11px] text-[var(--color-text-muted)]">
          Also:{' '}
          {noun.also.map((a, i) => (
            <span key={a}>
              {i ? ', ' : ''}
              <code style={{ fontFamily: MONO }}>{`${tool.name} ${a}`}</code>
            </span>
          ))}
        </div>
      ) : null}
      {noun.example ? (
        <div className="mt-1">
          <CodeLine text={noun.example} testId={`help-example-${noun.key}`} />
        </div>
      ) : null}
      {expanded ? (
        <div className="mt-2 pl-2 border-l border-[var(--color-border)]" data-testid={`help-verbs-${noun.key}`}>
          {count ? (
            <ul className="m-0 p-0 list-none">
              {noun.verbs.map((v) => (
                <li
                  key={`${v.name}::${v.usage}`}
                  data-testid="help-verb"
                  data-verb={v.name}
                  data-highlight={highlightVerb === v.name ? 'true' : undefined}
                  className={`mb-2 last:mb-0 px-1 ${highlightVerb === v.name ? 'bg-[var(--color-bg-elevated)]' : ''}`}
                >
                  <CodeLine text={v.usage} />
                  {v.description ? (
                    <div className="mt-0.5 text-[11.5px] leading-[1.45] text-[var(--color-text-secondary)]">
                      {fixK2Name(v.description)}
                    </div>
                  ) : null}
                </li>
              ))}
            </ul>
          ) : noun.help ? (
            <pre
              className="m-0 text-[11.5px] leading-[1.5] text-[var(--color-text-primary)]"
              style={{ fontFamily: MONO, whiteSpace: 'pre-wrap' }}
            >
              {noun.help}
            </pre>
          ) : null}
        </div>
      ) : null}
      {!count && !noun.help && tool.id === 'k2' ? (
        <div className="mt-1 text-[11px] text-[var(--color-text-muted)]">
          Every command: <code style={{ fontFamily: MONO }}>{`k2 help ${noun.noun}`}</code>
        </div>
      ) : null}
    </li>
  )
}

/** `scope`: the server whose glossary Help reads (the top bar's server). */
export default function HelpExplorer({
  scope,
  onClose,
}: {
  scope: ServerScope
  onClose: () => void
}): React.JSX.Element {
  const [target, setTarget] = useState<ExplorerTarget>({ kind: 'tool', toolId: 'k2' })
  const [expanded, setExpanded] = useState<Set<string>>(() => new Set())
  const [query, setQuery] = useState('')
  const [active, setActive] = useState(0)
  const [glossary, setGlossary] = useState<GlossaryState>({ status: 'loading' })
  const [definitions, setDefinitions] = useState<Record<string, string | { error: string }>>({})
  const inputRef = useRef<HTMLInputElement>(null)
  const mainRef = useRef<HTMLDivElement>(null)
  const listId = useId()

  // The glossary is the daemon's (GET /cli/glossary). Without it the rest
  // of Help still works.
  useEffect(() => {
    let live = true
    daemonCliGet<GlossaryEntry[]>(scope, 'glossary')
      .then((rows) => {
        if (!live) return
        if (!Array.isArray(rows)) throw new Error('glossary: not a list')
        setGlossary({ status: 'ok', pages: glossaryPages(rows) })
      })
      .catch((e: unknown) => {
        if (live) setGlossary({ status: 'error', message: e instanceof Error ? e.message : String(e) })
      })
    return () => {
      live = false
    }
  }, [scope])

  const glossaryList = glossary.status === 'ok' ? glossary.pages : []
  const results = useMemo(() => searchExplorer(MODEL, glossaryList, query), [glossaryList, query])
  const showResults = query.trim().length > 0

  useEffect(() => {
    setActive(0)
  }, [query])

  // Load a glossary definition when its page opens.
  const openTerm = target.kind === 'pages' && target.set === 'glossary' ? target.pageId : undefined
  useEffect(() => {
    if (!openTerm || definitions[openTerm] !== undefined) return
    let live = true
    daemonCliGet<{ definition?: string; summary?: string }>(scope, 'glossary/get', { term: openTerm })
      .then((row) => {
        if (live) setDefinitions((d) => ({ ...d, [openTerm]: fixK2Name(row.definition ?? row.summary ?? '') }))
      })
      .catch((e: unknown) => {
        if (live) setDefinitions((d) => ({ ...d, [openTerm]: { error: e instanceof Error ? e.message : String(e) } }))
      })
    return () => {
      live = false
    }
  }, [scope, openTerm, definitions])

  const jump = useCallback((t: ExplorerTarget) => {
    setTarget(t)
    if (t.kind === 'tool' && t.nounKey) {
      const key = t.nounKey
      setExpanded((s) => (s.has(key) ? s : new Set(s).add(key)))
    }
    setQuery('')
  }, [])

  // Scroll the jumped-to noun (or the top) into view.
  useEffect(() => {
    const key = target.kind === 'tool' ? target.nounKey : undefined
    const el = key ? document.getElementById(`help-noun-${key}`) : null
    if (el && typeof el.scrollIntoView === 'function') el.scrollIntoView({ block: 'start' })
    else if (mainRef.current) mainRef.current.scrollTop = 0
  }, [target])

  const pick = useCallback((r: SearchResult | undefined) => {
    if (r) jump(r.target)
  }, [jump])

  // Esc clears the search first, then closes; ⌘K / Ctrl+K focuses search.
  useEffect(() => {
    const onKey = (e: KeyboardEvent): void => {
      if ((e.metaKey || e.ctrlKey) && !e.altKey && (e.key === 'k' || e.key === 'K')) {
        e.preventDefault()
        e.stopPropagation()
        inputRef.current?.focus()
        inputRef.current?.select()
        return
      }
      if (e.key !== 'Escape') return
      e.preventDefault()
      e.stopPropagation()
      if (inputRef.current && inputRef.current.value) {
        setQuery('')
        return
      }
      onClose()
    }
    window.addEventListener('keydown', onKey, true)
    return () => window.removeEventListener('keydown', onKey, true)
  }, [onClose])

  const onSearchKey = (e: React.KeyboardEvent<HTMLInputElement>): void => {
    if (!showResults) return
    if (e.key === 'ArrowDown') {
      e.preventDefault()
      setActive((i) => Math.min(i + 1, Math.max(0, results.length - 1)))
    } else if (e.key === 'ArrowUp') {
      e.preventDefault()
      setActive((i) => Math.max(0, i - 1))
    } else if (e.key === 'Enter') {
      e.preventDefault()
      pick(results[active])
    }
  }

  useEffect(() => {
    if (!showResults) return
    const el = document.getElementById(`${listId}-opt-${active}`)
    if (el && typeof el.scrollIntoView === 'function') el.scrollIntoView({ block: 'nearest' })
  }, [active, showResults, listId])

  const toggle = (key: string): void =>
    setExpanded((s) => {
      const next = new Set(s)
      if (next.has(key)) next.delete(key)
      else next.add(key)
      return next
    })

  const tool = target.kind === 'tool' ? MODEL.tools.find((t) => t.id === target.toolId) : undefined
  const groups = tool ? (target.kind === 'tool' && target.groupId ? tool.groups.filter((g) => g.id === target.groupId) : tool.groups) : []

  const renderTool = (t: ExplorerTool): React.JSX.Element => (
    <div data-testid={`help-tool-${t.id}`}>
      <h2 className="m-0 text-[15px] font-semibold text-[var(--color-text-primary)]" style={{ fontFamily: MONO }}>
        {t.name}
        {target.kind === 'tool' && target.groupId && t.groups.length > 1 ? (
          <span className="text-[var(--color-text-muted)] font-normal" style={{ fontFamily: SANS }}>
            {' '}
            · {groups[0]?.title}
          </span>
        ) : null}
      </h2>
      <p className="m-0 mt-1 mb-2 text-[12px] leading-[1.5] text-[var(--color-text-secondary)]">{t.intro}</p>
      {groups.map((g) => (
        <section key={g.id} className="mb-3 last:mb-0" data-testid={`help-group-${g.id}`}>
          {t.groups.length > 1 ? (
            <h3 className="m-0 mt-2 text-[11px] font-semibold uppercase tracking-wider text-[var(--color-text-muted)]">
              {g.title}
            </h3>
          ) : null}
          <ul className="m-0 p-0 list-none">
            {g.nouns.map((n) => (
              <NounCard
                key={n.key}
                tool={t}
                noun={n}
                expanded={expanded.has(n.key)}
                onToggle={() => toggle(n.key)}
                highlightVerb={target.kind === 'tool' && target.nounKey === n.key ? target.verbName : undefined}
              />
            ))}
          </ul>
        </section>
      ))}
      {t.footer ? <p className="m-0 mt-3 text-[11.5px] text-[var(--color-text-muted)]">{t.footer}</p> : null}
    </div>
  )

  const renderPages = (set: 'study' | 'glossary', pageId?: string): React.JSX.Element => {
    if (set === 'glossary' && glossary.status !== 'ok') {
      return (
        <div data-testid="help-glossary-unavailable">
          <h2 className="m-0 text-[15px] font-semibold text-[var(--color-text-primary)]">Glossary</h2>
          <p className="m-0 mt-2 text-[12px] text-[var(--color-text-secondary)]">
            {glossary.status === 'loading'
              ? 'Loading the glossary from K2…'
              : 'The glossary comes from the K2 server, which did not answer. Commands and k2 study pages still work.'}
          </p>
        </div>
      )
    }
    const pages = set === 'study' ? MODEL.study : glossaryList
    const page = pageId ? pages.find((p) => p.id === pageId) : undefined
    if (page) {
      const def = set === 'glossary' ? definitions[page.id] : undefined
      return (
        <div data-testid={`help-page-${page.key}`}>
          <button
            type="button"
            onClick={() => setTarget({ kind: 'pages', set })}
            className="mb-2 px-0 text-[11.5px] text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] cursor-pointer bg-transparent"
          >
            ← {set === 'study' ? 'All pages' : 'All terms'}
          </button>
          <h2 className="m-0 text-[15px] font-semibold text-[var(--color-text-primary)]">{page.title}</h2>
          <div className="mt-1 mb-2">
            <CodeLine text={set === 'study' ? `k2 study ${page.id}` : `k2 glossary ${page.id}`} />
          </div>
          {set === 'study' ? (
            <pre
              data-testid="help-page-body"
              className="m-0 text-[11.5px] leading-[1.55] text-[var(--color-text-primary)]"
              style={{ fontFamily: MONO, whiteSpace: 'pre-wrap' }}
            >
              {page.body}
            </pre>
          ) : def === undefined ? (
            <p className="m-0 text-[12px] text-[var(--color-text-muted)]">{page.summary}</p>
          ) : typeof def === 'string' ? (
            <p data-testid="help-page-body" className="m-0 text-[12.5px] leading-[1.55] text-[var(--color-text-primary)]" style={{ whiteSpace: 'pre-wrap' }}>
              {def}
            </p>
          ) : (
            <p role="alert" className="m-0 text-[12px] text-[var(--color-status-error-soft)]">
              {page.summary} (Full definition unavailable: {def.error})
            </p>
          )}
        </div>
      )
    }
    return (
      <div data-testid={`help-pages-${set}`}>
        <h2 className="m-0 text-[15px] font-semibold text-[var(--color-text-primary)]">
          {set === 'study' ? 'k2 study' : 'Glossary'}
        </h2>
        <p className="m-0 mt-1 mb-2 text-[12px] leading-[1.5] text-[var(--color-text-secondary)]">
          {set === 'study'
            ? 'Bounded teaching pages: concepts, not a flag dump. Agents read them with k2 study <topic>.'
            : 'K2 words, from the K2 server. Agents read them with k2 glossary <term>.'}
        </p>
        <ul className="m-0 p-0 list-none">
          {pages.map((p) => (
            <li key={p.key} className="m-0">
              <button
                type="button"
                data-testid={`help-page-link-${p.key}`}
                onClick={() => setTarget({ kind: 'pages', set, pageId: p.id })}
                className="block w-full text-left py-1.5 px-1 cursor-pointer bg-transparent hover:bg-[var(--color-bg-hover)] border-b border-[var(--color-border)]"
              >
                <span className="text-[12.5px] text-[var(--color-text-primary)]">{p.title}</span>
                <span className="ml-1.5 text-[11px] text-[var(--color-text-muted)]" style={{ fontFamily: MONO }}>
                  {p.id}
                </span>
                <span className="block text-[11.5px] text-[var(--color-text-secondary)]">{p.summary}</span>
              </button>
            </li>
          ))}
        </ul>
      </div>
    )
  }

  const k2 = MODEL.tools[0]
  return (
    <>
      <DialogScrim
        onMouseDown={(e) => {
          e.stopPropagation()
          onClose()
        }}
      />
      <Surface
        role2="surface"
        elevation={5}
        className="no-drag"
        role="dialog"
        aria-modal="true"
        aria-labelledby="help-explorer-title"
        data-testid="help-explorer"
        style={{
          position: 'fixed',
          top: '50%',
          left: '50%',
          transform: 'translate(-50%, -50%)',
          zIndex: 99999,
          width: 'min(880px, calc(100vw - 32px))',
          height: 'min(640px, calc(100vh - 64px))',
          display: 'flex',
          flexDirection: 'column',
          overflow: 'hidden',
          fontFamily: SANS,
        }}
        onMouseDown={(e) => e.stopPropagation()}
      >
        <div
          className="grid items-center gap-2 flex-shrink-0"
          style={{
            gridTemplateColumns: '1fr minmax(0, 380px) 1fr',
            padding: '10px 14px',
            borderBottom: '1px solid var(--color-border)',
          }}
        >
          <h2 id="help-explorer-title" className="m-0 text-[14px] font-semibold text-[var(--color-text-primary)] truncate">
            Help
          </h2>
          <div className="relative min-w-0">
            <Input
              ref={inputRef}
              type="search"
              role="combobox"
              aria-label="Search commands and pages"
              aria-expanded={showResults}
              aria-controls={listId}
              aria-autocomplete="list"
              aria-activedescendant={showResults && results.length ? `${listId}-opt-${active}` : undefined}
              data-testid="help-search"
              autoFocus
              value={query}
              placeholder="Search commands and pages  ⌘K"
              onChange={(e) => setQuery(e.target.value)}
              onKeyDown={onSearchKey}
              className="w-full"
              style={{ fontSize: 12.5 }}
            />
            {showResults ? (
              <Surface
                role2="elevated"
                elevation={4}
                id={listId}
                role="listbox"
                aria-label="Search results"
                data-testid="help-search-results"
                className="absolute left-1/2 -translate-x-1/2 mt-1"
                style={{
                  width: 'min(480px, calc(100vw - 48px))',
                  maxHeight: 340,
                  overflowY: 'auto',
                  zIndex: 2,
                }}
              >
                {results.length === 0 ? (
                  <div className="px-3 py-2 text-[12px] text-[var(--color-text-muted)]" data-testid="help-search-empty">
                    Nothing matches “{query.trim()}”.
                  </div>
                ) : (
                  results.map((r, i) => (
                    <div
                      key={r.key}
                      id={`${listId}-opt-${i}`}
                      role="option"
                      aria-selected={i === active}
                      data-testid="help-search-result"
                      data-kind={r.kind}
                      onMouseDown={(e) => {
                        e.preventDefault()
                        pick(r)
                      }}
                      onMouseEnter={() => setActive(i)}
                      className={`flex items-baseline gap-2 px-2.5 py-1.5 cursor-pointer ${
                        i === active ? 'bg-[var(--color-bg-hover)]' : ''
                      }`}
                    >
                      <span className="flex-shrink-0 w-[58px] text-[9.5px] uppercase tracking-wider text-[var(--color-text-muted)]">
                        {SEARCH_BADGES[r.kind]}
                      </span>
                      <span className="min-w-0 flex-1">
                        <span
                          className="block truncate text-[12px] text-[var(--color-text-primary)]"
                          style={{ fontFamily: r.kind === 'page' ? SANS : MONO }}
                        >
                          {r.label}
                        </span>
                        <span className="block truncate text-[11px] text-[var(--color-text-secondary)]">{fixK2Name(r.detail)}</span>
                      </span>
                    </div>
                  ))
                )}
              </Surface>
            ) : null}
          </div>
          <button
            type="button"
            onClick={onClose}
            aria-label="Close"
            className="justify-self-end flex h-6 w-6 items-center justify-center text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] hover:bg-[var(--color-bg-hover)]"
            style={{ borderRadius: 0 }}
          >
            ×
          </button>
        </div>
        <div className="flex flex-1 min-h-0">
          <nav
            aria-label="CLI tools and pages"
            data-testid="help-rail"
            className="flex-shrink-0 overflow-y-auto overflow-x-hidden pb-3"
            style={{ width: 'clamp(116px, 22%, 168px)', borderRight: '1px solid var(--color-border)' }}
          >
            <RailHeading>CLI tools</RailHeading>
            {MODEL.tools.map((t) => (
              <div key={t.id}>
                <RailButton
                  testId={`help-rail-${t.id}`}
                  mono
                  selected={target.kind === 'tool' && target.toolId === t.id && !(t.id === 'k2' && target.groupId)}
                  onClick={() => setTarget({ kind: 'tool', toolId: t.id })}
                >
                  {t.name}
                </RailButton>
                {t.id === k2.id
                  ? t.groups.map((g) => (
                      <RailButton
                        key={g.id}
                        indent
                        testId={`help-rail-k2-${g.id}`}
                        selected={target.kind === 'tool' && target.toolId === 'k2' && target.groupId === g.id}
                        onClick={() => setTarget({ kind: 'tool', toolId: 'k2', groupId: g.id })}
                      >
                        {g.title}
                      </RailButton>
                    ))
                  : null}
              </div>
            ))}
            <RailHeading>Pages</RailHeading>
            <RailButton
              testId="help-rail-study"
              mono
              selected={target.kind === 'pages' && target.set === 'study'}
              onClick={() => setTarget({ kind: 'pages', set: 'study' })}
            >
              k2 study
            </RailButton>
            <RailButton
              testId="help-rail-glossary"
              selected={target.kind === 'pages' && target.set === 'glossary'}
              onClick={() => setTarget({ kind: 'pages', set: 'glossary' })}
            >
              Glossary
              {glossary.status === 'error' ? <span className="text-[var(--color-text-muted)]"> · offline</span> : null}
            </RailButton>
          </nav>
          <div ref={mainRef} className="flex-1 min-w-0 overflow-y-auto" style={{ padding: '12px 18px 18px' }} data-testid="help-main">
            {target.kind === 'tool' && tool ? renderTool(tool) : null}
            {target.kind === 'pages' ? renderPages(target.set, target.pageId) : null}
          </div>
        </div>
      </Surface>
    </>
  )
}
