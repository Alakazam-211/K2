// The sections inside Home are "groups" wherever people see them; the page
// itself stays "Home" (as do "Add to Home" and "Home sync"). The phone app
// (K2 Companion 4.3.0) made the same change, and where the desktop has the
// same control it uses the phone's words, character for character (the
// curly apostrophe included). The phone's list is copied here as written.
import { readdirSync, readFileSync, statSync } from 'node:fs'
import { join, relative } from 'node:path'
import { describe, expect, it } from 'vitest'
import {
  ALREADY_IN_GROUP_NOTE,
  CURRENT_GROUP_NOTE,
  GROUP_EMPTY_TEXT,
  GROUP_NAME_LABEL,
  GROUPS_LABEL,
  LAST_GROUP_TEXT,
  MOVE_TO_GROUP_LABEL,
  NEW_GROUP_LABEL,
  removeFromGroupLabel,
} from './group-copy'

/** The phone's strings for Home groups, as it ships them. */
const PHONE = {
  moveTo: 'Move to group…',
  moveSheetTitle: 'Move <agent> to a group',
  groupsList: 'Groups',
  current: 'Current',
  alreadyIn: 'Already in it',
  cancel: 'Cancel',
  closeGroups: 'Close groups',
  newGroupItem: 'New group…',
  newGroupForm: 'New group',
  groupName: 'Group name',
  rename: 'Rename…',
  delete: 'Delete',
  lastGroup: 'The last group can’t be deleted.',
  titleButton: 'Group: <name>',
  removeFrom: 'Remove from <name>',
  empty: 'This group is empty',
  noMatchesAnywhere: 'No matches in any group.',
} as const

describe('Home groups: the desktop says what the phone says', () => {
  it('the controls both have use the phone’s exact words', () => {
    expect(MOVE_TO_GROUP_LABEL).toBe(PHONE.moveTo)
    expect(GROUPS_LABEL).toBe(PHONE.groupsList)
    expect(CURRENT_GROUP_NOTE).toBe(PHONE.current)
    expect(ALREADY_IN_GROUP_NOTE).toBe(PHONE.alreadyIn)
    expect(NEW_GROUP_LABEL).toBe(PHONE.newGroupForm)
    expect(GROUP_NAME_LABEL).toBe(PHONE.groupName)
    expect(LAST_GROUP_TEXT).toBe(PHONE.lastGroup)
    expect(GROUP_EMPTY_TEXT).toBe(PHONE.empty)
    expect(removeFromGroupLabel('<name>')).toBe(PHONE.removeFrom)
    expect(removeFromGroupLabel('Group 2')).toBe('Remove from Group 2')
  })

  it('the apostrophe is the curly one, as on the phone', () => {
    expect(LAST_GROUP_TEXT).toContain('’')
    expect(LAST_GROUP_TEXT).not.toContain("'")
  })
})

// The words the phone retired, and the desktop's own old ones for the same
// things. None may come back in the renderer's own text (tests aside).
const RETIRED = [
  'Move to Home',
  'New Home',
  'Home name',
  'at least one Home',
  'Your Home is empty',
  'any Home',
  'Homes Sync',
  'Homes sync',
  'sync Homes',
  'Homes stay',
  'Homes could',
  'your Homes',
  'Your Homes',
  'Its Homes',
  'its Homes',
  'the Homes',
  'every Home',
  'this Home',
  'This Home',
  'Pick a Home',
  'on a Home',
  'a Home id',
  'another Home',
  'the Home’s order',
  'Close Homes',
] as const

const renderer = join(__dirname, '..', '..', '..')

function sources(dir: string, out: string[] = []): string[] {
  for (const name of readdirSync(dir)) {
    const path = join(dir, name)
    if (statSync(path).isDirectory()) {
      if (name === 'zen-lib' || name === 'node_modules') continue
      sources(path, out)
    } else if (/\.(ts|tsx)$/.test(name) && !/\.(test|mstest)\.tsx?$/.test(name)) {
      out.push(path)
    }
  }
  return out
}

/** Only the words in quotes: comments and code names may still say Home. */
function quoted(src: string): string[] {
  const noComments = src.replace(/\/\*[\s\S]*?\*\//g, ' ').replace(/(^|[^:'"`])\/\/.*$/gm, '$1')
  return noComments.match(/'(?:[^'\\\n]|\\.)*'|"(?:[^"\\\n]|\\.)*"|`(?:[^`\\]|\\.)*`|>[^<>{}]*</g) ?? []
}

describe('Home groups: the old words are gone from the renderer', () => {
  it('no text in quotes or between tags uses a retired phrase', () => {
    const hits: string[] = []
    for (const file of sources(renderer)) {
      for (const s of quoted(readFileSync(file, 'utf8'))) {
        for (const word of RETIRED) {
          if (new RegExp(`\\b${word}\\b`).test(s)) hits.push(`${relative(renderer, file)}: ${word} in ${s.slice(0, 90)}`)
        }
      }
    }
    expect(hits).toEqual([])
  })
})
