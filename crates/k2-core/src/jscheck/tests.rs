use super::*;

fn rules(text: &str, lang: Lang, surface: Surface) -> Vec<&'static str> {
    check(text, lang, surface).into_iter().map(|f| f.rule).collect()
}

fn widget(text: &str) -> Vec<Finding> {
    check(text, Lang::Script, Surface::Widget)
}

fn only(findings: &[Finding], rule: &str) -> Vec<Finding> {
    findings.iter().filter(|f| f.rule == rule).cloned().collect()
}

// ── parse ──────────────────────────────────────────────────────────────

#[test]
fn a_syntax_error_is_an_error_with_its_line_and_a_suggestion() {
    let src = "const a = 1\nconst b = ;\nconsole.log(a, b)\n";
    let f = widget(src);
    assert_eq!(f.len(), 1, "{f:#?}");
    assert_eq!(f[0].severity, Severity::Error);
    assert_eq!(f[0].rule, "parse");
    let (line, _) = line_col(src, f[0].offset as usize);
    assert_eq!(line, 2, "{f:#?}");
    assert!(f[0].suggestion.contains("doesn't parse"), "{f:#?}");
}

#[test]
fn early_errors_from_the_semantic_pass_are_errors() {
    // The parser accepts this; a browser throws SyntaxError (redeclared let).
    let src = "let a = 1\nlet a = 2\n";
    let f = widget(src);
    assert_eq!(f.len(), 1, "{f:#?}");
    assert_eq!(f[0].severity, Severity::Error);
    assert_eq!(line_col(src, f[0].offset as usize).0, 2, "{f:#?}");
}

#[test]
fn valid_sloppy_script_parses_clean() {
    // Sloppy-mode only: `with`, octal, a function in a block.
    let src = "var o = {a: 1}\nwith (o) { a = 2 }\nvar n = 010\nif (true) { function g() {} }\n";
    assert_eq!(widget(src), Vec::new());
}

#[test]
fn import_in_a_classic_script_is_one_error_that_says_so() {
    let src = "// helper\nimport { x } from './x.js'\nconsole.log(x)\n";
    let f = widget(src);
    assert_eq!(f.len(), 1, "{f:#?}");
    assert_eq!(f[0].rule, "parse-import");
    assert_eq!(f[0].severity, Severity::Error);
    assert_eq!(line_col(src, f[0].offset as usize), (2, 1));
    assert!(f[0].suggestion.contains("<script src"), "{f:#?}");
}

#[test]
fn import_in_a_module_script_is_a_warning() {
    let f = check("import { x } from './x.js'\nconsole.log(x)\n", Lang::Module, Surface::Widget);
    assert_eq!(f.iter().map(|f| (f.rule, f.severity)).collect::<Vec<_>>(), vec![("widget-import", Severity::Warning)]);
}

#[test]
fn typescript_and_jsx_parse_by_file_kind() {
    assert_eq!(Lang::for_file("a.ts"), Some(Lang::TypeScript));
    assert_eq!(Lang::for_file("A.TSX"), Some(Lang::Tsx));
    assert_eq!(Lang::for_file("a.jsx"), Some(Lang::Unambiguous));
    assert_eq!(Lang::for_file("a.cjs"), Some(Lang::CommonJs));
    assert_eq!(Lang::for_file("a.d.ts"), None);
    assert_eq!(Lang::for_file("vendor.min.js"), None);
    assert_eq!(Lang::for_file("a.css"), None);
    assert_eq!(rules("type A = { n: number }\nexport const f = (a: A): number => a.n\n", Lang::TypeScript, Surface::App), Vec::<&str>::new());
    assert_eq!(rules("export default function P() { return <div>hi</div> }\n", Lang::Tsx, Surface::App), Vec::<&str>::new());
    assert_eq!(rules("const x = <b>hi</b>\nmodule.exports = x\n", Lang::Unambiguous, Surface::App), Vec::<&str>::new());
    let bad = check("const f = (a: number => a\n", Lang::TypeScript, Surface::App);
    assert_eq!(bad.first().map(|f| f.severity), Some(Severity::Error), "{bad:#?}");
}

#[test]
fn script_types_that_are_not_javascript_are_skipped() {
    assert_eq!(script_lang(None), Some(Lang::Script));
    assert_eq!(script_lang(Some("text/javascript")), Some(Lang::Script));
    assert_eq!(script_lang(Some(" Module ")), Some(Lang::Module));
    assert_eq!(script_lang(Some("application/json")), None);
    assert_eq!(script_lang(Some("importmap")), None);
    assert_eq!(script_lang(Some("text/template")), None);
}

// ── widget rules (AST versions of UW10's substring warnings) ───────────

#[test]
fn words_in_strings_and_comments_no_longer_warn() {
    let src = "// we never call fetch( or use localStorage here\nconst s = 'innerHTML eval( new Function WebSocket'\nel.textContent = s\n";
    assert_eq!(widget(src), Vec::new());
}

#[test]
fn innerhtml_warns_on_writes_only() {
    assert_eq!(rules("const h = el.innerHTML\n", Lang::Script, Surface::Widget), Vec::<&str>::new());
    let f = widget("el.innerHTML = text\nel.innerHTML += more\n");
    assert_eq!(f.len(), 1, "one per pattern: {f:#?}");
    assert_eq!(f[0].rule, "widget-html");
    assert!(f[0].message.starts_with("innerHTML: "), "{f:#?}");
}

#[test]
fn every_widget_rule_fires_on_real_use() {
    let src = "\
fetch('/x')
new XMLHttpRequest()
new WebSocket('wss://x')
new EventSource('/e')
localStorage.setItem('a', 1)
window.sessionStorage.clear()
indexedDB.open('x')
eval('1')
new Function('return 1')
el.outerHTML = s
el.insertAdjacentHTML('beforeend', s)
document.write(s)
";
    let mut got: Vec<String> = widget(src).iter().map(|f| f.message.split(':').next().unwrap_or_default().to_string()).collect();
    got.sort();
    let mut want = vec![
        "fetch", "XMLHttpRequest", "WebSocket", "EventSource", "localStorage", "sessionStorage", "indexedDB", "eval",
        "new Function", "outerHTML", "insertAdjacentHTML", "document.write",
    ];
    want.sort();
    assert_eq!(got, want);
}

#[test]
fn a_local_named_like_a_global_is_not_the_global() {
    let src = "function f(fetch, localStorage) { fetch(1); localStorage.x = 1 }\nf(a, b)\n";
    assert_eq!(widget(src), Vec::new());
}

#[test]
fn widget_rules_do_not_apply_to_apps() {
    assert_eq!(rules("fetch('/api')\nlocalStorage.x = 1\nel.innerHTML = s\n", Lang::Unambiguous, Surface::App), Vec::<&str>::new());
}

// ── catalog lint ───────────────────────────────────────────────────────

#[test]
fn known_widget_verbs_with_the_right_arguments_are_clean() {
    let src = "\
k2.connected.then(() => {
  if (k2.can('agents.subscribe')) k2.agents.subscribe((rows) => draw(rows))
  k2.thread.read('k2/1', { limit: 5 }).then(draw)
  k2.thread.post('k2/1', 'hi')
  k2.call('thread.read', 'k2/1')
  k2.subscribe('agents.subscribe', draw, onError)
  k2.theme.changed(draw)
  k2.ready()
})
function draw() {}
function onError() {}
";
    assert_eq!(widget(src), Vec::new());
}

#[test]
fn an_unknown_verb_warns_with_the_nearest_real_one() {
    let f = widget("k2.thread.raed('k2/1')\n");
    assert_eq!(f.len(), 1, "{f:#?}");
    assert_eq!((f[0].rule, f[0].severity), ("catalog-unknown", Severity::Warning));
    assert!(f[0].suggestion.contains("thread.read"), "{f:#?}");
}

#[test]
fn a_verb_closed_to_widgets_says_so() {
    let f = widget("k2.gardens.create('x', 'k2-blank-1')\n");
    assert_eq!(f.len(), 1, "{f:#?}");
    assert_eq!(f[0].rule, "catalog-unknown");
    assert!(f[0].message.contains("isn't open to widgets"), "{f:#?}");
}

#[test]
fn argument_counts_are_checked() {
    let f = widget("k2.thread.post('k2/1')\nk2.thread.read('k2/1', {}, 3)\nk2.agents.subscribe()\nk2.call('thread.post', 'a')\n");
    let arity = only(&f, "catalog-arity");
    assert_eq!(arity.len(), 4, "{f:#?}");
    assert!(arity[0].message.contains("needs address, text"), "{arity:#?}");
    assert!(arity[0].suggestion.contains("thread.post(address, text)"), "{arity:#?}");
}

#[test]
fn call_vs_subscribe_mismatch_and_bad_runtime_members() {
    let f = widget("k2.subscribe('thread.read', cb)\nk2.fetch('x')\nfunction cb() {}\n");
    let r: Vec<&str> = f.iter().map(|f| f.rule).collect();
    assert_eq!(r, vec!["catalog-kind", "catalog-unknown"], "{f:#?}");
}

#[test]
fn a_local_k2_is_not_linted_and_spreads_skip_arity() {
    assert_eq!(widget("const k2 = makeFake()\nk2.thread.nope()\n"), Vec::new());
    assert_eq!(widget("k2.thread.post(...args)\n"), Vec::new());
}

// ── leak lints (prd-k2-app-runtime-v1 §6.8) ────────────────────────────

#[test]
fn module_map_growing_in_a_request_handler_warns() {
    let src = "\
const express = require('express')
const app = express()
const seen = new Map()
app.get('/hit', (req, res) => {
  seen.set(req.ip, Date.now())
  res.send('ok')
})
";
    let f = check(src, Lang::CommonJs, Surface::App);
    assert_eq!(f.len(), 1, "{f:#?}");
    assert_eq!(f[0].rule, "leak-growth");
    assert_eq!(f[0].severity, Severity::Warning);
    assert_eq!(line_col(src, f[0].offset as usize).0, 5);
    assert!(f[0].message.contains("'seen'") && f[0].message.contains(".set"), "{f:#?}");
}

#[test]
fn growth_with_a_delete_somewhere_or_at_load_is_fine() {
    let pruned = "\
const seen = new Map()
app.get('/hit', (req, res) => { seen.set(req.ip, 1) })
setInterval(() => { for (const k of seen.keys()) seen.delete(k) }, 60000)
";
    assert_eq!(rules(pruned, Lang::CommonJs, Surface::App), Vec::<&str>::new());
    let at_load = "const list = []\nfor (const x of [1, 2]) list.push(x)\n";
    assert_eq!(rules(at_load, Lang::CommonJs, Surface::App), Vec::<&str>::new());
    let local = "app.get('/x', (req, res) => { const a = []; a.push(1); res.json(a) })\n";
    assert_eq!(rules(local, Lang::CommonJs, Surface::App), Vec::<&str>::new());
    let reassigned = "let log = []\napp.post('/x', (req) => { log.push(req.body); if (log.length > 100) log = log.slice(-50) })\n";
    assert_eq!(rules(reassigned, Lang::CommonJs, Surface::App), Vec::<&str>::new());
}

#[test]
fn growth_in_a_named_handler_declared_before_its_collection_warns() {
    let src = "\
function onHit(req, res) { cache[req.url] = res.body }
const cache = {}
server.on('request', onHit)
";
    assert_eq!(rules(src, Lang::CommonJs, Surface::App), vec!["leak-growth"]);
}

#[test]
fn exported_route_handlers_and_widget_subscriptions_are_handlers() {
    let next = "const hits = []\nexport async function POST(req) { hits.push(await req.json()); return Response.json({ ok: true }) }\n";
    assert_eq!(rules(next, Lang::TypeScript, Surface::App), vec!["leak-growth"]);
    let iife = "(function () {\n  const rows = []\n  k2.agents.subscribe((r) => { rows.push(r) })\n})()\n";
    assert_eq!(rules(iife, Lang::Script, Surface::Widget), vec!["leak-growth"]);
}

#[test]
fn session_stores_with_no_expiry_warn() {
    let map = "const sessions = new Map()\napp.post('/login', (req, res) => { sessions.set(token(), req.body.user) })\n";
    assert_eq!(rules(map, Lang::CommonJs, Surface::App), vec!["leak-session-store"]);
    let express = "import session from 'express-session'\napp.use(session({ secret: 's', resave: false }))\n";
    assert_eq!(rules(express, Lang::Module, Surface::App), vec!["leak-session-store"]);
    let with_store = "const session = require('express-session')\napp.use(session({ secret: 's', store: new PgStore() }))\n";
    assert_eq!(rules(with_store, Lang::CommonJs, Surface::App), Vec::<&str>::new());
    let mem = "const store = new session.MemoryStore()\n";
    assert_eq!(rules(mem, Lang::CommonJs, Surface::App), vec!["leak-session-store"]);
}

#[test]
fn set_interval_in_a_handler_without_a_clear_warns() {
    let bad = "io.on('connection', (socket) => {\n  setInterval(() => socket.emit('tick'), 1000)\n})\n";
    let f = check(bad, Lang::CommonJs, Surface::App);
    assert_eq!(f.iter().map(|f| f.rule).collect::<Vec<_>>(), vec!["leak-interval"], "{f:#?}");
    assert_eq!(line_col(bad, f[0].offset as usize).0, 2);
    let kept = "io.on('connection', (socket) => {\n  const t = setInterval(() => socket.emit('tick'), 1000)\n  socket.on('disconnect', () => clearInterval(t))\n})\n";
    assert_eq!(rules(kept, Lang::CommonJs, Surface::App), Vec::<&str>::new());
    // One timer at load is not a leak (the hello widget's clock).
    assert_eq!(rules("setInterval(tick, 1000)\nfunction tick() {}\n", Lang::Script, Surface::Widget), Vec::<&str>::new());
}

#[test]
fn listeners_added_per_request_to_long_lived_targets_warn() {
    let bad = "const bus = new EventEmitter()\napp.get('/w', (req, res) => {\n  bus.on('change', () => res.write('x'))\n})\n";
    assert_eq!(rules(bad, Lang::CommonJs, Surface::App), vec!["leak-listener"]);
    let global = "k2.agents.subscribe(() => { window.addEventListener('resize', draw) })\nfunction draw() {}\n";
    assert_eq!(rules(global, Lang::Script, Surface::Widget), vec!["leak-listener"]);
    let removed = "const bus = new EventEmitter()\napp.get('/w', (req, res) => {\n  const f = () => res.write('x')\n  bus.on('change', f)\n  req.on('close', () => bus.off('change', f))\n})\n";
    assert_eq!(rules(removed, Lang::CommonJs, Surface::App), Vec::<&str>::new());
    let per_request_object = "app.get('/w', (req, res) => { req.on('close', done) })\nfunction done() {}\n";
    assert_eq!(rules(per_request_object, Lang::CommonJs, Surface::App), Vec::<&str>::new());
    let once = "el.addEventListener('click', () => { window.addEventListener('keyup', f, { once: true }) })\nfunction f() {}\n";
    assert_eq!(rules(once, Lang::Script, Surface::Widget), Vec::<&str>::new());
}

#[test]
fn a_k2_subscription_inside_a_handler_warns() {
    let src = "button.addEventListener('click', () => {\n  k2.thread.subscribe('k2/1', draw)\n})\nfunction draw() {}\n";
    assert_eq!(rules(src, Lang::Script, Surface::Widget), vec!["leak-listener"]);
}

// ── K2's own widgets stay clean ────────────────────────────────────────

#[test]
fn every_example_and_builtin_widget_script_is_clean() {
    let mut checked = 0;
    // Examples by name; built-ins by `k2:<name>@<n>`, since a finding is
    // pinned to the released version that has it.
    let mut sets: Vec<(String, Vec<(&str, &str)>)> = crate::zen::widgets::EXAMPLES
        .iter()
        .map(|e| (e.name.to_string(), e.files.iter().map(|(n, t)| (*n, *t)).collect()))
        .collect();
    for w in crate::zen::builtin_widgets::BUILTIN_WIDGETS {
        sets.push((w.id(), w.files.iter().map(|(n, t)| (*n, *t)).collect()));
    }
    for (name, files) in sets {
        let name = name.as_str();
        for (file, text) in files {
            if !file.ends_with(".js") {
                continue;
            }
            let f = check(text, Lang::Script, Surface::Widget);
            // Released built-ins are pinned by hash (TUWB6) and never change,
            // so a finding in one is recorded here instead of fixed in place;
            // its next version must come out clean.
            let known: Vec<&str> = PINNED_BUILTIN_FINDINGS
                .iter()
                .filter(|(n, fl, _, _)| *n == name && *fl == file)
                .map(|(_, _, rule, _)| *rule)
                .collect();
            let rules: Vec<&str> = f.iter().map(|x| x.rule).collect();
            assert_eq!(rules, known, "{name}/{file}: {f:#?}");
            for (n, fl, _, word) in PINNED_BUILTIN_FINDINGS.iter().filter(|(n, fl, _, _)| *n == name && *fl == file) {
                assert!(f.iter().any(|x| x.message.contains(word)), "{n}/{fl}: the recorded finding names {word}: {f:#?}");
            }
            checked += 1;
        }
    }
    assert!(checked >= 4, "only {checked} scripts checked");
}

/// Findings in released, hash-pinned built-ins: (widget, file, rule, a word
/// the message names). kanban@1's dock keeps one label per agent address
/// it has seen (bounded by the agent list, so not a real leak); kanban@2's
/// rebuilds it from each agent list, so it comes out clean.
const PINNED_BUILTIN_FINDINGS: &[(&str, &str, &str, &str)] = &[("k2:kanban@1", "dock.js", "leak-growth", "seenLabels")];
