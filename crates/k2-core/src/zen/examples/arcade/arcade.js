// Agent Arcade: one character per agent the widget can see.
//
// - working → the character hops; needs you → it waves; idle → it naps;
// - a speech bubble shows the agent's last message (with thread:read);
// - click a character: K2's Conversation beside this widget opens it
//   (conversation.open), and a talk box sends from the game (thread.post).
//
// Everything goes through the `k2` object (k2 zen guide api). Agent text is
// always put in with textContent, never innerHTML.
(function () {
  const SPRITES = ['🧙', '🤖', '🐉', '🦊', '🐙', '🦉', '🐢', '🦄', '🐝', '🐧', '🦖', '🐳']
  const stage = document.getElementById('stage')
  const empty = document.getElementById('empty')
  const score = document.getElementById('score')
  const talk = document.getElementById('talk')
  const talkTo = document.getElementById('talk-to')
  const talkText = document.getElementById('talk-text')
  const talkClose = document.getElementById('talk-close')

  const heroes = new Map() // address → {el, sprite, name, state, bubble}
  let picked = null

  function spriteFor(row, i) {
    if (row.avatar) return null
    let h = 0
    for (const ch of row.address) h = (h * 31 + ch.charCodeAt(0)) >>> 0
    return SPRITES[(h + i) % SPRITES.length]
  }

  function makeHero(row, i) {
    const el = document.createElement('button')
    el.type = 'button'
    el.className = 'hero'
    const bubble = document.createElement('span')
    bubble.className = 'bubble'
    const sprite = document.createElement('span')
    sprite.className = 'sprite'
    const glyph = spriteFor(row, i)
    if (glyph) {
      sprite.textContent = glyph
    } else {
      const img = document.createElement('img')
      img.src = row.avatar
      img.alt = ''
      img.width = 34
      img.height = 34
      img.style.borderRadius = '50%'
      sprite.appendChild(img)
    }
    const name = document.createElement('span')
    name.className = 'name'
    const state = document.createElement('span')
    state.className = 'state'
    el.append(bubble, sprite, name, state)
    el.addEventListener('click', () => pick(row.address))
    stage.appendChild(el)
    const hero = { el, sprite, name, state, bubble }
    heroes.set(row.address, hero)
    return hero
  }

  function draw(rows) {
    rows = Array.isArray(rows) ? rows : []
    const seen = new Set()
    rows.forEach((row, i) => {
      seen.add(row.address)
      const hero = heroes.get(row.address) || makeHero(row, i)
      hero.name.textContent = row.label
      hero.state.textContent = row.stateLabel || row.state || ''
      hero.el.classList.toggle('working', !!row.working)
      hero.el.classList.toggle('needs', !!row.needsYou)
      hero.el.classList.toggle('asleep', !row.working && !row.needsYou)
      hero.el.classList.toggle('picked', row.address === picked)
      hero.el.title = row.label + (row.detail ? ' · ' + row.detail : '')
      const said = row.preview && row.preview.text ? row.preview.text : ''
      hero.bubble.textContent = said.length > 60 ? said.slice(0, 59) + '…' : said
    })
    for (const [address, hero] of heroes) {
      if (!seen.has(address)) {
        hero.el.remove()
        heroes.delete(address)
      }
    }
    empty.hidden = rows.length > 0
    const busy = rows.filter((r) => r.working).length
    score.textContent = busy + ' working'
    if (picked && !seen.has(picked)) closeTalk()
  }

  function pick(address) {
    picked = address
    for (const [a, hero] of heroes) hero.el.classList.toggle('picked', a === address)
    const hero = heroes.get(address)
    talkTo.textContent = hero ? 'To ' + hero.name.textContent + ':' : ''
    if (k2.can('conversation.open')) {
      k2.conversation.open(address).catch(report)
    }
    talk.hidden = !k2.can('thread.post')
    if (!talk.hidden) talkText.focus()
  }

  function closeTalk() {
    picked = null
    talk.hidden = true
    talkText.value = ''
    for (const hero of heroes.values()) hero.el.classList.remove('picked')
    if (k2.can('conversation.close')) k2.conversation.close().catch(report)
  }

  function report(err) {
    // A refused call is a K2Error with a code (cap_not_granted,
    // sending_off, not_bound, rate_limited, …). Show it in the HUD.
    score.textContent = err && err.code ? 'K2 said: ' + err.code : 'Something went wrong'
  }

  talk.addEventListener('submit', (ev) => {
    ev.preventDefault()
    const text = talkText.value.trim()
    if (!picked || !text) return
    k2.thread
      .post(picked, text)
      .then(() => {
        talkText.value = ''
        const hero = heroes.get(picked)
        if (hero) hero.bubble.textContent = 'you: ' + (text.length > 50 ? text.slice(0, 49) + '…' : text)
      })
      .catch(report)
  })
  talkClose.addEventListener('click', closeTalk)
  document.addEventListener('keydown', (ev) => {
    if (ev.key === 'Escape' && picked) closeTalk()
  })

  // K2 connects the frame after this script runs: wait for it before
  // k2.can (false until then), k2.config or k2.motion. Calls wait on their own.
  k2.connected.then(
    () => {
      if (k2.can('agents.subscribe')) {
        k2.agents.subscribe(draw, (err) => {
          empty.hidden = false
          empty.textContent = 'The game can’t reach your agents right now.'
          report(err)
        })
      } else {
        empty.hidden = false
        empty.textContent = 'The game can’t reach your agents right now.'
      }
      k2.ready()
    },
    () => {
      empty.hidden = false
      empty.textContent = 'The game can’t reach your agents right now.'
    },
  )
})()
