//! The body `k2 --schema` prints after its version line, put together
//! from the same parts `scripts/build-cli.sh` joins into `cli/k2`: the end
//! of `cli/src/schema-head.sh` (after `cmd_schema`'s `cat <<'EOF'`), each
//! `cli/src/schema/<noun>.entries` in `cli/src/ORDER`'s order with its
//! spec keys left out ([`strip_spec_keys`], as the join leaves them out),
//! and `cli/src/schema-tail.sh` up to its `EOF` line.
//!
//! `build.rs` runs [`body`] over the files on disk and embeds the result;
//! `tests/schema_parity.rs` checks it is the heredoc in `cli/k2` byte for
//! byte. No Python, no JSON parse: the join is plain text, as the
//! shell's is. `scripts/build-cli.sh` has already refused a part the join
//! could not take.

/// The line that opens `cmd_schema`'s heredoc in `schema-head.sh`.
pub const OPEN: &str = "    cat <<'EOF'";
/// The line that closes it in `schema-tail.sh`.
pub const CLOSE: &str = "EOF";
/// The keys that say how the Rust CLI answers an entry (R4). The join
/// leaves them out of what `--schema` prints.
pub const SPEC_KEYS: &[&str] = &["impl", "parse", "call", "output", "exits", "auth"];

/// The parts `ORDER` (the text of `cli/src/ORDER`) lists, in its order:
/// a line that is empty or starts with `#` is not a part.
pub fn parts(order: &str) -> Vec<&str> {
    order
        .lines()
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .collect()
}

/// Whether `line` opens an entry's spec keys: six spaces, then one of
/// [`SPEC_KEYS`] quoted, a colon and a space.
fn spec_line(line: &str) -> bool {
    line.strip_prefix("      \"").is_some_and(|rest| {
        SPEC_KEYS
            .iter()
            .any(|k| rest.strip_prefix(k).is_some_and(|r| r.starts_with("\": ")))
    })
}

/// `strip_spec_keys` of `scripts/build-cli.sh`: each entry's lines from
/// its first spec key up to the entry's closing `    },` go, and so does
/// the comma before them. `None` where the shell's join refuses the file:
/// no closing line, or no comma to take off.
pub fn strip_spec_keys(text: &str) -> Option<String> {
    let lines: Vec<&str> = text.split('\n').collect();
    let mut kept: Vec<String> = Vec::with_capacity(lines.len());
    let mut i = 0;
    while i < lines.len() {
        if !spec_line(lines[i]) {
            kept.push(lines[i].to_string());
            i += 1;
            continue;
        }
        let mut j = i;
        while j < lines.len() && lines[j] != "    }," {
            j += 1;
        }
        let last = kept.last_mut()?;
        if j == lines.len() || !last.ends_with(',') {
            return None;
        }
        last.pop();
        i = j;
    }
    Some(kept.join("\n"))
}

/// The heredoc's body, from `order` (the text of `cli/src/ORDER`) and
/// `read`, which gives a part's text by its name under `cli/src`. An
/// error says what is wrong with the parts.
pub fn body(order: &str, read: impl Fn(&str) -> Result<String, String>) -> Result<String, String> {
    let names = parts(order);
    let at = |n: &str| {
        names
            .iter()
            .position(|p| *p == n)
            .ok_or_else(|| format!("cli/src/ORDER does not list {n}"))
    };
    let (head_at, tail_at) = (at("schema-head.sh")?, at("schema-tail.sh")?);
    if tail_at < head_at {
        return Err("cli/src/ORDER lists schema-tail.sh before schema-head.sh".into());
    }
    let head = read("schema-head.sh")?;
    let start = head
        .split_inclusive('\n')
        .scan(0, |off, l| {
            *off += l.len();
            Some((*off, l))
        })
        .find(|(_, l)| l.strip_suffix('\n') == Some(OPEN))
        .map(|(off, _)| off)
        .ok_or_else(|| format!("schema-head.sh has no line {OPEN:?}"))?;
    let mut out = head[start..].to_string();
    for name in &names[head_at + 1..tail_at] {
        if !name.starts_with("schema/") || !name.ends_with(".entries") {
            return Err(format!(
                "cli/src/ORDER lists {name} between schema-head.sh and schema-tail.sh"
            ));
        }
        let text = read(name)?;
        let text = strip_spec_keys(&text)
            .ok_or_else(|| format!("cli/src/{name}: spec keys the join can't leave out"))?;
        out.push_str(&text);
    }
    let tail = read("schema-tail.sh")?;
    let mut end = None;
    let mut off = 0;
    for l in tail.split_inclusive('\n') {
        if l.strip_suffix('\n').unwrap_or(l) == CLOSE {
            end = Some(off);
            break;
        }
        off += l.len();
    }
    let end = end.ok_or_else(|| format!("schema-tail.sh has no line {CLOSE:?}"))?;
    out.push_str(&tail[..end]);
    Ok(out)
}
