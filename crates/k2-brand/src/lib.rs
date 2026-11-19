//! The brand's names, written into this crate when building from one brand
//! file.
//!
//! `build.rs` reads `brand/<id>/brand.toml` (the folder `K2_BRAND_DIR`
//! names, else `brand/k2`) and writes the command's name, the other names
//! it answers to with their modes, the product's name and the brand's id.
//! `k2_client::brand` re-exports them; code uses that module, not this
//! crate. No dependencies at run time.
//!
//! The text forms are macros as well as consts, so a `const` sentence can
//! be built with `concat!` and still be one `&'static str`. Inside this
//! crate the macros are named by their bare names only, after the
//! `include!`: a `#[macro_export]` macro that comes from `include!` can't
//! be named by an absolute path in its own crate.

/// What an alias does when the command is called by it. Nothing acts on a
/// mode yet.
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

    /// The macros still work inside `concat!` in a `const`.
    #[test]
    fn the_macros_build_const_sentences() {
        const LINE: &str = concat!("run ", cli_name!(), " msg in ", product_name!());
        assert_eq!(LINE, format!("run {CLI_NAME} msg in {PRODUCT_NAME}"));
    }
}
