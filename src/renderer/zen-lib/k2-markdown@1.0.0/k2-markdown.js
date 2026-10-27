/*! k2-markdown 1.0.0 (FSL-1.1-Apache-2.0), K2's widget kit: markdown, safe by default. Copyright 2025-2026 Alakazam Labs. */
// K2Markdown.render(text, opts) -> DocumentFragment; K2Markdown.into(el, text, opts).
// Needs marked@18 and dompurify@3 (K2 loads them first). Safe by default:
//  - raw HTML in the text is escaped, never passed on;
//  - links show as text with the URL visible; with opts.links = "open" (and
//    the widget's links:open cap) each URL is a button that calls k2.open;
//    no href ever reaches the page;
//  - images show as text; opts.images = "data" allows data: PNG, JPEG, GIF
//    and WebP images only (never SVG, never a URL);
//  - a private DOMPurify instance, made when this file loads, keeps a short
//    tag list and no attributes but class, start and align, so a widget's
//    own DOMPurify.setConfig or addHook can't loosen these rules;
//  - it returns a DOM fragment: no string ever goes in as markup.
(function () {
  'use strict'
  var W = window
  var TAGS = ['p', 'br', 'strong', 'em', 'del', 's', 'code', 'pre', 'blockquote', 'ul', 'ol', 'li',
    'h1', 'h2', 'h3', 'h4', 'h5', 'h6', 'hr', 'table', 'thead', 'tbody', 'tr', 'th', 'td', 'span']
  var ATTR = ['class', 'start', 'align']
  var FORBID = ['style', 'svg', 'math', 'form', 'input', 'iframe', 'img', 'a', 'button', 'script', 'object', 'embed']
  var DATA_IMG = /^data:image\/(png|jpeg|gif|webp);base64,[A-Za-z0-9+/=\s]+$/

  function esc(s) {
    return String(s == null ? '' : s).replace(/&/g, '&amp;').replace(/</g, '&lt;')
      .replace(/>/g, '&gt;').replace(/"/g, '&quot;').replace(/'/g, '&#39;')
  }
  function urlSpan(href) {
    return '<span class="md-url">' + esc(href) + '</span>'
  }

  var purify = null
  var md = null
  var imagesOn = false
  try {
    if (typeof W.DOMPurify === 'function') purify = W.DOMPurify(W)
    if (purify && typeof purify.addHook === 'function') {
      // Only a data: image's src survives, and only while images are on.
      purify.addHook('uponSanitizeAttribute', function (node, data) {
        if (data.attrName === 'src' || data.attrName === 'alt') {
          var ok = imagesOn && node.nodeName === 'IMG' && (data.attrName === 'alt' || DATA_IMG.test(String(data.attrValue)))
          if (!ok) data.keepAttr = false
        }
      })
    }
    var M = W.marked
    if (M && typeof M.Marked === 'function') {
      md = new M.Marked({
        gfm: true,
        breaks: true,
        async: false,
        renderer: {
          html: function (t) { return esc(t.text) },
          link: function (t) {
            var label = this.parser.parseInline(t.tokens || [])
            var plain = String(t.text || '')
            if (t.autolink || plain === t.href || plain === '') return urlSpan(t.href)
            return label + ' (' + urlSpan(t.href) + ')'
          },
          image: function (t) {
            if (imagesOn && DATA_IMG.test(String(t.href || ''))) {
              return '<img class="md-img" src="' + esc(t.href) + '" alt="' + esc(t.text || '') + '">'
            }
            return esc('[image' + (t.text ? ': ' + t.text : '') + ']')
          },
          checkbox: function (t) { return t.checked ? '[x] ' : '[ ] ' }
        }
      })
    }
  } catch (e) {
    md = null
  }

  var styled = false
  function style() {
    if (styled || !W.document) return
    styled = true
    var s = W.document.createElement('style')
    s.setAttribute('data-k2-lib', 'k2-markdown')
    s.textContent = [
      '.k2md{color:var(--zen-text);line-height:1.45;overflow-wrap:anywhere}',
      '.k2md>:first-child{margin-top:0}.k2md>:last-child{margin-bottom:0}',
      '.k2md code{font-family:var(--zen-font-mono,ui-monospace,monospace);font-size:.9em;padding:0 .25em;border-radius:4px;background:var(--zen-surface-raised,rgba(127,127,127,.15))}',
      '.k2md pre{padding:.6em .8em;overflow:auto;border-radius:var(--zen-radius,6px);background:var(--zen-surface-raised,rgba(127,127,127,.15))}',
      '.k2md pre code{padding:0;background:none}',
      '.k2md blockquote{margin:.5em 0;padding-left:.8em;border-left:3px solid var(--zen-border,rgba(127,127,127,.4));color:var(--zen-text-muted)}',
      '.k2md table{border-collapse:collapse}.k2md th,.k2md td{border:1px solid var(--zen-border,rgba(127,127,127,.4));padding:.2em .5em}',
      '.k2md .md-url{color:var(--zen-text-muted);word-break:break-all}',
      '.k2md button.md-url{font:inherit;color:var(--zen-accent);background:none;border:0;padding:0;cursor:pointer;text-decoration:underline}',
      '.k2md img.md-img{max-width:100%}'
    ].join('\n')
    ;(W.document.head || W.document.documentElement).appendChild(s)
  }

  function canOpen() {
    var k2 = W.k2
    return !!(k2 && typeof k2.open === 'function' && typeof k2.can === 'function' && k2.can('links.open'))
  }

  // A DocumentFragment for `text`: markdown when marked and DOMPurify are
  // loaded, else the plain text.
  function render(text, opts) {
    var o = opts || {}
    var doc = W.document
    var plain = function () {
      var f = doc.createDocumentFragment()
      f.appendChild(doc.createTextNode(String(text == null ? '' : text)))
      return f
    }
    if (!md || !purify) return plain()
    var frag
    imagesOn = o.images === 'data'
    try {
      var html = md.parse(String(text == null ? '' : text), { async: false })
      frag = purify.sanitize(html, {
        ALLOWED_TAGS: imagesOn ? TAGS.concat(['img']) : TAGS,
        ALLOWED_ATTR: imagesOn ? ATTR.concat(['src', 'alt']) : ATTR,
        ALLOW_DATA_ATTR: false,
        ALLOW_ARIA_ATTR: false,
        FORBID_TAGS: imagesOn ? FORBID.filter(function (t) { return t !== 'img' }) : FORBID,
        RETURN_DOM_FRAGMENT: true
      })
    } catch (e) {
      return plain()
    } finally {
      imagesOn = false
    }
    // Links: a button per URL that asks K2 to open it (after the person's
    // click, K2's own site card), only with links:open.
    if (o.links === 'open' && canOpen()) {
      var spans = frag.querySelectorAll('span.md-url')
      for (var i = 0; i < spans.length; i++) {
        var url = spans[i].textContent || ''
        var b = doc.createElement('button')
        b.type = 'button'
        b.className = 'md-url'
        b.textContent = url
        b.addEventListener('click', (function (u) {
          return function (ev) {
            ev.preventDefault()
            W.k2.open(u)
          }
        })(url))
        spans[i].replaceWith(b)
      }
    }
    return frag
  }

  // Replace el's children with `text` as markdown.
  function into(el, text, opts) {
    style()
    el.classList.add('k2md')
    el.replaceChildren(render(text, opts))
    return el
  }

  W.K2Markdown = Object.freeze({
    render: render,
    into: into,
    available: function () { return !!(md && purify) }
  })
})()
