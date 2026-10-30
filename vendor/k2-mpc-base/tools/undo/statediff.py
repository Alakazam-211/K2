#!/usr/bin/env python3
"""Compare TLC's state set (a `-dump` file of an `undo` instance) with undo_enum's (a
`bfs --dump` file of canonical JSON lines, bounded::View::json) exactly: print how many states
each side has that the other hasn't, and the first few of each.

Usage: statediff.py <tlc dump> <enum dump> [PIDS] [RESTARTS]"""
import json
import re
import sys


class P:
    def __init__(self, s):
        self.s, self.i = s, 0

    def ws(self):
        while self.i < len(self.s) and self.s[self.i] in " \t\r\n":
            self.i += 1

    def eat(self, t):
        self.ws()
        if not self.s.startswith(t, self.i):
            raise ValueError("%r expected at %d: %r" % (t, self.i, self.s[self.i:self.i + 40]))
        self.i += len(t)

    def peek(self, t):
        self.ws()
        return self.s.startswith(t, self.i)

    def val(self):
        self.ws()
        s = self.s
        if s.startswith("<<", self.i):
            self.i += 2
            out = []
            if not self.peek(">>"):
                while True:
                    out.append(self.val())
                    if self.peek(","):
                        self.eat(",")
                        continue
                    break
            self.eat(">>")
            return out
        c = s[self.i]
        if c == "[":
            self.i += 1
            out = {}
            if not self.peek("]"):
                while True:
                    self.ws()
                    m = re.compile(r"[A-Za-z_][A-Za-z0-9_]*").match(s, self.i)
                    name = m.group(0)
                    self.i = m.end()
                    self.eat("|->")
                    out[name] = self.val()
                    if self.peek(","):
                        self.eat(",")
                        continue
                    break
            self.eat("]")
            return out
        if c == "{":
            self.i += 1
            out = []
            if not self.peek("}"):
                while True:
                    out.append(self.val())
                    if self.peek(","):
                        self.eat(",")
                        continue
                    break
            self.eat("}")
            return ("set", out)
        if c == "(":
            self.i += 1
            out = {}
            while True:
                k = self.val()
                self.eat(":>")
                out[k] = self.val()
                if self.peek("@@"):
                    self.eat("@@")
                    continue
                break
            self.eat(")")
            return ("fun", out)
        if c == '"':
            j = s.index('"', self.i + 1)
            v = s[self.i + 1:j]
            self.i = j + 1
            return v
        if s.startswith("TRUE", self.i):
            self.i += 4
            return True
        if s.startswith("FALSE", self.i):
            self.i += 5
            return False
        m = re.compile(r"-?[0-9]+").match(s, self.i)
        if not m:
            raise ValueError("a value at %d: %r" % (self.i, s[self.i:self.i + 40]))
        self.i = m.end()
        return int(m.group(0))


def fun(v):
    """A function value: a dict (TLC prints a function on 1..n as a sequence)."""
    if isinstance(v, tuple) and v[0] == "fun":
        return v[1]
    if isinstance(v, list):
        return {i + 1: x for i, x in enumerate(v)}
    if isinstance(v, dict) and not v:
        return {}
    raise ValueError("not a function: %r" % (v,))


def letters(v):
    return "".join("ab"[c] for c in v)


def hunks(hs):
    return [[h["s"], h["e"], letters(h["ins"])] for h in hs]


def region(r):
    return [r["start"], r["end"], letters(r["before"]), letters(r["after"]),
            hunks(r["ours"]), list(r["br"]), list(r["ar"]), r["tainted"], r["bdn"], r["erased"]]


def members(v):
    return v[1] if isinstance(v, tuple) else v


def canon(st, pids, track):
    stacks = fun(st["stacks"])
    events = fun(st["events"])
    records = fun(st["records"])
    viol = st["viol"][1] if isinstance(st["viol"], tuple) else st["viol"]
    v = [
        [["ab"[e["ch"]], e["au"], e["rt"]] for e in st["doc"]],
        [[[[region(r) for r in b] for b in stacks[p]["undo"]],
          [[region(r) for r in b] for b in stacks[p]["redo"]],
          stacks[p]["cleared"]] for p in range(1, pids + 1)],
        [[k, records[k]["author"], [region(r) for r in records[k]["regions"]],
          [[e["s"], e["e"], e["ts"], e["te"]] for e in records[k]["edges"]]] for k in sorted(records)],
        st["head"],
        [events[p] for p in range(1, pids + 1)],
        st["undos"],
        st["reverts"],
        st["restarts"],
        [p in members(st["authors"]) for p in range(1, pids + 1)] if track else [],
        [p in members(st["lost"]) for p in range(1, pids + 1)] if track else [],
        sorted(viol),
    ]
    return json.dumps(v, separators=(",", ":"))


def tlc_states(path, pids, track):
    text = open(path).read()
    out = set()
    for block in re.split(r"\nState \d+:\n", "\n" + text):
        m = re.search(r"_st = ", block)
        if not m:
            continue
        p = P(block)
        p.i = m.end()
        out.add(canon(p.val(), pids, track))
    return out


def main():
    tlc, enum = sys.argv[1], sys.argv[2]
    pids = int(sys.argv[3]) if len(sys.argv) > 3 else 2
    track = len(sys.argv) > 4 and int(sys.argv[4]) > 0
    a = tlc_states(tlc, pids, track)
    b = set(l.strip() for l in open(enum) if l.strip())
    print("statediff: tlc %d states, enum %d states; only tlc %d, only enum %d" % (len(a), len(b), len(a - b), len(b - a)))
    for s in sorted(a - b)[:4]:
        print("ONLY TLC  " + s)
    for s in sorted(b - a)[:4]:
        print("ONLY ENUM " + s)


main()
