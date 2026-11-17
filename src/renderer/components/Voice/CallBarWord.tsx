// A slim bar action's face: its word, and the icon it folds to in a narrow
// top bar (call-bar-fit: the `icons` and `minimal` steps). The icon is hidden
// everywhere else; the button keeps its word as its name (aria-label) and in
// its tooltip, so the icon alone still says what it does.

import type { ReactNode } from 'react'

export default function CallBarWord({ icon, word }: { icon: ReactNode; word: string }): React.JSX.Element {
  return (
    <>
      <span aria-hidden data-call-bar-glyph="" className="items-center">
        {icon}
      </span>
      <span data-call-bar-label="">{word}</span>
    </>
  )
}
