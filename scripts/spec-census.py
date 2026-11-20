#!/usr/bin/env python3
"""The spec verbs' census (R4.0): every `k2 --schema` entry, where bash
answers it, what it sends and prints, and whether it can be a spec verb.

    python3 scripts/spec-census.py           write crates/k5-cli/tests/data/spec_census.tsv
    python3 scripts/spec-census.py --check   change nothing; exit 1 if the file is stale

The census is read from cli/src (the parts cli/src/ORDER lists), so it
moves with the code: crates/k5-cli/tests/spec_census.rs runs --check.
One row per entry, the entries in ORDER's order (cli/src/schema/*.entries,
then schema-tail.sh's). Columns:

  entry    the entry's name
  forms    2 when the entry takes --json (the JSON form and the text form),
           else 1. A verb form is one of these (K2's ruling: the target
           counts forms).
  where    cli/src/<file>:<line> of the bash code that answers it: the case
           arm of its last word, found from the router's arm for its first
           word through each dispatching function (`cmd_*`, or a function
           called with "$@"). `<file>:<line>~` when no arm names the last
           word and the line is the function that reads it; `-` when the
           router has no arm for the first word.
  via      the request functions its code calls (that code and the
           functions it calls in the same family, two levels), or `none`
  output   what its code prints with: body (printf of the body), text
           (_compute_text's lines), json (json.dumps), table (a shared
           printer), python (its own python3 printer), or `none`
  kind     spec (its row is a spec verb today), candidate, or out:<why>
           (the first that applies, in this order):
             out:no-arm       the router has no arm for it
             out:no-request   it sends nothing (help, local, a page)
             out:python-client  mail, calendar, hostmail, db (K2's ruling 10)
             out:open-tool    an open tool: bash sends the owner token over
                              TCP in a K2 terminal (COMPAT-58; ruling 4)
             out:secret       it reads a secret
             out:prompt       it asks the person (read)
             out:confirm      a confirm token or step
             out:poll         a loop with sleep (waits, long polls)
             out:file         it reads or writes a local file
             out:process      it starts a process
             out:many-requests  more than one request
  tier     for a candidate: 1 (both forms are body, text, json or none),
           2 (its text form is a shared printer), 3 (its own python);
           `-` otherwise

The kinds are found from the code's words, so they are a census, not a
review: a row's kind is what its words say, and a verb is read again
before it flips. The check keeps the list equal to the code.
"""

import json
import os
import re
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SRC = os.path.join(ROOT, "cli", "src")
OUT = "crates/k5-cli/tests/data/spec_census.tsv"

VIAS = [
    ("compute", r"\b_compute_(?:call|send)\b"),
    ("sidecar", r"\b_sidecar_send\b"),
    ("garden-data", r"\b_gd_call\b"),
    ("send-json", r"\bcli_send_json\b"),
    ("send", r"\bcli_send\b(?!_)"),
    ("request", r"\bcli_request\b(?!_)"),
    ("request-post", r"\bcli_request_post\b"),
    ("post-json", r"\bcli_post_json(?:_stdin)?\b"),
    ("post-form", r"\bcli_post_form\b"),
    ("python-client", r"\b_(?:mail|cal|db|project)_py\b|urllib\.request"),
    ("curl", r"\b_k2_curl\b"),
]
SHARED_PRINTERS = r"\b_(?:compute_render|hostchat_print|voice_print|chat_print|gd_out|sidecar_print_\w+|dns_handle_resp|dom_handle_resp)\b"
OUTPUTS = [
    ("body", r"printf '%s\\n' \"\$_(?:CP|SC)_BODY\""),
    ("text", r"\b_compute_text\b"),
    ("json", r"json\.dumps"),
    ("table", SHARED_PRINTERS),
    ("python", r"\bpython3\b"),
]
OUTS = [
    ("secret", r"_K2_SECRET|read -r?s|_k2_read_secret|--token\b"),
    ("prompt", r"\bread -r? ?-p\b|\bread -p\b|</dev/tty"),
    ("confirm", r"confirm"),
    ("poll", r"\bsleep \d"),
    ("file", r"\bmktemp\b|\bcat [\"<]|> *\"\$|\[ -f "),
    ("process", r"\bnohup\b|\bexec \b|\bopen -a\b| &$"),
]
PY_CLIENT_NOUNS = {"mail", "calendar", "cal", "hostmail", "db"}
# The shell's request and print helpers: never followed into.
SHARED_FILES = {"cli-tools-policy.sh", "cell-socket.sh", "prelude.sh", "compute-lib.sh", "sidecar.sh", "schema-head.sh", "schema-tail.sh"}


def parts():
    with open(os.path.join(SRC, "ORDER"), encoding="utf-8") as f:
        return [l.strip() for l in f if l.strip() and not l.startswith("#")]


def entries(order):
    out = []
    for name in order:
        if name.startswith("schema/"):
            with open(os.path.join(SRC, name), encoding="utf-8") as f:
                text = f.read()
            out.extend(json.loads("[" + text[:-2] + "]"))
    with open(os.path.join(SRC, "schema-tail.sh"), encoding="utf-8") as f:
        tail = f.read()
    body = tail[: tail.index("\n  ]\n")]
    start = body.index("    {\n")
    out.extend(json.loads("[" + body[start:] + "]"))
    return out


def functions(order):
    funcs = {}
    for name in order:
        if not name.endswith(".sh"):
            continue
        with open(os.path.join(SRC, name), encoding="utf-8") as f:
            lines = f.read().split("\n")
        i = 0
        while i < len(lines):
            m = re.match(r"^([A-Za-z_][A-Za-z0-9_]*)\(\) \{\s*$", lines[i])
            if m:
                j = i + 1
                while j < len(lines) and lines[j] != "}":
                    j += 1
                funcs[m.group(1)] = (name, i + 1, lines[i + 1 : j])
                i = j
            i += 1
    return funcs


def router_arms():
    with open(os.path.join(SRC, "router.sh"), encoding="utf-8") as f:
        lines = f.read().split("\n")
    start = next(i for i, l in enumerate(lines) if l.startswith('case "${1:-help}" in'))
    arms, cur = {}, None
    for i in range(start + 1, len(lines)):
        l = lines[i]
        if l == "esac":
            break
        m = re.match(r"^    ([^ (#][^)]*)\)\s*(.*)$", l)
        if m:
            cur = ("router.sh", i + 1, [m.group(2)])
            for w in m.group(1).split("|"):
                arms[w] = cur
        elif cur is not None:
            cur[2].append(l)
    return arms


def arm_in(file, base, body, word):
    """The case arm for `word` in `body` (its first line is base + 1):
    the last one whose code calls a function or sends, else the last."""
    pat = re.compile(r"^\s*\(?(?:[^\s()|]+\|)*" + re.escape(word) + r"(?:\|[^\s()|]+)*\)(.*)$")
    found = []
    for k, l in enumerate(body):
        m = pat.match(l)
        if not m:
            continue
        code = [m.group(1)]
        if not m.group(1).rstrip().endswith(";;"):
            # The arm ends at the `;;` of its own case, past any case
            # inside it.
            depth = 0
            for l2 in body[k + 1 :]:
                code.append(l2)
                s = l2.split("#", 1)[0] if not l2.lstrip().startswith("#") else ""
                depth += len(re.findall(r"(?:^|[\s;(])case\b.*\bin\b", s))
                depth -= len(re.findall(r"(?:^|[\s;])esac\b", s))
                if depth <= 0 and s.rstrip().endswith(";;"):
                    break
                depth = max(depth, 0)
        found.append((base + 1 + k, code))
    if not found:
        return None
    busy = [f for f in found if any(re.search(r"\b(cmd_\w+|_\w+)\b", l) for l in f[1])]
    line, code = (busy or found)[-1]
    return (file, line, code)


def calls(code, funcs):
    names = []
    for l in code:
        if l.lstrip().startswith("#"):
            continue
        for m in re.finditer(r"\b((?:cmd_|_)[A-Za-z0-9_]+)\b", l):
            n = m.group(1)
            if n in funcs and n not in names:
                names.append(n)
    return names


def dispatch(code, funcs):
    """The function the code hands its words to: the first `cmd_*` it
    calls, else the first function it calls with "$@"."""
    for n in calls(code, funcs):
        if n.startswith("cmd_"):
            return n
    for l in code:
        m = re.search(r'\b(_[A-Za-z0-9_]+) "\$@"', l)
        if m and m.group(1) in funcs and funcs[m.group(1)][0] not in SHARED_FILES:
            return m.group(1)
    return None


def locate(words, arms, funcs):
    arm = arms.get(words[0])
    if arm is None:
        return None, [], False
    file, line, code = arm
    exact = True
    for w in words[1:]:
        hit = arm_in(file, line, code, w)
        if hit is None:
            fn = dispatch(code, funcs)
            if fn is None:
                exact = False
                break
            ffile, fline, fbody = funcs[fn]
            hit = arm_in(ffile, fline, fbody, w)
            if hit is None:
                file, line, code, exact = ffile, fline, fbody, False
                break
        file, line, code = hit
    return (file, line), code, exact


def gather(code, funcs, file):
    """The code and the functions it calls in its family, two levels."""
    out, seen, todo = list(code), set(), [(code, 0)]
    while todo:
        c, depth = todo.pop()
        if depth == 2:
            continue
        for n in calls(c, funcs):
            f = funcs[n]
            if n in seen or f[0] in SHARED_FILES:
                continue
            seen.add(n)
            out.extend(f[2])
            todo.append((f[2], depth + 1))
    return out


def row(e, arms, funcs, spec_names):
    name = e["name"]
    words = name.split(" ")
    forms = 2 if any(f.get("name") == "--json" for f in e.get("flags", [])) else 1
    where, code, exact = locate(words, arms, funcs)
    if where is None:
        return [name, str(forms), "-", "none", "none", "out:no-arm", "-"]
    text = "\n".join(l for l in gather(code, funcs, where[0]) if not l.lstrip().startswith("#"))
    via = [v for v, pat in VIAS if re.search(pat, text)]
    output = [o for o, pat in OUTPUTS if re.search(pat, text)]
    if "table" in output or "body" in output or "text" in output:
        output = [o for o in output if o != "python" or not re.search(SHARED_PRINTERS, text)]
    requests = sum(len(re.findall(pat, text)) for v, pat in VIAS)
    kind = None
    if name in spec_names:
        kind = "spec"
    elif not via:
        kind = "out:no-request"
    elif words[0] in PY_CLIENT_NOUNS or "python-client" in via:
        kind = "out:python-client"
    else:
        tool = tool_id(words[0], words[1] if len(words) > 1 else "")
        if tool_mode(tool) == "open":
            kind = "out:open-tool"
    if kind is None:
        for why, pat in OUTS:
            if re.search(pat, text):
                kind = "out:" + why
                break
    if kind is None and requests > 1:
        kind = "out:many-requests"
    if kind is None:
        kind = "candidate"
    tier = "-"
    if kind in ("candidate", "spec"):
        if "python" in output and "table" not in output:
            tier = "3"
        elif "table" in output:
            tier = "2"
        else:
            tier = "1"
    w = "cli/src/%s:%d%s" % (where[0], where[1], "" if exact else "~")
    return [name, str(forms), w, ",".join(via) or "none", ",".join(output) or "none", kind, tier]


_TOOLS = {}


def tool_id(verb, sub):
    return _TOOLS["id"][(verb, sub)]


def tool_mode(tool):
    return _TOOLS["mode"][tool]


def main():
    check = sys.argv[1:] == ["--check"]
    if sys.argv[1:] not in ([], ["--check"]):
        print("spec-census: unknown arguments: %s" % " ".join(sys.argv[1:]), file=sys.stderr)
        return 2
    order = parts()
    ents = entries(order)
    funcs = functions(order)
    arms = router_arms()
    spec_names = {e["name"] for e in ents if e.get("impl") == "spec"}
    pairs = sorted({(e["name"].split(" ")[0], (e["name"].split(" ") + [""])[1]) for e in ents})
    _TOOLS.update(shell_tools(pairs))
    rows = [row(e, arms, funcs, spec_names) for e in ents]
    head = ["entry", "forms", "where", "via", "output", "kind", "tier"]
    lines = ["\t".join(head)] + ["\t".join(r) for r in rows]
    counts = {}
    for r in rows:
        k = r[5] if r[6] == "-" else "%s tier %s" % (r[5], r[6])
        c = counts.setdefault(k, [0, 0])
        c[0] += 1
        c[1] += int(r[1])
    total = [len(rows), sum(int(r[1]) for r in rows)]
    summary = ["# %d entries, %d verb forms" % tuple(total)] + [
        "# %-28s %4d entries %5d forms" % (k, v[0], v[1]) for k, v in sorted(counts.items())
    ]
    text = "\n".join(["# GENERATED by scripts/spec-census.py from cli/src. Do not edit: run it again."] + summary + lines) + "\n"
    path = os.path.join(ROOT, OUT)
    current = open(path, encoding="utf-8").read() if os.path.isfile(path) else None
    if check:
        if current == text:
            print("spec-census: %s is current (%s)" % (OUT, summary[0][2:]))
            return 0
        print("spec-census: %s is stale: run python3 scripts/spec-census.py and commit it" % OUT, file=sys.stderr)
        return 1
    if current != text:
        with open(path, "w", encoding="utf-8") as f:
            f.write(text)
        print("spec-census: wrote %s (%s)" % (OUT, summary[0][2:]))
    else:
        print("spec-census: %s is already current" % OUT)
    return 0


def shell_tools(pairs):
    """Runs the shell's _cli_tool_id_for_verb and _cli_tool_default_mode
    over `pairs` (noun, verb) in one bash."""
    import subprocess

    with open(os.path.join(SRC, "cli-tools-policy.sh"), encoding="utf-8") as f:
        text = f.read()
    block = text[text.index("# BEGIN_CLI_TOOL_POLICY") : text.index("# END_CLI_TOOL_POLICY")]
    script = block + '\nwhile IFS=$\'\\t\' read -r v s; do t="$(_cli_tool_id_for_verb "$v" "$s")"; printf \'%s\\t%s\\t%s\\t%s\\n\' "$v" "$s" "$t" "$(_cli_tool_default_mode "$t")"; done\n'
    data = "".join("%s\t%s\n" % p for p in pairs)
    out = subprocess.run(["bash", "-c", script], input=data, capture_output=True, text=True, check=True).stdout
    ids, modes = {}, {}
    for l in out.split("\n"):
        if not l:
            continue
        v, s, t, m = l.split("\t")
        ids[(v, s)] = t
        modes[t] = m
    return {"id": ids, "mode": modes}


if __name__ == "__main__":
    sys.exit(main())
