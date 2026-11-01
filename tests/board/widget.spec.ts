// Ported from Board's own browser checks (widget), which ran in Chrome only: now in Chromium and WebKit,
// on the readable widget files, against the stand-in `k2` (tests/board/fake-k2.js) with K2's Garden data.
// The functional checks: canvas, content, frames, groups, layers, voting, mind map, tables, comments, templates, search, copy and paste.
// Each check keeps its own wording; a miss fails the test and lists every check that missed.
/* eslint-disable */
import { expect, test } from '@playwright/test'
import { collector, openBoard, sticky, threeBoards } from './harness'

test('widget: The functional checks: canvas, content, frames, groups, layers, voting, mind map, tables, comments, templates, search, copy and paste', async ({ page }) => {
  const p = page
  const { ok, failures, count: checks } = collector()
  await openBoard(p, { seed: [['seeded', { at: '2026-01-01T00:00:00Z' }], ['boards/b1', { id: 'b1', name: 'Test', project: null, format: 1 }], ['board/b1/meta', { format: 1, title: 'Test' }]] }, { board: 'b1' })
  const ev = (f, a) => p.evaluate(f, a)
  const objs = () => ev(() => BoardApp.model.all().map((o) => Object.assign({}, o)))
  const scr = (x, y) => ev(([x, y]) => BoardApp.toScreen(x, y), [x, y])
  const cam = () => ev(() => Object.assign({}, BoardApp.cam))
  const drag = async (a, b, opts) => { await p.mouse.move(a[0], a[1]); await p.mouse.down(); await p.mouse.move((a[0] + b[0]) / 2, (a[1] + b[1]) / 2, { steps: 4 }); await p.mouse.move(b[0], b[1], { steps: 4 }); await p.mouse.up() }

  // --- Phase 1: canvas ---
  let c0 = await cam()
  await p.keyboard.down(' '); await drag([300, 300], [400, 360]); await p.keyboard.up(' ')
  let c1 = await cam(); ok('pan with space-drag', Math.round(c1.x - c0.x) === 100 && Math.round(c1.y - c0.y) === 60, [c0, c1])
  await p.mouse.move(700, 450); await p.keyboard.down('Control'); await p.mouse.wheel(0, -200); await p.keyboard.up('Control')
  const c2 = await cam(); ok('ctrl+wheel zooms in', c2.s > c1.s * 1.5, c2)
  const before = await ev(() => BoardApp.toWorld(700, 450)); ok('zoom keeps the point under the cursor', Math.abs(before[0] - (700 - c2.x) / c2.s) < 0.01)
  await ev(() => { BoardApp.cam.x = 0; BoardApp.cam.y = 0; BoardApp.cam.s = 1; BoardApp.applyCam() })
  await p.mouse.wheel(0, 120); ok('wheel pans', (await cam()).y === -120)
  await ev(() => { BoardApp.cam.y = 0; BoardApp.applyCam() })

  // double-click → sticky + typing
  await p.mouse.dblclick(400, 300); await p.keyboard.type('First note'); await p.keyboard.press('Escape')
  let o = (await objs()).find((x) => x.type === 'sticky')
  ok('double-click makes a sticky and typing fills it', o && o.text === 'First note' && o.x === 300 && o.y === 200, o)
  // move
  await drag([400, 300], [520, 340])
  o = (await objs()).find((x) => x.id === o.id); ok('drag moves it', o.x === 420 && o.y === 240, o)
  // resize (sticky keeps its aspect)
  let h = await p.$('.handle.se'); let hb = await h.boundingBox()
  await drag([hb.x + 5, hb.y + 5], [hb.x + 105, hb.y + 25])
  o = (await objs()).find((x) => x.id === o.id); ok('resize from a corner keeps a sticky square', o.w === o.h && o.w > 260, o)
  // rotate
  h = await p.$('.handle.rot'); hb = await h.boundingBox()
  const ctr = await scr(o.x + o.w / 2, o.y + o.h / 2)
  await drag([hb.x + 7, hb.y + 7], [ctr[0] + 200, ctr[1] - 200])
  o = (await objs()).find((x) => x.id === o.id); ok('rotate handle rotates (≈45°)', Math.abs(o.rot - 45) <= 2, o.rot)
  // undo ×3, redo ×1
  await p.keyboard.press('Meta+z'); o = (await objs()).find((x) => x.id === o.id); ok('undo rotation', !o.rot, o.rot)
  await p.keyboard.press('Meta+z'); o = (await objs()).find((x) => x.id === o.id); ok('undo resize', o.w === 200, o.w)
  await p.keyboard.press('Meta+z'); o = (await objs()).find((x) => x.id === o.id); ok('undo move', o.x === 300, o.x)
  await p.keyboard.press('Meta+Shift+z'); o = (await objs()).find((x) => x.id === o.id); ok('redo move', o.x === 420, o.x)
  ok('undo button is enabled after an edit', await ev(() => !document.querySelector('#topleft .btn').disabled))
  // keyboard: N creates a sticky in the middle
  await ev(() => BoardApp.select([])); await p.keyboard.press('n')
  ok('N picks the sticky tool', await ev(() => BoardApp.tool === 'sticky'))
  await p.mouse.click(800, 300); await p.keyboard.type('Second'); await p.keyboard.press('Escape')
  ok('click with sticky tool places one', (await objs()).filter((x) => x.type === 'sticky').length === 2)
  // multi-select with shift-click and marquee
  await ev(() => BoardApp.select([])); const s = (await objs()).filter((x) => x.type === 'sticky')
  const a = await scr(s[0].x + 20, s[0].y + 20); const b = await scr(s[1].x + 20, s[1].y + 20)
  await p.mouse.click(a[0], a[1]); await p.keyboard.down('Shift'); await p.mouse.click(b[0], b[1]); await p.keyboard.up('Shift')
  ok('shift-click adds to the selection', await ev(() => BoardApp.sel.size === 2))
  await p.mouse.click(1300, 800); ok('click on empty canvas clears selection', await ev(() => BoardApp.sel.size === 0))
  await drag([150, 120], [1100, 700]); ok('marquee selects both', await ev(() => BoardApp.sel.size === 2))
  // copy / paste / duplicate / delete
  await p.keyboard.press('Meta+c'); await ev(() => BoardApp.pasteInternal())
  ok('copy + paste makes 2 new objects', (await objs()).length === 4)
  await p.keyboard.press('Meta+d'); ok('duplicate makes 2 more', (await objs()).length === 6)
  await p.keyboard.press('Backspace'); ok('delete removes the selection', (await objs()).length === 4)
  await p.keyboard.press('Meta+z'); ok('undo brings them back', (await objs()).length === 6)
  await p.keyboard.press('Meta+a'); await p.keyboard.press('Backspace'); await p.keyboard.press('Meta+z')
  ok('select-all, delete, undo', (await objs()).length === 6)
  await ev(() => BoardApp.model.apply(BoardApp.model.all().map((o) => ({ op: 'del', id: o.id }))))
  // nudge
  await ev(() => BoardApp.create('sticky', { x: 0, y: 0 }))
  await p.keyboard.press('ArrowRight'); await p.keyboard.press('Shift+ArrowDown')
  o = (await objs())[0]; ok('arrow keys nudge (1 and 24)', o.x === 1 && o.y === 24, o)

  // --- Phase 2: content ---
  await ev(() => { BoardApp.model.apply(BoardApp.model.all().map((o) => ({ op: 'del', id: o.id }))); BoardApp.model.apply([{ op: 'add', obj: { id: 'h-a', type: 'shape', x: 100, y: 100, w: 160, h: 100 } }, { op: 'add', obj: { id: 'h-b', type: 'shape', x: 600, y: 300, w: 160, h: 100 } }]) })
  await p.keyboard.press('l')
  let pa = await scr(180, 150); let pb = await scr(680, 350)
  await drag(pa, pb)
  let conn = (await objs()).find((x) => x.type === 'connector')
  ok('line tool connects two shapes', conn && conn.from.id === 'h-a' && conn.to.id === 'h-b', conn && [conn.from, conn.to])
  await ev(() => BoardApp.select([])); pb = await scr(680, 350); await drag(pb, [pb[0] + 0, pb[1] + 200])
  const route = await ev(() => { const c = BoardApp.model.all().find((x) => x.type === 'connector'); return BoardApp.R.el(c.id)._route.pts.slice(-1)[0] })
  const bnow = (await objs()).find((x) => x.id === 'h-b')
  ok('connector stays attached when its end moves', bnow.y === 500 && Math.abs(route[1] - (bnow.y + (route[1] > bnow.y + 10 ? bnow.h : 0))) < 60 && route[0] >= bnow.x - 1 && route[0] <= bnow.x + bnow.w + 1, { route, bnow: [bnow.x, bnow.y] })
  // hover dots → connector
  await ev(() => BoardApp.select([]))
  const am = await scr(180, 150); await p.mouse.move(am[0], am[1]); await p.waitForTimeout(50)
  const dot = await p.$('.cdot[data-side="right"]'); ok('hovering a shape shows connection dots', !!dot)
  // shapes, all kinds render
  const nShapes = await ev(() => { const ops = BoardApp.S.SHAPES.map((s, i) => ({ op: 'add', obj: { id: 'h-s' + i, type: 'shape', shape: s, x: i * 130, y: 900, w: 120, h: 80 } })); BoardApp.model.apply(ops); return document.querySelectorAll('.t-shape path[d]').length })
  ok('every shape kind renders a path', nShapes >= 24, nShapes)
  // pen + eraser
  await ev(() => { BoardApp.cam.x = 0; BoardApp.cam.y = 0; BoardApp.cam.s = 1; BoardApp.applyCam() })
  await p.keyboard.press('p'); await drag([300, 600], [600, 650])
  const pen = (await objs()).find((x) => x.type === 'pen'); ok('pen draws a stroke', pen && pen.points.length >= 2 && pen.w > 250, pen && pen.points.length)
  await p.keyboard.press('Shift+P'); await drag([300, 700], [600, 700])
  ok('highlighter draws a highlighter stroke', (await objs()).some((x) => x.type === 'pen' && x.highlighter))
  await p.keyboard.press('e'); await drag([450, 560], [450, 720])
  ok('eraser erases both strokes it crosses', !(await objs()).some((x) => x.type === 'pen'))
  // text tool
  await p.keyboard.press('t'); await p.mouse.click(300, 400); await p.keyboard.type('Hello text'); await p.keyboard.press('Escape')
  ok('text tool makes a text object', (await objs()).some((x) => x.type === 'text' && x.text === 'Hello text'))

  // --- Phase 3: frames, groups, lock, layers, align ---
  await ev(() => BoardApp.model.apply(BoardApp.model.all().map((o) => ({ op: 'del', id: o.id }))))
  await ev(() => BoardApp.model.apply([{ op: 'add', obj: { id: 's-in', type: 'sticky', x: 200, y: 200 } }]))
  await p.keyboard.press('f'); await drag([100, 120], [700, 600])
  const fr = (await objs()).find((x) => x.type === 'frame'); ok('frame tool draws a frame', fr && fr.w === 600, fr && [fr.w, fr.h])
  await p.keyboard.press('Escape'); await ev(() => BoardApp.select([]))
  const t = await p.$('.t-frame .ftitle'); const tb = await t.boundingBox()
  await drag([tb.x + 10, tb.y + 5], [tb.x + 110, tb.y + 5])
  ok('moving a frame by its title carries what is inside', (await objs()).find((x) => x.id === 's-in').x === 300)
  await ev(() => { BoardApp.model.apply([{ op: 'add', obj: { id: 's-g1', type: 'sticky', x: 900, y: 100 } }, { op: 'add', obj: { id: 's-g2', type: 'sticky', x: 1150, y: 100 } }]); BoardApp.select(['s-g1', 's-g2'], { raw: true }) })
  await p.keyboard.press('Meta+g'); await ev(() => BoardApp.select([]))
  const g1 = await scr(1000, 200); await p.mouse.click(g1[0], g1[1])
  ok('Cmd+G groups; clicking one selects the group', await ev(() => BoardApp.sel.size === 2))
  await p.keyboard.press('Meta+Shift+g'); await ev(() => BoardApp.select(['s-g1'])); await p.keyboard.press('Meta+Shift+l')
  ok('Shift+Cmd+L locks', (await objs()).find((x) => x.id === 's-g1').locked === true)
  await ev(() => BoardApp.select([])); await drag(g1, [g1[0] + 100, g1[1]])
  ok('a locked object does not move', (await objs()).find((x) => x.id === 's-g1').x === 900)
  await ev(() => { BoardApp.model.apply([{ op: 'set', id: 's-g1', f: { locked: false } }, { op: 'set', id: 's-g2', f: { x: 1000, y: 150 } }]); BoardApp.select(['s-g1'], { raw: true }) })
  const z0 = (await objs()).find((x) => x.id === 's-g2').z
  await p.keyboard.press(']'); ok('] brings forward past the overlapping note', (await objs()).find((x) => x.id === 's-g1').z > z0)
  await ev(() => BoardApp.select(['s-in', 's-g1', 's-g2'], { raw: true })); await ev(() => BoardApp.align('left'))
  const xs = (await objs()).filter((x) => ['s-in', 's-g1', 's-g2'].includes(x.id)).map((x) => x.x); ok('align left lines them up', new Set(xs).size === 1, xs)
  await ev(() => BoardApp.distribute('v'))
  const ys = (await objs()).filter((x) => ['s-in', 's-g1', 's-g2'].includes(x.id)).map((x) => x.y).sort((a, b) => a - b)
  ok('distribute spaces them evenly', Math.abs((ys[1] - ys[0]) - (ys[2] - ys[1])) <= 1, ys)
  await ev(() => { BoardApp.snap = true; BoardApp.model.apply([{ op: 'add', obj: { id: 's-snap', type: 'sticky', x: 1300, y: 700 } }]); BoardApp.select([]) })
  const si = (await objs()).find((x) => x.id === 's-snap'); const sp = await scr(si.x + 50, si.y + 50); await drag(sp, [sp[0] + 37, sp[1] + 13])
  const si2 = (await objs()).find((x) => x.id === 's-snap'); ok('grid snap moves in 24s', si2.x % 24 === 0 && si2.y % 24 === 0, [si2.x, si2.y]); await ev(() => { BoardApp.snap = false })
  await p.click('#topright .btn.primary'); ok('Present enters presentation mode', await ev(() => BoardApp.presenting))
  await p.keyboard.press('Escape'); ok('Esc leaves it', await ev(() => !BoardApp.presenting))

  // --- Phase 4 ---
  await ev(() => BoardApp.model.apply(BoardApp.model.all().map((o) => ({ op: 'del', id: o.id }))))
  await ev(() => BoardApp.model.apply([{ op: 'add', obj: { id: 's-v', type: 'sticky', x: 200, y: 200 } }, { op: 'add', obj: { id: 'm-r', type: 'mind', x: 600, y: 200, w: 200, h: 60, text: 'Root' } }]))
  await ev(() => BoardApp.toggleReaction('s-v', '👍')); ok('reaction toggles on', JSON.stringify((await objs()).find((x) => x.id === 's-v').reactions) === '{"person:owner":["👍"]}')
  await ev(() => BoardApp.model.apply([{ op: 'meta', f: { voting: { active: true, perPerson: 2 } } }]))
  await p.waitForTimeout(50)
  const vb = await p.$('.t-sticky .votes.live'); const vbb = await vb.boundingBox()
  await p.mouse.click(vbb.x + 5, vbb.y + 5); await p.mouse.click(vbb.x + 5, vbb.y + 5); await p.mouse.click(vbb.x + 5, vbb.y + 5)
  ok('voting: badge click votes, capped at the allowance', (await objs()).find((x) => x.id === 's-v').votes['person:owner'] === 2)
  await ev(() => BoardApp.select(['m-r'], { raw: true })); await p.keyboard.press('Tab'); await p.keyboard.type('Child'); await p.keyboard.press('Escape')
  ok('mind map: Tab adds a child', (await objs()).some((x) => x.type === 'mind' && x.parent === 'm-r' && x.text === 'Child'))
  await ev(() => BoardApp.model.apply([{ op: 'add', obj: { id: 'b-t', type: 'table', x: 100, y: 500 } }]))
  const cell = await p.$('.t-table .cell[data-r="1"][data-c="1"]'); const cb = await cell.boundingBox()
  await p.mouse.dblclick(cb.x + 10, cb.y + 10); await p.keyboard.type('x1'); await p.keyboard.press('Tab'); await p.keyboard.type('x2'); await p.keyboard.press('Escape')
  const tbl = (await objs()).find((x) => x.id === 'b-t'); ok('table: edit a cell, Tab to the next', tbl.rows[1][1] === 'x1' && tbl.rows[1][2] === 'x2', tbl.rows)
  await p.keyboard.press('c'); await p.mouse.click(900, 600); await p.keyboard.type('A comment'); await p.keyboard.press('Meta+Enter')
  ok('comment tool: place and write', (await objs()).some((x) => x.type === 'comment' && x.text === 'A comment'), await ev(() => ({ active: document.activeElement.tagName + '.' + document.activeElement.className, pop: !document.getElementById('pop').hidden, tool: BoardApp.tool, comments: BoardApp.model.all().filter((o) => o.type === 'comment').map((o) => o.text) })))
  await ev(() => BoardApp.model.apply([{ op: 'meta', f: { timer: { state: 'running', duration: 60, endsAt: Date.now() + 60000 } } }])); await p.waitForTimeout(300)
  ok('timer shows in the HUD', /0:5\d|1:00/.test(await p.textContent('#hud')))

  // --- Phase 5 ---
  await p.click('#toolbar .btn[aria-label="Templates"]'); await p.click('.tcard:has-text("Kanban")'); await p.waitForTimeout(100)
  ok('template inserts a kanban', (await objs()).filter((x) => x.type === 'card').length === 6)   // 4 columns' first cards + 2 more
  await p.keyboard.press('Meta+f'); await p.keyboard.type('onboarding')   // only "Fix the onboarding copy" has it; await p.waitForTimeout(80)
  ok('search finds text', /1 found/.test(await p.textContent('.sinfo').catch(() => 'none')), await ev(() => ({ pop: document.getElementById('pop').textContent.slice(0, 80), active: document.activeElement.tagName + '.' + document.activeElement.className })))
  await p.keyboard.press('Escape')
  // Copy as in the real Garden: the host's Edit menu takes Cmd+C (no keydown reaches the widget, only the
  // copy event), and the async clipboard is blocked. The event's own clipboard carries the objects.
  await ev(() => { if (navigator.clipboard) navigator.clipboard.writeText = () => Promise.reject(new Error('blocked')); BoardApp.select(BoardApp.model.all().filter((o) => o.type === 'sticky').slice(0, 2).map((o) => o.id)) })
  const k = await ev(() => BoardApp.withConnectors(BoardApp.selected()).length)
  let n0 = (await objs()).length
  const copied = await ev(() => { const dt = new DataTransfer(); document.dispatchEvent(new ClipboardEvent('copy', { clipboardData: dt, bubbles: true, cancelable: true })); return dt.getData('text/plain') })
  ok('Garden copy (event only, clipboard API blocked) puts the selection on the clipboard', /^⧉boardclip \[/.test(copied) && k > 0 && JSON.parse(copied.slice(11)).length === k, [k, copied.length, (() => { try { return JSON.parse(copied.slice(11)).map((o) => o.id + ':' + o.type) } catch (e) { return e.message } })()])
  await ev((t) => { const dt = new DataTransfer(); dt.setData('text/plain', t); document.dispatchEvent(new ClipboardEvent('paste', { clipboardData: dt, bubbles: true, cancelable: true })) }, copied)
  ok('…and Garden paste of it makes as many new objects', (await objs()).length === n0 + k, [n0, (await objs()).length])
  n0 = (await objs()).length
  const k2 = await ev(() => BoardApp.withConnectors(BoardApp.selected()).length)
  const cutText = await ev(() => { const dt = new DataTransfer(); document.dispatchEvent(new KeyboardEvent('keydown', { key: 'x', metaKey: true, bubbles: true })); document.dispatchEvent(new ClipboardEvent('cut', { clipboardData: dt, bubbles: true, cancelable: true })); return dt.getData('text/plain') })
  await p.waitForTimeout(50)
  ok('Cmd+X with its cut event: cut once (gone, on the clipboard)', k2 > 0 && (await objs()).length === n0 - k2 && /^⧉boardclip /.test(cutText), [(await objs()).length, n0])
  await p.keyboard.press('Meta+z'); ok('…one undo brings them back', (await objs()).length === n0)
  const typed = await ev(() => { const s = BoardApp.model.all().find((o) => o.type === 'sticky'); BoardApp.select([]); BoardApp.startEdit(s.id); const dt = new DataTransfer(); document.activeElement.dispatchEvent(new ClipboardEvent('copy', { clipboardData: dt, bubbles: true, cancelable: true })); const r = dt.getData('text/plain'); BoardApp.endEdit(true); return r })
  ok('copy while editing text leaves it to the text box', typed === '')
  // Text highlighted in a panel: Cmd+X is the browser's (a no-op on plain text), never a cut of the selected objects.
  n0 = (await objs()).length
  await ev(() => { BoardApp.select(BoardApp.model.all().filter((o) => o.type === 'sticky').slice(0, 1).map((o) => o.id)); const d = document.createElement('div'); d.id = 'seltest'; d.style.userSelect = 'text'; d.style.webkitUserSelect = 'text'; d.textContent = 'some panel text'; document.body.appendChild(d); const rg = document.createRange(); rg.selectNodeContents(d); const ts = document.getSelection(); ts.removeAllRanges(); ts.addRange(rg); document.dispatchEvent(new KeyboardEvent('keydown', { key: 'x', metaKey: true, bubbles: true })); document.dispatchEvent(new ClipboardEvent('cut', { clipboardData: new DataTransfer(), bubbles: true, cancelable: true })) })
  await p.waitForTimeout(50)
  ok('Cmd+X with panel text highlighted deletes nothing', (await objs()).length === n0 && await ev(() => BoardApp.sel.size === 1), [n0, (await objs()).length, await ev(() => [BoardApp.sel.size, String(document.getSelection())])])
  // The right-click Copy goes through the same copy event, so the system clipboard gets the objects too.
  const viaMenu = await ev(() => { BoardApp.select(BoardApp.model.all().filter((o) => o.type === 'sticky').slice(0, 1).map((o) => o.id)); let got = ''; const h = (e) => { setTimeout(() => {}, 0); got = 'event' }; document.addEventListener('copy', h); BoardApp.copyCmd(false); document.removeEventListener('copy', h); return got })
  ok('menu Copy fires the copy event (system clipboard), not only the internal copy', viaMenu === 'event')
  n0 = (await objs()).length
  await ev(() => document.dispatchEvent(new KeyboardEvent('keydown', { key: 'x', metaKey: true, bubbles: true })))   // no cut event: cut is disabled on plain text
  await p.waitForTimeout(50)
  ok('…nor does Cmd+X when the browser fires no cut event at all', (await objs()).length === n0, [n0, (await objs()).length])
  // A click on the canvas drops that old text selection, so the next Cmd+C copies objects again.
  await p.mouse.click(1300, 800)
  ok('a canvas click clears an old text selection', await ev(() => { const r = document.getSelection().isCollapsed; document.getElementById('seltest').remove(); return r }))
  await p.keyboard.press('Escape')

  ok('no page errors', (await p.evaluate(() => (window as any).__errors)).length === 0, await p.evaluate(() => (window as any).__errors))
  expect(failures(), checks() + ' checks').toEqual([])
})
