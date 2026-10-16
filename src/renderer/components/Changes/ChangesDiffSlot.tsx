// C2/C3 seam — the Changes tab's diff area. C3 replaces the body of this
// component with its @codemirror/merge viewer (`./viewer/*`); the props are
// the contract (`./diff-slot-types.ts`). Until then it shows a plain
// placeholder with the file, the +/- counts and the side labels, so the tab
// is usable for list work.

import type { ChangesDiffSlotProps } from './diff-slot-types'

export type { ChangesDiffSlotProps } from './diff-slot-types'

export function ChangesDiffSlot(props: ChangesDiffSlotProps): React.JSX.Element {
  const { view, file, files, sides, mode } = props
  let body: React.ReactNode
  if (view === 'all') {
    body = files.length === 0 ? 'No changes' : `${files.length} changed file${files.length === 1 ? '' : 's'} (view all)`
  } else if (!file) {
    body = 'Select a file to see its changes'
  } else if (sides.status === 'error') {
    body = `Could not load the diff: ${sides.error}`
  } else if (sides.status === 'loading' && !sides.data) {
    body = 'Loading…'
  } else if (sides.status === 'idle') {
    body = file.path
  } else {
    const d = sides.data
    body = d
      ? d.binary
        ? `${file.path} — binary file`
        : d.tooLarge
          ? `${file.path} — too large to show`
          : `${file.path} — ${d.old.label} → ${d.new.label}`
      : file.path
  }
  return (
    <div
      data-changes-diff-slot=""
      data-mode={mode}
      data-view={view}
      className="flex h-full w-full items-center justify-center px-4 text-center text-xs text-[var(--color-text-muted)]"
    >
      {body}
    </div>
  )
}
