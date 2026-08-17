/** Project-group chip shown under an agent in the main nav. */

export interface ProjectNavTag {
  id: string
  name: string
  color: string | null
}

/** How many named chips sit in the row before the rest collapse to +N. */
export const NAV_TAG_VISIBLE_MAX = 2

/** Nav chip fill — soft steel-blue, not the group’s hashed amber/orange. */
export const NAV_TAG_BLUE = '#7eabd0'

/** Stable empty list — `?? []` in a zustand selector re-renders forever. */
export const EMPTY_NAV_TAGS: ProjectNavTag[] = []

/**
 * Invert group → member workspace ids into workspace → tags
 * (name order, de-duped). Pure so the nav can stay a view.
 */
export function buildTagsByWorkspaceId(
  groups: ReadonlyArray<{ id: string; name: string; color: string | null }>,
  membersByGroupId: Readonly<Record<string, readonly string[]>>,
): Record<string, ProjectNavTag[]> {
  const out: Record<string, ProjectNavTag[]> = {}
  const sorted = [...groups].sort((a, b) =>
    a.name.localeCompare(b.name, undefined, { sensitivity: 'base' }),
  )
  for (const g of sorted) {
    const members = membersByGroupId[g.id] ?? []
    const tag: ProjectNavTag = { id: g.id, name: g.name, color: g.color }
    for (const workspaceId of members) {
      if (!workspaceId) continue
      const list = out[workspaceId] ?? (out[workspaceId] = [])
      if (list.some((t) => t.id === g.id)) continue
      list.push(tag)
    }
  }
  return out
}

/** First `maxVisible` chips stay in the row; the rest ride `+N`. */
export function packNavTags(
  tags: readonly ProjectNavTag[],
  maxVisible: number = NAV_TAG_VISIBLE_MAX,
): { visible: ProjectNavTag[]; overflow: ProjectNavTag[] } {
  const cap = maxVisible > 0 ? maxVisible : 0
  if (tags.length <= cap) return { visible: [...tags], overflow: [] }
  return {
    visible: tags.slice(0, cap),
    overflow: tags.slice(cap),
  }
}

export function navTagsTooltip(tags: readonly ProjectNavTag[]): string {
  return tags.map((t) => t.name).join(', ')
}
