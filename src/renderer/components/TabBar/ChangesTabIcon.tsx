/** Changes tab glyph: a plus over a minus (a diff), same stroke and size as
 *  the shell icon. */
export function ChangesTabIcon(): React.JSX.Element {
  return (
    <svg
      data-changes-tab-icon=""
      className="h-3.5 w-3.5 text-[var(--color-text-muted)]"
      viewBox="0 0 16 16"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.3"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden
    >
      <rect x="1.75" y="1.75" width="12.5" height="12.5" rx="1" />
      <path d="M8 4.25v4M6 6.25h4M6 11h4" />
    </svg>
  )
}
