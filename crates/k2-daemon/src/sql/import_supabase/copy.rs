//! Rows out of a dump's `COPY … FROM stdin;` block (PostgreSQL's text
//! format), and one-dimensional text arrays (`{"a","b c"}`).

/// One table's rows, by column name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CopyTable {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Option<String>>>,
}

impl CopyTable {
    pub fn col(&self, name: &str) -> Option<usize> {
        self.columns.iter().position(|c| c == name)
    }
    /// The value of `name` in `row` (`None` for SQL NULL or no such column).
    pub fn get<'a>(&self, row: &'a [Option<String>], name: &str) -> Option<&'a str> {
        self.col(name).and_then(|i| row.get(i)).and_then(|v| v.as_deref())
    }
}

/// Split a column list `(a, "B c", d)` into names.
fn columns(list: &str) -> Result<Vec<String>, String> {
    let inner = list.trim().strip_prefix('(').and_then(|x| x.strip_suffix(')')).ok_or_else(|| format!("COPY column list {list:?}"))?;
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    let mut chars = inner.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' if quoted && chars.peek() == Some(&'"') => {
                cur.push('"');
                chars.next();
            }
            '"' => quoted = !quoted,
            ',' if !quoted => {
                out.push(cur.trim().to_string());
                cur.clear();
            }
            c if !quoted && c == ' ' && cur.is_empty() => {}
            c => cur.push(c),
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur.trim().to_string());
    }
    Ok(out)
}

/// One field of the text format: `\N` is NULL; backslash escapes decoded.
pub fn decode_field(f: &str) -> Result<Option<String>, String> {
    if f == "\\N" {
        return Ok(None);
    }
    let b = f.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] != b'\\' {
            out.push(b[i]);
            i += 1;
            continue;
        }
        let Some(&n) = b.get(i + 1) else { return Err("a COPY field ends in a backslash".into()) };
        i += 2;
        match n {
            b'b' => out.push(8),
            b'f' => out.push(12),
            b'n' => out.push(b'\n'),
            b'r' => out.push(b'\r'),
            b't' => out.push(b'\t'),
            b'v' => out.push(11),
            b'x' => {
                let mut v: u32 = 0;
                let mut k = 0;
                while k < 2 && i < b.len() && b[i].is_ascii_hexdigit() {
                    v = v * 16 + (b[i] as char).to_digit(16).unwrap_or(0);
                    i += 1;
                    k += 1;
                }
                if k == 0 {
                    out.push(b'x');
                } else {
                    out.push(v as u8);
                }
            }
            b'0'..=b'7' => {
                let mut v: u32 = u32::from(n - b'0');
                let mut k = 1;
                while k < 3 && i < b.len() && (b'0'..=b'7').contains(&b[i]) {
                    v = v * 8 + u32::from(b[i] - b'0');
                    i += 1;
                    k += 1;
                }
                out.push((v & 0xff) as u8);
            }
            other => out.push(other),
        }
    }
    String::from_utf8(out).map(Some).map_err(|_| "a COPY field isn't UTF-8".into())
}

/// The rows of `COPY <qualified> (…) FROM stdin;` in a `pg_restore -f -`
/// script. `None` when the script has no such block. A dump made with
/// `--inserts` (INSERT statements instead of COPY) is refused.
pub fn parse_copy(script: &str, qualified: &str) -> Result<Option<CopyTable>, String> {
    let head = format!("COPY {qualified} ");
    let mut lines = script.lines();
    let cols = loop {
        let Some(l) = lines.next() else {
            if script.lines().any(|l| l.starts_with(&format!("INSERT INTO {qualified} "))) {
                return Err(format!("{qualified} is in the dump as INSERT statements: make the dump without --inserts"));
            }
            return Ok(None);
        };
        if let Some(rest) = l.strip_prefix(&head) {
            let list = rest.strip_suffix(" FROM stdin;").ok_or_else(|| format!("unreadable COPY line for {qualified}"))?;
            break columns(list)?;
        }
    };
    let mut rows = Vec::new();
    for l in lines {
        if l == "\\." {
            return Ok(Some(CopyTable { columns: cols, rows }));
        }
        let row: Vec<Option<String>> = l.split('\t').map(decode_field).collect::<Result<_, _>>()?;
        if row.len() != cols.len() {
            return Err(format!("a {qualified} row has {} fields for {} columns", row.len(), cols.len()));
        }
        rows.push(row);
    }
    Err(format!("the COPY block for {qualified} never ends"))
}

/// A one-dimensional text array literal (`{a,"b c",NULL}`).
pub fn parse_text_array(s: &str) -> Result<Vec<Option<String>>, String> {
    let inner = s.trim().strip_prefix('{').and_then(|x| x.strip_suffix('}')).ok_or_else(|| "not an array literal".to_string())?;
    let mut out = Vec::new();
    if inner.is_empty() {
        return Ok(out);
    }
    let b: Vec<char> = inner.chars().collect();
    let mut i = 0;
    loop {
        if b.get(i) == Some(&'"') {
            i += 1;
            let mut cur = String::new();
            loop {
                match b.get(i) {
                    None => return Err("an array element's quote never closes".into()),
                    Some('\\') => {
                        cur.push(*b.get(i + 1).ok_or("an array element ends in a backslash")?);
                        i += 2;
                    }
                    Some('"') => {
                        i += 1;
                        break;
                    }
                    Some(c) => {
                        cur.push(*c);
                        i += 1;
                    }
                }
            }
            out.push(Some(cur));
        } else {
            let start = i;
            while i < b.len() && b[i] != ',' {
                if b[i] == '{' || b[i] == '"' {
                    return Err("a nested or malformed array".into());
                }
                i += 1;
            }
            let raw: String = b[start..i].iter().collect();
            let raw = raw.trim();
            out.push(if raw.eq_ignore_ascii_case("NULL") { None } else { Some(raw.to_string()) });
        }
        match b.get(i) {
            None => break,
            Some(',') => i += 1,
            Some(_) => return Err("junk after an array element".into()),
        }
    }
    Ok(out)
}
