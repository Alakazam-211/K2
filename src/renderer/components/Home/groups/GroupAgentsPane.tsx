// Home groups: the Groups dialog's Agents segment (K2 Design's Home nav).
// A search, how many agents the group holds ("3 in Studio"), then every
// agent this computer can add (the same sources as Add to Home:
// `useAddableAgents`), each with a checkbox: ticked means it is in the
// group. Ticking adds it and unticking removes it, saved at once; removing
// a pinned agent takes its pin with it (the pin lives on the row).
//
// Keyboard: the list is one tab stop; ↑/↓ move between the agents, Space
// ticks or unticks the one in focus.

import { useId, useMemo, useRef, useState } from 'react'
import { Checkbox } from '@/components/ui/Checkbox'
import { SearchField } from '@/components/ui/SearchField'
import { DIALOG_EDGE_TINT, ScrollEdgeFrame } from '@/components/ui/ScrollEdgeFade'
import ProjectAvatar from '@/components/Sidebar/ProjectAvatar'
import { useHomesStore, type Home } from '@/stores/homes'
import { AddSourceHead, SectionStatus, addableMatches, useAddableAgents, type AddableAgent } from '../HomeAddPanels'

/** "3 in Studio": how many agents the group holds. */
export function groupAgentCountText(count: number, name: string): string {
  return `${count} in ${name}`
}

function AgentCheckRow({
  agent,
  group,
  tabbable,
  onToggle,
}: {
  agent: AddableAgent
  group: string
  tabbable: boolean
  onToggle: (a: AddableAgent, on: boolean) => void
}): React.JSX.Element {
  const id = useId()
  return (
    <label
      htmlFor={id}
      className="flex w-full cursor-pointer items-center gap-2.5 rounded-[var(--radius-selector)] px-2 py-1.5 text-[var(--color-text-secondary)] hover:bg-[var(--color-row-hover)] has-[:focus-visible]:bg-[var(--color-row-hover)]"
      data-group-agent={agent.value}
      data-in-group={agent.inGroup ? '' : undefined}
    >
      <Checkbox
        id={id}
        checked={agent.inGroup}
        tabIndex={tabbable ? 0 : -1}
        aria-label={`${agent.name} in ${group}`}
        data-group-agent-check=""
        onChange={(e) => onToggle(agent, e.target.checked)}
      />
      <ProjectAvatar
        projectPath={agent.avatar.kind === 'workspace' ? agent.avatar.path : agent.value}
        projectName={agent.name}
        projectColor={agent.avatar.kind === 'workspace' ? agent.avatar.color : 'var(--color-text-muted)'}
        projectId={agent.avatar.kind === 'workspace' ? agent.avatar.id : undefined}
        iconUrl={agent.avatar.kind === 'workspace' ? agent.avatar.iconUrl : null}
        size={24}
        fetchIcon={agent.avatar.kind === 'workspace' ? agent.avatar.fetchIcon : false}
      />
      <span className="flex min-w-0 flex-1 flex-col">
        <span className="truncate type-name text-[var(--color-text-primary)]">{agent.name}</span>
        <span className="truncate type-hint text-[var(--color-text-muted)]">{agent.handle}</span>
      </span>
    </label>
  )
}

export default function GroupAgentsPane({ groupId }: { groupId: string }): React.JSX.Element | null {
  const home = useHomesStore((s) => s.homes.find((h) => h.id === groupId) ?? null)
  if (!home) return null
  return <GroupAgentsList home={home} />
}

function GroupAgentsList({ home }: { home: Home }): React.JSX.Element {
  const { sources, add } = useAddableAgents(home)
  const [query, setQuery] = useState('')
  const listRef = useRef<HTMLDivElement>(null)
  const [focusValue, setFocusValue] = useState<string | null>(null)

  const shown = useMemo(() => {
    const q = query.trim()
    return sources
      .map((s) => ({ ...s, agents: q ? s.agents.filter((a) => addableMatches(a, q)) : s.agents }))
      .filter((s) => s.agents.length > 0 || (!q && s.listing.kind !== 'ok'))
  }, [sources, query])
  const all = shown.flatMap((s) => s.agents)
  const stop = all.some((a) => a.value === focusValue) ? focusValue : (all[0]?.value ?? null)

  const toggle = (a: AddableAgent, on: boolean): void => {
    setFocusValue(a.value)
    if (on) add(a.value)
    else useHomesStore.getState().removeRow(home.id, a.rowAddress ?? a.address)
  }

  const move = (from: HTMLElement, dir: 1 | -1): void => {
    const boxes = Array.from(listRef.current?.querySelectorAll<HTMLInputElement>('[data-group-agent-check]') ?? [])
    const i = boxes.indexOf(from as HTMLInputElement)
    if (i < 0) return
    const next = boxes[Math.min(boxes.length - 1, Math.max(0, i + dir))]
    const value = next.closest('[data-group-agent]')?.getAttribute('data-group-agent') ?? null
    setFocusValue(value)
    next.focus()
    next.scrollIntoView?.({ block: 'nearest' })
  }

  return (
    <div className="flex min-h-0 flex-1 flex-col" data-group-agents={home.id}>
      <div className="flex flex-shrink-0 items-center gap-3 px-5 relative z-[2]" data-edge-header="">
        <SearchField
          className="min-w-0 flex-1"
          value={query}
          placeholder="Search agents"
          aria-label={`Search agents for ${home.name}`}
          data-group-agents-search=""
          autoComplete="off"
          spellCheck={false}
          onChange={(e) => setQuery(e.target.value)}
          onKeyDown={(e) => {
            if (e.key !== 'ArrowDown') return
            const first = listRef.current?.querySelector<HTMLInputElement>('[data-group-agent-check]')
            if (!first) return
            e.preventDefault()
            setFocusValue(first.closest('[data-group-agent]')?.getAttribute('data-group-agent') ?? null)
            first.focus()
          }}
        />
        <span role="status" className="flex-shrink-0 type-hint tabular-nums text-[var(--color-text-secondary)]" data-group-agents-count="">
          {groupAgentCountText(home.rows.length, home.name)}
        </span>
      </div>
      <ScrollEdgeFrame className="min-h-0 flex-1" scrollClassName="overflow-y-auto overflow-x-hidden px-3 pb-2 pt-2.5" tint={DIALOG_EDGE_TINT} heavy testId="group-agents-frame">
        <div
          ref={listRef}
          role="group"
          aria-label={`Agents in ${home.name}`}
          data-group-agents-list=""
          onKeyDown={(e) => {
            if (e.key !== 'ArrowDown' && e.key !== 'ArrowUp') return
            if (!(e.target instanceof HTMLInputElement) || !e.target.hasAttribute('data-group-agent-check')) return
            e.preventDefault()
            move(e.target, e.key === 'ArrowDown' ? 1 : -1)
          }}
        >
          {shown.map((s) => (
            <div key={s.key} role="group" aria-label={s.label} data-add-source={s.key}>
              <AddSourceHead label={s.label} sub={s.sub} sourceKey={s.key} />
              {s.listing.kind !== 'ok' && (
                <div data-add-source-status={s.listing.kind}>
                  <SectionStatus listing={s.listing} label={s.label} onRetry={s.retry} />
                </div>
              )}
              {s.agents.map((a) => (
                <AgentCheckRow key={a.value} agent={a} group={home.name} tabbable={a.value === stop} onToggle={toggle} />
              ))}
            </div>
          ))}
          {shown.length === 0 && (
            <p role="status" className="m-0 px-3 py-6 text-center type-hint text-[var(--color-text-muted)]" data-group-agents-empty="">
              {query.trim() ? `No agents match “${query.trim()}”` : 'No agents to add'}
            </p>
          )}
        </div>
      </ScrollEdgeFrame>
    </div>
  )
}
