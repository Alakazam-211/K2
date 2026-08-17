import type { JSX } from 'react'
import { useProjectGroupsStore } from '@/stores/project-groups'
import { usePageViewStore } from '@/stores/page-view'
import {
  EMPTY_NAV_TAGS,
  NAV_TAG_BLUE,
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
  return (
    <button
      type="button"
      className="max-w-[6.5rem] truncate px-1 py-px text-[9px] font-medium leading-4 no-drag cursor-pointer"
      style={{
        color: '#e8f3fa',
        background: `color-mix(in srgb, ${NAV_TAG_BLUE} 38%, transparent)`,
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

/** Second-line chips: project groups this agent belongs to. Overflow is +N.
 *  Always occupies flex-1 so shortcut numbers stay on the far right
 *  even when the agent has no memberships. */
export function NavProjectTags({ workspaceId }: { workspaceId: string }): JSX.Element {
  const tags = useProjectGroupsStore((s) => s.tagsByWorkspaceId[workspaceId] ?? EMPTY_NAV_TAGS)
  if (tags.length === 0) {
    return <div className="min-w-0 flex-1" />
  }

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
          className="flex-shrink-0 px-1 py-px text-[9px] font-medium leading-4"
          style={{
            color: '#e8f3fa',
            background: `color-mix(in srgb, ${NAV_TAG_BLUE} 38%, transparent)`,
          }}
          title={navTagsTooltip(overflow)}
        >
          +{overflow.length}
        </span>
      )}
    </div>
  )
}
