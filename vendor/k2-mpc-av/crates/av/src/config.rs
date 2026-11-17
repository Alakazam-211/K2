//! The project's conventions, one `key = value` per line (`#` starts a comment; lists are
//! comma-separated). Every key is optional; a feature whose keys are absent finds nothing. An
//! unknown key, or a value of the wrong shape, is refused ([`crate::BAD_CONFIG`]), naming the
//! accepted keys.

use k2_mpc_cad_model::error::CadError;

use crate::BAD_CONFIG;

/// Every key the configuration takes.
pub const KEYS: [&str; 26] = [
    "device.tags",
    "device.letters",
    "device.digits",
    "boundary.layers",
    "boundary.ltypes",
    "cable.layers",
    "cable.blocks",
    "cable.label_tags",
    "cable.snap",
    "cable.label_within",
    "cable.label_letters",
    "cable.label_digits",
    "flag.out_blocks",
    "flag.in_blocks",
    "flag.blocks",
    "flag.id_tag",
    "flag.sheet_tag",
    "flag.label_tag",
    "flag.kind_tag",
    "title.block",
    "title.sheet_block",
    "title.sheet_tag",
    "title.name_tag",
    "title.rev_prefix",
    "title.date_prefix",
    "title.number_from",
];

/// A project's conventions. Names are compared without case.
#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    /// The attribute tags that hold a device's identifier.
    pub device_tags: Vec<String>,
    /// An identifier's shape: letters, a hyphen, digits; how many of each.
    pub letters: (usize, usize),
    pub digits: (usize, usize),
    /// Closed polylines on these layers, or drawn in these linetypes, are boundaries.
    pub boundary_layers: Vec<String>,
    pub boundary_ltypes: Vec<String>,
    /// Lines and polylines on these layers are cables.
    pub cable_layers: Vec<String>,
    /// References of these blocks are cables too: a straight run drawn as a block, its ends
    /// those of its block's long axis as placed.
    pub cable_blocks: Vec<String>,
    /// A cable block's attribute tags that can hold its label, in order: the first value of the
    /// label's shape labels it.
    pub cable_label_tags: Vec<String>,
    /// How near a cable's end must be to a device's insertion point to end at it.
    pub cable_snap: f64,
    /// How near a text must be to a cable to label it.
    pub label_within: f64,
    /// A label's shape, when either is given: letters, a hyphen, digits, how many of each (an
    /// absent count takes any number from one). Without either, any text labels a cable.
    pub label_letters: Option<(usize, usize)>,
    pub label_digits: Option<(usize, usize)>,
    /// Blocks that are outgoing and incoming off-sheet flags.
    pub flag_out_blocks: Vec<String>,
    pub flag_in_blocks: Vec<String>,
    /// Blocks that are off-sheet flags of one kind, with no direction: two of them with the
    /// same identifier are a pair.
    pub flag_blocks: Vec<String>,
    /// A flag's identifier tag, and the tag naming the sheet it points to.
    pub flag_id_tag: Option<String>,
    pub flag_sheet_tag: Option<String>,
    /// Flags that don't pair but each name where they lead: their label's tag (a device, a
    /// sheet, a place) and, optionally, their kind's tag. With a label tag, flags are read as
    /// labelled ends, never paired.
    pub flag_label_tag: Option<String>,
    pub flag_kind_tag: Option<String>,
    /// The title block, and its tags: the sheet's number, its name, the revision rows' prefix
    /// (`REV` reads `REV1`, `REV2`…). The title block may be on the sheet, or in a drawing of
    /// the set the sheet references.
    pub title_block: Option<String>,
    /// The block that holds the sheet's own number and name, when the title block doesn't (a
    /// title block shared by reference).
    /// (Several blocks, several tags: a sheet's number or name is the first configured tag with
    /// a value on its first such reference; references that disagree are a finding.)
    pub title_sheet_blocks: Vec<String>,
    pub title_sheet_tags: Vec<String>,
    pub title_name_tags: Vec<String>,
    pub rev_prefix: String,
    /// The revision rows' dates' prefix, paired with them by number (`DATE` reads `DATE2` with
    /// `REV2`).
    pub date_prefix: Option<String>,
    /// Where a sheet's number may come from, in order, the first that gives one: `blocks` (the
    /// sheet blocks), `layout` (its paper layout's name, when it is shaped as a sheet number),
    /// `file-stem` (a sheet number in its file's name).
    pub number_from: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            device_tags: Vec::new(),
            letters: (2, 4),
            digits: (2, 3),
            boundary_layers: Vec::new(),
            boundary_ltypes: Vec::new(),
            cable_layers: Vec::new(),
            cable_blocks: Vec::new(),
            cable_label_tags: Vec::new(),
            cable_snap: 1.0,
            label_within: 2.0,
            label_letters: None,
            label_digits: None,
            flag_out_blocks: Vec::new(),
            flag_in_blocks: Vec::new(),
            flag_blocks: Vec::new(),
            flag_id_tag: None,
            flag_sheet_tag: None,
            flag_label_tag: None,
            flag_kind_tag: None,
            title_block: None,
            title_sheet_blocks: Vec::new(),
            title_sheet_tags: Vec::new(),
            title_name_tags: Vec::new(),
            rev_prefix: "REV".into(),
            date_prefix: None,
            number_from: vec!["blocks".into()],
        }
    }
}

fn bad(why: String) -> CadError {
    CadError::new(BAD_CONFIG, why, format!("the keys: {}", KEYS.join(", ")))
}

fn list(v: &str) -> Vec<String> {
    v.split(',')
        .map(|x| x.trim().to_owned())
        .filter(|x| !x.is_empty())
        .collect()
}

fn range(key: &str, v: &str) -> Result<(usize, usize), CadError> {
    let (a, b) = v.split_once('-').unwrap_or((v, v));
    match (a.trim().parse::<usize>(), b.trim().parse::<usize>()) {
        (Ok(a), Ok(b)) if a >= 1 && a <= b => Ok((a, b)),
        _ => Err(bad(format!("{key} takes a count or a range, as 2-4"))),
    }
}

fn distance(key: &str, v: &str) -> Result<f64, CadError> {
    v.trim()
        .parse::<f64>()
        .ok()
        .filter(|d| d.is_finite() && *d >= 0.0)
        .ok_or_else(|| bad(format!("{key} takes a distance, a number from 0")))
}

impl Config {
    /// The configuration `text` gives.
    pub fn parse(text: &str) -> Result<Config, CadError> {
        let mut c = Config::default();
        let mut seen: Vec<&str> = Vec::new();
        for (n, line) in text.lines().enumerate() {
            let line = line.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            let Some((k, v)) = line.split_once('=') else {
                return Err(bad(format!("line {}: not key = value", n + 1)));
            };
            let (k, v) = (k.trim(), v.trim());
            let Some(&key) = KEYS
                .iter()
                .find(|x| **x == k || k2_mpc_cad_model::mutant::active("UnknownKeyAccepted"))
            else {
                return Err(bad(format!("line {}: no key {k:?}", n + 1)));
            };
            if key != k {
                continue; // the planted bug: an unknown key passed over
            }
            if seen.contains(&key) {
                return Err(bad(format!("line {}: {key} given twice", n + 1)));
            }
            seen.push(key);
            let one = || Some(v.to_owned()).filter(|x| !x.is_empty());
            match key {
                "device.tags" => c.device_tags = list(v),
                "device.letters" => c.letters = range(key, v)?,
                "device.digits" => c.digits = range(key, v)?,
                "boundary.layers" => c.boundary_layers = list(v),
                "boundary.ltypes" => c.boundary_ltypes = list(v),
                "cable.layers" => c.cable_layers = list(v),
                "cable.blocks" => c.cable_blocks = list(v),
                "cable.label_tags" => c.cable_label_tags = list(v),
                "cable.snap" => c.cable_snap = distance(key, v)?,
                "cable.label_within" => c.label_within = distance(key, v)?,
                "cable.label_letters" => c.label_letters = Some(range(key, v)?),
                "cable.label_digits" => c.label_digits = Some(range(key, v)?),
                "flag.out_blocks" => c.flag_out_blocks = list(v),
                "flag.in_blocks" => c.flag_in_blocks = list(v),
                "flag.blocks" => c.flag_blocks = list(v),
                "flag.id_tag" => c.flag_id_tag = one(),
                "flag.sheet_tag" => c.flag_sheet_tag = one(),
                "title.block" => c.title_block = one(),
                "title.sheet_block" => c.title_sheet_blocks = list(v),
                "title.sheet_tag" => c.title_sheet_tags = list(v),
                "title.name_tag" => c.title_name_tags = list(v),
                "title.date_prefix" => c.date_prefix = one(),
                "title.number_from" => {
                    let l = list(v);
                    if l.is_empty()
                        || l.iter()
                            .any(|w| !["blocks", "layout", "file-stem"].contains(&w.as_str()))
                    {
                        return Err(bad(format!(
                            "line {}: title.number_from takes blocks, layout, file-stem",
                            n + 1
                        )));
                    }
                    c.number_from = l;
                }
                "flag.label_tag" => c.flag_label_tag = one(),
                "flag.kind_tag" => c.flag_kind_tag = one(),
                _ => c.rev_prefix = v.to_owned(),
            }
        }
        Ok(c)
    }

    /// Whether `v` has a device identifier's shape: letters, a hyphen, digits.
    pub fn is_device_id(&self, v: &str) -> bool {
        shaped(v, self.letters, self.digits)
    }

    /// Whether `v` can label a cable: any text, or one of the configured label shape.
    pub fn is_label(&self, v: &str) -> bool {
        if self.label_letters.is_none() && self.label_digits.is_none() {
            return true;
        }
        let any = (1, usize::MAX);
        shaped(
            v.trim(),
            self.label_letters.unwrap_or(any),
            self.label_digits.unwrap_or(any),
        )
    }
}

/// Whether `v` is uppercase letters, a hyphen and digits, with as many of each as the ranges say.
fn shaped(v: &str, letters: (usize, usize), digits: (usize, usize)) -> bool {
    let Some((l, d)) = v.split_once('-') else {
        return false;
    };
    let in_range = |n: usize, (a, b): (usize, usize)| n >= a && n <= b;
    in_range(l.len(), letters)
        && in_range(d.len(), digits)
        && l.bytes().all(|b| b.is_ascii_uppercase())
        && d.bytes().all(|b| b.is_ascii_digit())
}

/// Whether `name` is one of `list`, without case.
pub fn among(list: &[String], name: &str) -> bool {
    list.iter().any(|x| x.eq_ignore_ascii_case(name))
}
