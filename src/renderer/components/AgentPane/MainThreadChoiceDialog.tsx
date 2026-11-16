import { useState } from 'react'
import { Button, DialogFrame, DialogScrim, Radio } from '@/components/ui'
import type { MainThreadChoice } from '@/lib/main-thread-choice'

/** A person makes another chat (`name`, a sidecar or a past chat) the main
 *  chat while both chats have a Thread: which one the main chat shows from
 *  now on. Nothing is deleted; the other Thread keeps an address of its own.
 *  Enter makes the switch with the picked choice; Esc, the × and Cancel
 *  change nothing. */
export function MainThreadChoiceDialog({
  name,
  onChoose,
  onCancel,
}: {
  name: string
  onChoose: (choice: MainThreadChoice) => void
  onCancel: () => void
}): React.JSX.Element {
  const [choice, setChoice] = useState<MainThreadChoice>('keep_main')
  const option = (
    value: MainThreadChoice,
    label: string,
    hint: string,
  ): React.JSX.Element => (
    <>
      <label
        style={{ display: 'flex', alignItems: 'center', gap: 8, fontSize: 12, color: 'var(--color-text-primary)', marginBottom: 2, cursor: 'pointer' }}
      >
        <Radio
          name="main-thread-choice"
          value={value}
          checked={choice === value}
          autoFocus={value === 'keep_main'}
          onChange={() => setChoice(value)}
          data-main-thread-choice={value}
        />
        {label}
      </label>
      <div style={{ fontSize: 11, color: 'var(--color-text-muted)', margin: '0 0 10px 22px' }}>{hint}</div>
    </>
  )
  return (
    <>
      <DialogScrim />
      <DialogFrame
        data-testid="main-thread-choice"
        role="dialog"
        aria-modal="true"
        aria-labelledby="main-thread-choice-title"
        onCancel={onCancel}
        style={{ minWidth: 340, maxWidth: 440, padding: '20px 24px', fontFamily: 'var(--font-ui)' }}
      >
        <form
          onSubmit={(e) => {
            e.preventDefault()
            onChoose(choice)
          }}
        >
          <div
            id="main-thread-choice-title"
            style={{ fontSize: 14, fontWeight: 600, color: 'var(--color-text-primary)', marginBottom: 8, paddingRight: 16 }}
          >
            Make {name} the main chat
          </div>
          <div style={{ fontSize: 12, color: 'var(--color-text-secondary)', marginBottom: 14 }}>
            Both chats have a Thread. Which one should the main chat show?
          </div>
          <div role="radiogroup" aria-label="Thread for the main chat">
            {option(
              'keep_main',
              "Keep the main chat's Thread",
              `${name}'s Thread is kept at an address of its own.`,
            )}
            {option(
              'adopt_sidecar',
              `Use ${name}'s Thread`,
              "The main chat's Thread is kept with the old chat, at an address of its own.",
            )}
          </div>
          <div style={{ display: 'flex', justifyContent: 'flex-end', gap: 8, marginTop: 8 }}>
            <Button type="button" variant="ghost" size="md" onClick={onCancel}>
              Cancel
            </Button>
            <Button type="submit" variant="accent" size="md" data-main-thread-confirm="">
              Make main chat
            </Button>
          </div>
        </form>
      </DialogFrame>
    </>
  )
}
