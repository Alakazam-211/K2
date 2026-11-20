//! The brand's names, written into this crate when building from one brand
//! file.
//!
//! `build.rs` reads `brand/<id>/brand.toml` (the folder `K2_BRAND_DIR`
//! names, else `brand/k2`) and writes the command's name, the other names
//! it answers to with their modes, the product's name and the brand's id.
//! `k2_client::brand` re-exports them; code uses that module, not this
//! crate. No dependencies at run time.
//!
//! It also writes the tokens an install builds from the command's name:
//! the PATH block's marker lines, the fish file's name, the system-wide
//! path, and the same for each name the brand is retiring ([`RETIRED`]).
//! They are made in the build script and nowhere else. The install code
//! (`k2_client::cli_install`) and the root helper's paths
//! (`k2_root_proto::paths`) read them from here.
//!
//! The text forms are macros as well as consts, so a `const` sentence can
//! be built with `concat!` and still be one `&'static str`. Inside this
//! crate the macros are named by their bare names only, after the
//! `include!`: a `#[macro_export]` macro that comes from `include!` can't
//! be named by an absolute path in its own crate.

/// What an alias does when the command is called by it. The root helper's
/// `cli-refresh` acts on it for the links in `/usr/local/bin`
/// (`k2_root_helper::core::tend_aliases`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AliasMode {
    /// Runs as the command and says nothing.
    Silent,
    /// Runs as the command, with one line on stderr that names the
    /// brand's command.
    Notice,
    /// Runs nothing: prints the brand's form of the command and exits 127.
    Tombstone,
}

/// A name the brand is retiring (an alias in mode `notice` or
/// `tombstone`), with the PATH block and the fish file its installs left.
/// A build of this brand takes those over; it never touches a block or a
/// file of a name that is not its own or in this list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RetiredName {
    /// The old command name.
    pub name: &'static str,
    /// The first line of its PATH block.
    pub block_begin: &'static str,
    /// The last line of its PATH block.
    pub block_end: &'static str,
    /// Its fish startup file, in fish's `conf.d`.
    pub fish_conf: &'static str,
}

include!(concat!(env!("OUT_DIR"), "/brand.rs"));

#[cfg(test)]
mod brand_file;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_name_and_every_alias_have_the_command_shape() {
        assert!(brand_file::is_command_name(CLI_NAME), "{CLI_NAME:?}");
        for alias in CLI_ALIASES {
            assert!(brand_file::is_command_name(alias), "{alias:?}");
        }
    }

    #[test]
    fn no_alias_is_the_name_and_none_repeats() {
        for (i, alias) in CLI_ALIASES.iter().enumerate() {
            assert_ne!(*alias, CLI_NAME, "an alias is the command's own name");
            assert!(
                !CLI_ALIASES[..i].contains(alias),
                "{alias:?} is listed twice"
            );
        }
    }

    #[test]
    fn the_two_alias_lists_name_the_same_names_in_order() {
        let from_modes: Vec<&str> = CLI_ALIAS_MODES.iter().map(|(name, _)| *name).collect();
        assert_eq!(from_modes, CLI_ALIASES);
    }

    #[test]
    fn macros_and_consts_agree() {
        assert_eq!(cli_name!(), CLI_NAME);
        assert_eq!(product_name!(), PRODUCT_NAME);
        assert!(!PRODUCT_NAME.is_empty());
        assert!(!BRAND_ID.is_empty());
    }

    /// The compiled install tokens are the generator's, for this brand's
    /// name (any brand: these tests also run under `K2_BRAND_DIR`).
    #[test]
    fn the_install_tokens_are_built_from_the_name() {
        let own = brand_file::install_tokens(CLI_NAME);
        assert_eq!(PATH_BLOCK_BEGIN, own.block_begin);
        assert_eq!(PATH_BLOCK_END, own.block_end);
        assert_eq!(FISH_CONF, own.fish_conf);
        assert_eq!(SYSTEM_CLI_PATH, brand_file::system_cli_path(CLI_NAME));
        assert!(PATH_BLOCK_BEGIN.starts_with("# >>> ") && PATH_BLOCK_END.starts_with("# <<< "));
    }

    /// RETIRED is the aliases in mode notice or tombstone, in order, each
    /// with its own tokens; a silent alias is not there.
    #[test]
    fn retired_is_the_retiring_aliases_with_their_tokens() {
        let want: Vec<&str> = CLI_ALIAS_MODES
            .iter()
            .filter(|(_, mode)| matches!(mode, AliasMode::Notice | AliasMode::Tombstone))
            .map(|(name, _)| *name)
            .collect();
        let got: Vec<&str> = RETIRED.iter().map(|r| r.name).collect();
        assert_eq!(got, want);
        for r in RETIRED {
            let t = brand_file::install_tokens(r.name);
            assert_eq!(
                (r.block_begin, r.block_end, r.fish_conf),
                (t.block_begin.as_str(), t.block_end.as_str(), t.fish_conf.as_str()),
                "{}",
                r.name
            );
            assert_ne!(r.block_begin, PATH_BLOCK_BEGIN, "{} has the command's own marker", r.name);
        }
    }

    /// The macros still work inside `concat!` in a `const`.
    #[test]
    fn the_macros_build_const_sentences() {
        const LINE: &str = concat!("run ", cli_name!(), " msg in ", product_name!());
        assert_eq!(LINE, format!("run {CLI_NAME} msg in {PRODUCT_NAME}"));
    }
}
