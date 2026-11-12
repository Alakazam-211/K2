//! Part names: keys, the relationships part of a part, target resolution, and minting a new
//! name in a part's own numbering (`image3.png` beside `image7.png` becomes `image8.png`).
//!
//! A part's **key** is its name case folded, without a leading `/`: OPC compares part names
//! ASCII case-insensitively, so two names that fold the same are one part (the zip reader
//! refuses a package holding both).

use std::collections::BTreeSet;

/// A part name's key: case folded, without a leading `/`.
pub fn key(name: &str) -> String {
    name.strip_prefix('/').unwrap_or(name).to_ascii_lowercase()
}

/// The key of `[Content_Types].xml`, which is not a part.
pub const CONTENT_TYPES: &str = "[content_types].xml";

/// The name of the relationships part for `source` (`""` is the package itself).
pub fn rels_part_for(source: &str) -> String {
    let source = source.strip_prefix('/').unwrap_or(source);
    match source.rsplit_once('/') {
        Some((dir, file)) => format!("{dir}/_rels/{file}.rels"),
        None => format!("_rels/{source}.rels"),
    }
}

/// The source part whose relationships `rels` holds (`Some("")`: the package's own), or
/// `None` when `rels` isn't a relationships part's name.
pub fn source_of_rels(rels: &str) -> Option<String> {
    let rels = rels.strip_prefix('/').unwrap_or(rels);
    let (dir, file) = rels.rsplit_once('/')?;
    let cut = file.len().checked_sub(5)?;
    if !file.as_bytes()[cut..].eq_ignore_ascii_case(b".rels") {
        return None;
    }
    let file = &file[..cut];
    let parent = match dir.rsplit_once('/') {
        Some((p, last)) if last.eq_ignore_ascii_case("_rels") => Some(p),
        None if dir.eq_ignore_ascii_case("_rels") => None,
        _ => return None,
    };
    match (parent, file.is_empty()) {
        (None, true) => Some(String::new()),
        (None, false) => Some(file.to_owned()),
        (Some(_), true) => None,
        (Some(p), false) => Some(format!("{p}/{file}")),
    }
}

/// Whether a key names a relationships part.
pub fn is_rels(key: &str) -> bool {
    source_of_rels(key).is_some()
}

/// The part a relative or absolute target names, from `source` (`""`: the package), with `.`
/// and `..` segments resolved and any fragment dropped; `None` when it escapes the package or
/// names nothing. Not case folded.
pub fn resolve(source: &str, target: &str) -> Option<String> {
    let target = target.split('#').next().unwrap_or("");
    if target.is_empty() || target.contains("://") || target.contains('\\') {
        return None;
    }
    let mut segs: Vec<&str> = Vec::new();
    let path = if let Some(abs) = target.strip_prefix('/') {
        abs.to_owned()
    } else {
        let source = source.strip_prefix('/').unwrap_or(source);
        match source.rsplit_once('/') {
            Some((dir, _)) => format!("{dir}/{target}"),
            None => target.to_owned(),
        }
    };
    for s in path.split('/') {
        match s {
            "" | "." => {}
            ".." => {
                segs.pop()?;
            }
            s => segs.push(s),
        }
    }
    if segs.is_empty() {
        None
    } else {
        Some(segs.join("/"))
    }
}

/// `name` split as (folder with its `/`, stem without its trailing number, the number, the
/// extension with its dot).
fn numbered(name: &str) -> (&str, &str, Option<u64>, &str) {
    let (dir, file) = match name.rfind('/') {
        Some(i) => name.split_at(i + 1),
        None => ("", name),
    };
    let (stem, ext) = match file.rfind('.') {
        Some(i) if i > 0 => file.split_at(i),
        _ => (file, ""),
    };
    let digits = stem.len() - stem.trim_end_matches(|c: char| c.is_ascii_digit()).len();
    let (prefix, num) = stem.split_at(stem.len() - digits);
    (dir, prefix, num.parse().ok(), ext)
}

/// A new name for `name` in its own numbering, free among `taken` (keys): one more than the
/// highest number any taken name in the same folder with the same stem and extension has
/// (`image3.png`, with `image7.png` taken, gives `image8.png`; `notes.xml` gives `notes2.xml`).
/// The folder and the stem keep `name`'s case. A pure function of its inputs.
pub fn mint(name: &str, taken: &BTreeSet<String>) -> String {
    let (dir, prefix, own, ext) = numbered(name);
    let fold = |s: &str| s.to_ascii_lowercase();
    let mut high = own.unwrap_or(1);
    for t in taken {
        let (d, p, n, e) = numbered(t);
        if fold(d) == fold(dir) && fold(p) == fold(prefix) && fold(e) == fold(ext) {
            high = high.max(n.unwrap_or(1));
        }
    }
    let mut n = high.saturating_add(1);
    loop {
        let candidate = format!("{dir}{prefix}{n}{ext}");
        if !taken.contains(&key(&candidate)) {
            return candidate;
        }
        n = n.saturating_add(1);
    }
}

/// A target with its last segment replaced (the folder part kept as written).
pub fn retarget(target: &str, new_file: &str) -> String {
    match target.rfind('/') {
        Some(i) => format!("{}{new_file}", &target[..=i]),
        None => new_file.to_owned(),
    }
}

/// The last segment of a name.
pub fn file_name(name: &str) -> &str {
    name.rsplit('/').next().unwrap_or(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rels_names_round_trip() {
        assert_eq!(rels_part_for(""), "_rels/.rels");
        assert_eq!(
            rels_part_for("word/document.xml"),
            "word/_rels/document.xml.rels"
        );
        assert_eq!(rels_part_for("/a.xml"), "_rels/a.xml.rels");
        assert_eq!(source_of_rels("_rels/.rels").as_deref(), Some(""));
        assert_eq!(
            source_of_rels("word/_rels/document.xml.rels").as_deref(),
            Some("word/document.xml")
        );
        assert_eq!(source_of_rels("_rels/a.xml.rels").as_deref(), Some("a.xml"));
        assert_eq!(source_of_rels("word/document.xml"), None);
        assert_eq!(source_of_rels("word/_rels/.rels"), None);
        assert_eq!(source_of_rels("x/rels/a.rels"), None);
    }

    #[test]
    fn targets_resolve() {
        assert_eq!(
            resolve("word/document.xml", "media/image1.png").as_deref(),
            Some("word/media/image1.png")
        );
        assert_eq!(
            resolve("word/document.xml", "../customXml/item1.xml").as_deref(),
            Some("customXml/item1.xml")
        );
        assert_eq!(
            resolve("", "word/document.xml").as_deref(),
            Some("word/document.xml")
        );
        assert_eq!(
            resolve("ppt/slides/slide1.xml", "/ppt/media/a.png#x").as_deref(),
            Some("ppt/media/a.png")
        );
        assert_eq!(resolve("a.xml", "../../x"), None);
        assert_eq!(resolve("a.xml", "https://example.invalid/x"), None);
    }

    #[test]
    fn minting_follows_the_numbering() {
        let taken: BTreeSet<String> = ["word/media/image3.png", "word/media/image7.png"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(
            mint("word/media/image3.png", &taken),
            "word/media/image8.png"
        );
        assert_eq!(
            mint("word/media/Image3.PNG", &taken),
            "word/media/Image8.PNG"
        );
        let taken: BTreeSet<String> = ["ppt/notes.xml".to_owned()].into_iter().collect();
        assert_eq!(mint("ppt/notes.xml", &taken), "ppt/notes2.xml");
        assert_eq!(
            retarget("../media/image3.png", "image8.png"),
            "../media/image8.png"
        );
        assert_eq!(retarget("image3.png", "image8.png"), "image8.png");
    }
}
