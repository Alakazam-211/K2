//! The file's own schema (the `DNA1` block): every struct the file's blocks hold, with each
//! field's name, type, size and offset. Nothing here is a table of ours: a reader looks a field
//! up by name in the schema of the file in hand, so a file from any version reads by what it says
//! of itself.
//!
//! The block's body: `SDNA`; `NAME`, a count and that many NUL-terminated field names; `TYPE`, a
//! count and that many type names; `TLEN`, a 2-byte length per type; `STRC`, a count and, per
//! struct, its type, its field count and a (type, name) pair per field. Each section starts on a
//! 4-byte boundary of the body.
//!
//! **A field's size comes from its name:** a name starting `*` (or `(*`, a function pointer) is a
//! pointer, the file's pointer size; otherwise the type's length. Each `[n]` multiplies. A field
//! whose type is a struct embeds it. The fields of every struct must add up to the struct's
//! length, and a struct may not embed itself: anything else is refused as malformed.

use std::collections::HashMap;

use crate::error::ReadError;

/// Most fields one struct may have, and most array elements a field may name.
const FIELDS_MAX: usize = 1 << 16;
/// Deepest nesting of embedded structs a layout walks.
pub const EMBED_DEPTH_MAX: usize = 32;

/// One field of a struct, as the schema gives it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Field {
    /// The name as written: `*next`, `name[66]`, `(*func)()`, `mat[4][4]`.
    pub raw_name: String,
    /// The name without pointer marks and dimensions: `next`, `name`, `func`, `mat`.
    pub name: String,
    /// Index into [`Sdna::type_name`].
    pub type_index: u16,
    /// From the start of the struct.
    pub offset: u32,
    /// In bytes, every element included.
    pub size: u32,
    /// How many `*` the name has (a function pointer counts one).
    pub pointer_depth: u8,
    /// Written `(*name)(...)`.
    pub function_pointer: bool,
    /// The product of its dimensions; 1 without any.
    pub array_len: u32,
}

impl Field {
    /// Whether the field holds pointers (of any depth, function pointers included).
    pub fn is_pointer(&self) -> bool {
        self.pointer_depth > 0
    }
}

/// One struct of the schema.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct StructDef {
    /// Its type, in [`Sdna::type_name`].
    pub type_index: u16,
    /// In bytes.
    pub size: u32,
    /// In order: each one's offset is the sum of the sizes before it.
    pub fields: Vec<Field>,
}

/// The schema of one file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sdna {
    pointer_size: u8,
    types: Vec<String>,
    type_len: Vec<u16>,
    structs: Vec<StructDef>,
    /// type index -> struct index
    struct_of_type: HashMap<u16, u32>,
    by_name: HashMap<String, u32>,
}

struct Cursor<'a> {
    b: &'a [u8],
    at: usize,
    big: bool,
}

impl<'a> Cursor<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], ReadError> {
        let end = self
            .at
            .checked_add(n)
            .filter(|&e| e <= self.b.len())
            .ok_or(ReadError::malformed("the schema block is cut short"))?;
        let s = &self.b[self.at..end];
        self.at = end;
        Ok(s)
    }
    fn tag(&mut self, t: &[u8; 4]) -> Result<(), ReadError> {
        if self.take(4)? != t {
            return Err(ReadError::malformed("the schema block lacks a section"));
        }
        Ok(())
    }
    fn u32(&mut self) -> Result<u32, ReadError> {
        let s: [u8; 4] = self.take(4)?.try_into().expect("4 bytes");
        Ok(if self.big {
            u32::from_be_bytes(s)
        } else {
            u32::from_le_bytes(s)
        })
    }
    fn u16(&mut self) -> Result<u16, ReadError> {
        let s: [u8; 2] = self.take(2)?.try_into().expect("2 bytes");
        Ok(if self.big {
            u16::from_be_bytes(s)
        } else {
            u16::from_le_bytes(s)
        })
    }
    fn align4(&mut self) -> Result<(), ReadError> {
        let pad = (4 - self.at % 4) % 4;
        self.take(pad).map(|_| ())
    }
    fn strings(&mut self) -> Result<Vec<String>, ReadError> {
        let n = self.u32()? as usize;
        if n > self.b.len().saturating_sub(self.at) {
            return Err(ReadError::malformed(
                "a schema count is past the block's end",
            ));
        }
        let mut out = Vec::with_capacity(n);
        for _ in 0..n {
            let rest = &self.b[self.at..];
            let len = rest
                .iter()
                .position(|&c| c == 0)
                .ok_or(ReadError::malformed("a schema name has no end"))?;
            let s = std::str::from_utf8(&rest[..len])
                .map_err(|_| ReadError::malformed("a schema name isn't UTF-8"))?;
            out.push(s.to_owned());
            self.at += len + 1;
        }
        self.align4()?;
        Ok(out)
    }
}

/// What a field's written name says: (bare name, pointer depth, function pointer, element count).
fn parse_name(raw: &str) -> Option<(String, u8, bool, u32)> {
    let mut s = raw;
    let mut paren = false;
    if let Some(r) = s.strip_prefix('(') {
        paren = true;
        s = r;
    }
    let stars = s.bytes().take_while(|&b| b == b'*').count();
    s = &s[stars..];
    let ident_len = s
        .bytes()
        .take_while(|b| b.is_ascii_alphanumeric() || *b == b'_')
        .count();
    if ident_len == 0 {
        return None;
    }
    let name = s[..ident_len].to_owned();
    s = &s[ident_len..];
    let mut function = false;
    if paren {
        // `(*name)(...)`: a function pointer
        let r = s.strip_prefix(')')?;
        if !r.starts_with('(') || stars == 0 {
            return None;
        }
        let close = r.find(')')?;
        s = &r[close + 1..];
        function = true;
    }
    let mut count: u32 = 1;
    while let Some(r) = s.strip_prefix('[') {
        let close = r.find(']')?;
        let n: u32 = r[..close].parse().ok()?;
        count = count.checked_mul(n)?;
        s = &r[close + 1..];
    }
    if !s.is_empty() || stars > 8 || count as usize > FIELDS_MAX * 1024 {
        return None;
    }
    Some((name, stars as u8, function, count))
}

impl Sdna {
    /// Reads the schema block's body.
    pub fn parse(body: &[u8], pointer_size: u8, big_endian: bool) -> Result<Self, ReadError> {
        let mut c = Cursor {
            b: body,
            at: 0,
            big: big_endian,
        };
        c.tag(b"SDNA")?;
        c.tag(b"NAME")?;
        let names = c.strings()?;
        c.tag(b"TYPE")?;
        let types = c.strings()?;
        c.tag(b"TLEN")?;
        let mut type_len = Vec::with_capacity(types.len());
        for _ in 0..types.len() {
            type_len.push(c.u16()?);
        }
        c.align4()?;
        c.tag(b"STRC")?;
        let n = c.u32()? as usize;
        if n > body.len() / 4 {
            return Err(ReadError::malformed(
                "a schema count is past the block's end",
            ));
        }
        if types.len() > usize::from(u16::MAX) + 1 || names.len() > usize::from(u16::MAX) + 1 {
            return Err(ReadError::malformed("a schema has too many names"));
        }
        let mut raw: Vec<(u16, Vec<(u16, u16)>)> = Vec::with_capacity(n);
        let mut struct_of_type = HashMap::with_capacity(n);
        for i in 0..n {
            let t = c.u16()?;
            let nf = usize::from(c.u16()?);
            if usize::from(t) >= types.len() {
                return Err(ReadError::malformed("a struct's type is out of range"));
            }
            if struct_of_type.insert(t, i as u32).is_some() {
                return Err(ReadError::malformed("two structs share a type"));
            }
            let mut fs = Vec::with_capacity(nf);
            for _ in 0..nf {
                let ft = c.u16()?;
                let fname = c.u16()?;
                if usize::from(ft) >= types.len() || usize::from(fname) >= names.len() {
                    return Err(ReadError::malformed(
                        "a field's type or name is out of range",
                    ));
                }
                fs.push((ft, fname));
            }
            raw.push((t, fs));
        }
        let mut structs = Vec::with_capacity(n);
        let ps = u32::from(pointer_size);
        for (t, fs) in raw {
            let mut fields = Vec::with_capacity(fs.len());
            let mut off: u64 = 0;
            for (ft, fname) in fs {
                let raw_name = &names[usize::from(fname)];
                let (name, pointer_depth, function_pointer, array_len) = parse_name(raw_name)
                    .ok_or(ReadError::malformed("a field name doesn't parse"))?;
                let elem = if pointer_depth > 0 {
                    ps
                } else {
                    u32::from(type_len[usize::from(ft)])
                };
                let size = u64::from(elem) * u64::from(array_len);
                fields.push(Field {
                    raw_name: raw_name.clone(),
                    name,
                    type_index: ft,
                    offset: u32::try_from(off)
                        .map_err(|_| ReadError::malformed("a struct is too large"))?,
                    size: u32::try_from(size)
                        .map_err(|_| ReadError::malformed("a field is too large"))?,
                    pointer_depth,
                    function_pointer,
                    array_len,
                });
                off += size;
            }
            if off != u64::from(type_len[usize::from(t)]) {
                return Err(ReadError::malformed(
                    "a struct's fields don't add up to its length",
                ));
            }
            structs.push(StructDef {
                type_index: t,
                size: u32::from(type_len[usize::from(t)]),
                fields,
            });
        }
        let by_name = struct_of_type
            .iter()
            .map(|(&t, &s)| (types[usize::from(t)].clone(), s))
            .collect();
        let sdna = Sdna {
            pointer_size,
            types,
            type_len,
            structs,
            struct_of_type,
            by_name,
        };
        sdna.check_acyclic()?;
        Ok(sdna)
    }

    /// Refuses a struct that embeds itself, directly or through others, or nests deeper than
    /// [`EMBED_DEPTH_MAX`].
    fn check_acyclic(&self) -> Result<(), ReadError> {
        // 0 unvisited, 1 on the path, 2 done; depth per struct when done
        let mut state = vec![0u8; self.structs.len()];
        let mut depth = vec![0usize; self.structs.len()];
        for root in 0..self.structs.len() {
            if state[root] != 0 {
                continue;
            }
            // (struct, next field to look at)
            let mut stack: Vec<(usize, usize)> = vec![(root, 0)];
            state[root] = 1;
            while let Some(top) = stack.last_mut() {
                let (s, next) = *top;
                let fields = &self.structs[s].fields;
                if next < fields.len() {
                    top.1 += 1;
                    let f = &fields[next];
                    if f.is_pointer() {
                        continue;
                    }
                    let Some(&child) = self.struct_of_type.get(&f.type_index) else {
                        continue;
                    };
                    let child = child as usize;
                    match state[child] {
                        1 => return Err(ReadError::malformed("a struct embeds itself")),
                        2 => {}
                        _ => {
                            if stack.len() >= EMBED_DEPTH_MAX {
                                return Err(ReadError::malformed("structs nest too deep"));
                            }
                            state[child] = 1;
                            stack.push((child, 0));
                        }
                    }
                } else {
                    let d = fields
                        .iter()
                        .filter(|f| !f.is_pointer())
                        .filter_map(|f| self.struct_of_type.get(&f.type_index))
                        .map(|&c| depth[c as usize] + 1)
                        .max()
                        .unwrap_or(0);
                    if d > EMBED_DEPTH_MAX {
                        return Err(ReadError::malformed("structs nest too deep"));
                    }
                    depth[s] = d;
                    state[s] = 2;
                    stack.pop();
                }
            }
        }
        Ok(())
    }

    /// The file's pointer size, which every pointer field takes.
    pub fn pointer_size(&self) -> u8 {
        self.pointer_size
    }

    /// How many structs the schema has.
    pub fn struct_count(&self) -> usize {
        self.structs.len()
    }

    /// Struct `index`.
    pub fn struct_def(&self, index: u32) -> Option<&StructDef> {
        self.structs.get(index as usize)
    }

    /// The struct called `name`.
    pub fn struct_index(&self, name: &str) -> Option<u32> {
        self.by_name.get(name).copied()
    }

    /// The struct a type is, if it is one.
    pub fn struct_of_type(&self, type_index: u16) -> Option<u32> {
        self.struct_of_type.get(&type_index).copied()
    }

    /// The name of struct `index`.
    pub fn struct_name(&self, index: u32) -> Option<&str> {
        let s = self.structs.get(index as usize)?;
        Some(&self.types[usize::from(s.type_index)])
    }

    /// A type's name (`float`, `Object`); empty when out of range.
    pub fn type_name(&self, type_index: u16) -> &str {
        self.types
            .get(usize::from(type_index))
            .map_or("", String::as_str)
    }

    /// A type's length in bytes; 0 when out of range.
    pub fn type_len(&self, type_index: u16) -> u16 {
        self.type_len
            .get(usize::from(type_index))
            .copied()
            .unwrap_or(0)
    }

    /// A field of struct `index` by its bare name.
    pub fn field(&self, index: u32, name: &str) -> Option<&Field> {
        self.structs
            .get(index as usize)?
            .fields
            .iter()
            .find(|f| f.name == name)
    }

    /// The first of `names` that struct `index` has (a field renamed across versions).
    pub fn field_any(&self, index: u32, names: &[&str]) -> Option<&Field> {
        names.iter().find_map(|n| self.field(index, n))
    }

    /// A field reached through embedded structs: `id.name` is `name` of the `ID` that struct
    /// `index` embeds as `id`. The offset is from the start of struct `index`.
    pub fn field_path(&self, index: u32, path: &str) -> Option<(u32, &Field)> {
        let mut s = index;
        let mut base = 0u32;
        let mut parts = path.split('.').peekable();
        while let Some(p) = parts.next() {
            let f = self.field(s, p)?;
            if parts.peek().is_none() {
                return Some((base + f.offset, f));
            }
            if f.is_pointer() || f.array_len != 1 {
                return None;
            }
            s = self.struct_of_type(f.type_index)?;
            base += f.offset;
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn field_names_parse() {
        assert_eq!(parse_name("*next"), Some(("next".into(), 1, false, 1)));
        assert_eq!(parse_name("name[66]"), Some(("name".into(), 0, false, 66)));
        assert_eq!(parse_name("mat[4][4]"), Some(("mat".into(), 0, false, 16)));
        assert_eq!(parse_name("**mat"), Some(("mat".into(), 2, false, 1)));
        assert_eq!(parse_name("(*func)()"), Some(("func".into(), 1, true, 1)));
        assert_eq!(
            parse_name("(*cb)(void *, int)"),
            Some(("cb".into(), 1, true, 1))
        );
        assert_eq!(parse_name("*vec[3]"), Some(("vec".into(), 1, false, 3)));
        for bad in [
            "",
            "[3]",
            "a[x]",
            "a[3",
            "(*f)",
            "(g)()",
            "a b",
            "a[4294967296]",
        ] {
            assert_eq!(parse_name(bad), None, "{bad:?}");
        }
    }
}
