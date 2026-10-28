//! Properties of the text tools, and a std-only fuzz pass over random argv and scripts.
//!
//! - `patch` of `diff -u(a, b)` applied to `a` gives `b`; `-R` takes it back.
//! - `sed -n p` is `cat` (the doc text), and `sed ''` too.
//! - Random argv, sed scripts, patterns and patches never panic and finish quickly
//!   (`FUZZ_CASES`, ≥ 10^4 in CI; longer runs belong on a compute node).

use std::time::{Duration, Instant};

use k2_mpc_base::tools::{unified, Kind, Tool, ToolDoc};
use proptest::prelude::*;

fn argv(a: &[&str]) -> Vec<String> {
    a.iter().map(|s| s.to_string()).collect()
}

fn read(a: &[&str], text: &str, stdin: &str) -> (Vec<u8>, u8) {
    let t = Tool::parse(&argv(a)).unwrap();
    let doc = ToolDoc {
        name: "p.md",
        text,
        bytes: text.as_bytes(),
    };
    let o = t.read(&doc, stdin.as_bytes()).unwrap();
    (o.stdout, o.exit)
}

fn patch(a: &[&str], text: &str, diff: &str) -> (Option<String>, u8) {
    let t = Tool::parse(&argv(a)).unwrap();
    let o = t.edit("p.md", text, diff.as_bytes()).unwrap();
    (o.text, o.exit)
}

/// Texts made of a few repeated lines (so diffs have context to find), sometimes without a
/// final newline.
fn texts() -> impl Strategy<Value = String> {
    (
        prop::collection::vec(
            prop::sample::select(vec!["a", "b", "c", "", "é日", "a b", "x"]),
            0..30,
        ),
        any::<bool>(),
    )
        .prop_map(|(ls, nl)| {
            let mut s = ls.join("\n");
            if nl && !s.is_empty() {
                s.push('\n');
            }
            s
        })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn patch_of_diff_gives_the_new_text(a in texts(), b in texts(), ctx in 0usize..4) {
        let d = unified(&a, &b, ctx, "a/p.md", "b/p.md");
        if a == b {
            prop_assert!(d.is_empty());
        } else {
            // the diff tool says the same
            let (out, exit) = read(&["diff", "-U", &ctx.to_string(), "--label", "a/p.md", "--label", "b/p.md"], &b, &a);
            prop_assert_eq!(String::from_utf8(out).unwrap(), d.clone());
            prop_assert_eq!(exit, 1);
            let (t, exit) = patch(&["patch", "-p1"], &a, &d);
            prop_assert_eq!(exit, 0, "{}", d);
            prop_assert_eq!(t.as_deref(), Some(b.as_str()), "{}", d);
            let (t, exit) = patch(&["patch", "-R"], &b, &d);
            prop_assert_eq!(exit, 0, "{}", d);
            prop_assert_eq!(t.as_deref(), Some(a.as_str()), "{}", d);
        }
    }

    #[test]
    fn sed_n_p_is_cat(a in texts()) {
        let (cat, _) = read(&["cat"], &a, "");
        prop_assert_eq!(&read(&["sed", "-n", "p"], &a, "").0, &cat);
        prop_assert_eq!(&read(&["sed", ""], &a, "").0, &cat);
        prop_assert_eq!(&read(&["sed", "-n", "1,$p"], &a, "").0, &cat);
        prop_assert_eq!(&read(&["view", "--lines", "1,$"], &a, "").0, &cat);
    }

    #[test]
    fn head_and_tail_split_the_doc(a in texts(), k in 0usize..35) {
        let lines = a.split_inclusive('\n').count();
        let (h, _) = read(&["head", "-n", &k.to_string()], &a, "");
        let rest = lines.saturating_sub(k);
        let (t, _) = read(&["tail", "-n", &rest.to_string()], &a, "");
        prop_assert_eq!([h, t].concat(), a.as_bytes().to_vec());
    }
}

// ---------- fuzz: random argv never panics ----------

/// Cases per run; CI size. Set `TOOLS_FUZZ_CASES` for a long run on a compute node.
fn fuzz_cases() -> usize {
    std::env::var("TOOLS_FUZZ_CASES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(12_000)
}

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        // xorshift64*
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
    fn pick<'a>(&mut self, v: &[&'a str]) -> &'a str {
        v[self.below(v.len())]
    }
    fn chars(&mut self, alphabet: &[char], max: usize) -> String {
        let n = self.below(max + 1);
        (0..n)
            .map(|_| alphabet[self.below(alphabet.len())])
            .collect()
    }
}

const SED_ALPHA: &[char] = &[
    's', '/', 'a', 'b', 'p', 'd', 'g', 'i', 'I', 'c', 'q', 'y', '=', '!', ',', ';', '$', '+', '1',
    '2', '0', '9', '\\', '(', ')', '|', '*', '.', '^', '[', ']', '{', '}', '&', '\n', ' ', '#',
    'n', 'e', 'w', 'r', 'é', '-', ':', '?', 'x', '~', 'M',
];
const RE_ALPHA: &[char] = &[
    'a', 'b', '.', '*', '+', '?', '(', ')', '|', '{', '}', '[', ']', '^', '$', '\\', '1', ',', ':',
    '<', '>', 'w', 'é', '-', '=', 'd', 'n', ' ',
];
const TEXT_ALPHA: &[char] = &['a', 'b', ' ', '\n', '\n', 'é', '日', 'x', '+', '-', '@'];

fn random_argv(r: &mut Rng) -> Vec<String> {
    let verb = r.pick(&[
        "cat", "head", "tail", "wc", "grep", "sed", "diff", "view", "patch", "apply", "append",
        "insert", "replace", "delete", "sed",
    ]);
    let mut v = vec![verb.to_string()];
    let flags: &[&str] = match verb {
        "grep" => &[
            "-n", "-i", "-v", "-c", "-w", "-x", "-F", "-E", "-o", "-l", "-L", "-H", "-q", "-m",
            "1", "-A", "2", "-B", "-C", "-3", "-e",
        ],
        "sed" => &["-n", "-E", "-e", "-i", "-s", ""],
        "head" | "tail" => &["-n", "-c", "+2", "-2", "3", "-q", "K", "-f"],
        "patch" | "apply" => &["-R", "--dry-run", "-F", "0", "1", "--partial", "-p1", "-s"],
        "insert" | "replace" | "delete" | "view" => &[
            "--line", "--after", "--at", "--lines", "--range", "1", "$", "2,3", "1:1", "1:2-2:1",
            "0",
        ],
        _ => &[
            "-l",
            "-w",
            "-c",
            "-m",
            "-n",
            "-b",
            "-u",
            "-U",
            "1",
            "--label",
            "--ensure-newline",
        ],
    };
    for _ in 0..r.below(4) {
        v.push(r.pick(flags).to_string());
    }
    match verb {
        "sed" => v.push(r.chars(SED_ALPHA, 14)),
        "grep" => v.push(r.chars(RE_ALPHA, 10)),
        _ => {}
    }
    v
}

fn random_patch(r: &mut Rng, text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let mut p = String::new();
    if r.below(2) == 0 {
        p.push_str("--- a/p.md\n+++ b/p.md\n");
    }
    for _ in 0..1 + r.below(3) {
        let (os, ol, ns, nl) = (r.below(8), r.below(5), r.below(8), r.below(5));
        p.push_str(&format!("@@ -{os},{ol} +{ns},{nl} @@\n"));
        for _ in 0..r.below(8) {
            let k = r.pick(&[" ", "-", "+", "\\", ""]);
            let l = if lines.is_empty() {
                "a"
            } else {
                lines[r.below(lines.len())]
            };
            p.push_str(k);
            p.push_str(l);
            p.push('\n');
        }
    }
    p
}

#[test]
fn random_argv_never_panics() {
    let mut r = Rng(0x9E37_79B9_7F4A_7C15);
    let n = fuzz_cases();
    let started = Instant::now();
    let mut ran = 0usize;
    let mut slowest = Duration::ZERO;
    for _ in 0..n {
        let text = r.chars(TEXT_ALPHA, 60);
        let a = random_argv(&mut r);
        let stdin = if a[0] == "patch" || a[0] == "apply" {
            random_patch(&mut r, &text)
        } else {
            r.chars(TEXT_ALPHA, 20)
        };
        let t0 = Instant::now();
        let res = std::panic::catch_unwind(|| {
            let Ok(tool) = Tool::parse(&a) else {
                return false;
            };
            match tool.kind() {
                Kind::Read => {
                    let doc = ToolDoc {
                        name: "p.md",
                        text: &text,
                        bytes: text.as_bytes(),
                    };
                    if let Ok(o) = tool.read(&doc, stdin.as_bytes()) {
                        assert!(o.exit <= 1, "{a:?}: exit {}", o.exit);
                    }
                }
                Kind::Edit => {
                    if let Ok(o) = tool.edit("p.md", &text, stdin.as_bytes()) {
                        assert!(o.exit <= 1, "{a:?}: exit {}", o.exit);
                    }
                }
            }
            true
        });
        let dt = t0.elapsed();
        slowest = slowest.max(dt);
        match res {
            Ok(true) => ran += 1,
            Ok(false) => {}
            Err(_) => panic!("panicked on argv {a:?}, text {text:?}, stdin {stdin:?}"),
        }
        assert!(
            dt < Duration::from_secs(2),
            "argv {a:?} took {dt:?} on text {text:?}"
        );
    }
    eprintln!(
        "fuzz: {n} cases, {ran} parsed and ran, slowest {slowest:?}, total {:?}",
        started.elapsed()
    );
    assert!(n >= 10_000, "fuzz needs at least 10^4 cases, got {n}");
    assert!(ran > n / 10, "too few cases got past parsing: {ran}");
}

#[test]
fn hostile_inputs_are_bounded() {
    // pathological-looking patterns run in linear time
    let text = "a".repeat(20_000) + "\n";
    let t0 = Instant::now();
    let (_, exit) = read(&["grep", "-cE", "(a*)*b"], &text, "");
    assert_eq!(exit, 1);
    let (_, exit) = read(&["grep", "-cw", "a*"], &text, "");
    assert_eq!(exit, 0);
    let (out, _) = read(&["sed", "s/a*$/x/"], &text, "");
    assert_eq!(out, b"x\n");
    assert!(t0.elapsed() < Duration::from_secs(10), "{:?}", t0.elapsed());
    // output is bounded: each sed pass may grow the text, but never past the limit
    let big = "a".repeat(1 << 20) + "\n";
    let t = Tool::parse(&argv(&[
        "sed",
        "s/a/&&&&&&&&&&&&&&&&&&&&&&&&&&&&&&&&&&&&&&&&/g",
    ]))
    .unwrap();
    let doc = ToolDoc {
        name: "p.md",
        text: &big,
        bytes: big.as_bytes(),
    };
    assert!(
        t.read(&doc, b"").is_err(),
        "a 40x expansion of 1 MiB must hit the output limit"
    );
    // huge repetition counts are refused, not compiled
    assert!(Tool::parse(&argv(&["grep", "a\\{1001\\}"])).is_err());
    assert!(Tool::parse(&argv(&["grep", "-E", "((a{1000}){1000}){1000}"])).is_err());
}
