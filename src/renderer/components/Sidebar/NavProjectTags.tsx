import type { JSX } from 'react'
import { useProjectGroupsStore } from '@/stores/project-groups'
import { usePageViewStore } from '@/stores/page-view'
import { groupAvatarColor } from '@/components/Projects/ProjectGroupAvatar'
import {
  navTagsTooltip,
  packNavTags,
  type ProjectNavTag,
} from '@/lib/nav-project-tags'

function TagChip({
  tag,
  onOpen,
}: {
  tag: ProjectNavTag
  onOpen: (id: string) => void
}): JSX.Element {
  const color = tag.color || groupAvatarColor(tag.id)
  return (
    <button
      type="button"
      className="max-w-[6.5rem] truncate px-1 py-px text-[9px] leading-4 text-[var(--color-text-secondary)] no-drag cursor-pointer hover:text-[var(--color-text-primary)]"
      style={{
        background: `color-mix(in srgb, ${color} 22%, transparent)`,
      }}
      title={tag.name}
      onClick={(e) => {
        e.stopPropagation()
        onOpen(tag.id)
      }}
    >
      {tag.name}
    </button>
  )
}

/** Second-line chips: project groups this agent belongs to. Overflow is +N. */
export function NavProjectTags({ workspaceId }: { workspaceId: string }): JSX.Element | null {
  const tags = useProjectGroupsStore((s) => s.tagsByWorkspaceId[workspaceId] ?? [])
  if (tags.length === 0) return null

  const { visible, overflow } = packNavTags(tags)
  const openGroup = (id: string): void => {
    usePageViewStore.getState().setPage('projects')
    useProjectGroupsStore.getState().selectGroup(id)
  }

  return (
    <div
      className="flex min-w-0 flex-1 items-center gap-0.5 overflow-hidden"
      title={navTagsTooltip(tags)}
    >
      {visible.map((tag) => (
        <TagChip key={tag.id} tag={tag} onOpen={openGroup} />
      ))}
      {overflow.length > 0 && (
        <span
          className="flex-shrink-0 px-1 py-px text-[9px] leading-4 text-[var(--color-text-muted)]"
          style={{ background: 'var(--color-overlay-soft-bg)' }}
          title={navTagsTooltip(overflow)}
        >
          +{overflow.length}
        </span>
      )}
    </div>
  )
}
