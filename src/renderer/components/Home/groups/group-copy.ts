// The words people see for Home groups: the sections inside Home that
// people and agents talk in (`homes` in code and storage). The page itself
// is still "Home", as are "Add to Home" and "Home sync". The phone app uses
// these same words for the same controls (group-copy.test.ts holds its
// list), so the two read alike.

/** The screen-reader name of a list or bar of groups. */
export const GROUPS_LABEL = 'Groups'

/** A group with no agents in it. */
export const GROUP_EMPTY_TEXT = 'This group is empty'

/** Why Delete is off on the only group. */
export const LAST_GROUP_TEXT = 'The last group can’t be deleted.'

/** The name field's label. */
export const GROUP_NAME_LABEL = 'Group name'

/** Adds a group (named "Group N" until renamed). */
export const NEW_GROUP_LABEL = 'New group'

/** Notes on a group in a pick-a-group list. */
export const CURRENT_GROUP_NOTE = 'Current'
export const ALREADY_IN_GROUP_NOTE = 'Already in it'

/** A row's menu item that moves it to another of the person's groups. */
export const MOVE_TO_GROUP_LABEL = 'Move to group…'

/** Why "Move to group…" is off when there is only one group. */
export const ONLY_ONE_GROUP_TEXT = 'Make another group first.'

/** A row's menu item that takes it out of the group it is in. */
export const removeFromGroupLabel = (name: string): string => `Remove from ${name}`
