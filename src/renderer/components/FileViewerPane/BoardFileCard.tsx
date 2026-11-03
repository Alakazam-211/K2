import { BinaryEmptyState } from './BinaryEmptyState'

/**
 * A Board file (`.k5board`) on a build with no Board tab. It reads nothing and
 * writes nothing: a board is a zip, and only the format's codec may touch it.
 */
export function BoardFileCard({ filePath }: { filePath: string }): React.JSX.Element {
  return <BinaryEmptyState filePath={filePath} message="This is a Board file. Update K2 to open it." />
}
