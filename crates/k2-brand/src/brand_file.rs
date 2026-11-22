//! Reads one brand file and writes the Rust that `lib.rs` includes.
//!
//! The build script uses this module (`#[path]` in `build.rs`), and so do
//! this crate's tests, on small brand files of their own. Every refusal
//! names the file and the field.
//!
//! What it reads today: `id`, `schema`, `[names] product` and the `[cli]`
//! table. Other tables are left for later steps.
//!
//! The tokens an install writes (the PATH block's two marker lines, the
//! fish file's name, the system-wide path) are made here, by
//! [`path_markers`], [`install_tokens`] and [`system_cli_path`], and nowhere
//! else: other crates take them as generated constants.
//!
//! The product's name and the command's name are two values a brand sets
//! apart; neither is made from the other. The PATH block's markers carry
//! the product's name (`# >>> <product> CLI >>>`, K2's ruling); the file
//! names and paths carry the command's. Builds before that ruling wrote the
//! markers with the command in upper case; when that form differs from
//! the product's, it is listed as an earlier form ([`earlier_markers`]),
//! so an install or an undo still finds and replaces such a block.

use std::path::Path;

/// The one schema this reader knows.
const SCHEMA: i64 = 1;

/// The keys `[cli]` may hold.
const CLI_KEYS: &[&str] = &["name", "aliases"];

/// The keys one alias may hold.
const ALIAS_KEYS: &[&str] = &["name", "mode"];

/// A brand file, checked.
#[derive(Debug)]
pub struct Brand {
    pub id: String,
    pub product: String,
    pub cli_name: String,
    pub aliases: Vec<Alias>,
}

/// Another name the command answers to.
#[derive(Debug)]
pub struct Alias {
    pub name: String,
    pub mode: Mode,
}

/// What an alias does. The same three as `AliasMode` in `lib.rs`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Silent,
    Notice,
    Tombstone,
}

impl Mode {
    fn from_word(word: &str) -> Option<Mode> {
        match word {
            "silent" => Some(Mode::Silent),
            "notice" => Some(Mode::Notice),
            "tombstone" => Some(Mode::Tombstone),
            _ => None,
        }
    }

    /// The brand is retiring this name: its installs left a PATH block and
    /// a fish file, which this brand's build takes over. A `silent` alias
    /// is a second name for the command and never had an install of its
    /// own.
    pub fn retires(self) -> bool {
        matches!(self, Mode::Notice | Mode::Tombstone)
    }

    fn variant(self) -> &'static str {
        match self {
            Mode::Silent => "Silent",
            Mode::Notice => "Notice",
            Mode::Tombstone => "Tombstone",
        }
    }
}

/// A command name or an alias: `^[a-z][a-z0-9]{1,11}$`.
pub fn is_command_name(s: &str) -> bool {
    let b = s.as_bytes();
    (2..=12).contains(&b.len())
        && b[0].is_ascii_lowercase()
        && b[1..]
            .iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
}

/// The folder the command is installed in system-wide.
pub const SYSTEM_BIN: &str = "/usr/local/bin";

/// What an install by one command name wrote before the PATH markers
/// carried the product's name: its PATH block's first and last lines, and
/// its fish startup file's name. A retired alias's old install, and this
/// command's own earlier form, are found by these.
#[derive(Debug, PartialEq, Eq)]
pub struct InstallTokens {
    pub block_begin: String,
    pub block_end: String,
    pub fish_conf: String,
}

/// The PATH block's first and last lines for the product called
/// `product`: `# >>> <product> CLI >>>` and `# <<< <product> CLI <<<`,
/// the product's name as the brand file spells it.
pub fn path_markers(product: &str) -> (String, String) {
    (
        format!("# >>> {product} CLI >>>"),
        format!("# <<< {product} CLI <<<"),
    )
}

/// The earlier marker pairs of a brand with product `product` and command
/// `name`: the command-form pair ([`install_tokens`]) when it is not the
/// product-form pair, else none. For K2 and K5 the two forms are the same
/// text, so there is none.
pub fn earlier_markers(product: &str, name: &str) -> Vec<(String, String)> {
    let old = install_tokens(name);
    let (begin, end) = path_markers(product);
    if (old.block_begin.as_str(), old.block_end.as_str()) == (begin.as_str(), end.as_str()) {
        Vec::new()
    } else {
        vec![(old.block_begin, old.block_end)]
    }
}

/// The install tokens a build by the command `name` wrote before the
/// markers carried the product's name: `# >>> <NAME> CLI >>>`,
/// `# <<< <NAME> CLI <<<` (the name in upper case) and `<name>-cli.fish`
/// (the fish file's name is still this). A name is ASCII
/// (`is_command_name`), so upper case is one byte each.
pub fn install_tokens(name: &str) -> InstallTokens {
    let upper = name.to_ascii_uppercase();
    InstallTokens {
        block_begin: format!("# >>> {upper} CLI >>>"),
        block_end: format!("# <<< {upper} CLI <<<"),
        fish_conf: format!("{name}-cli.fish"),
    }
}

/// `/usr/local/bin/<name>`.
pub fn system_cli_path(name: &str) -> String {
    format!("{SYSTEM_BIN}/{name}")
}

/// Reads and checks the brand file at `path`.
pub fn load(path: &Path) -> Result<Brand, String> {
    let shown = path.display();
    let text = std::fs::read_to_string(path).map_err(|e| {
        format!("brand file {shown}: can't read it ({e}); K2_BRAND_DIR names the folder that holds brand.toml")
    })?;
    parse(&text).map_err(|e| format!("brand file {shown}: {e}"))
}

fn parse(text: &str) -> Result<Brand, String> {
    let doc: toml::Table = toml::from_str(text).map_err(|e| format!("not valid TOML: {e}"))?;
    match doc.get("schema") {
        Some(toml::Value::Integer(SCHEMA)) => {}
        Some(v) => return Err(format!("schema must be {SCHEMA}, not {v}")),
        None => return Err(format!("schema is missing; it must be {SCHEMA}")),
    }
    let id = text_field(&doc, "id", "id")?;
    let product = text_field(table(&doc, "names")?, "product", "names.product")?;
    if product.chars().any(char::is_control) {
        return Err(format!(
            "names.product {product:?} must be one line with no control characters (it is in the PATH block's marker lines)"
        ));
    }
    let cli = table(&doc, "cli")?;
    if let Some(key) = cli.keys().find(|k| !CLI_KEYS.contains(&k.as_str())) {
        return Err(format!(
            "[cli] holds a key it does not know: {key} (it knows name and aliases)"
        ));
    }
    let cli_name = text_field(cli, "name", "cli.name")?;
    if !is_command_name(&cli_name) {
        return Err(format!(
            "cli.name {cli_name:?} must match ^[a-z][a-z0-9]{{1,11}}$"
        ));
    }
    let items = match cli.get("aliases") {
        None => &[][..],
        Some(toml::Value::Array(items)) => &items[..],
        Some(_) => return Err("cli.aliases must be a list of { name, mode } tables".into()),
    };
    let mut aliases: Vec<Alias> = Vec::new();
    for (i, item) in items.iter().enumerate() {
        let field = format!("cli.aliases[{i}]");
        let toml::Value::Table(t) = item else {
            return Err(format!(
                "{field} must be a table: {{ name = \"...\", mode = \"silent\" }}"
            ));
        };
        if let Some(key) = t.keys().find(|k| !ALIAS_KEYS.contains(&k.as_str())) {
            return Err(format!(
                "{field} holds a key it does not know: {key} (it knows name and mode)"
            ));
        }
        let name = text_field(t, "name", &format!("{field}.name"))?;
        if !is_command_name(&name) {
            return Err(format!(
                "{field}.name {name:?} must match ^[a-z][a-z0-9]{{1,11}}$"
            ));
        }
        if name == cli_name {
            return Err(format!("{field}.name {name:?} is cli.name itself"));
        }
        if aliases.iter().any(|a| a.name == name) {
            return Err(format!("{field}.name {name:?} is listed twice"));
        }
        let word = text_field(t, "mode", &format!("{field}.mode"))?;
        let mode = Mode::from_word(&word)
            .ok_or_else(|| format!("{field}.mode {word:?} must be silent, notice or tombstone"))?;
        aliases.push(Alias { name, mode });
    }
    Ok(Brand {
        id,
        product,
        cli_name,
        aliases,
    })
}

fn table<'a>(doc: &'a toml::Table, key: &str) -> Result<&'a toml::Table, String> {
    match doc.get(key) {
        Some(toml::Value::Table(t)) => Ok(t),
        Some(_) => Err(format!("[{key}] must be a table")),
        None => Err(format!("[{key}] is missing")),
    }
}

fn text_field(t: &toml::Table, key: &str, field: &str) -> Result<String, String> {
    match t.get(key) {
        Some(toml::Value::String(s)) if !s.is_empty() => Ok(s.clone()),
        Some(toml::Value::String(_)) => Err(format!("{field} is empty")),
        Some(_) => Err(format!("{field} must be a string")),
        None => Err(format!("{field} is missing")),
    }
}

/// The Rust `lib.rs` includes. Strings are written as Rust literals
/// (`{:?}` escapes quotes, backslashes and control characters).
pub fn render(b: &Brand) -> String {
    let names: Vec<String> = b.aliases.iter().map(|a| format!("{:?}", a.name)).collect();
    let own = install_tokens(&b.cli_name);
    let (begin, end) = path_markers(&b.product);
    let earlier: Vec<String> = earlier_markers(&b.product, &b.cli_name)
        .iter()
        .map(|(begin, end)| format!("({begin:?}, {end:?})"))
        .collect();
    let retired: Vec<String> = b
        .aliases
        .iter()
        .filter(|a| a.mode.retires())
        .map(|a| {
            let t = install_tokens(&a.name);
            format!(
                "RetiredName {{ name: {:?}, block_begin: {:?}, block_end: {:?}, fish_conf: {:?} }}",
                a.name, t.block_begin, t.block_end, t.fish_conf
            )
        })
        .collect();
    let modes: Vec<String> = b
        .aliases
        .iter()
        .map(|a| format!("({:?}, AliasMode::{})", a.name, a.mode.variant()))
        .collect();
    format!(
        "// Written by k2-brand's build script from the brand file. Do not edit.

/// The product's name as a literal, for `concat!` in a `const`.
#[macro_export]
macro_rules! product_name {{
    () => {{
        {product:?}
    }};
}}

/// The command's name as a literal, for `concat!` in a `const`.
#[macro_export]
macro_rules! cli_name {{
    () => {{
        {cli:?}
    }};
}}

/// The product's name, as people read it.
pub const PRODUCT_NAME: &str = product_name!();

/// The command people and agents type.
pub const CLI_NAME: &str = cli_name!();

/// The brand's id: its folder under `brand/`.
pub const BRAND_ID: &str = {id:?};

/// Other names the command answers to.
pub const CLI_ALIASES: &[&str] = &[{names}];

/// The same names, in the same order, each with what it does.
pub const CLI_ALIAS_MODES: &[(&str, AliasMode)] = &[{modes}];

/// First line of the PATH block an install writes into a shell startup
/// file: the product's name.
pub const PATH_BLOCK_BEGIN: &str = {begin:?};

/// Last line of the PATH block.
pub const PATH_BLOCK_END: &str = {end:?};

/// Earlier forms of this build's own PATH block markers (begin, end): the
/// command in upper case, when that is not the product's form. An install
/// replaces such a block in place, and the undo removes it.
pub const PATH_BLOCK_EARLIER: &[(&str, &str)] = &[{earlier}];

/// The fish startup file an install owns (the whole file is its block).
pub const FISH_CONF: &str = {fish:?};

/// The command's system-wide path: the Linux copy and the root helper's.
pub const SYSTEM_CLI_PATH: &str = {system:?};

/// The aliases this brand is retiring (mode notice or tombstone), with the
/// PATH block and fish file their installs left.
pub const RETIRED: &[RetiredName] = &[{retired}];
",
        product = b.product,
        cli = b.cli_name,
        id = b.id,
        names = names.join(", "),
        modes = modes.join(", "),
        begin = begin,
        end = end,
        earlier = earlier.join(", "),
        fish = own.fish_conf,
        system = system_cli_path(&b.cli_name),
        retired = retired.join(", "),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// A brand file in a folder of its own under the temp dir, removed
    /// when dropped.
    struct TempBrand {
        dir: PathBuf,
    }

    impl TempBrand {
        fn new(tag: &str, text: Option<&str>) -> TempBrand {
            let dir =
                std::env::temp_dir().join(format!("k2-brand-test-{}-{tag}", std::process::id()));
            if dir.exists() {
                std::fs::remove_dir_all(&dir).expect("clear an old test folder");
            }
            std::fs::create_dir_all(&dir).expect("make the test folder");
            if let Some(text) = text {
                std::fs::write(dir.join("brand.toml"), text).expect("write the test brand file");
            }
            TempBrand { dir }
        }

        fn file(&self) -> PathBuf {
            self.dir.join("brand.toml")
        }
    }

    impl Drop for TempBrand {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    const GOOD: &str = r#"
id = "ab"
schema = 1
[names]
product = "AB"
[cli]
name = "ab"
aliases = [{ name = "abc", mode = "notice" }, { name = "ab1", mode = "silent" }, { name = "ab2", mode = "tombstone" }]
"#;

    /// `GOOD` with `from` replaced by `to` (once; it must be there).
    fn good_with(from: &str, to: &str) -> String {
        assert!(GOOD.contains(from), "the good file holds {from:?}");
        GOOD.replacen(from, to, 1)
    }

    /// Loading `text` fails, and the message names the file and holds `want`.
    fn refused(tag: &str, text: &str, want: &str) {
        let t = TempBrand::new(tag, Some(text));
        let err = load(&t.file()).expect_err("the brand file is refused");
        assert!(
            err.starts_with(&format!("brand file {}: ", t.file().display())),
            "names the file: {err}"
        );
        assert!(err.contains(want), "{want:?} in: {err}");
    }

    #[test]
    fn a_good_file_gives_the_expected_text() {
        let t = TempBrand::new("good", Some(GOOD));
        let text = render(&load(&t.file()).expect("the good file loads"));
        let want = r##"// Written by k2-brand's build script from the brand file. Do not edit.

/// The product's name as a literal, for `concat!` in a `const`.
#[macro_export]
macro_rules! product_name {
    () => {
        "AB"
    };
}

/// The command's name as a literal, for `concat!` in a `const`.
#[macro_export]
macro_rules! cli_name {
    () => {
        "ab"
    };
}

/// The product's name, as people read it.
pub const PRODUCT_NAME: &str = product_name!();

/// The command people and agents type.
pub const CLI_NAME: &str = cli_name!();

/// The brand's id: its folder under `brand/`.
pub const BRAND_ID: &str = "ab";

/// Other names the command answers to.
pub const CLI_ALIASES: &[&str] = &["abc", "ab1", "ab2"];

/// The same names, in the same order, each with what it does.
pub const CLI_ALIAS_MODES: &[(&str, AliasMode)] = &[("abc", AliasMode::Notice), ("ab1", AliasMode::Silent), ("ab2", AliasMode::Tombstone)];

/// First line of the PATH block an install writes into a shell startup
/// file: the product's name.
pub const PATH_BLOCK_BEGIN: &str = "# >>> AB CLI >>>";

/// Last line of the PATH block.
pub const PATH_BLOCK_END: &str = "# <<< AB CLI <<<";

/// Earlier forms of this build's own PATH block markers (begin, end): the
/// command in upper case, when that is not the product's form. An install
/// replaces such a block in place, and the undo removes it.
pub const PATH_BLOCK_EARLIER: &[(&str, &str)] = &[];

/// The fish startup file an install owns (the whole file is its block).
pub const FISH_CONF: &str = "ab-cli.fish";

/// The command's system-wide path: the Linux copy and the root helper's.
pub const SYSTEM_CLI_PATH: &str = "/usr/local/bin/ab";

/// The aliases this brand is retiring (mode notice or tombstone), with the
/// PATH block and fish file their installs left.
pub const RETIRED: &[RetiredName] = &[RetiredName { name: "abc", block_begin: "# >>> ABC CLI >>>", block_end: "# <<< ABC CLI <<<", fish_conf: "abc-cli.fish" }, RetiredName { name: "ab2", block_begin: "# >>> AB2 CLI >>>", block_end: "# <<< AB2 CLI <<<", fish_conf: "ab2-cli.fish" }];
"##;
        assert_eq!(text, want);
    }

    #[test]
    fn no_aliases_gives_empty_lists() {
        for (tag, aliases) in [("empty", "aliases = []\n"), ("absent", "")] {
            let text = format!("id = \"ab\"\nschema = 1\n[names]\nproduct = \"AB\"\n[cli]\nname = \"ab\"\n{aliases}");
            let t = TempBrand::new(tag, Some(&text));
            let out = render(&load(&t.file()).expect("loads"));
            assert!(
                out.contains("pub const CLI_ALIASES: &[&str] = &[];\n"),
                "{tag}: {out}"
            );
            assert!(
                out.contains("pub const CLI_ALIAS_MODES: &[(&str, AliasMode)] = &[];\n"),
                "{tag}: {out}"
            );
            assert!(
                out.contains("pub const RETIRED: &[RetiredName] = &[];\n"),
                "{tag}: {out}"
            );
        }
    }

    /// The default brand's install tokens are today's bytes: the lines in
    /// people's shell startup files, the fish file's name and the system
    /// path. A change to the generator that moves one fails here.
    #[test]
    fn the_default_brand_gives_todays_install_tokens() {
        let file = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../brand/k2/brand.toml");
        let out = render(&load(&file).expect("the default brand file loads"));
        for line in [
            "pub const PATH_BLOCK_BEGIN: &str = \"# >>> K2 CLI >>>\";\n",
            "pub const PATH_BLOCK_END: &str = \"# <<< K2 CLI <<<\";\n",
            "pub const FISH_CONF: &str = \"k2-cli.fish\";\n",
            "pub const SYSTEM_CLI_PATH: &str = \"/usr/local/bin/k2\";\n",
            // Its one alias is silent: nothing to take over.
            "pub const RETIRED: &[RetiredName] = &[];\n",
            // The product's form and the command's form are one text.
            "pub const PATH_BLOCK_EARLIER: &[(&str, &str)] = &[];\n",
        ] {
            assert!(out.contains(line), "{line:?} in:\n{out}");
        }
        assert_eq!(
            install_tokens("k2"),
            InstallTokens {
                block_begin: "# >>> K2 CLI >>>".into(),
                block_end: "# <<< K2 CLI <<<".into(),
                fish_conf: "k2-cli.fish".into(),
            }
        );
        assert_eq!(system_cli_path("k2"), "/usr/local/bin/k2");
    }

    /// A `notice` or `tombstone` alias is listed under RETIRED with its
    /// own tokens; a `silent` one is not.
    #[test]
    fn a_retiring_alias_is_listed_and_a_silent_one_is_not() {
        let text = "id = \"xy\"\nschema = 1\n[names]\nproduct = \"XY\"\n[cli]\nname = \"xy\"\n\
            aliases = [{ name = \"xyold\", mode = \"notice\" }, { name = \"xy2\", mode = \"silent\" }]\n";
        let t = TempBrand::new("retired", Some(text));
        let out = render(&load(&t.file()).expect("loads"));
        let want = "pub const RETIRED: &[RetiredName] = &[RetiredName { name: \"xyold\", \
            block_begin: \"# >>> XYOLD CLI >>>\", block_end: \"# <<< XYOLD CLI <<<\", \
            fish_conf: \"xyold-cli.fish\" }];\n";
        assert!(out.contains(want), "{want:?} in:\n{out}");
        assert!(out.contains("pub const PATH_BLOCK_BEGIN: &str = \"# >>> XY CLI >>>\";\n"), "{out}");
        assert!(out.contains("pub const FISH_CONF: &str = \"xy-cli.fish\";\n"), "{out}");
        assert!(out.contains("pub const SYSTEM_CLI_PATH: &str = \"/usr/local/bin/xy\";\n"), "{out}");
        let retired = out
            .lines()
            .find(|l| l.starts_with("pub const RETIRED"))
            .expect("a RETIRED line");
        assert!(!retired.contains("xy2"), "a silent alias is not retired: {retired}");

        let tomb = TempBrand::new(
            "tombstone",
            Some(&text.replace("\"notice\"", "\"tombstone\"")),
        );
        let out = render(&load(&tomb.file()).expect("loads"));
        assert!(out.contains(want), "a tombstone alias is retired too:\n{out}");
    }

    /// With a product and a command that are unrelated words, the PATH
    /// markers carry the product, the files and paths the command, and the
    /// command's upper-case form is listed as an earlier marker. A retired
    /// alias keeps the form its old install wrote.
    #[test]
    fn the_markers_carry_the_product_and_the_old_form_is_earlier() {
        let text =
            "id = \"gd\"\nschema = 1\n[names]\nproduct = \"Globex Desk\"\n[cli]\nname = \"gd\"\n\
            aliases = [{ name = \"gdold\", mode = \"notice\" }]\n";
        let t = TempBrand::new("apart", Some(text));
        let out = render(&load(&t.file()).expect("loads"));
        for line in [
            "pub const PATH_BLOCK_BEGIN: &str = \"# >>> Globex Desk CLI >>>\";\n",
            "pub const PATH_BLOCK_END: &str = \"# <<< Globex Desk CLI <<<\";\n",
            "pub const PATH_BLOCK_EARLIER: &[(&str, &str)] = &[(\"# >>> GD CLI >>>\", \"# <<< GD CLI <<<\")];\n",
            "pub const FISH_CONF: &str = \"gd-cli.fish\";\n",
            "pub const SYSTEM_CLI_PATH: &str = \"/usr/local/bin/gd\";\n",
            "block_begin: \"# >>> GDOLD CLI >>>\", block_end: \"# <<< GDOLD CLI <<<\"",
        ] {
            assert!(out.contains(line), "{line:?} in:\n{out}");
        }
    }

    /// The forms, one by one: the product's markers as spelled; an earlier
    /// form only when the command's upper case is other text.
    #[test]
    fn markers_and_earlier_forms() {
        assert_eq!(
            path_markers("Globex Desk"),
            (
                "# >>> Globex Desk CLI >>>".to_string(),
                "# <<< Globex Desk CLI <<<".to_string()
            )
        );
        assert_eq!(
            earlier_markers("Globex Desk", "gd"),
            vec![(
                "# >>> GD CLI >>>".to_string(),
                "# <<< GD CLI <<<".to_string()
            )]
        );
        for (product, name) in [("K2", "k2"), ("K5", "k5"), ("XY", "xy")] {
            assert_eq!(earlier_markers(product, name), Vec::new(), "{product}");
            assert_eq!(
                path_markers(product).0,
                install_tokens(name).block_begin,
                "{product}"
            );
        }
        // Case matters: a product spelled other than the upper-case
        // command is another marker.
        assert_eq!(earlier_markers("Xy", "xy").len(), 1);
    }

    /// The k5 brand's markers are the text a K5 install writes today.
    #[test]
    fn the_k5_brand_keeps_its_markers() {
        let file = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../brand/k5/brand.toml");
        let out = render(&load(&file).expect("the k5 brand file loads"));
        for line in [
            "pub const PATH_BLOCK_BEGIN: &str = \"# >>> K5 CLI >>>\";\n",
            "pub const PATH_BLOCK_END: &str = \"# <<< K5 CLI <<<\";\n",
            "pub const PATH_BLOCK_EARLIER: &[(&str, &str)] = &[];\n",
        ] {
            assert!(out.contains(line), "{line:?} in:\n{out}");
        }
    }

    /// The product is in the marker lines, so it must be one line.
    #[test]
    fn a_product_with_a_control_character_is_refused() {
        for (tag, product) in [("nl", "A\\nB"), ("tab", "A\\tB")] {
            refused(
                &format!("product-{tag}"),
                &good_with(r#"product = "AB""#, &format!("product = \"{product}\"")),
                "must be one line with no control characters",
            );
        }
    }

    #[test]
    fn strings_are_written_escaped_for_rust() {
        let t = TempBrand::new(
            "escape",
            Some(&good_with(
                r#"product = "AB""#,
                r#"product = "A \"B\" \\ C""#,
            )),
        );
        let out = render(&load(&t.file()).expect("loads"));
        assert!(
            out.contains(
                r#"        "A \"B\" \\ C"
"#
            ),
            "{out}"
        );
    }

    #[test]
    fn a_missing_file_is_refused() {
        let t = TempBrand::new("missing", None);
        let err = load(&t.file()).expect_err("no file");
        assert!(
            err.starts_with(&format!("brand file {}: can't read it", t.file().display())),
            "{err}"
        );
        assert!(err.contains("K2_BRAND_DIR"), "{err}");
    }

    #[test]
    fn a_schema_other_than_1_is_refused() {
        refused(
            "schema2",
            &good_with("schema = 1", "schema = 2"),
            "schema must be 1, not 2",
        );
        refused(
            "schema-text",
            &good_with("schema = 1", "schema = \"1\""),
            "schema must be 1, not \"1\"",
        );
        refused(
            "schema-none",
            &good_with("schema = 1", ""),
            "schema is missing; it must be 1",
        );
    }

    #[test]
    fn a_name_of_the_wrong_shape_is_refused() {
        for (i, bad) in ["a", "Ab", "1ab", "a-b", "a_b", "abcdefghijklm", "ab "]
            .iter()
            .enumerate()
        {
            let want = format!("cli.name {bad:?} must match ^[a-z][a-z0-9]{{1,11}}$");
            refused(
                &format!("name-{i}"),
                &good_with(r#"name = "ab""#, &format!("name = {bad:?}")),
                &want,
            );
        }
        refused(
            "name-empty",
            &good_with(r#"name = "ab""#, r#"name = """#),
            "cli.name is empty",
        );
        refused(
            "name-int",
            &good_with(r#"name = "ab""#, "name = 7"),
            "cli.name must be a string",
        );
        refused(
            "name-none",
            &good_with("name = \"ab\"\n", ""),
            "cli.name is missing",
        );
    }

    #[test]
    fn an_alias_of_the_wrong_shape_is_refused() {
        refused(
            "alias-shape",
            &good_with(r#"name = "abc""#, r#"name = "A-B""#),
            "cli.aliases[0].name \"A-B\" must match",
        );
        refused(
            "alias-long",
            &good_with(r#"name = "ab2""#, r#"name = "abcdefghijklm""#),
            "cli.aliases[2].name",
        );
        refused(
            "alias-none",
            &good_with(r#"name = "abc", "#, ""),
            "cli.aliases[0].name is missing",
        );
        refused(
            "alias-text",
            &good_with(r#"{ name = "ab1", mode = "silent" }"#, r#""ab1""#),
            "cli.aliases[1] must be a table",
        );
    }

    #[test]
    fn an_alias_that_is_the_name_is_refused() {
        refused(
            "alias-is-name",
            &good_with(r#"name = "ab1""#, r#"name = "ab""#),
            "cli.aliases[1].name \"ab\" is cli.name itself",
        );
    }

    #[test]
    fn an_alias_listed_twice_is_refused() {
        refused(
            "alias-twice",
            &good_with(r#"name = "ab2""#, r#"name = "abc""#),
            "cli.aliases[2].name \"abc\" is listed twice",
        );
    }

    #[test]
    fn a_mode_other_than_the_three_is_refused() {
        refused(
            "mode-loud",
            &good_with(r#"mode = "silent""#, r#"mode = "loud""#),
            "cli.aliases[1].mode \"loud\" must be silent, notice or tombstone",
        );
        refused(
            "mode-case",
            &good_with(r#"mode = "notice""#, r#"mode = "Notice""#),
            "cli.aliases[0].mode \"Notice\"",
        );
        refused(
            "mode-none",
            &good_with(r#", mode = "tombstone""#, ""),
            "cli.aliases[2].mode is missing",
        );
    }

    #[test]
    fn an_unknown_key_is_refused() {
        refused(
            "cli-key",
            &good_with("[cli]\n", "[cli]\nnmae = \"ab\"\n"),
            "[cli] holds a key it does not know: nmae",
        );
        refused(
            "alias-key",
            &good_with(r#"mode = "notice""#, r#"mode = "notice", why = "x""#),
            "cli.aliases[0] holds a key it does not know: why",
        );
    }

    #[test]
    fn missing_tables_and_fields_are_refused() {
        refused(
            "no-cli",
            "id = \"ab\"\nschema = 1\n[names]\nproduct = \"AB\"\n",
            "[cli] is missing",
        );
        refused(
            "no-names",
            &good_with("[names]\nproduct = \"AB\"\n", ""),
            "[names] is missing",
        );
        refused(
            "no-product",
            &good_with("product = \"AB\"\n", ""),
            "names.product is missing",
        );
        refused("no-id", &good_with("id = \"ab\"\n", ""), "id is missing");
        refused("aliases-text", "id = \"ab\"\nschema = 1\n[names]\nproduct = \"AB\"\n[cli]\nname = \"ab\"\naliases = \"abc\"\n", "cli.aliases must be a list");
        refused("not-toml", "id = ", "not valid TOML");
    }

    #[test]
    fn command_name_shape() {
        for good in ["ab", "k9", "a1b2", "abcdefghijkl"] {
            assert!(is_command_name(good), "{good}");
        }
        for bad in [
            "",
            "a",
            "1a",
            "Ab",
            "a-b",
            "a_b",
            "a b",
            "abcdefghijklm",
            "\u{e9}a",
        ] {
            assert!(!is_command_name(bad), "{bad}");
        }
    }

    /// Every brand folder in the repo holds a file this build can use, and
    /// its id is its folder's name.
    #[test]
    fn every_brand_folder_in_the_repo_loads() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../brand");
        let mut seen = 0;
        for entry in std::fs::read_dir(&root).expect("the repo has a brand folder") {
            let dir = entry.expect("read a brand folder entry").path();
            if !dir.is_dir() {
                continue;
            }
            let brand = load(&dir.join("brand.toml")).unwrap_or_else(|e| panic!("{e}"));
            let folder = dir
                .file_name()
                .and_then(|n| n.to_str())
                .expect("a folder name");
            assert_eq!(brand.id, folder, "the id in {}", dir.display());
            seen += 1;
        }
        assert!(seen >= 1, "no brand folder under {}", root.display());
    }
}
