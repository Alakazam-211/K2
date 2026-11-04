// k2:board-file@1 from a real pointer to the body of a save. See harness.ts.
/* eslint-disable @typescript-eslint/no-explicit-any */
import { expect, test } from '@playwright/test'
import { collector, openBoardFile, sticky } from './harness'

declare const BoardApp: any
declare const __d: any
declare const __s: any
declare const __log: any
declare const __errors: string[]

test('opening writes nothing: a rounded number, an unknown type and a stale file stay as they are', async ({ page }) => {
  await openBoardFile(page, {
    objects: [
      { ...sticky('s-a', 100.25, 200.75, 'first'), rot: 1.2345 },
      { id: 'u-a', type: 'hologram', x: 500, y: 100, w: 300, h: 200, spin: 9 },
    ],
    meta: { title: 'In the file', voting: { active: false } },
  })
  await page.waitForTimeout(1500)
  expect(await page.evaluate(() => __d.writes.length)).toBe(0)
  expect(await page.evaluate(() => __errors)).toEqual([])
  // The number shows rounded; the file keeps its own.
  expect(await page.evaluate(() => BoardApp.model.get('s-a').x)).toBeCloseTo(100.3, 5)
  expect(await page.evaluate(() => __d.objects()[0].x)).toBe(100.25)
  // The tab shows the file's name, not settings.title, and clicking it renames nothing.
  await expect(page.locator('.boardname .name')).toContainText('Plan')
  await page.locator('.boardname .name').click()
  await page.waitForTimeout(600)
  expect(await page.evaluate(() => __d.writes.length)).toBe(0)
})

test('a stale timer in what a server sends writes nothing: the widget’s drawHud would, on view', async ({ page }) => {
  await openBoardFile(page, {
    objects: [sticky('s-a', 100, 100)],
    meta: { title: 'x', timer: { state: 'running', endsAt: 1000 } },
  })
  await page.waitForTimeout(1500)
  expect(await page.evaluate(() => __d.writes.length)).toBe(0)
  expect(await page.evaluate(() => BoardApp.model.meta.timer)).toBeUndefined()
})

test('no timer, history, activity pill or JSON export in a tab', async ({ page }) => {
  await openBoardFile(page, { objects: [sticky('s-a', 100, 100)] })
  await expect(page.locator('[aria-label="Timer"], [title="Timer"]')).toHaveCount(0)
  await expect(page.locator('[title="Changes in the last day"], [aria-label="Changes in the last day"]')).toHaveCount(0)
  await expect(page.locator('[title="For agents: let an agent work on this board"]')).toHaveCount(0)
  await page.locator('[title="Export"], [aria-label="Export"]').first().click()
  await expect(page.locator('[title="Download the board as a JSON file"], [aria-label="Download the board as a JSON file"]')).toHaveCount(0)
  await expect(page.locator('[title="Download a picture of the board (PNG)"], [aria-label="Download a picture of the board (PNG)"]')).toHaveCount(1)
})

test('an unknown type shows a placeholder and can’t be selected, moved or deleted', async ({ page }) => {
  await openBoardFile(page, { objects: [sticky('s-a', 100, 100), { id: 'u-a', type: 'hologram', x: 500, y: 100, w: 300, h: 200 }] })
  await expect(page.locator('.foreign')).toContainText('hologram')
  await page.evaluate(() => BoardApp.select(['u-a', 's-a'], { raw: true }))
  expect(await page.evaluate(() => [...BoardApp.sel])).toEqual(['s-a'])
  await page.evaluate(() => BoardApp.select(['u-a'], { raw: true }))
  await page.keyboard.press('Backspace')
  await page.waitForTimeout(600)
  expect(await page.evaluate(() => !!BoardApp.model.get('u-a'))).toBe(true)
  expect(await page.evaluate(() => __d.writes.length)).toBe(0)
  // Undo isn't stuck behind it.
  expect(await page.evaluate(() => BoardApp.model.canUndo())).toBe(false)
})

test('a person’s edits: one gesture, one group, a rev on each op, nothing but the fields named', async ({ page }) => {
  await openBoardFile(page, { objects: [sticky('s-a', 300, 200, 'first')] })
  const c = collector()
  const obj = () => page.evaluate(() => __d.objects())
  // Double-click makes a sticky; typing fills it; Escape ends.
  await page.mouse.dblclick(800, 500)
  await page.keyboard.type('Second')
  await page.keyboard.press('Escape')
  await page.keyboard.press('Meta+s')
  await page.waitForFunction(() => __d.objects().some((o: any) => o.text === 'Second'))
  const made = (await obj()).find((o: any) => o.text === 'Second')
  c.ok('the new sticky reached the file', !!made)
  c.ok('an add carries no z', made.z === undefined, made)
  c.ok('a person’s typing went through', made.text === 'Second')

  // A drag is one group.
  const before = await page.evaluate(() => __d.writes.length)
  const scr = await page.evaluate(() => { const o = BoardApp.model.get('s-a'); const r = BoardApp.toScreen ? BoardApp.toScreen(o.x + 50, o.y + 50) : [o.x + 50 + BoardApp.cam.x, o.y + 50 + BoardApp.cam.y]; return r })
  await page.mouse.move(scr[0], scr[1])
  await page.mouse.down()
  await page.mouse.move(scr[0] + 40, scr[1] + 30, { steps: 6 })
  await page.mouse.up()
  await page.keyboard.press('Meta+s')
  await page.waitForFunction((n) => __d.writes.length > n, before)
  const group = await page.evaluate((n) => __d.writes[n].body.groups[0], before)
  c.ok('a drag is one set', group.ops.length === 1 && group.ops[0].op === 'set', group)
  c.ok('it names only what moved', Object.keys(group.ops[0].f).sort().join() === 'x,y', group.ops[0])
  c.ok('it carries the rev', typeof group.ops[0].rev === 'string', group.ops[0])
  const moved = (await obj()).find((o: any) => o.id === 's-a')
  c.ok('the file has the move, and nothing else changed', moved.x !== 300 && moved.text === 'first' && moved.color === 'yellow', moved)
  expect(c.failures()).toEqual([])
  expect(await page.evaluate(() => __errors)).toEqual([])
})

test('saves hold while typing, and a bring-to-front sends a z op, never a number', async ({ page }) => {
  await openBoardFile(page, { objects: [sticky('s-a', 100, 100), sticky('s-b', 400, 100)] })
  await page.evaluate(() => { BoardApp.select(['s-a'], { raw: true }); BoardApp.startEdit('s-a') })
  await page.keyboard.type('hello')
  await page.waitForFunction(() => __log.holds.includes(true))
  await page.waitForTimeout(800)
  expect(await page.evaluate(() => __d.writes.filter((w: any) => w.route === 'board/file/save').length)).toBe(0)
  await page.keyboard.press('Escape')
  await page.waitForFunction(() => __d.objects()[0].text === 'hello')

  await page.evaluate(() => { const o = BoardApp.model.get('s-a'); BoardApp.model.apply([{ op: 'set', id: o.id, f: { z: BoardApp.model.maxZ() + 1 } }]) })
  await page.waitForFunction(() => __d.objects().at(-1).id === 's-a')
  const last = await page.evaluate(() => __d.writes.at(-1).body.groups.at(-1).ops)
  expect(last).toEqual([{ op: 'z', id: 's-a', to: 'front' }])
})

test('a remote change shows at once, blocks undo over it, and the entry stays', async ({ page }) => {
  await openBoardFile(page, { objects: [sticky('s-a', 100, 100, 'mine')] })
  await page.evaluate(() => BoardApp.model.apply([{ op: 'set', id: 's-a', f: { text: 'edited' } }]))
  await page.waitForFunction(() => __d.objects()[0].text === 'edited')
  await page.evaluate(() => { __d.outsideSet('s-a', { text: 'agent' }); __s.onBoardChanged(__d.hash()) })
  await page.waitForFunction(() => BoardApp.model.get('s-a').text === 'agent')
  const writes = await page.evaluate(() => __d.writes.length)
  expect(await page.evaluate(() => BoardApp.model.undo())).toBe(false)
  await expect(page.locator('#toast')).toContainText('Someone changed this since')
  expect(await page.evaluate(() => BoardApp.model.canUndo())).toBe(true)
  await page.waitForTimeout(600)
  expect(await page.evaluate(() => __d.writes.length)).toBe(writes)
  expect(await page.evaluate(() => __d.objects()[0].text)).toBe('agent')
})

test('a change of order alone doesn’t block undo', async ({ page }) => {
  await openBoardFile(page, { objects: [sticky('s-a', 100, 100, 'mine'), sticky('s-b', 500, 100)] })
  await page.evaluate(() => BoardApp.model.apply([{ op: 'set', id: 's-a', f: { text: 'edited' } }]))
  await page.waitForFunction(() => __d.objects()[0].text === 'edited')
  await page.evaluate(() => { BoardApp.model.applyRemote({ by: 'agent', ops: [{ op: 'set', id: 's-a', f: { z: 9 } }] }) })
  expect(await page.evaluate(() => BoardApp.model.undo())).toBe(true)
  await page.waitForFunction(() => __d.objects()[0].text === 'mine')
})

test('an undo that meets a change it hadn’t seen is refused, goes back, and keeps its entry', async ({ page }) => {
  await openBoardFile(page, { objects: [sticky('s-a', 100, 100, 'mine')] })
  await page.evaluate(() => BoardApp.model.apply([{ op: 'set', id: 's-a', f: { text: 'edited' } }]))
  await page.waitForFunction(() => __d.objects()[0].text === 'edited')
  await page.evaluate(() => __d.outsideSet('s-a', { text: 'agent' })) // no event: the tab hasn't seen it
  expect(await page.evaluate(() => BoardApp.model.undo())).toBe(true)
  await page.evaluate(() => __s.flush())
  await page.waitForFunction(() => BoardApp.model.canUndo())
  // The file keeps theirs; the tab draws theirs after the re-read.
  await page.waitForFunction(() => BoardApp.model.get('s-a').text === 'agent')
  expect(await page.evaluate(() => __d.objects()[0].text)).toBe('agent')
  expect(await page.evaluate(() => __s.heldCount())).toBe(0)
  await expect(page.locator('#toast')).toContainText('undo can’t apply')
})

test('undo of a delete puts back the raw object, unknown fields and exact numbers included', async ({ page }) => {
  await openBoardFile(page, { objects: [{ ...sticky('s-a', 100.25, 100), Foo: 9 }, sticky('s-b', 500, 100)] })
  await page.evaluate(() => BoardApp.model.apply([{ op: 'del', id: 's-a' }]))
  await page.waitForFunction(() => __d.objects().length === 1)
  await page.evaluate(() => BoardApp.model.undo())
  await page.waitForFunction(() => __d.objects().length === 2)
  const back = await page.evaluate(() => __d.objects().find((o: any) => o.id === 's-a'))
  expect(back.x).toBe(100.25)
  expect(back.Foo).toBe(9)
  const op = await page.evaluate(() => __d.writes.at(-1).body.groups[0].ops[0])
  expect(op.op).toBe('putback')
})

test('a conflict keeps the edit, shows theirs, and nothing resolves itself', async ({ page }) => {
  await openBoardFile(page, { objects: [sticky('s-a', 100, 100, 'first')] })
  await page.evaluate(() => { __d.outsideSet('s-a', { text: 'theirs' }); BoardApp.model.apply([{ op: 'set', id: 's-a', f: { text: 'mine' } }]) })
  await page.waitForFunction(() => __s.heldCount() === 1)
  await page.waitForFunction(() => BoardApp.model.get('s-a').text === 'theirs')
  await expect(page.locator('.boardname .status')).toContainText('1 edit isn’t saved')
  await page.waitForTimeout(800)
  expect(await page.evaluate(() => __d.objects()[0].text)).toBe('theirs')
  await page.evaluate(() => __s.useMine(__s.view().held[0].id))
  await page.waitForFunction(() => __d.objects()[0].text === 'mine')
  await page.waitForFunction(() => BoardApp.model.get('s-a').text === 'mine')
  await expect(page.locator('.boardname .status')).toContainText('Saved')
})

test('read only: no tools, no edits by key or pointer, the board still shows, and it lifts when the file is back', async ({ page }) => {
  await openBoardFile(page, { objects: [sticky('s-a', 300, 200, 'first')] })
  await page.evaluate(async () => {
    __d.failAll(__d.notFound())
    await __s.pollInfo()
  })
  await expect(page.locator('#app.read-only')).toHaveCount(1)
  await expect(page.locator('#toolbar')).toBeHidden()
  expect(await page.evaluate(() => BoardApp.model.apply([{ op: 'set', id: 's-a', f: { x: 1 } }]))).toBe(false)
  await expect(page.locator('#toast')).toContainText('moved or deleted')
  // A pointer doesn't move it.
  const at = await page.evaluate(() => { const o = BoardApp.model.get('s-a'); return [o.x + 50 + BoardApp.cam.x, o.y + 50 + BoardApp.cam.y] })
  await page.mouse.move(at[0], at[1])
  await page.mouse.down()
  await page.mouse.move(at[0] + 60, at[1] + 60, { steps: 4 })
  expect(await page.evaluate(() => BoardApp.model.get('s-a').x)).toBe(300) // not even as a preview
  await page.mouse.up()
  expect(await page.evaluate(() => BoardApp.model.get('s-a').x)).toBe(300)
  // Double-click doesn't start an edit.
  await page.mouse.dblclick(at[0], at[1])
  expect(await page.evaluate(() => !!BoardApp.editing)).toBe(false)
  expect(await page.evaluate(() => __d.writes.length)).toBe(0)
  await page.evaluate(async () => {
    __d.failAll(null)
    await __s.pollInfo()
  })
  await expect(page.locator('#app.read-only')).toHaveCount(0)
  expect(await page.evaluate(() => BoardApp.model.apply([{ op: 'set', id: 's-a', f: { x: 1 } }]))).toBe(true)
})

test('a gesture over 64 KB crosses in parts and lands whole', async ({ page }) => {
  await openBoardFile(page, { objects: [sticky('s-a', 100, 100)] })
  const big = 'x'.repeat(4000)
  const n = await page.evaluate((text) => {
    const ops = []
    for (let i = 0; i < 40; i++) ops.push({ op: 'add', obj: { id: 's-' + String(i).padStart(10, '0'), type: 'sticky', x: i * 10, y: 500, w: 100, h: 100, text, color: 'yellow' } })
    return BoardApp.model.apply(ops) ? ops.length : 0
  }, big)
  expect(n).toBe(40)
  await page.waitForFunction(() => __d.objects().length === 41)
  const log = await page.evaluate(() => ({ parts: __log.parts, largest: __log.largestPart }))
  expect(log.parts).toBeGreaterThan(1)
  expect(log.largest).toBeLessThanOrEqual(64 * 1024)
})

test('an object over the Garden’s 60 KB is editable in a file', async ({ page }) => {
  const rows = Array.from({ length: 40 }, () => Array.from({ length: 10 }, () => 'y'.repeat(200)))
  await openBoardFile(page, { objects: [{ id: 't-a', type: 'table', x: 100, y: 100, w: 800, h: 600, rows, rowIds: rows.map((_, i) => 'rw-' + i), colIds: rows[0].map((_, i) => 'cl-' + i), colW: rows[0].map(() => 80) }] })
  const moved = await page.evaluate(() => BoardApp.model.apply([{ op: 'set', id: 't-a', f: { x: 200 } }]))
  expect(moved).toBe(true)
  await page.waitForFunction(() => __d.objects()[0].x === 200)
})

test('K2’s own tab keys cross back; Cmd+S flushes and stays in the tab', async ({ page }) => {
  await openBoardFile(page, { objects: [sticky('s-a', 100, 100)] })
  await page.locator('#viewport').click({ position: { x: 900, y: 700 } })
  await page.keyboard.press('Meta+t')
  await page.keyboard.press('Meta+2')
  await page.waitForFunction(() => __log.keys.length === 2)
  expect(await page.evaluate(() => __log.keys.map((k: any) => k.code))).toEqual(['KeyT', 'Digit2'])
  await page.evaluate(() => BoardApp.model.apply([{ op: 'set', id: 's-a', f: { x: 150 } }]))
  await page.keyboard.press('Meta+s')
  await page.waitForFunction(() => __d.objects()[0].x === 150)
})

test('voting’s clear-all says so, and a reaction carries its token', async ({ page }) => {
  await openBoardFile(page, { objects: [{ ...sticky('s-a', 100, 100), votes: { 'p-other': 2 } }], people: { 'p-other': { name: 'Ann', kind: 'person' } }, me: 'p-me' })
  expect(await page.evaluate(() => BoardApp.store.nameOf('p-other'))).toBe('Ann')
  expect(await page.evaluate(() => BoardApp.model.me)).toBe('p-me')
  await page.evaluate(() => BoardApp.model.apply([{ op: 'set', id: 's-a', f: { votes: {} } }], { byAll: true }))
  await page.waitForFunction(() => __d.writes.some((w: any) => w.route === 'board/file/save'))
  expect(await page.evaluate(() => __d.writes.at(-1).body.groups[0].byAll)).toBe(true)
})
