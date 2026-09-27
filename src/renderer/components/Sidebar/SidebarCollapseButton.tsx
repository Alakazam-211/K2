import { useSidebarStore } from '@/stores/sidebar'

/** Collapse or open the workspaces nav. Lives at the bottom of the nav. */
export function SidebarCollapseButton(): JSX.Element {
  const collapsed = useSidebarStore((s) => s.isCollapsed)
  const toggle = useSidebarStore((s) => s.toggle)
  return (
    <button
      type="button"
      className="no-drag flex items-center justify-center w-8 h-8 flex-shrink-0 text-[var(--color-text-muted)] hover:text-[var(--color-text-secondary)] hover:bg-white/[0.06] transition-colors"
      onClick={() => toggle()}
      title={collapsed ? 'Open workspaces sidebar' : 'Collapse workspaces sidebar'}
      aria-label={collapsed ? 'Open workspaces sidebar' : 'Collapse workspaces sidebar'}
    >
      <svg
        className="w-4 h-4"
        viewBox="0 0 14 14"
        fill="none"
        stroke="currentColor"
        strokeWidth="1.3"
        strokeLinecap="round"
        strokeLinejoin="round"
        aria-hidden
      >
        <rect x="1" y="2" width="12" height="10" rx="0" />
        <line x1="5" y1="2" x2="5" y2="12" strokeDasharray={collapsed ? '1.5 1.5' : undefined} />
      </svg>
    </button>
  )
}
