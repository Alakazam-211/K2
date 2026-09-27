/** A terminal window with a cursor. Plain shell tabs, not the `>_` prompt. */
export function ShellTabIcon(): JSX.Element {
  return (
    <svg
      data-shell-tab-icon=""
      className="h-3.5 w-3.5 text-[var(--color-text-muted)]"
      viewBox="0 0 16 16"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.3"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden
    >
      <rect x="1.75" y="2.25" width="12.5" height="11.5" rx="1" />
      <path d="M4 10.75h3.25" />
    </svg>
  )
}
