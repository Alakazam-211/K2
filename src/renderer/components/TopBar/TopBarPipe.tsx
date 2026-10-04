/** The top bar's vertical divider between items. A span (block, so it
 *  sizes the same in any flex row) so it may sit inside a button too, such
 *  as the usage chip's "{server} | 42%". */
export default function TopBarPipe(): React.JSX.Element {
  return <span aria-hidden="true" className="block w-px h-4 bg-[var(--color-border)] mx-1" data-top-bar-pipe="" />
}
