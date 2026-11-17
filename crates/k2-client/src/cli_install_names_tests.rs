//! The install code with a brand's names as a value ([`Names`]), under the
//! default build: which links, PATH blocks and fish files a build calls its
//! own. Every test runs in a temp home of its own and passes it in; nothing
//! here reads `$HOME`, a real startup file or `/usr/local/bin`.

use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};

use super::*;

/// A login PATH without `~/.local/bin` (a fresh Mac's).
const BARE_LOGIN_PATH: &str = "/usr/bin:/bin:/usr/sbin:/sbin";

/// A build named `xy` with no aliases.
const XY: Names = Names {
    name: "xy",
    aliases: &[],
    retired: &[],
    block_begin: "# >>> XY CLI >>>",
    block_end: "# <<< XY CLI <<<",
    fish_conf: "xy-cli.fish",
};

const K2_RETIRED: RetiredName = RetiredName {
    name: "k2",
    block_begin: "# >>> K2 CLI >>>",
    block_end: "# <<< K2 CLI <<<",
    fish_conf: "k2-cli.fish",
};

/// A build named `k5`, retiring `k2` (mode notice), with `k2so` as a second
/// name (mode silent).
const K5: Names = Names {
    name: "k5",
    aliases: &["k2", "k2so"],
    retired: &[K2_RETIRED],
    block_begin: "# >>> K5 CLI >>>",
    block_end: "# <<< K5 CLI <<<",
    fish_conf: "k5-cli.fish",
};

/// The links run the script (the k5 switch off).
const SCRIPT: Choice = Choice::Script { note: None };

/// The blocks as bytes, as they sit in a person's startup files.
const K2_ZSH: &str = "# >>> K2 CLI >>>\nexport PATH=\"$HOME/.local/bin:$PATH\"\n# <<< K2 CLI <<<\n";
const K2_FISH: &str = "# >>> K2 CLI >>>\nif not contains -- $HOME/.local/bin $PATH\n    \
                       set -gx PATH $HOME/.local/bin $PATH\nend\n# <<< K2 CLI <<<\n";
const XY_ZSH: &str = "# >>> XY CLI >>>\nexport PATH=\"$HOME/.local/bin:$PATH\"\n# <<< XY CLI <<<\n";
const K5_ZSH: &str = "# >>> K5 CLI >>>\nexport PATH=\"$HOME/.local/bin:$PATH\"\n# <<< K5 CLI <<<\n";

/// A temp home, removed when dropped.
struct Home(PathBuf);

impl Home {
    fn new(tag: &str) -> Home {
        let dir = std::env::temp_dir().join(format!("k2-cli-names-{tag}-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).expect("make the temp home");
        Home(dir)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// A fake `<home>/Applications/<app>.app` with `cli/<name>` and, when
/// given, the alias shim `cli/<shim>`.
fn bundle(home: &Path, app: &str, name: &str, shim: Option<&str>) -> (PathBuf, Option<PathBuf>) {
    let dir = home.join(format!("Applications/{app}.app/Contents/Resources/_up_/cli"));
    fs::create_dir_all(&dir).expect("make the bundle");
    let cli = dir.join(name);
    fs::write(&cli, "#!/bin/bash\nK2_CLI_VERSION=\"1.0.0\"\n").expect("write the CLI");
    let shim = shim.map(|s| {
        let p = dir.join(s);
        fs::write(&p, "#!/bin/bash\nexec cli \"$@\"\n").expect("write the shim");
        p
    });
    (cli, shim)
}

fn input<'a>(cli: &'a Path, shim: Option<&'a Path>, shell: &'a str) -> HealInput<'a> {
    HealInput {
        cli,
        legacy_shim: shim,
        system_bin: None,
        shell: Path::new(shell),
        zdotdir: None,
        login_path: Some(BARE_LOGIN_PATH),
        force_path: false,
    }
}

fn read(p: &Path) -> String {
    fs::read_to_string(p).unwrap_or_else(|e| panic!("read {}: {e}", p.display()))
}

fn link_target(p: &Path) -> PathBuf {
    fs::read_link(p).unwrap_or_else(|e| panic!("{} must be a link: {e}", p.display()))
}

fn conf_d(home: &Path) -> PathBuf {
    home.join(".config/fish/conf.d")
}

/// The default brand's names, as literal bytes: the lines already in
/// people's startup files, the fish file and the system path. A change to
/// the generator that moves one fails here.
#[test]
fn the_default_names_are_todays_bytes() {
    assert_eq!(BLOCK_BEGIN, "# >>> K2 CLI >>>");
    assert_eq!(BLOCK_END, "# <<< K2 CLI <<<");
    assert_eq!(FISH_CONF, "k2-cli.fish");
    assert_eq!(SYSTEM_CLI_PATH, "/usr/local/bin/k2");
    assert_eq!((CLI_NAME, LEGACY_NAME), ("k2", "k2so"));
    assert_eq!(
        Names::compiled(),
        Names {
            name: "k2",
            aliases: &["k2so"],
            retired: &[],
            block_begin: "# >>> K2 CLI >>>",
            block_end: "# <<< K2 CLI <<<",
            fish_conf: "k2-cli.fish",
        }
    );
    assert_eq!(path_block(ShellKind::Zsh), K2_ZSH);
    assert_eq!(path_block(ShellKind::Bash), K2_ZSH);
    assert_eq!(path_block(ShellKind::Fish), K2_FISH);
    let h = Path::new("/home/p");
    assert_eq!(
        block_candidates(h, None),
        [".zprofile", ".bash_profile", ".bash_login", ".profile", ".config/fish/conf.d/k2-cli.fish"]
            .map(|f| h.join(f))
            .to_vec()
    );
}

/// "Is this ours" by name: the command and its aliases in a `cli` folder,
/// never another brand's command.
#[test]
fn a_cli_target_is_ours_only_by_this_builds_names() {
    let cli = |name: &str| PathBuf::from(format!("/Applications/Any.app/Contents/Resources/_up_/cli/{name}"));
    for name in ["k5", "k2", "k2so"] {
        assert!(K5.is_cli_target(&cli(name)), "{name}");
    }
    assert!(!K5.is_cli_target(&cli("xy")));
    assert!(!K5.is_cli_target(Path::new("/opt/homebrew/bin/k5")));
    assert!(XY.is_cli_target(&cli("xy")));
    for name in ["k2", "k2so", "k5"] {
        assert!(!XY.is_cli_target(&cli(name)), "{name} is not an xy build's");
    }
}

/// An `xy` build on a machine with another brand's install: its links, its
/// PATH block and its fish file stay as they are, and `xy` gets its own
/// beside them. The undo takes only `xy`'s.
#[test]
fn an_xy_build_leaves_another_brands_links_block_and_fish_file() {
    let home = Home::new("xy");
    let h = home.path();
    let (cli, _) = bundle(h, "XY", "xy", None);
    let bin = user_bin_dir(h);
    fs::create_dir_all(&bin).unwrap();
    let k2_target = h.join("Old/K2.app/Contents/Resources/_up_/cli/k2");
    let k5_target = h.join("Old/K5.app/Contents/Resources/_up_/cli/k5");
    symlink(&k2_target, bin.join("k2")).unwrap();
    symlink(&k5_target, bin.join("k5")).unwrap();
    let zprofile = h.join(".zprofile");
    let mine = format!("# mine\n{K2_ZSH}alias ll='ls -l'\n");
    fs::write(&zprofile, &mine).unwrap();
    fs::create_dir_all(conf_d(h)).unwrap();
    let k2_fish = conf_d(h).join("k2-cli.fish");
    fs::write(&k2_fish, K2_FISH).unwrap();

    // A link to another brand's script is not an xy build's to re-point;
    // the default build would call the same link its own.
    assert!(matches!(XY.link_state(&bin.join("k2"), &cli), LinkState::Foreign { .. }));
    assert_eq!(
        Names::compiled().link_state(&bin.join("k2"), &cli),
        LinkState::Stale { target: k2_target.clone() }
    );

    let report = XY.heal_at(h, &input(&cli, None, "/bin/zsh"), &SCRIPT).expect("heal");
    assert_eq!(report.user_link, LinkOutcome::Created);
    assert_eq!(report.user_link_path, bin.join("xy"));
    assert_eq!(link_target(&bin.join("xy")), cli);
    assert_eq!(report.user_aliases, Vec::<AliasLink>::new());
    assert_eq!(report.user_legacy, None);
    assert_eq!(link_target(&bin.join("k2")), k2_target, "another brand's k2 is not re-pointed");
    assert_eq!(link_target(&bin.join("k5")), k5_target, "another brand's k5 is not re-pointed");
    assert_eq!(report.path, PathOutcome::BlockAdded { file: zprofile.clone() });
    assert_eq!(read(&zprofile), format!("{mine}{XY_ZSH}"), "the K2 block stays byte for byte; XY's goes beside it");

    let fish = XY.heal_at(h, &input(&cli, None, "/usr/bin/fish"), &SCRIPT).expect("fish heal");
    let xy_fish = conf_d(h).join("xy-cli.fish");
    assert_eq!(fish.path, PathOutcome::BlockAdded { file: xy_fish.clone() });
    assert_eq!(read(&xy_fish), XY.path_block(ShellKind::Fish));
    assert_eq!(read(&k2_fish), K2_FISH, "another brand's fish file is left alone");

    let changed = XY.remove_path_block_at(h, None).expect("undo");
    assert_eq!(changed, vec![zprofile.clone(), xy_fish.clone()]);
    assert_eq!(read(&zprofile), mine, "only XY's lines go");
    assert!(!xy_fish.exists(), "XY's fish file was all its block");
    assert_eq!(read(&k2_fish), K2_FISH);
    assert_eq!(link_target(&bin.join("k2")), k2_target, "not removed");
    assert_eq!(link_target(&bin.join("k5")), k5_target, "not removed");
}

/// A `k5` build retiring `k2`: the K2 block becomes a K5 block in the same
/// place, once; a second run changes nothing. The links an earlier install
/// left for its aliases (`k2`, `k2so`) are re-pointed beside the new `k5`.
#[test]
fn a_k5_build_takes_over_the_k2_block_once_and_links_its_aliases() {
    let home = Home::new("k5");
    let h = home.path();
    let (cli, shim) = bundle(h, "K5", "k5", Some("k2so"));
    let shim = shim.expect("the bundle has a shim");
    let bin = user_bin_dir(h);
    fs::create_dir_all(&bin).unwrap();
    let old_k2 = h.join("Old/K2.app/Contents/Resources/_up_/cli/k2");
    let old_k2so = h.join("Old/K2.app/Contents/Resources/_up_/cli/k2so");
    symlink(&old_k2, bin.join("k2")).unwrap();
    symlink(&old_k2so, bin.join("k2so")).unwrap();
    let zprofile = h.join(".zprofile");
    let (before, after) = ("# before\n", "alias gs='git status'\n");
    fs::write(&zprofile, format!("{before}{K2_ZSH}{after}")).unwrap();

    let report = K5.heal_at(h, &input(&cli, Some(&shim), "/bin/zsh"), &SCRIPT).expect("heal");
    assert_eq!(report.user_link, LinkOutcome::Created);
    assert_eq!(link_target(&bin.join("k5")), cli);
    assert_eq!(
        report.user_aliases,
        vec![
            AliasLink { path: bin.join("k2"), outcome: LinkOutcome::Repointed { from: old_k2.clone() } },
            AliasLink { path: bin.join("k2so"), outcome: LinkOutcome::Repointed { from: old_k2so } },
        ]
    );
    assert_eq!(report.user_legacy, Some(LinkOutcome::Repointed { from: old_k2 }), "the first alias");
    assert_eq!(link_target(&bin.join("k2")), shim);
    assert_eq!(link_target(&bin.join("k2so")), shim);
    assert_eq!(report.path, PathOutcome::BlockReplaced { file: zprofile.clone() });
    let healed = format!("{before}{K5_ZSH}{after}");
    assert_eq!(read(&zprofile), healed, "the K5 block takes the K2 block's place");

    let again = K5.heal_at(h, &input(&cli, Some(&shim), "/bin/zsh"), &SCRIPT).expect("second heal");
    assert_eq!(again.user_link, LinkOutcome::Current);
    assert_eq!(
        again.user_aliases.iter().map(|a| &a.outcome).collect::<Vec<_>>(),
        vec![&LinkOutcome::Current, &LinkOutcome::Current]
    );
    assert_eq!(again.path, PathOutcome::BlockPresent { file: zprofile.clone() });
    assert_eq!(read(&zprofile), healed, "a second run changes nothing");
}

/// fish: `k2-cli.fish` (all K2's block) is replaced by `k5-cli.fish`, even
/// when the login shell did not answer: the line was already on PATH.
#[test]
fn a_k5_build_takes_over_the_k2_fish_file() {
    let home = Home::new("k5-fish");
    let h = home.path();
    let (cli, shim) = bundle(h, "K5", "k5", Some("k2so"));
    fs::create_dir_all(conf_d(h)).unwrap();
    let k2_fish = conf_d(h).join("k2-cli.fish");
    fs::write(&k2_fish, K2_FISH).unwrap();
    let k5_fish = conf_d(h).join("k5-cli.fish");

    for run in 1..=2 {
        let r = K5
            .heal_at(h, &HealInput { login_path: None, ..input(&cli, shim.as_deref(), "/usr/bin/fish") }, &SCRIPT)
            .expect("fish heal");
        let want = if run == 1 {
            PathOutcome::BlockReplaced { file: k5_fish.clone() }
        } else {
            PathOutcome::BlockPresent { file: k5_fish.clone() }
        };
        assert_eq!(r.path, want, "run {run}");
        assert_eq!(read(&k5_fish), K5.path_block(ShellKind::Fish), "run {run}");
        assert!(!k2_fish.exists(), "run {run}: k2-cli.fish was all K2's block, so it goes");
    }
}

/// The undo takes this build's block and a retired name's, and never a
/// block with any other marker.
#[test]
fn the_undo_takes_this_builds_and_retired_blocks_and_no_other() {
    let home = Home::new("k5-undo");
    let h = home.path();
    let zprofile = h.join(".zprofile");
    fs::write(&zprofile, format!("# a\n{K5_ZSH}# b\n{K2_ZSH}# c\n{XY_ZSH}")).unwrap();
    fs::create_dir_all(conf_d(h)).unwrap();
    let k2_fish = conf_d(h).join("k2-cli.fish");
    fs::write(&k2_fish, K2_FISH).unwrap();
    let xy_fish = conf_d(h).join("xy-cli.fish");
    let xy_fish_block = XY.path_block(ShellKind::Fish);
    fs::write(&xy_fish, &xy_fish_block).unwrap();

    let changed = K5.remove_path_block_at(h, None).expect("undo");
    assert_eq!(changed, vec![zprofile.clone(), k2_fish.clone()]);
    assert_eq!(read(&zprofile), format!("# a\n# b\n# c\n{XY_ZSH}"));
    assert!(!k2_fish.exists());
    assert_eq!(read(&xy_fish), xy_fish_block, "another brand's fish file stays");
    assert!(path_opted_out(h));
}

/// A startup file with this build's block and another brand's, healed twice,
/// ends with the same two blocks: by an `xy` build and by the default one.
#[test]
fn a_file_with_two_brands_blocks_keeps_both_through_two_heals() {
    for (names, app) in [(XY, "XY"), (Names::compiled(), "K2")] {
        let home = Home::new("two-blocks");
        let h = home.path();
        let (cli, _) = bundle(h, app, names.name, None);
        let zprofile = h.join(".zprofile");
        let both = format!("# mine\n{K2_ZSH}{XY_ZSH}# end\n");
        fs::write(&zprofile, &both).unwrap();
        for run in 1..=2 {
            let r = names.heal_at(h, &input(&cli, None, "/bin/zsh"), &SCRIPT).expect("heal");
            assert_eq!(r.path, PathOutcome::BlockPresent { file: zprofile.clone() }, "{} run {run}", names.name);
            assert_eq!(read(&zprofile), both, "{} run {run}", names.name);
        }
    }
}

/// The names of the entries in `dir`, sorted (none when it is missing).
fn entries(dir: &Path) -> Vec<String> {
    let mut out: Vec<String> = match fs::read_dir(dir) {
        Ok(rd) => rd.map(|e| e.expect("read an entry").file_name().to_string_lossy().into_owned()).collect(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => panic!("read {}: {e}", dir.display()),
    };
    out.sort();
    out
}

/// A default-brand install gets exactly today's links, so the brand's
/// [`Names`] change nothing a normal user has: `k2` in `~/.local/bin` and in
/// the system bin folder, at the script; `k5` beside each ONLY when the
/// links run the k5 binary, and then both names run it; nothing else (no
/// alias link on a new install, no temp link left behind). Twice each, so a
/// second heal adds nothing either.
#[test]
fn a_default_brand_install_makes_exactly_todays_links() {
    for on in [false, true] {
        let home = Home::new(if on { "default-k5-on" } else { "default-k5-off" });
        let h = home.path();
        let (cli, shim) = bundle(h, "K2", "k2", Some("k2so"));
        let sys = h.join("usr-local-bin");
        fs::create_dir_all(&sys).expect("make the system bin folder");
        let runs = if on {
            let dir = h.join("Applications/K2.app/Contents/MacOS");
            fs::create_dir_all(&dir).expect("make the MacOS folder");
            let binary = dir.join("k5-cli");
            fs::write(&binary, "stand-in").expect("write the stand-in binary");
            Choice::Binary { binary }
        } else {
            SCRIPT
        };
        let target = runs.binary().map(Path::to_path_buf).unwrap_or_else(|| cli.clone());
        let input = HealInput { system_bin: Some(&sys), ..input(&cli, shim.as_deref(), "/bin/zsh") };
        let bin = user_bin_dir(h);
        let want: Vec<&str> = if on { vec!["k2", "k5"] } else { vec!["k2"] };

        for run in 1..=2 {
            let report = Names::compiled().heal_at(h, &input, &runs).expect("heal");
            assert_eq!(entries(&bin), want, "k5 {on}, run {run}: ~/.local/bin");
            assert_eq!(entries(&sys), want, "k5 {on}, run {run}: the system bin folder");
            for dir in [&bin, &sys] {
                for name in &want {
                    let link = dir.join(name);
                    assert_eq!(link_target(&link), target, "k5 {on}, run {run}: {}", link.display());
                }
            }
            assert_eq!(report.user_link_path, bin.join("k2"));
            assert!(report.user_link.is_ours(), "k5 {on}, run {run}: {:?}", report.user_link);
            assert!(report.system_link.as_ref().is_some_and(LinkOutcome::is_ours), "k5 {on}, run {run}");
            assert_eq!(report.user_k5.is_some(), on, "k5 {on}, run {run}: the user k5 link");
            assert_eq!(report.system_k5.is_some(), on, "k5 {on}, run {run}: the system k5 link");
            assert_eq!(report.user_aliases, Vec::<AliasLink>::new(), "k5 {on}, run {run}: no alias on a new install");
            assert_eq!(report.system_aliases, Vec::<AliasLink>::new(), "k5 {on}, run {run}");
            assert_eq!(report.swept, Vec::<PathBuf>::new(), "k5 {on}, run {run}");
        }
    }
}
