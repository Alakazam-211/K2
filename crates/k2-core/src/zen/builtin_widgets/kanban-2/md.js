// Markdown for card notes and Thread messages, with K2's bundled marked +
// DOMPurify (manifest requires.libs). Safe by construction:
//   1. raw HTML in the source is escaped (renderer.html), never passed on;
//   2. links and images become text with the URL visible: the bridge has no
//      open-link verb and the sealed frame can't open pages;
//   3. DOMPurify then keeps only a short list of formatting tags, no
//      attributes but class/start/align, and returns a DOM fragment (no
//      string ever goes into the document as markup).
// Without the libraries, notes fall back to plain text.
(function () {
  'use strict'
  var ALLOWED_TAGS = [
    'p', 'br', 'strong', 'em', 'del', 's', 'code', 'pre', 'blockquote', 'ul', 'ol', 'li',
    'h1', 'h2', 'h3', 'h4', 'h5', 'h6', 'hr', 'table', 'thead', 'tbody', 'tr', 'th', 'td', 'span'
  ]
  var ALLOWED_ATTR = ['class', 'start', 'align']

  function esc(s) {
    return String(s == null ? '' : s)
      .replace(/&/g, '&amp;')
      .replace(/</g, '&lt;')
      .replace(/>/g, '&gt;')
      .replace(/"/g, '&quot;')
      .replace(/'/g, '&#39;')
  }

  function urlSpan(href) {
    return '<span class="md-url">' + esc(href) + '</span>'
  }

  var md = null
  var M = window.marked
  if (M && typeof M.Marked === 'function' && window.DOMPurify && typeof window.DOMPurify.sanitize === 'function') {
    md = new M.Marked({
      gfm: true,
      breaks: true,
      renderer: {
        html: function (t) { return esc(t.text) },
        link: function (t) {
          var label = this.parser.parseInline(t.tokens || [])
          var plain = String(t.text || '')
          if (t.autolink || plain === t.href || plain === '') return urlSpan(t.href)
          return label + ' (' + urlSpan(t.href) + ')'
        },
        image: function (t) {
          return esc('[image' + (t.text ? ': ' + t.text : '') + ']') + ' (' + urlSpan(t.href) + ')'
        },
        checkbox: function (t) { return t.checked ? '[x] ' : '[ ] ' }
      }
    })
  }

  // A DocumentFragment for `text`, or null when markdown isn't available.
  function render(text) {
    if (!md) return null
    var html = md.parse(String(text || ''), { async: false })
    return window.DOMPurify.sanitize(html, {
      ALLOWED_TAGS: ALLOWED_TAGS,
      ALLOWED_ATTR: ALLOWED_ATTR,
      ALLOW_DATA_ATTR: false,
      RETURN_DOM_FRAGMENT: true
    })
  }

  // Fill `el` with `text` as markdown (or plain text without the libraries).
  function fill(el, text) {
    var frag = null
    try {
      frag = render(text)
    } catch (e) {
      frag = null
    }
    if (frag) {
      el.classList.add('md')
      el.replaceChildren(frag)
    } else {
      el.classList.remove('md')
      el.textContent = String(text || '')
    }
  }

  window.KanbanMarkdown = { fill: fill, render: render, available: function () { return !!md } }
})()
