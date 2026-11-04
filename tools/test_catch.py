# Test tooling, not shipped. catch.py's self-test: each kind that counts is named, and each near
# miss is refused. `@@` marks the column a failure reports.
import importlib.util, os, sys

# No bytecode: a mutant of catch.py and the restored file can share a size and a second, and a
# cached copy of the mutant would then stand in for the restored file.
sys.dont_write_bytecode = True

spec = importlib.util.spec_from_file_location(
    'catch', os.path.join(os.path.dirname(os.path.abspath(__file__)), 'catch.py'))
catch = importlib.util.module_from_spec(spec)
spec.loader.exec_module(catch)

T = 'crates/board/tests/file.rs'
S = 'crates/board/tests/support/verbs.rs'
L = 'crates/board/src/zip.rs'
BT = 'bridge/tests/keys.rs'
BS = 'bridge/tests/support/mod.rs'
BL = 'bridge/src/report.rs'
UNIT = 'fn f() {}\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn t() {\n'
CASES = [
    # (what, path, source, wanted kind)
    ('assert_eq', T, '    @@assert_eq!(a, b, "same");', 'assert'),
    ('assert', T, '    @@assert!(ok);', 'assert'),
    ('assert_ne', T, '    @@assert_ne!(a, b);', 'assert'),
    ('assert of matches', T, '    @@assert!(matches!(e, Conflict::Changed { .. }), "{e:?}");', 'assert'),
    ('debug_assert', T, '    @@debug_assert_eq!(a, b);', 'assert'),
    ('expect_err', T, '    let e = f.@@expect_err("stale");', 'expect_err'),
    ('expect_err on its own line', T, '        .@@expect_err("refused");', 'expect_err'),
    ('panic with a literal', T, '    @@panic!("wanted a refusal for {name}");', 'panic'),
    ('panic with arguments', T, '    @@panic!("wanted {}, got {:?}", a, b);', 'panic'),
    ('panic, the literal below', T, '    @@panic!(\n        "wanted a refusal, got {e:?}"\n    );', 'panic'),
    ('expect with the check prefix', T, '    f.@@expect("check: a clean merge");', 'check'),
    ('a raw literal with the prefix', T, '    f.@@expect(r"check: it saves");', 'check'),
    ('a plain expect', T, '    let f = open().@@expect("it opens");', None),
    ('unwrap', T, '    let f = open().@@unwrap();', None),
    ('an index', T, '    let x = @@c.extra[0].name;', None),
    ('arithmetic', T, '    let n = @@a - b;', None),
    ('an expect with no literal', T, '    f.@@expect(&format!("{name}"));', None),
    ('a panic with no literal', T, '    @@panic!(msg);', None),
    ('an expect_err with no literal', T, '    f.@@expect_err(msg);', None),
    ('library source', L, '    @@assert_eq!(crc, want);', None),
    ('library source above its tests', L, '    @@assert_eq!(crc, want);\n' + UNIT + '    }\n}', None),
    ('a unit test in library source', L, UNIT + '        @@assert_eq!(crc, want);\n    }\n}', 'assert'),
    ('a plain expect in a unit test', L, UNIT + '        x.@@expect("it reads");\n    }\n}', None),
    ('a file that is not test code', 'tools/catch.py', '    @@assert_eq!(a, b);', None),
    ('support: an assert', S, '    @@assert_eq!(a, b);', 'assert'),
    ('support: the check prefix', S, '    p.@@expect("check: it plans");', 'check'),
    ('support: a panic', S, '    .unwrap_or_else(|e| @@panic!("the inverse failed: {e:?}"))', None),
    ('support: an expect_err', S, '    f.@@expect_err("refused");', None),
    ('support: a plain expect', S, '    p.@@expect("it applies");', None),
    ('no such line', T, None, None),
    ('bridge: an assert', BT, '    @@assert_eq!(r.id(), want);', 'assert'),
    ('bridge: a panic with a literal', BT, '    @@panic!("{want}: a host bug: {bug}");', 'panic'),
    ('bridge: the check prefix', BT, '    r.@@expect("check: the kit holds the row");', 'check'),
    ('bridge: a plain expect', BT, '    r.@@expect("it reports");', None),
    ('bridge support: an assert', BS, '    @@assert!(ok);', 'assert'),
    ('bridge support: an expect_err', BS, '    p.@@expect_err("the verb refuses");', None),
    ('bridge support: the check prefix', BS, '    p.@@expect("check: it plans");', 'check'),
    ('bridge library source', BL, '    @@assert_eq!(a, b);', None),
    ('a unit test in bridge source', BL, UNIT + '        @@assert_eq!(a, b);\n    }\n}', 'assert'),
    ('a bridge file that is not test code', 'bridge/README.md', '    @@assert_eq!(a, b);', None),
]

fails = 0
for what, path, source, want in CASES:
    if source is None:
        got, why = catch.classify(path, 9, 1, ['x'])
    else:
        lines = source.split('\n')
        mark = next(i for i, l in enumerate(lines) if '@@' in l)
        col = lines[mark].index('@@') + 1
        lines[mark] = lines[mark].replace('@@', '', 1)
        got, why = catch.classify(path, mark + 1, col, lines)
    if got == want:
        print(f'ok   {what}: {got}')
    else:
        print(f'FAILED {what}: wanted {want}, got {got} ({why})')
        fails += 1

for what, line, want in [
    ('a script names its case', 'FAILED vectors/widget/a.json: wanted exit 1, got 0: {}', 'vectors/widget/a.json'),
    ('the checker names its case', 'FAILED bad-ids-past-cap: bad ids differ: {}', 'bad-ids-past-cap'),
    ('a case with spaces', 'FAILED main a MPL-2.0: wanted exit 1, got 0', 'main a MPL-2.0'),
    ('a bare FAILED line', 'FAILED the fidelity layer did not run', None),
    ('a bare FAILED with a colon at the end', 'FAILED nothing here:', None),
    ('not a FAILED line', 'ok   vectors/widget/a.json exit 1', None),
]:
    got = catch.script_case(line)
    if got == want:
        print(f'ok   {what}: {got}')
    else:
        print(f'FAILED {what}: wanted {want}, got {got}')
        fails += 1

log = ("thread 'a_test' panicked at crates/board/tests/merge.rs:52:40:\nwanted a clean merge\n"
       "thread 'other' (12) panicked at crates/board/src/zip.rs:743:9:\nkept\n")
want = [('a_test', 'crates/board/tests/merge.rs', 52, 40), ('other', 'crates/board/src/zip.rs', 743, 9)]
if catch.locations(log) == want:
    print('ok   locations: both failures, in order')
else:
    print(f'FAILED locations: {catch.locations(log)}')
    fails += 1

blog = ("thread 'k' panicked at tests/keys.rs:24:9:\nmissing\n"
        "thread 's' panicked at tests/support/mod.rs:91:37:\nsetup\n"
        "thread 'b' panicked at /node/src/k2-mpc-board/crates/board/src/ops.rs:10:5:\nlib\n"
        "thread 'p' panicked at bridge/src/rows.rs:3:1:\nprefixed\n")
bwant = [('k', 'bridge/tests/keys.rs', 24, 9), ('s', 'bridge/tests/support/mod.rs', 91, 37),
         ('b', '/node/src/k2-mpc-board/crates/board/src/ops.rs', 10, 5), ('p', 'bridge/src/rows.rs', 3, 1)]
if catch.locations(blog, 'bridge/') == bwant:
    print('ok   locations under bridge/: relative paths put back under it, the others kept')
else:
    print(f'FAILED locations under bridge/: {catch.locations(blog, "bridge/")}')
    fails += 1
if catch.locations(blog) == [(t, p.replace('bridge/', '', 1) if p.startswith('bridge/tests') else p, l, c) for t, p, l, c in bwant]:
    print('ok   locations with no folder: paths as printed')
else:
    print(f'FAILED locations with no folder: {catch.locations(blog)}')
    fails += 1

for what, counts, problems, want_code, want_last in [
    ('verdict-clean', {'caught': 2, 'survived': 0}, [], 0, 'mutants finished'),
    ('verdict-problems', {'caught': 1, 'survived': 1}, ['m'], 4, "needs a look: ['m']"),
    ('verdict-none-ran', {'caught': 0, 'survived': 0}, [], 3, 'mutants result: none ran'),
]:
    lines, code = catch.verdict(counts, problems)
    if code != want_code or lines[-1] != want_last or (code != 0) == ('mutants finished' in lines):
        print(f'FAILED {what}: wanted exit {want_code} ending {want_last!r}, got {code} {lines}')
        fails += 1
    else:
        print(f'ok   {what}: exit {code}')

# A mutant that deletes a line above an assert: the run's failure is at the mutated file's line 2,
# which is the assert there and a plain statement in the restored file.
restored = ['fn t() {', '    let a = 1;', '    assert!(a == 2);', '}']
mutated = {T: 'fn t() {\n    assert!(a == 2);\n}'}
lines = catch.run_lines(T, mutated, lambda p: restored)
got, why = catch.classify(T, 2, 5, lines)
other = catch.run_lines(S, mutated, lambda p: ['from disk'])
if got == 'assert' and other == ['from disk']:
    print('ok   mutated-lines: read against the run\'s own source')
else:
    print(f'FAILED mutated-lines: wanted assert from the mutated text and the disk elsewhere, got {got} ({why}), {other}')
    fails += 1

for what, path, text, old, new, want in [
    ('guard-test-file', T, 'a\nb\n', 'b\n', '', True),
    ('guard-same-count', T, 'a\nb\n', 'b\n', 'c\n', False),
    ('guard-test-data', 'crates/board/tests/data/x.json', 'a\nb\n', 'b\n', '', False),
    ('guard-unit-tests', L, 'fn f() {}\n' + UNIT + '    }\n}', 'fn f() {}\n', '', True),
    ('guard-library', L, 'fn f() {}\nfn g() {}\n', 'fn f() {}\n', '', False),
    ('guard-bridge-test', BT, 'a\nb\n', 'b\n', '', True),
]:
    got = catch.moves_test_lines(path, text, old, new)
    if got == want:
        print(f'ok   {what}: {got}')
    else:
        print(f'FAILED {what}: wanted {want}, got {got}')
        fails += 1

n = len(CASES) + 6 + 3 + 3 + 1 + 6
if fails:
    print(f'catch self-test: {fails} of {n} failed')
    sys.exit(1)
print(f'catch self-test: ok ({n} cases)')
