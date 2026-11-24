//! The brand's names, written into this crate when building from one brand
//! file.
//!
//! `build.rs` reads `brand/<id>/brand.toml` (the folder `K2_BRAND_DIR`
//! names, else `brand/k2`) and writes the command's name, the other names
//! it answers to with their modes, the product's name and the brand's id.
//! `k2_client::brand` re-exports them; code uses that module, not this
//! crate. No dependencies at run time.
//!
//! [`CLI_NO_BASH`] is the brand file's `cli.no_bash`: a build that carries
//! no shell CLI (`cli/k2`) at all, no copy and no fallback. Left out of a
//! brand file it is true; brand/k2 and brand/k5 say false.
//!
//! It also writes the tokens an install writes: the PATH block's marker
//! lines (from the product's name, with the command-form markers of
//! earlier builds in [`PATH_BLOCK_EARLIER`]), the fish file's name and
//! the system-wide path (from the command's name), and the same for each
//! name the brand is retiring ([`RETIRED`]).
//! They are made in the build script and nowhere else. The install code
//! (`k2_client::cli_install`) and the root helper's paths
//! (`k2_root_proto::paths`) read them from here.
//!
//! A helper program's file name is the command's name, a dash and the
//! helper's part: `helper_name!("agent")` is `k2-agent` in the default
//! brand. It is a macro only, a literal for `concat!` and `const`s.
//!
//! The brand file's other tables come out as one module each
//! ([`names`], [`zones`], [`control`], [`feeds`], [`machine`], [`look`],
//! [`links`], [`mail`], [`phone`], [`sew`]): what a hosted box
//! of the brand needs, each value the brand's own, the default brand's
//! (`brand/k2`), or made from the brand's names. Most have no reader in
//! the code yet; code that still spells a value moves to these as it is
//! tended. A brand that isn't ours ([`IS_OURS`] false: not k2 or k5 in
//! this repo's own `brand/<id>`) sets its own value for every field that
//! names us or reaches us; the build refuses one that doesn't.
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

/// The brand mark a binary carries (static data; a release reads it).
pub mod mark;

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
        assert_eq!(login_name!(), LOGIN_NAME);
        assert_eq!(LOGIN_NAME, names::LOGIN);
        assert!(!PRODUCT_NAME.is_empty());
        assert!(!BRAND_ID.is_empty());
    }

    /// A build from this repo's own brand/<id> is ours exactly when the id
    /// is one of ours (under K2_BRAND_DIR=brand/xy it is not).
    #[test]
    fn is_ours_is_the_brand_ids() {
        assert_eq!(IS_OURS, brand_file::ours::OUR_IDS.contains(&BRAND_ID));
    }

    /// The compiled install tokens are the generator's: the PATH markers
    /// from the product's name, the earlier form from the command's, the
    /// files and paths from the command's (any brand: these tests also run
    /// under `K2_BRAND_DIR`).
    #[test]
    fn the_install_tokens_are_built_from_the_names() {
        let (begin, end) = brand_file::path_markers(PRODUCT_NAME);
        assert_eq!(
            (PATH_BLOCK_BEGIN, PATH_BLOCK_END),
            (begin.as_str(), end.as_str())
        );
        let earlier: Vec<(String, String)> = PATH_BLOCK_EARLIER
            .iter()
            .map(|(b, e)| (b.to_string(), e.to_string()))
            .collect();
        assert_eq!(earlier, brand_file::earlier_markers(PRODUCT_NAME, CLI_NAME));
        for (b, _) in PATH_BLOCK_EARLIER {
            assert_ne!(
                *b, PATH_BLOCK_BEGIN,
                "an earlier form is not the current one"
            );
        }
        let own = brand_file::install_tokens(CLI_NAME);
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
                (
                    t.block_begin.as_str(),
                    t.block_end.as_str(),
                    t.fish_conf.as_str()
                ),
                "{}",
                r.name
            );
            assert_ne!(
                r.block_begin, PATH_BLOCK_BEGIN,
                "{} has the command's own marker",
                r.name
            );
        }
    }

    /// The macros still work inside `concat!` in a `const`.
    #[test]
    fn the_macros_build_const_sentences() {
        const LINE: &str = concat!("run ", cli_name!(), " msg in ", product_name!());
        assert_eq!(LINE, format!("run {CLI_NAME} msg in {PRODUCT_NAME}"));
    }

    /// The other fields keep the shapes the reader checked, in any brand.
    #[test]
    fn the_compiled_fields_have_their_shapes() {
        assert!(brand_file::is_domain(zones::LABEL), "{}", zones::LABEL);
        assert!(
            brand_file::is_domain(control::RELAY_HOST),
            "{}",
            control::RELAY_HOST
        );
        for url in [
            control::CONNECT,
            control::CERT,
            control::PUSH,
            control::RELAY_LIST,
            control::WEB_API,
            feeds::DAEMON,
            feeds::DOWNLOADS,
            links::SITE,
            links::REPO,
        ] {
            assert!(brand_file::is_https_url(url), "{url}");
        }
        for url in [
            links::SUPPORT_URL,
            links::TERMS,
            links::PRIVACY,
            links::DASHBOARD,
            links::FEEDBACK,
        ]
        .into_iter()
        .flatten()
        {
            assert!(brand_file::is_https_url(url), "{url}");
        }
        for name in [
            names::SHORT,
            names::COMPANY,
            names::LOCKUP,
            names::SERVER,
            names::LOGIN,
            mail::SYSTEM_FROM_NAME,
            sew::CALL_NAME,
        ] {
            assert!(!name.is_empty() && name.trim() == name, "{name:?}");
        }
        assert!(
            mail::SYSTEM_FROM.contains('@')
                && links::SUPPORT_EMAIL.contains('@')
                && links::CONTACT_EMAIL.contains('@')
        );
        // `{server}` is the box's own name; any other brace is a typo.
        let domain = mail::SYSTEM_FROM.rsplit('@').next().expect("a domain");
        assert!(
            domain == brand_file::SERVER_SLOT || brand_file::is_domain(domain),
            "{}",
            mail::SYSTEM_FROM
        );
        assert!(machine::HOME_DIR_NAME.starts_with('.'));
        assert!(brand_file::is_command_name(machine::BINARY_PREFIX));
        assert!(brand_file::is_command_name(machine::UNIT_PREFIX));
    }

    /// A helper's name is the command's name, a dash and the part, as a
    /// literal a `const` can hold.
    #[test]
    fn helper_names_are_the_command_a_dash_and_the_part() {
        const AGENT: &str = helper_name!("agent");
        assert_eq!(AGENT, format!("{CLI_NAME}-agent"));
        assert_eq!(helper_name!("ptyd"), format!("{CLI_NAME}-ptyd"));
        assert_eq!(
            concat!("/usr/bin/", helper_name!("daemon")),
            format!("/usr/bin/{CLI_NAME}-daemon")
        );
    }
}
