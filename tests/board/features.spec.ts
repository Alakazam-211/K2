// Ported from Board's own browser checks (features), which ran in Chrome only: now in Chromium and WebKit,
// on the readable widget files, against the stand-in `k2` (tests/board/fake-k2.js) with K2's Garden data.
// The feature checks: quick add, copy style, tidy up, links, checklists, crop, stamps, history and more.
// Each check keeps its own wording; a miss fails the test and lists every check that missed.
/* eslint-disable */
import { expect, test } from '@playwright/test'
import { collector, openBoard, sticky, threeBoards } from './harness'

test('features: The feature checks: quick add, copy style, tidy up, links, checklists, crop, stamps, history and more', async ({ page }) => {
  const p = page
  const { ok, failures, count: checks } = collector()
  await openBoard(p, threeBoards([sticky('a', 400, 100, 'Top right'), sticky('b', 100, 110, 'Top left'), sticky('c', 100, 400, 'Bottom')]), { board: 'b1', objects: 3 })
  const ev = (f, a) => p.evaluate(f, a)
  const active = () => ev(() => { const a = document.activeElement; return { tag: a.tagName, id: a.id, cls: a.className, label: a.getAttribute('aria-label'), role: a.getAttribute('role'), text: a.textContent.slice(0, 40) } })
  const sel = () => ev(() => [...BoardApp.sel])
  const TEXT = '#toolbar .btn[aria-label^="Text"]'

  const centre = async (id) => ev((oid) => { const b = BoardApp.R.box(oid); return BoardApp.toScreen(b.x + b.w / 2, b.y + b.h / 2) }, id)
  const dot = async (id, side) => { const q = await centre(id); await p.mouse.move(q[0], q[1]); await p.waitForTimeout(60); return ev(([oid, sd]) => { const d = [...document.querySelectorAll('#overlay .cdot')].find((x) => x.dataset.id === oid && x.dataset.side === sd); if (!d) return null; const r = d.getBoundingClientRect(); return [r.left + r.width / 2, r.top + r.height / 2] }, [id, side]) }
  const count = () => ev(() => BoardApp.model.all().length)

  // --- Quick add: click a connection dot ---
  let d = await dot('a', 'right')
  ok('a sticky shows a + dot', !!d && await ev(() => !!document.querySelector('#overlay .cdot.plus')))
  let n = await count(); await p.mouse.click(d[0], d[1]); await p.waitForTimeout(80)
  const qa = await ev(() => { const o = BoardApp.selected()[0]; const a = BoardApp.model.get('a'); const line = BoardApp.model.all().find((x) => x.type === 'connector' && x.to && x.to.id === o.id); return { type: o.type, color: o.color, w: o.w, dx: o.x - a.x, dy: o.y - a.y, text: o.text, editing: BoardApp.editing && BoardApp.editing.id === o.id, from: line && line.from, to: line && line.to.anchor } })
  ok('click on the right dot: an empty copy to the right, joined by a line, typing', (await count()) === n + 2 && qa.type === 'sticky' && qa.color === 'yellow' && qa.dx === 280 && qa.dy === 0 && qa.text === '' && qa.editing && qa.from.id === 'a' && qa.from.anchor === 'right' && qa.to === 'left', qa)
  await p.keyboard.type('Next step'); await p.keyboard.press('Escape'); await p.waitForTimeout(50)
  await ev(() => BoardApp.model.undo()); await ev(() => BoardApp.model.undo())
  ok('undo takes the copy and its line away together', (await count()) === n, await count())
  // a crowded side: it steps past what's there ('a' sits right of 'b')
  d = await dot('b', 'right'); n = await count(); await p.mouse.click(d[0], d[1]); await p.waitForTimeout(80); await p.keyboard.press('Escape')
  const step = await ev(() => { const o = BoardApp.selected()[0]; return o && o.x - BoardApp.model.get('b').x })
  ok('a side that’s taken: the copy goes past what’s there', step === 560, step)
  await ev(() => { BoardApp.model.undo() })
  // a drag from the dot still draws a line
  d = await dot('c', 'right'); n = await count()
  await p.mouse.move(d[0], d[1]); await p.mouse.down(); await p.mouse.move(d[0] + 60, d[1] + 10, { steps: 4 }); await p.mouse.move(d[0] + 160, d[1] + 20, { steps: 4 }); await p.mouse.up(); await p.waitForTimeout(50)
  ok('a drag from the dot still draws a line (no copy)', (await count()) === n + 1 && await ev(() => BoardApp.selected()[0].type) === 'connector')
  await ev(() => BoardApp.model.undo())
  // a locked object: no quick add
  await ev(() => BoardApp.model.apply([{ op: 'set', id: 'c', f: { locked: true } }])); n = await count()
  ok('a locked object shows no dots', !(await dot('c', 'right')))
  await ev(() => BoardApp.model.apply([{ op: 'set', id: 'c', f: { locked: false } }]))

  // --- Copy style / paste style ---
  await ev(() => BoardApp.model.apply([
    { op: 'set', id: 'a', f: { color: 'pink', fontSize: 32, align: 'left' } },
    { op: 'add', obj: { id: 'sh', type: 'shape', shape: 'round', x: 700, y: 400, w: 200, h: 120, text: 'Shape', fill: 'blue', stroke: 'ink', color: 'ink', fontSize: 18, z: 20 } }]))
  await ev(() => { BoardApp.select(['a']); document.getElementById('viewport').focus() }); await p.keyboard.press('Meta+Alt+KeyC')
  await ev(() => BoardApp.select(['b', 'c', 'sh'])); await p.keyboard.press('Meta+Alt+KeyV')
  const st = await ev(() => ['b', 'c', 'sh'].map((id) => { const o = BoardApp.model.get(id); return [o.color, o.fontSize, o.align, o.text, o.fill] }))
  ok('⌥⌘C then ⌥⌘V: stickies take the colour, size and alignment, keep their text', st[0].slice(0, 4).join() === 'pink,32,left,Top left' && st[1].slice(0, 4).join() === 'pink,32,left,Bottom', st)
  ok('…a shape takes only what it shares (size), not the paper colour', st[2][0] === 'ink' && st[2][4] === 'blue' && st[2][1] === 32, st[2])
  await ev(() => BoardApp.model.undo()); ok('one undo puts every style back', await ev(() => BoardApp.model.get('b').color) === 'yellow' && await ev(() => BoardApp.model.get('sh').fontSize) === 18)
  await ev(() => BoardApp.select([])); let q = await centre('sh'); await p.mouse.click(q[0], q[1], { button: 'right' }); await p.waitForTimeout(50)
  ok('the right-click menu offers Copy style and Paste style', await ev(() => /Copy style/.test(document.getElementById('pop').textContent) && /Paste style/.test(document.getElementById('pop').textContent)))
  await p.keyboard.press('Escape')
  await ev(() => BoardApp.model.apply([{ op: 'del', id: 'sh' }, { op: 'set', id: 'a', f: { color: 'yellow', fontSize: 0, align: 'center' } }]))

  // --- Tidy up ---
  await ev(() => BoardApp.model.apply([
    { op: 'add', obj: { id: 't1', type: 'sticky', x: 2000, y: 2010, w: 200, h: 200, text: '1', color: 'yellow', z: 30 } },
    { op: 'add', obj: { id: 't2', type: 'sticky', x: 2260, y: 1985, w: 200, h: 200, text: '2', color: 'yellow', z: 31 } },
    { op: 'add', obj: { id: 't3', type: 'sticky', x: 2490, y: 2040, w: 200, h: 200, text: '3', color: 'yellow', z: 32 } },
    { op: 'add', obj: { id: 't4', type: 'sticky', x: 2030, y: 2300, w: 200, h: 200, text: '4', color: 'yellow', z: 33 } },
    { op: 'add', obj: { id: 't5', type: 'sticky', x: 2310, y: 2270, w: 200, h: 200, text: '5', color: 'yellow', z: 34 } },
    { op: 'add', obj: { id: 'tl', type: 'connector', from: { id: 't1' }, to: { id: 't5' }, z: 35 } }]))
  await ev(() => BoardApp.select(['t1', 't2', 't3', 't4', 't5', 'tl'])); await ev(() => BoardApp.tidy())
  const td = await ev(() => ['t1', 't2', 't3', 't4', 't5'].map((id) => { const o = BoardApp.model.get(id); return [o.x, o.y] }))
  ok('tidy up: two neat rows in the same order, evenly spaced', JSON.stringify(td) === JSON.stringify([[2000, 1985], [2224, 1985], [2448, 1985], [2000, 2209], [2224, 2209]]), td)
  await ev(() => BoardApp.model.undo()); ok('one undo puts them back', (await ev(() => BoardApp.model.get('t3').x)) === 2490)
  await ev(() => BoardApp.model.apply(['t1', 't2', 't3', 't4', 't5', 'tl'].map((id) => ({ op: 'del', id })))); await ev(() => BoardApp.select([]))

  // --- Review fixes: wobble, drawn sizes, rotation, groups ---
  d = await dot('a', 'right'); n = await count()
  await p.mouse.move(d[0], d[1]); await p.mouse.down(); await p.mouse.move(d[0] + 2, d[1] + 1); await p.mouse.up(); await p.waitForTimeout(80); await p.keyboard.press('Escape')
  ok('a click that wobbles 2px still quick-adds (no stray line)', (await count()) === n + 2 && await ev(() => !BoardApp.model.all().some((o) => o.type === 'connector' && o.to && !o.to.id)), await count())
  await ev(() => { BoardApp.model.undo() })
  await ev(() => BoardApp.model.apply([{ op: 'add', obj: { id: 'tb', type: 'table', x: 680, y: -300, w: 300, h: 40, rows: Array.from({ length: 12 }, (_, i) => ['Row ' + i, 'x']), z: 40 } }])); await p.waitForTimeout(50)
  d = await dot('a', 'right'); await p.mouse.click(d[0], d[1]); await p.waitForTimeout(80); await p.keyboard.press('Escape')
  const nx = await ev(() => { const o = BoardApp.selected()[0]; const t = BoardApp.R.box('tb'); return { x: o.x, overlaps: o.x < t.x + t.w && o.x + o.w > t.x && o.y < t.y + t.h && o.y + o.h > t.y } })
  ok('quick add steps past a table by its drawn height', !nx.overlaps, nx)
  await ev(() => BoardApp.model.undo())
  await ev(() => BoardApp.model.apply([
    { op: 'add', obj: { id: 'u1', type: 'sticky', x: 3000, y: 3500, w: 200, h: 200, text: 'u1', color: 'yellow', z: 41 } },
    { op: 'add', obj: { id: 'u2', type: 'shape', shape: 'rect', x: 3200, y: 3600, w: 300, h: 60, rot: 90, text: '', fill: 'blue', stroke: 'ink', color: 'ink', z: 42 } },
    { op: 'set', id: 'tb', f: { x: 3600, y: 3480 } }])); await p.waitForTimeout(50)
  await ev(() => { BoardApp.select(['u1', 'u2', 'tb']); BoardApp.tidy() })
  const tb2 = await ev(() => ['u1', 'u2', 'tb'].map((id) => BoardApp.drawnBox(BoardApp.model.get(id))).map((b) => [Math.round(b.x), Math.round(b.y), Math.round(b.w), Math.round(b.h)]))
  const gaps = [tb2[1][0] - (tb2[0][0] + tb2[0][2]), tb2[2][0] - (tb2[1][0] + tb2[1][2])]
  const anyOverlap = tb2.some((p1, i) => tb2.some((p2, j) => i < j && p1[0] < p2[0] + p2[2] && p1[0] + p1[2] > p2[0] && p1[1] < p2[1] + p2[3] && p1[1] + p1[3] > p2[1]))
  ok('tidy spaces by drawn, rotated size: 24px gaps, no overlap (table, rotated shape)', gaps.every((g) => g === 24) && !anyOverlap, [tb2, gaps])
  await ev(() => BoardApp.model.apply(['u1', 'u2', 'tb'].map((id) => ({ op: 'del', id }))))
  await ev(() => BoardApp.model.apply([
    { op: 'add', obj: { id: 'g1', type: 'sticky', x: 3000, y: 4200, w: 200, h: 200, text: 'g1', color: 'green', fontSize: 28, group: 'gg', z: 50 } },
    { op: 'add', obj: { id: 'g2', type: 'sticky', x: 3240, y: 4200, w: 200, h: 200, text: 'g2', color: 'green', fontSize: 28, group: 'gg', z: 51 } }]))
  await ev(() => { BoardApp.select(['g1']); BoardApp.hover = null; document.getElementById('viewport').focus() })
  ok('clicking a group member selects the group', (await ev(() => BoardApp.sel.size)) === 2)
  await p.keyboard.press('Meta+Alt+KeyC'); await ev(() => BoardApp.select(['b'])); await p.keyboard.press('Meta+Alt+KeyV')
  ok('copy style works from a grouped object', await ev(() => BoardApp.model.get('b').color) === 'green')
  await ev(() => { BoardApp.model.undo(); BoardApp.model.apply(['g1', 'g2'].map((id) => ({ op: 'del', id }))) })

  // A line dragged to empty canvas never attaches to itself (its own loose end is under the pointer).
  const selfs = []; await ev(() => { BoardApp.select([]); BoardApp.fitTo(BoardApp.R.box('c'), { instant: true, max: 1, pad: 300 }) }); await p.waitForTimeout(50)
  for (const [dist, steps] of [[8, 3], [10, 2], [60, 10], [40, 5], [7, 3], [90, 12]]) {
    d = await dot('c', 'bottom'); if (!d) continue
    await p.mouse.move(d[0], d[1]); await p.mouse.down(); await p.mouse.move(d[0] + dist / 3, d[1] + dist, { steps }); await p.mouse.up(); await p.waitForTimeout(40)
    selfs.push(await ev(() => BoardApp.model.all().filter((o) => o.type === 'connector' && o.to && o.to.id === o.id).length))
    await ev(() => { const l = BoardApp.model.all().filter((o) => o.type === 'connector' && o.from && o.from.id === 'c'); if (l.length) BoardApp.model.apply(l.map((o) => ({ op: 'del', id: o.id }))); BoardApp.select([]) })
  }
  ok('lines dragged to empty canvas never attach to themselves (6 drags)', selfs.length === 6 && selfs.every((x) => x === 0), selfs)

  // Dragged out and back onto the start object: no line.
  { const backs = []
    for (const [ox, oy] of [[0, 0], [2, 2], [-30, 0], [-100, 0]]) {
      d = await dot('c', 'right'); if (!d) continue; const n0 = await count()
      await p.mouse.move(d[0], d[1]); await p.mouse.down(); await p.mouse.move(d[0] + 80, d[1], { steps: 4 }); await p.mouse.move(d[0] + ox, d[1] + oy, { steps: 4 }); await p.mouse.up(); await p.waitForTimeout(40)
      backs.push((await count()) - n0); await ev(() => BoardApp.select([]))
    }
    ok('a line dragged back onto its own object is dropped', backs.length === 4 && backs.every((x) => x === 0), backs) }

  // A target right next to the source (10px gap): a drop on its near edge attaches.
  await ev(() => { BoardApp.model.apply([{ op: 'add', obj: { id: 'nx', type: 'sticky', x: 610, y: 100, w: 200, h: 200, text: 'next door', color: 'blue', z: 60 } }]); BoardApp.select([]); BoardApp.animateTo({ s: 1, x: 100, y: 200 }, 0) }); await p.waitForTimeout(50)
  { const att = []
    for (const wx of [612, 615, 640]) {
      d = await dot('a', 'right'); const tgt = await ev((x) => BoardApp.toScreen(x, 200), wx)
      await p.mouse.move(d[0], d[1]); await p.mouse.down(); await p.mouse.move(tgt[0] + 40, tgt[1] + 60, { steps: 4 }); await p.mouse.move(tgt[0], tgt[1], { steps: 4 }); await p.mouse.up(); await p.waitForTimeout(40)
      att.push(await ev(() => { const l = BoardApp.model.all().filter((o) => o.type === 'connector' && o.from && o.from.id === 'a'); const r = l.map((o) => o.to.id || 'loose').join(); BoardApp.model.apply(l.map((o) => ({ op: 'del', id: o.id }))); return r }))
    }
    ok('a drop on the near edge of a neighbour attaches to it', att.every((x) => x === 'nx'), att) }
  await ev(() => BoardApp.model.apply([{ op: 'del', id: 'nx' }]))

  // Dragging an existing line's loose end slowly onto an object attaches (its own handle is under the pointer).
  await ev(() => { BoardApp.model.apply([
    { op: 'add', obj: { id: 'n2', type: 'sticky', x: 900, y: 100, w: 200, h: 200, text: 'target', color: 'green', z: 70 } },
    { op: 'add', obj: { id: 'ex', type: 'connector', from: { id: 'a' }, to: { x: 760, y: 200 }, z: 71 } }]); BoardApp.select(['ex'], { raw: true }); BoardApp.animateTo({ s: 1, x: 100, y: 200 }, 0) }); await p.waitForTimeout(80)
  { const h = await ev(() => { const e = document.querySelector('#overlay .handle[data-end="to"]'); const r = e.getBoundingClientRect(); return [r.left + r.width / 2, r.top + r.height / 2] })
    await p.mouse.move(h[0], h[1]); await p.mouse.down(); for (let k = 1; k <= 100; k++) await p.mouse.move(h[0] + k * 2, h[1]); await p.mouse.up(); await p.waitForTimeout(50)
    ok('a line end dragged slowly onto an object attaches', await ev(() => (BoardApp.model.get('ex').to || {}).id) === 'n2', await ev(() => BoardApp.model.get('ex').to)) }
  await ev(() => BoardApp.model.apply([{ op: 'del', id: 'ex' }, { op: 'del', id: 'n2' }]))

  // A new line dropped on another object's (visible) connection dot attaches to it.
  await ev(() => { BoardApp.model.apply([{ op: 'add', obj: { id: 'n3', type: 'sticky', x: 900, y: 100, w: 200, h: 200, text: 'dot target', color: 'green', z: 72 } }]); BoardApp.select([]); BoardApp.animateTo({ s: 1, x: 100, y: 200 }, 0) }); await p.waitForTimeout(60)
  { d = await dot('a', 'right'); const body = await ev(() => BoardApp.toScreen(1000, 200))
    await p.mouse.move(d[0], d[1]); await p.mouse.down(); await p.mouse.move(body[0], body[1], { steps: 8 }); await p.waitForTimeout(60)
    const nd = await ev(() => { const x = [...document.querySelectorAll('#overlay .cdot')].find((e) => e.dataset.id === 'n3' && e.dataset.side === 'left'); if (!x) return null; const r = x.getBoundingClientRect(); return [r.left + r.width / 2, r.top + r.height / 2] })
    if (nd) await p.mouse.move(nd[0], nd[1], { steps: 6 }); await p.mouse.up(); await p.waitForTimeout(50)
    ok('a line dropped on another object’s dot attaches to it', !!nd && await ev(() => { const l = BoardApp.model.all().find((o) => o.type === 'connector' && o.from && o.from.id === 'a'); return l && l.to.id }) === 'n3', nd)
    await ev(() => { const l = BoardApp.model.all().filter((o) => o.type === 'connector' && o.from && o.from.id === 'a'); BoardApp.model.apply(l.map((o) => ({ op: 'del', id: o.id })).concat([{ op: 'del', id: 'n3' }])) }) }

  // --- Link, checklist, crop (2026-10-23) ---
  await ev(() => { BoardApp.select(['a']); BoardApp.animateTo({ s: 1, x: 100, y: 200 }, 0) }); await p.waitForTimeout(80)
  await p.click('#ctx .btn[aria-label="Add a link"]'); await p.keyboard.type('javascript:alert(1)'); await p.keyboard.press('Enter')
  ok('link: a non-web address is refused, with a reason', !(await ev(() => BoardApp.model.get('a').link)) && /web address/.test(await ev(() => document.querySelector('#pop .err').textContent)))
  await ev(() => { document.querySelector('#pop input').value = '' }); await p.click('#pop input'); await p.keyboard.type('example.com/doc'); await p.keyboard.press('Enter')
  ok('link: "example.com/doc" is saved as https://', await ev(() => BoardApp.model.get('a').link) === 'https://example.com/doc')
  ok('…and a link badge shows on the sticky', await ev(() => { const b = BoardApp.R.el('a').querySelector('.linkb'); return !!b && b.title === 'https://example.com/doc' }))
  { const lb = await ev(() => { const r = BoardApp.R.el('a').querySelector('.linkb').getBoundingClientRect(); return [r.left + r.width / 2, r.top + r.height / 2] })
    await p.mouse.click(lb[0], lb[1]); await p.waitForTimeout(60)
    ok('clicking the badge shows the address (as text), with Open first and Copy link', await ev(() => !document.getElementById('pop').hidden && document.querySelector('#pop .linkurl').textContent === 'https://example.com/doc' && !document.querySelector('#pop .linkurl a') && [...document.querySelectorAll('#pop .prow .btn')].map((b) => b.textContent).slice(0, 2).join() === 'Open,Copy link'))
    await ev(() => { window.__declineOpen = true; [...document.querySelectorAll('#pop .btn')].find((b) => b.textContent === 'Open').click() }); await p.waitForTimeout(80)
    ok('Open when the person says no on K2’s card: says so, the panel stays', await ev(() => JSON.stringify(window.__opened) === '["https://example.com/doc"]' && !document.getElementById('pop').hidden && /Not opened/.test(document.getElementById('toast').textContent)))
    await ev(() => { window.__declineOpen = false; [...document.querySelectorAll('#pop .btn')].find((b) => b.textContent === 'Open').click() }); await p.waitForTimeout(80)
    ok('Open: K2 opens it in the browser and the panel closes', await ev(() => window.__opened.length === 2 && document.getElementById('pop').hidden))
    await ev(() => { delete window.k2.open }); await p.mouse.click(lb[0], lb[1]); await p.waitForTimeout(60)
    await ev(() => [...document.querySelectorAll('#pop .btn')].find((b) => b.textContent === 'Open').click()); await p.waitForTimeout(80)
    ok('Open on an older K2 (no open call): says to copy it instead', await ev(() => /copy it into your browser/.test(document.getElementById('toast').textContent)))
    await p.keyboard.press('Escape') }
  await ev(() => BoardApp.select(['a'])); await p.click('#ctx .btn[aria-label^="Link: "]'); await p.waitForTimeout(40)
  await ev(() => [...document.querySelectorAll('#pop .btn')].find((b) => /Remove/.test(b.textContent)).click())
  ok('link: Remove takes it away (badge gone)', !(await ev(() => BoardApp.model.get('a').link)) && !(await ev(() => BoardApp.R.el('a').querySelector('.linkb'))))

  await ev(() => { BoardApp.model.apply([{ op: 'add', obj: { id: 'kc', type: 'card', x: 700, y: 400, w: 300, h: 220, title: 'Launch', z: 80 } }]); BoardApp.select(['kc']) }); await p.waitForTimeout(60)
  await p.click('#ctx .btn[aria-label="Checklist"]'); await p.waitForTimeout(40); await p.keyboard.press('Enter')
  await p.waitForTimeout(40); await p.keyboard.type('Write the brief'); await p.keyboard.press('Enter'); await p.keyboard.type('Book the room'); await p.keyboard.press('Tab')
  await p.waitForTimeout(40); await p.keyboard.press('Escape'); await p.waitForTimeout(40)
  await ev(() => BoardApp.select(['kc'])); await p.click('#ctx .btn[aria-label="Checklist"]'); await p.waitForTimeout(40)
  await ev(() => [...document.querySelectorAll('#pop .btn')].find((b) => /Add item/.test(b.textContent)).click()); await p.keyboard.type('Typed then Escape'); await p.keyboard.press('Escape'); await p.waitForTimeout(40)
  ok('checklist: an item typed and closed with Escape is kept', await ev(() => BoardApp.model.get('kc').checks.length === 3 && BoardApp.model.get('kc').checks[2].text === 'Typed then Escape'), await ev(() => BoardApp.model.get('kc').checks))
  await ev(() => BoardApp.model.apply([{ op: 'set', id: 'kc', f: { checks: BoardApp.model.get('kc').checks.slice(0, 2) } }]))
  const ck = await ev(() => BoardApp.model.get('kc').checks)
  ok('checklist: items typed in the panel, Enter adds the next', JSON.stringify(ck.map((c) => ({ text: c.text, done: c.done }))) === JSON.stringify([{ text: 'Write the brief', done: false }, { text: 'Book the room', done: false }]) && ck.every((c) => /^ck-[0-9a-f]{8}$/.test(c.id)) && ck[0].id !== ck[1].id, ck)
  ok('…shown on the card with a 0/2 count', await ev(() => BoardApp.R.el('kc').querySelectorAll('.chk').length === 2 && BoardApp.R.el('kc').querySelector('.ckn').textContent === '0/2'))
  { const bx = await ev(() => { const r = BoardApp.R.el('kc').querySelector('.chk .box').getBoundingClientRect(); return [r.left + r.width / 2, r.top + r.height / 2] })
    await ev(() => BoardApp.select([])); await p.mouse.click(bx[0], bx[1]); await p.waitForTimeout(40)
    ok('ticking an item on the board marks it done, without selecting the card', await ev(() => BoardApp.model.get('kc').checks[0].done === true && BoardApp.R.el('kc').querySelector('.ckn').textContent === '1/2' && BoardApp.sel.size === 0)) }
  await ev(() => BoardApp.model.undo()); ok('…and undo un-ticks it', await ev(() => BoardApp.model.get('kc').checks[0].done === false))

  const png = await ev(() => { const c = document.createElement('canvas'); c.width = 400; c.height = 200; const x = c.getContext('2d'); x.fillStyle = '#e33'; x.fillRect(0, 0, 200, 200); x.fillStyle = '#33e'; x.fillRect(200, 0, 200, 200); return c.toDataURL('image/png') })
  await ev((src) => { BoardApp.model.apply([{ op: 'add', obj: { id: 'im', type: 'image', x: 200, y: 700, w: 400, h: 200, src, z: 81 } }]); BoardApp.select(['im']); BoardApp.fitTo({ x: 150, y: 650, w: 500, h: 300 }, { instant: true, max: 1 }) }, png); await p.waitForTimeout(80)
  await p.click('#ctx .btn[aria-label="Crop"]'); await p.waitForTimeout(80)
  { const h = await ev(() => { const r = document.querySelector('#pop .ch.se').getBoundingClientRect(); const b = document.querySelector('#pop .cropbox').getBoundingClientRect(); return [r.left + r.width / 2, r.top + r.height / 2, b.width] })
    await p.mouse.move(h[0], h[1]); await p.mouse.down(); await p.mouse.move(h[0] - h[2] / 2, h[1], { steps: 6 }); await p.mouse.up()
    await ev(() => [...document.querySelectorAll('#pop .btn')].find((b) => /Done/.test(b.textContent)).click()); await p.waitForTimeout(60) }
  const cr = await ev(() => { const o = BoardApp.model.get('im'); return { crop: o.crop, w: o.w, h: o.h, x: o.x } })
  ok('crop: the right half dragged off keeps the left half, at the same scale', cr.crop && Math.abs(cr.crop.w - 0.5) < 0.02 && cr.crop.x === 0 && cr.crop.h === 1 && Math.abs(cr.w - 200) <= 8 && cr.h === 200 && cr.x === 200, cr)
  ok('…and the board shows only that part, unstretched (left half of 400×200 in a 200×200 box)', await ev(() => { const i = BoardApp.R.el('im').querySelector('img'); return BoardApp.R.el('im').classList.contains('cropped') && Math.abs(parseFloat(i.style.width) - 400) < 8 && Math.abs(parseFloat(i.style.height) - 200) < 4 && Math.abs(parseFloat(i.style.left)) < 4 }), await ev(() => BoardApp.R.el('im').querySelector('img').style.cssText))
  await ev(() => BoardApp.model.undo()); ok('…undo restores the whole picture', await ev(() => !BoardApp.model.get('im').crop && BoardApp.model.get('im').w === 400))
  await ev(() => BoardApp.model.apply(['kc', 'im'].map((id) => ({ op: 'del', id })))); await ev(() => BoardApp.select([]))

  // --- Review fixes for the new fields ---
  await ev(() => { BoardApp.model.apply([{ op: 'add', obj: { id: 'k5', type: 'card', x: 3000, y: 3000, w: 300, h: 200, title: 'Live', checks: [{ text: 'A', done: false }, { text: 'B', done: false }], z: 90 } }]); BoardApp.select(['k5']); BoardApp.fitTo({ x: 2900, y: 2900, w: 600, h: 500 }, { instant: true, max: 1 }) }); await p.waitForTimeout(60)
  await p.click('#ctx .btn[aria-label="Checklist"]'); await p.waitForTimeout(40)
  await ev(() => BoardApp.model.applyRemote({ ops: [{ op: 'set', id: 'k5', f: { checks: [{ text: 'NEW', done: false }, { text: 'A', done: false }, { text: 'B', done: false }] } }], by: 'agent:helper' }))
  await p.keyboard.press('Escape'); await p.waitForTimeout(40)
  ok('checklist panel open while an agent adds an item: nothing lost or doubled', JSON.stringify(await ev(() => BoardApp.model.get('k5').checks.map((c) => c.text))) === '["NEW","A","B"]', await ev(() => BoardApp.model.get('k5').checks))
  await ev(() => { BoardApp.model.apply([{ op: 'set', id: 'k5', f: { checks: Array.from({ length: 50 }, (_, i) => ({ text: 'i' + i, done: false })) } }]); BoardApp.select(['k5']) })
  await p.click('#ctx .btn[aria-label="Checklist"]'); await p.waitForTimeout(40); await ev(() => document.querySelectorAll('#pop .ckrow .field')[3].focus()); await p.keyboard.press('Enter'); await p.keyboard.press('Escape')
  ok('Enter on a full checklist (50) adds nothing and loses nothing', await ev(() => { const c = BoardApp.model.get('k5').checks; return c.length === 50 && c[49].text === 'i49' }))
  await ev(() => BoardApp.model.apply([{ op: 'add', obj: { id: 'fz', type: 'frame', x: 3400, y: 2900, w: 300, h: 200, title: 'F', link: 'https://example.com/f', z: 0 } }])); await p.waitForTimeout(40)
  { const lb = await ev(() => { const r = BoardApp.R.el('fz').querySelector('.linkb').getBoundingClientRect(); return [r.left + r.width / 2, r.top + r.height / 2] }); await ev(() => BoardApp.select([])); await p.mouse.click(lb[0], lb[1]); await p.waitForTimeout(40)
    ok('a frame’s link badge opens its link', await ev(() => !document.getElementById('pop').hidden && /example.com\/f/.test(document.getElementById('pop').textContent))); await p.keyboard.press('Escape') }
  // Crop: a 400×200 picture in a 300×300 box (cover), left half → the box becomes square and shows natural x 0–200.
  const pic = await ev(() => { const c = document.createElement('canvas'); c.width = 400; c.height = 200; const x = c.getContext('2d'); x.fillStyle = '#e33'; x.fillRect(0, 0, 400, 200); return c.toDataURL() })
  await ev((src) => { BoardApp.model.apply([{ op: 'add', obj: { id: 'i9', type: 'image', x: 3000, y: 3300, w: 300, h: 300, src, z: 91 } }]); BoardApp.select(['i9']); BoardApp.fitTo({ x: 2900, y: 3200, w: 600, h: 500 }, { instant: true, max: 1 }) }, pic); await p.waitForTimeout(80)
  await p.click('#ctx .btn[aria-label="Crop"]'); await p.waitForTimeout(100)
  { const h = await ev(() => { const r = document.querySelector('#pop .ch.se').getBoundingClientRect(); const b = document.querySelector('#pop .cropbox').getBoundingClientRect(); return [r.left + r.width / 2, r.top + r.height / 2, b.width, b.height] })
    ok('the crop panel fits the picture (no huge panel)', h[2] <= 302 && h[3] <= 362, h)
    ok('…and starts on the part showing (the middle half of a wide picture in a square box)', await ev(() => { const r = document.querySelector('#pop .crect'); return Math.abs(parseFloat(r.style.left) - 25) < 1 && Math.abs(parseFloat(r.style.width) - 50) < 1 }))
    const rc = await ev(() => { const r = document.querySelector('#pop .crect').getBoundingClientRect(); return [r.left + r.width / 2, r.top + r.height / 2] })
    await p.mouse.move(rc[0], rc[1]); await p.mouse.down(); await p.mouse.move(rc[0] - h[2], rc[1], { steps: 6 }); await p.mouse.up()   // slide it to the left edge
    await ev(() => [...document.querySelectorAll('#pop .btn')].find((b) => /Done/.test(b.textContent)).click()); await p.waitForTimeout(80) }
  const c9 = await ev(() => { const o = BoardApp.model.get('i9'); const i = BoardApp.R.el('i9').querySelector('img'); return { w: o.w, h: o.h, crop: o.crop, left: parseFloat(i.style.left), iw: parseFloat(i.style.width) } })
  ok('crop shows what was chosen: a square box, picture starting at its left edge', Math.abs(c9.w - c9.h) <= 3 && Math.abs(c9.left) < 3 && Math.abs(c9.iw - 2 * c9.w) < 6, c9)
  // Done with no change, on a box whose shape differs from the picture's: nothing moves.
  await ev((src) => { BoardApp.model.apply([{ op: 'add', obj: { id: 'i8', type: 'image', x: 3000, y: 3300, w: 300, h: 300, src, z: 93 } }]); BoardApp.select(['i8']) }, pic); await p.waitForTimeout(80)
  await p.click('#ctx .btn[aria-label="Crop"]'); await p.waitForTimeout(100); await ev(() => [...document.querySelectorAll('#pop .btn')].find((b) => /Done/.test(b.textContent)).click()); await p.waitForTimeout(60)
  ok('crop: Done with no change moves nothing', await ev(() => { const o = BoardApp.model.get('i8'); return o.x === 3000 && o.y === 3300 && o.w === 300 && o.h === 300 && !o.crop }), await ev(() => BoardApp.model.get('i8')))
  await ev(() => BoardApp.model.apply([{ op: 'del', id: 'i8' }]))
  // A rotated image: the kept part stays where it was.
  await ev((src) => { BoardApp.model.apply([{ op: 'add', obj: { id: 'ir', type: 'image', x: 3000, y: 3700, w: 400, h: 200, rot: 90, src, z: 92 } }]); BoardApp.select(['ir']); BoardApp.fitTo({ x: 2900, y: 3500, w: 600, h: 600 }, { instant: true, max: 1 }) }, pic); await p.waitForTimeout(80)
  await p.click('#ctx .btn[aria-label="Crop"]'); await p.waitForTimeout(100)
  { const h = await ev(() => { const r = document.querySelector('#pop .ch.se').getBoundingClientRect(); const b = document.querySelector('#pop .cropbox').getBoundingClientRect(); return [r.left + r.width / 2, r.top + r.height / 2, b.width] })
    await p.mouse.move(h[0], h[1]); await p.mouse.down(); await p.mouse.move(h[0] - h[2] / 2, h[1], { steps: 6 }); await p.mouse.up()
    await ev(() => [...document.querySelectorAll('#pop .btn')].find((b) => /Done/.test(b.textContent)).click()); await p.waitForTimeout(80) }
  const rr = await ev(() => { const o = BoardApp.model.get('ir'); return [o.x + o.w / 2, o.y + o.h / 2, o.w, o.h] })
  ok('cropping a rotated (90°) image keeps the kept half in place', Math.abs(rr[0] - 3200) <= 3 && Math.abs(rr[1] - 3700) <= 3 && Math.abs(rr[2] - 200) <= 3, rr)
  await ev(() => BoardApp.model.apply(['k5', 'fz', 'i9', 'ir'].map((id) => ({ op: 'del', id })))); await ev(() => BoardApp.select([]))

  // --- Stamps: reactions drawn big on the corner ---
  await ev(() => { BoardApp.select(['b']); BoardApp.animateTo({ s: 1, x: 100, y: 200 }, 0) }); await p.waitForTimeout(60)
  await p.click('#ctx .btn[aria-label="React"]'); await p.waitForTimeout(40)
  ok('the React panel offers the four stamps', await ev(() => [...document.querySelectorAll('#pop .emo.big')].map((b) => b.textContent).join('') === '✅❌❓⭐'))
  await ev(() => [...document.querySelectorAll('#pop .emo.big')].find((b) => b.textContent === '⭐').click()); await p.waitForTimeout(40)
  ok('a stamp shows big on the corner, not as a chip', await ev(() => { const el = BoardApp.R.el('b'); const st = el.querySelector('.stamps .stamp'); return !!st && st.dataset.emoji === '⭐' && st.getBoundingClientRect().width >= 36 && !el.querySelector('.reacts .react[data-emoji="⭐"]') }))
  await ev(() => BoardApp.select([])); { const q = await ev(() => { const r = BoardApp.R.el('b').querySelector('.stamp').getBoundingClientRect(); return [r.left + r.width / 2, r.top + r.height / 2] }); await p.mouse.click(q[0], q[1]); await p.waitForTimeout(40) }
  ok('clicking your stamp takes it off', await ev(() => !BoardApp.R.el('b').querySelector('.stamps')))
  await ev(() => BoardApp.select(['b'])); await p.waitForTimeout(40)
  { const q = await ev(() => { BoardApp.toggleReaction('b', '✅'); const r = BoardApp.R.el('b').querySelector('.stamp').getBoundingClientRect(); return [r.left + r.width / 2, r.top + r.height / 2] }); await p.mouse.click(q[0], q[1]); await p.waitForTimeout(40)
    ok('…even on a selected object (not under a handle)', await ev(() => !BoardApp.R.el('b').querySelector('.stamps') && BoardApp.model.get('b').w === 200)) }
  await ev(() => BoardApp.select([]))

  // Stamps sit in the same place on screen at any zoom; on frames, clear of the name; never clipped by a crop.
  await ev(() => BoardApp.toggleReaction('c', '⭐'))
  const stampAt = (z) => ev((zz) => { BoardApp.animateTo({ s: zz, x: 300, y: 300 }, 0); const o = BoardApp.R.el('c').getBoundingClientRect(); const st = BoardApp.R.el('c').querySelector('.stamp').getBoundingClientRect(); return [Math.round(st.left - o.left), Math.round(st.top - o.top), Math.round(st.width)] }, z)
  const z4 = await stampAt(4); const zq = await stampAt(0.25)
  ok('a stamp keeps its screen offset and size at zoom 4 and 0.25', JSON.stringify(z4) === JSON.stringify(zq) && z4[1] < -15 && z4[1] > -30 && z4[2] >= 36, [z4, zq])
  await ev(() => { BoardApp.toggleReaction('c', '⭐'); BoardApp.animateTo({ s: 1, x: 100, y: 200 }, 0) })
  await ev(() => { BoardApp.model.apply([{ op: 'add', obj: { id: 'fs', type: 'frame', x: 1200, y: 100, w: 400, h: 300, title: 'A frame with a long name', reactions: { '✅': ['agent:helper'] }, z: 0 } }]) }); await p.waitForTimeout(40)
  ok('on a frame the stamp stays clear of the name', await ev(() => { const a = BoardApp.R.el('fs').querySelector('.stamp').getBoundingClientRect(); const t = BoardApp.R.el('fs').querySelector('.ftitle').getBoundingClientRect(); return a.right < t.left || a.left > t.right || a.bottom < t.top || a.top > t.bottom }))
  await ev(() => BoardApp.model.apply([{ op: 'del', id: 'fs' }]))
  const pic2 = await ev(() => { const c = document.createElement('canvas'); c.width = 400; c.height = 200; c.getContext('2d').fillRect(0, 0, 400, 200); return c.toDataURL() })
  await ev((src) => { BoardApp.model.apply([{ op: 'add', obj: { id: 'ic', type: 'image', x: 700, y: 300, w: 200, h: 200, src, crop: { x: 0, y: 0, w: 0.5, h: 1 }, reactions: { '❓': ['agent:helper'], '👍': ['agent:helper'] }, z: 95 } }]) }, pic2); await p.waitForTimeout(80)
  ok('a stamp on a cropped image isn’t cut off (and its chips show)', await ev(() => { const el = BoardApp.R.el('ic'); const st = el.querySelector('.stamp').getBoundingClientRect(); const hit = document.elementFromPoint(st.left + st.width / 2, st.top + 4); const ch = el.querySelector('.reacts .react').getBoundingClientRect(); const hit2 = document.elementFromPoint(ch.left + ch.width / 2, ch.top + ch.height / 2); return !!hit && !!hit.closest('.stamp') && !!hit2 && !!hit2.closest('.react') }))
  ok('…and the picture is still clipped to its box', await ev(() => /inset/.test(BoardApp.R.el('ic').querySelector('img').style.clipPath)))
  await ev(() => BoardApp.model.apply([{ op: 'del', id: 'ic' }]))

  ok('no page errors', (await p.evaluate(() => (window as any).__errors)).length === 0, await p.evaluate(() => (window as any).__errors))
  expect(failures(), checks() + ' checks').toEqual([])
})
