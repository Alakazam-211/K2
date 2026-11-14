// A storable Home group icon name: a Lucide name, kebab-case, the shape the
// daemon accepts (`homes_sync::valid_icon`). Whether this build can draw it
// is `lib/group-icons.ts`'s question; an unknown name draws as a dot.
export const GROUP_ICON_NAME_RE = /^[a-z0-9-]{1,64}$/
