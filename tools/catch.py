# Test tooling, not shipped. The written rule for what counts as a mutant's catch, used by sweep.py.
#
# A failing line counts when it's in test code, under crates/board/tests/ or bridge/tests/, and it's
# one of:
#  - an assert macro: assert!, assert_eq!, assert_ne!, or a debug_assert form       [assert]
#  - .expect_err("...") with a string literal: refusing is the check                 [expect_err]
#  - panic!("...", ...) whose first argument is a literal, which may start on the
#    next line                                                                        [panic]
#  - .expect("check: ..."): the test's own stated check that a call succeeds         [check]
# A unit test in library source, inside its #[cfg(test)] module, is test code as a test file is.
# In tests/support/ (either crate's), helpers are mostly setup, so only an assert macro or a message
# starting check: counts there. Nothing else counts: a plain expect, unwrap, an index, arithmetic, a message that
# isn't a literal, or anything in library source outside a test module.
#
# A script's or the JavaScript checker's own FAILED line is its catch, when it names the planted
# case before a colon: `FAILED <case>: <what went wrong>`. A bare FAILED line doesn't count.
import re

TESTS = ('crates/board/tests/', 'bridge/tests/')
SUPPORT = ('crates/board/tests/support/', 'bridge/tests/support/')
SRC = ('crates/board/src/', 'bridge/src/')
# The bridge is its own workspace: cargo prints its paths from bridge/, which locations() adds back.
ASSERT = re.compile(r'(debug_)?assert(_eq|_ne)?!\s*\(')
CALL = re.compile(r'(expect_err|expect|panic!)\s*\(\s*(r#*)?"(.*)', re.S)


def classify(path, line, col, lines):
    """The kind that counts for a failure at path:line:col, or None, with a word on why.

    `lines` is the file's text as a list of lines, at the commit the run was made at."""
    if line < 1 or line > len(lines):
        return None, 'no such line'
    if path.startswith(SRC):
        start = test_module(lines)
        if start is None or line < start:
            return None, 'library'
    elif not path.startswith(TESTS):
        return None, 'not test code'
    # The failing expression starts at the column, and a literal may follow on the next lines.
    text = lines[line - 1][col - 1:] + '\n' + '\n'.join(lines[line:line + 3])
    text = text.lstrip('.')
    if ASSERT.match(text):
        return 'assert', ''
    m = CALL.match(text)
    if not m:
        return None, 'not a check'
    what, rest = m.group(1), m.group(3)
    if rest.startswith('check:'):
        return 'check', ''
    if path.startswith(SUPPORT):
        return None, 'support'
    if what == 'expect_err':
        return 'expect_err', ''
    if what == 'panic!':
        return 'panic', ''
    return None, 'expect'


def test_module(lines):
    """The line a file's `#[cfg(test)] mod` starts at, 1-based, or None. Unit tests sit at the end."""
    for i, l in enumerate(lines[:-1]):
        if l.strip() == '#[cfg(test)]' and re.match(r'\s*(pub(\(\w+\))? )?mod \w+', lines[i + 1]):
            return i + 1
    return None


def script_case(line):
    """The planted case a script's FAILED line names, or None for a bare one."""
    m = re.match(r'FAILED ([^:\s][^:]*): \S', line)
    return m.group(1) if m else None


def run_lines(path, mutated, read):
    """The lines a failure at `path` is read against: the run's own source. For the file a mutant
    changed that's its mutated text, kept from before the restore (`mutated`, by path), so a mutant
    that adds or removes lines can't move a catch; every other file is as `read` gives it."""
    if path in mutated:
        return mutated[path].split('\n')
    return read(path)


def moves_test_lines(path, text, old, new):
    """Whether turning `old` into `new` changes the line count of a Rust file that holds test code:
    a test file, or library source with a `#[cfg(test)]` module. A sweep refuses such a mutant, as a
    backstop to `run_lines`."""
    if not path.endswith('.rs') or old.count('\n') == new.count('\n'):
        return False
    if path.startswith(TESTS):
        return True
    return path.startswith(SRC) and test_module(text.split('\n')) is not None


def verdict(counts, problems):
    """A sweep's last lines and its exit: 3 when no mutant ran, 4 when one needs a look (a survivor,
    a stale pattern, a build that failed, a failure at no line that counts), 0 when every one held.
    `mutants finished` is printed only then. A sweep exits with this code, never with a crash."""
    total = sum(counts.values())
    if total == 0:
        return ['mutants result: none ran'], 3
    lines = [f'mutants result: total={total} ' + ' '.join(f'{k}={v}' for k, v in counts.items()),
             f'needs a look: {problems}']
    if problems:
        return lines, 4
    return lines + ['mutants finished'], 0


def locations(out, under=''):
    """Every failure in a cargo test log, in order: (test, path, line, col). `under` is the folder
    of the workspace that ran, from the repo root (`bridge/`): a relative path it printed is put
    back under it, so a path is always from the repo root."""
    found = []
    for m in re.finditer(r"thread '([^']+)'[^\n]* panicked at ([^\s:]+):(\d+):(\d+):", out):
        path = m.group(2)
        if under and not path.startswith('/') and not path.startswith(under):
            path = under + path
        found.append((m.group(1), path, int(m.group(3)), int(m.group(4))))
    return found
