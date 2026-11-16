"""A test checker, never shipped: reads each public corpus file with its own small reader (it
shares no code with the pack) and prints what `crates/blend/tests/corpus.rs` compares, as
`file<TAB>row...` lines for `crates/blend/tests/corpus/expected.tsv`.

    python3 -I scripts/corpus-expected.py DIR > crates/blend/tests/corpus/expected.tsv

DIR holds the files `scripts/fetch-corpus.sh` fetched. Python 3.14 or later reads the zstd
files (`compression.zstd`); gzip is in every version.

Rows per file: its version, header form, storage and pointer size; one `id` row per data-block
(type code, name, library); one `object` row per object (data, parent, and its `loc` as the
float bits); one `holds` row per scene and collection (the objects and collections it holds).
Names are written as the pack's keys write them: whitespace, control characters, `%`, `/` and
bytes that aren't UTF-8 as %XX."""
import collections
import gzip
import os
import struct
import sys

UI = ('wmWindowManager', 'WorkSpace', 'bScreen')


def esc(raw):
    out = []
    i = 0
    while i < len(raw):
        # one UTF-8 character, or one byte that isn't one
        for n in (1, 2, 3, 4):
            try:
                ch = raw[i:i + n].decode('utf-8')
            except UnicodeDecodeError:
                continue
            break
        else:
            out.append('%%%02X' % raw[i])
            i += 1
            continue
        b = raw[i:i + n]
        if ch.isspace() or ch in '%/' or ord(ch) < 0x20 or 0x7f <= ord(ch) < 0xa0:
            out.extend('%%%02X' % x for x in b)
        else:
            out.append(ch)
        i += n
    return ''.join(out)


def decompress(data):
    if data[:2] == b'\x1f\x8b':
        return 'gzip', gzip.decompress(data)
    if data[:4] == b'\x28\xb5\x2f\xfd':
        from compression import zstd
        return 'zstd', zstd.decompress(data)
    return 'none', data


class Blend:
    def __init__(self, data):
        if data[7:8] in (b'-', b'_'):
            self.psize = 8 if data[7:8] == b'-' else 4
            self.e = '<' if data[8:9] == b'v' else '>'
            self.version = '%d.%02d' % (int(data[9:12]) // 100, int(data[9:12]) % 100)
            self.form, pos, large = 'classic', 12, False
        else:
            v = int(data[13:17])
            self.psize, self.e, pos, large = 8, '<', int(data[7:9]), True
            self.version = '%d.%02d' % (v // 100, v % 100)
            self.form = 'large'
        e, ps = self.e, self.psize
        P = 'Q' if ps == 8 else 'I'
        self.P = P
        self.blocks = []
        while True:
            code = data[pos:pos + 4]
            if large:
                sdna, old, ln, nr = struct.unpack(e + 'IQqq', data[pos + 4:pos + 32])
                hl = 32
            else:
                ln, = struct.unpack(e + 'i', data[pos + 4:pos + 8])
                old, = struct.unpack(e + P, data[pos + 8:pos + 8 + ps])
                sdna, nr = struct.unpack(e + 'ii', data[pos + 8 + ps:pos + 16 + ps])
                hl = 16 + ps
            self.blocks.append(dict(code=code, sdna=sdna, old=old, body=data[pos + hl:pos + hl + ln]))
            if code == b'ENDB':
                break
            pos += hl + ln
        self.schema()
        self.owner, cur = [], None
        self.multi = collections.defaultdict(list)
        for i, b in enumerate(self.blocks):
            if self.is_id(b):
                cur = i
            elif b['code'] != b'DATA':
                cur = None
            self.owner.append(cur)
            if b['old']:
                self.multi[b['old']].append(i)

    @staticmethod
    def is_id(b):
        c = b['code']
        return c[2:] == b'\0\0' and 65 <= c[0] <= 90 and 65 <= c[1] <= 90

    def schema(self):
        body = [b for b in self.blocks if b['code'] == b'DNA1'][0]['body']
        e, q = self.e, 4

        def strs():
            nonlocal q
            q += 4
            n, = struct.unpack(e + 'I', body[q:q + 4])
            q += 4
            out = []
            for _ in range(n):
                z = body.index(b'\0', q)
                out.append(body[q:z].decode('utf-8'))
                q = z + 1
            q = (q + 3) & ~3
            return out
        names, types = strs(), strs()
        q += 4
        tlen = struct.unpack(e + 'H' * len(types), body[q:q + 2 * len(types)])
        q = (q + 2 * len(types) + 3) & ~3
        q += 4
        ns, = struct.unpack(e + 'I', body[q:q + 4])
        q += 4
        self.structs = []
        for _ in range(ns):
            t, nf = struct.unpack(e + 'HH', body[q:q + 4])
            q += 4
            fs, off = {}, 0
            for i in range(nf):
                ft, fn = struct.unpack(e + 'HH', body[q + 4 * i:q + 4 * i + 4])
                nm = names[fn]
                ptr = nm.startswith('*') or nm.startswith('(*')
                size = self.psize if ptr else tlen[ft]
                for part in nm.split('[')[1:]:
                    size *= int(part.split(']')[0])
                bare = nm.lstrip('(').lstrip('*').split('[')[0].split(')')[0]
                fs[bare] = dict(type=types[ft], off=off, size=size, depth=len(nm.lstrip('(')) - len(nm.lstrip('(').lstrip('*')))
                off += size
            q += 4 * nf
            self.structs.append((types[t], fs))
        self.by_name = {s[0]: s for s in self.structs}

    def sname(self, i):
        return self.structs[self.blocks[i]['sdna']][0]

    def fields(self, i):
        return self.structs[self.blocks[i]['sdna']][1]

    def ptr_at(self, i, off):
        return struct.unpack(self.e + self.P, self.blocks[i]['body'][off:off + self.psize])[0]

    def ptr(self, i, name):
        f = self.fields(i).get(name)
        if f is None or f['depth'] != 1:
            return None
        return self.ptr_at(i, f['off'])

    def resolve(self, p, frm):
        if not p:
            return None
        v = self.multi.get(p, [])
        if len(v) == 1:
            return v[0]
        same = [j for j in v if self.owner[frm] is not None and self.owner[j] == self.owner[frm]]
        if len(same) == 1:
            return same[0]
        ids = [j for j in v if self.is_id(self.blocks[j])]
        if len(ids) == 1:
            return ids[0]
        return None

    def id_header(self, i):
        """(name bytes after the code, library pointer)"""
        if self.sname(i) == 'ID':
            base = 0
        else:
            base = self.fields(i)['id']['off']
        idf = self.by_name['ID'][1]
        raw = self.blocks[i]['body'][base + idf['name']['off']:base + idf['name']['off'] + idf['name']['size']]
        raw = raw.split(b'\0')[0][2:]
        return raw, self.ptr_at(i, base + idf['lib']['off'])

    def listbase(self, i, name):
        f = self.fields(i).get(name)
        if f is None:
            return []
        first = self.ptr_at(i, f['off'])
        out, seen = [], set()
        j = self.resolve(first, i)
        while j is not None and j not in seen:
            seen.add(j)
            fs = self.fields(j)
            t = None
            for n in ('ob', 'object', 'collection'):
                if n in fs:
                    t = self.resolve(self.ptr_at(j, fs[n]['off']), j)
                    break
            if t is not None:
                out.append(t)
            j = self.resolve(self.ptr_at(j, fs['next']['off']), j)
        return out


def main():
    d = sys.argv[1]
    for name in sorted(os.listdir(d)):
        if not name.endswith('.blend'):
            continue
        how, data = decompress(open(os.path.join(d, name), 'rb').read())
        bl = Blend(data)
        rows = [('version', bl.version), ('form', bl.form), ('compression', how),
                ('pointer_size', str(bl.psize))]
        ids = [i for i, b in enumerate(bl.blocks) if bl.is_id(b)]
        names = {}
        for i in ids:
            raw, lib = bl.id_header(i)
            names[i] = (bl.blocks[i]['code'][:2].decode(), raw, lib)

        def key(i):
            code, raw, lib = names[i]
            k = '%s:%s' % (code.lower(), esc(raw))
            lb = bl.resolve(lib, i)
            if lb is not None and lb in names:
                return '@li:%s/%s' % (esc(names[lb][1]), k)
            return '@' + k
        for i in ids:
            if bl.sname(i) in UI:
                continue
            rows.append(('id', key(i)))
            if bl.sname(i) == 'Object':
                data_ = bl.resolve(bl.ptr(i, 'data'), i)
                parent = bl.resolve(bl.ptr(i, 'parent'), i)
                lf = bl.fields(i)['loc']
                loc = struct.unpack(bl.e + 'III', bl.blocks[i]['body'][lf['off']:lf['off'] + 12])
                rows.append(('object', key(i),
                             'data=' + (key(data_) if data_ in names else '-'),
                             'parent=' + (key(parent) if parent in names else '-'),
                             'loc=' + ','.join('%08x' % x for x in loc)))
            src = None
            if bl.sname(i) == 'Scene':
                src = bl.resolve(bl.ptr(i, 'master_collection'), i)
                if src is None:
                    objs = bl.listbase(i, 'base')
                    rows.append(('holds', key(i), 'objects=' + ' '.join(key(o) for o in objs if o in names), 'collections='))
                    continue
            elif bl.sname(i) in ('Collection', 'Group'):
                src = i
            if src is not None:
                objs = bl.listbase(src, 'gobject')
                cols = bl.listbase(src, 'children')
                rows.append(('holds', key(i), 'objects=' + ' '.join(key(o) for o in objs if o in names),
                             'collections=' + ' '.join(key(c) for c in cols if c in names)))
        for r in rows:
            print('\t'.join((name,) + tuple(r)))


main()
