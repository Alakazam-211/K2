//! `zen guide` (`cmd_zen_guide`, `cli/src/zen-guide.sh` and
//! `cli/src/zen-guide-data.sh`): static pages, no daemon.
//!
//! The pages, the examples and their titles are the shell's own quoted
//! heredocs and `case` lines (two of them written by contract-gen), so the
//! binary embeds those two parts and reads each page out of them, as
//! `families::app::call` reads the refusal registry: the same bytes, with
//! no second copy to keep in step. The usage page is `cli/text`'s.

use super::print::{self, Out};
use super::verbs::usage;
use crate::exit::Exit;
use crate::families::Shown;
use crate::out::json::{self, Options, Text, Value};
use k2_client::cli_name;

const GUIDE: &str = include_str!("../../../../../cli/src/zen-guide.sh");
const DATA: &str = include_str!("../../../../../cli/src/zen-guide-data.sh");

/// The body of the quoted heredoc right after `opener` in `src`, with its
/// last newline (what `cat <<'EOF'` prints).
fn heredoc_after(src: &'static str, opener: &str) -> Option<&'static str> {
    let at = src.find(opener)? + opener.len();
    let rest = &src[at..];
    let end = rest.find("\nEOF\n")?;
    Some(&rest[..=end])
}

/// A function's one `cat <<'EOF'` page.
fn function_page(src: &'static str, func: &str) -> Option<&'static str> {
    heredoc_after(src, &format!("\n{func}() {{\n    cat <<'EOF'\n"))
}

/// A `NAME="a b c"` list of the parts.
fn list(src: &'static str, name: &str) -> Vec<&'static str> {
    let key = format!("\n{name}=\"");
    let Some(at) = src.find(&key) else {
        return Vec::new();
    };
    let rest = &src[at + key.len()..];
    rest[..rest.find('"').unwrap_or(0)]
        .split_whitespace()
        .collect()
}

pub fn topics() -> Vec<&'static str> {
    list(GUIDE, "_ZEN_GUIDE_TOPICS")
}

pub fn examples() -> Vec<&'static str> {
    list(GUIDE, "_ZEN_GUIDE_EXAMPLES")
}

pub fn widget_examples() -> Vec<&'static str> {
    list(DATA, "_ZEN_GUIDE_WIDGET_EXAMPLES")
}

/// `_zen_guide_title ID`: its `echo "…"` line, empty for another id.
pub fn title(id: &str) -> String {
    let arm = format!("\n        {id}) echo \"");
    let Some(at) = GUIDE.find(&arm) else {
        return String::new();
    };
    let rest = &GUIDE[at + arm.len()..];
    rest[..rest.find("\" ;;").unwrap_or(0)].to_string()
}

/// `_zen_guide_page TOPIC` (and `index`).
pub fn page(topic: &str) -> Option<&'static str> {
    if topic == "index" {
        return function_page(GUIDE, "_zen_guide_index");
    }
    let func = format!("_zen_guide_page_{}", topic.replace('-', "_"));
    function_page(GUIDE, &func).or_else(|| function_page(DATA, &func))
}

/// `_zen_example_toml NAME`.
pub fn example_toml(name: &str) -> Option<&'static str> {
    let at = DATA.find("\n_zen_example_toml() {\n")?;
    heredoc_after(
        &DATA[at..],
        &format!("\n        {name})\n            cat <<'EOF'\n"),
    )
}

/// `_zen_example_page NAME`.
fn example_page(name: &str) -> String {
    format!(
        concat!(
            cli_name!(),
            " zen guide example {} — {}\n\nWrite it over a Garden's file, then validate:\n  ",
            cli_name!(),
            " zen guide example {} --toml > ~/.k2/zen/gardens/<id>.toml\n  ",
            cli_name!(),
            " zen validate --garden <id>\n\n{}"
        ),
        name,
        title(name),
        name,
        example_toml(name).unwrap_or_default()
    )
}

/// `$(…)` of a page: its trailing newlines gone.
fn held(s: &str) -> &str {
    s.trim_end_matches('\n')
}

fn compact(v: Value) -> String {
    json::to_string(
        &v,
        &Options {
            compact: true,
            ensure_ascii: false,
            ..Options::default()
        },
    )
    .unwrap_or_default()
}

fn obj(pairs: Vec<(&str, Value)>) -> Value {
    Value::Object(pairs.into_iter().map(|(k, v)| (Text::from(k), v)).collect())
}

/// `_zen_guide_emit_json ID TITLE [TOML]` on a page.
fn emit_json(id: &str, page: &str, toml: Option<&str>) -> String {
    let body = page.strip_suffix('\n').unwrap_or(page);
    let mut pairs = vec![
        ("id", print::jstr(id)),
        ("title", print::jstr(&title(id))),
        ("body", print::jstr(body)),
    ];
    if let Some(t) = toml {
        pairs.push(("toml", print::jstr(&format!("{}\n", held(t)))));
    }
    compact(obj(pairs))
}

fn out(text: String) -> Shown {
    Out {
        out: text,
        ..Out::default()
    }
    .done(Exit::Ok)
}

fn line(text: String) -> Shown {
    out(format!("{text}\n"))
}

fn guide_usage() -> &'static str {
    crate::text::help("zen/guide").expect("the zen guide help page is embedded")
}

/// `cmd_zen_guide …`: its words after `guide`.
pub fn guide(words: &[&str]) -> Shown {
    let (mut as_json, mut toml, mut list, mut script) = (false, false, false, false);
    let (mut p1, mut p2, mut extra) = ("", "", "");
    for w in words {
        match *w {
            "--json" => as_json = true,
            "--toml" => toml = true,
            "--list" => list = true,
            "--script" => script = true,
            _ if w.starts_with("--") => {
                return usage(format!(
                    concat!(
                        "Error: unknown flag '{}' (see ",
                        cli_name!(),
                        " zen guide --help)"
                    ),
                    w
                ))
            }
            _ => {
                if p1.is_empty() {
                    p1 = w;
                } else if p2.is_empty() {
                    p2 = w;
                } else {
                    extra = w;
                }
            }
        }
    }
    if !extra.is_empty() {
        return usage(format!(
            concat!(
                "Error: unexpected extra argument '{}' (see ",
                cli_name!(),
                " zen guide --help)"
            ),
            extra
        ));
    }
    if matches!(p1, "example" | "examples") {
        if list {
            if !p2.is_empty() || toml {
                return usage("Error: --list takes no example name");
            }
            if as_json {
                let ex: Vec<Value> = examples()
                    .into_iter()
                    .map(|n| {
                        obj(vec![
                            ("id", print::jstr(n)),
                            ("title", print::jstr(&title(n))),
                        ])
                    })
                    .collect();
                // This one is `json.dumps(…, separators=(",", ":"))`, ASCII.
                return line(
                    json::to_string(
                        &obj(vec![("examples", Value::Array(ex))]),
                        &Options {
                            compact: true,
                            ..Options::default()
                        },
                    )
                    .unwrap_or_default(),
                );
            }
            return out(examples().iter().map(|n| format!("{n}\n")).collect());
        }
        if p2.is_empty() {
            if toml {
                return usage(concat!(
                    "Error: --toml needs an example name (",
                    cli_name!(),
                    " zen guide example --list)"
                ));
            }
            let pg = page("examples").unwrap_or_default();
            return if as_json {
                line(emit_json("examples", pg, None))
            } else {
                out(pg.to_string())
            };
        }
        if widget_examples().contains(&p2) {
            if toml && script {
                return usage("Error: --toml or --script, not both");
            }
            let pg = function_page(DATA, "_zen_widget_example_page").unwrap_or_default();
            let tm = function_page(DATA, "_zen_widget_example_toml").unwrap_or_default();
            let sc = function_page(DATA, "_zen_widget_example_script").unwrap_or_default();
            if as_json {
                return line(compact(obj(vec![
                    ("id", print::jstr(p2)),
                    ("title", print::jstr(&title(p2))),
                    ("body", print::jstr(pg.trim_end_matches('\n'))),
                    ("toml", print::jstr(&format!("{}\n", held(tm)))),
                    ("script", print::jstr(&format!("{}\n", held(sc)))),
                ])));
            }
            return out(if toml {
                tm.to_string()
            } else if script {
                sc.to_string()
            } else {
                pg.to_string()
            });
        }
        if script {
            return usage(concat!(
                "Error: --script is for ",
                cli_name!(),
                " zen guide example board-lite"
            ));
        }
        if !examples().contains(&p2) {
            return usage(format!(
                "Error: no example '{p2}'. Examples: {} {}",
                list_text(GUIDE, "_ZEN_GUIDE_EXAMPLES"),
                list_text(DATA, "_ZEN_GUIDE_WIDGET_EXAMPLES")
            ));
        }
        let tm = example_toml(p2).unwrap_or_default();
        return if as_json {
            line(emit_json(p2, &example_page(p2), Some(tm)))
        } else if toml {
            out(tm.to_string())
        } else {
            out(example_page(p2))
        };
    }
    if toml || list || script {
        return usage(concat!(
            "Error: --toml, --list and --script are for ",
            cli_name!(),
            " zen guide example"
        ));
    }
    if !p2.is_empty() {
        return usage(format!(
            concat!(
                "Error: unexpected extra argument '{}' (see ",
                cli_name!(),
                " zen guide --help)"
            ),
            p2
        ));
    }
    let topic = if p1.is_empty() { "index" } else { p1 };
    if topic != "index" && !topics().contains(&topic) {
        return Shown::err(
            format!("Error: no guide topic '{topic}'\n{}", guide_usage()),
            Exit::Usage,
        );
    }
    let pg = page(topic).unwrap_or_default();
    if !as_json {
        return out(pg.to_string());
    }
    if topic == "index" {
        let ts: Vec<Value> = topics()
            .into_iter()
            .map(|t| {
                obj(vec![
                    ("id", print::jstr(t)),
                    ("title", print::jstr(&title(t))),
                ])
            })
            .collect();
        return line(compact(obj(vec![
            ("id", print::jstr("index")),
            ("title", print::jstr(&title("index"))),
            ("body", print::jstr(pg.trim_end_matches('\n'))),
            ("topics", Value::Array(ts)),
        ])));
    }
    line(emit_json(topic, pg, None))
}

/// `$NAME` as the shell expands it in a message (the value as written).
fn list_text(src: &'static str, name: &str) -> &'static str {
    let key = format!("\n{name}=\"");
    let Some(at) = src.find(&key) else {
        return "";
    };
    let rest = &src[at + key.len()..];
    &rest[..rest.find('"').unwrap_or(0)]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_page_example_and_title_is_found() {
        assert_eq!(topics().len(), 15, "{:?}", topics());
        for t in topics().into_iter().chain(["index"]) {
            let p = page(t).unwrap_or_else(|| panic!("no page for {t}"));
            assert!(p.ends_with('\n') && p.len() > 100, "{t}");
            assert!(!p.contains("\nEOF\n"), "{t}");
            assert!(!title(t).is_empty(), "{t}");
        }
        assert_eq!(examples().len(), 7);
        for e in examples() {
            let toml = example_toml(e).unwrap_or_else(|| panic!("no toml for {e}"));
            assert!(toml.starts_with("schema = 1\n"), "{e}");
            assert!(!title(e).is_empty(), "{e}");
        }
        assert_eq!(widget_examples(), vec!["board-lite"]);
        for f in [
            "_zen_widget_example_page",
            "_zen_widget_example_toml",
            "_zen_widget_example_script",
        ] {
            assert!(function_page(DATA, f).is_some(), "{f}");
        }
        assert_eq!(title("nope"), "");
    }
}
