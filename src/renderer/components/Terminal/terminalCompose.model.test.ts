import { describe, expect, it } from 'vitest'
import { composeModelTyped, filterComposeSlashMenu, normalizeComposeSlashCommand } from './terminalCompose'

describe('/model in the composer (0.46.13)', () => {
  it('the / menu offers /model after the two message commands', () => {
    expect(filterComposeSlashMenu('/').map((i) => i.command)).toEqual(['/compact', '/goal', '/model'])
    expect(filterComposeSlashMenu('/mo').map((i) => i.command)).toEqual(['/model'])
    expect(filterComposeSlashMenu('/x')).toEqual([])
  })

  it('/model never rides a message as a command', () => {
    expect(normalizeComposeSlashCommand('/model')).toBeNull()
  })

  it('“/model ” opens the picker, pre-filtered by what follows', () => {
    expect(composeModelTyped('/model ')).toBe('')
    expect(composeModelTyped('/model op')).toBe('op')
    expect(composeModelTyped('/MODEL  haiku ')).toBe('haiku')
    expect(composeModelTyped('/model')).toBeNull()
    expect(composeModelTyped('/modelx')).toBeNull()
    expect(composeModelTyped('use /model op')).toBeNull()
  })
})
