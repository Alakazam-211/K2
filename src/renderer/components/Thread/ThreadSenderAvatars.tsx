// The picture in a Thread row's sender column (threadSenders.ts has the
// rules: who counts, runs, motion). An agent: its own image and colour
// (`ProjectAvatar`, as the Agents page and the Connections picker paint
// it); another server's agent from the Home avatar cache. A person: their
// chip (`PresenceChip`, the roster's colour and names, else the hash). All
// from stores the client already has; `ProjectAvatar`'s one lookup is
// cached per path. No fetch per message.

import type { JSX } from 'react'
import ProjectAvatar from '@/components/Sidebar/ProjectAvatar'
import { PresenceChip } from '@/components/Presence/PresenceChip'
import { useProjectsStore } from '@/stores/projects'
import { usePresenceStore } from '@/stores/presence'
import { useConnectHostStore } from '@/stores/connect-host'
import { activeHomeHostKey, homeAddress, workspaceHandle } from '@/lib/home-address'
import { homeRowAvatarFrom, useHomeAvatarStore } from '@/lib/home-avatars'
import { SENDER_AVATAR_PX, type ThreadSenderSlot } from './threadSenders'

const fold = (s: string): string => s.trim().toLowerCase()

/** An agent's picture: this server's workspace (the projects store), or
 *  another server's from the Home avatar cache. */
function AgentAvatar({ addr, label, host }: { addr: string; label: string; host: string | null }): JSX.Element {
  const federated = addr.includes('::')
  const handle = fold(addr.split('::')[0].split('/')[0])
  const project = useProjectsStore((s) =>
    federated || host ? undefined : s.projects.find((p) => workspaceHandle(p) === handle),
  )
  const target = federated ? fold(addr) : homeAddress(handle, host ?? 'local')
  const connectedKey = useConnectHostStore((s) => activeHomeHostKey(s.activeHost))
  const cached = useHomeAvatarStore((s) => s.cached[target])
  const local = useHomeAvatarStore((s) => s.local[target])
  const color = useHomeAvatarStore((s) => s.colors[target])
  if (project) {
    return (
      <ProjectAvatar
        projectPath={project.path}
        projectName={project.name || label}
        projectColor={project.color || 'var(--color-neutral)'}
        projectId={project.id}
        iconUrl={project.iconUrl}
        size={SENDER_AVATAR_PX}
      />
    )
  }
  const url = homeRowAvatarFrom(target, connectedKey, {
    cached: cached ? { [target]: cached } : {},
    local: local === undefined ? {} : { [target]: local },
  })
  return (
    <ProjectAvatar
      projectPath={`thread-sender:${target}`}
      projectName={label}
      projectColor={color ?? 'var(--color-text-muted)'}
      iconUrl={url}
      size={SENDER_AVATAR_PX}
      fetchIcon={false}
    />
  )
}

/** A person's chip: the roster's colour and names for that login. */
function PersonAvatar({ login, label }: { login: string; label: string }): JSX.Element {
  const color = usePresenceStore((s) => s.colors[login] ?? null)
  const names = usePresenceStore((s) => s.names[login] ?? null)
  return <PresenceChip name={login || label} role="member" size={SENDER_AVATAR_PX} color={color} names={names} title="" />
}

/** The picture for a row's sender (inside `ThreadSenderFrame`). */
export function ThreadSenderPicture({ slot }: { slot: ThreadSenderSlot }): JSX.Element {
  return slot.kind === 'agent' ? (
    <AgentAvatar addr={slot.id} label={slot.label} host={slot.host} />
  ) : (
    <PersonAvatar login={slot.id} label={slot.label} />
  )
}
