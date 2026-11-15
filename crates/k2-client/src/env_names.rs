//! Settings read from the environment under more than one name.
//!
//! A setting that was named `K2SO_X` is now read as `K5_X` first and
//! `K2SO_X` after it, so an operator's old line keeps working. Each
//! setting keeps its list of names beside the code that reads it, newest
//! first, and asks [`first_set`].

/// The value of the first of `names` that is set, with the name it was
/// read under. A name that is set wins over the ones after it whatever
/// its value: an operator who sets the new name means it.
pub fn first_set(names: &[&'static str]) -> Option<(&'static str, String)> {
    first_set_in(names, |name| std::env::var(name).ok())
}

/// [`first_set`] over any lookup, so the rule can be tested without
/// touching the process environment.
pub fn first_set_in(names: &[&'static str], lookup: impl Fn(&str) -> Option<String>) -> Option<(&'static str, String)> {
    names.iter().find_map(|name| lookup(name).map(|v| (*name, v)))
}

#[cfg(test)]
mod tests {
    use super::*;

    const NAMES: &[&str] = &["K5_X", "K2_X", "K2SO_X"];

    fn env(pairs: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
        move |name| pairs.iter().find(|(k, _)| *k == name).map(|(_, v)| v.to_string())
    }

    #[test]
    fn nothing_set_is_none() {
        assert_eq!(first_set_in(NAMES, env(&[])), None);
        assert_eq!(first_set_in(NAMES, env(&[("K5_Y", "1")])), None);
        assert_eq!(first_set_in(&[], env(&[("K5_X", "1")])), None);
    }

    #[test]
    fn each_name_is_read_and_reported() {
        assert_eq!(first_set_in(NAMES, env(&[("K5_X", "a")])), Some(("K5_X", "a".to_string())));
        assert_eq!(first_set_in(NAMES, env(&[("K2_X", "b")])), Some(("K2_X", "b".to_string())));
        assert_eq!(first_set_in(NAMES, env(&[("K2SO_X", "c")])), Some(("K2SO_X", "c".to_string())));
    }

    #[test]
    fn the_earlier_name_wins_whatever_its_value() {
        assert_eq!(first_set_in(NAMES, env(&[("K2SO_X", "old"), ("K5_X", "new")])), Some(("K5_X", "new".to_string())));
        assert_eq!(first_set_in(NAMES, env(&[("K2SO_X", "old"), ("K2_X", "mid")])), Some(("K2_X", "mid".to_string())));
        assert_eq!(first_set_in(NAMES, env(&[("K2SO_X", "9"), ("K5_X", "")])), Some(("K5_X", String::new())), "set and empty still wins");
    }
}
