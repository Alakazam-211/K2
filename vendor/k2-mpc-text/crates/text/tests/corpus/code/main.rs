//! A tiny job queue: the shape of a real crate's main file.

use std::collections::VecDeque;
use std::fmt;

/// One unit of work. `id` is unique per queue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Job {
    pub id: u64,
    pub name: String,
}

impl fmt::Display for Job {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{} {}", self.id, self.name)
    }
}

fn main() {
    let mut queue: VecDeque<Job> = VecDeque::new();
    for (i, name) in ["build", "test", "ship \u{1F680}"].iter().enumerate() {
        queue.push_back(Job { id: i as u64, name: (*name).to_owned() });
    }
    while let Some(job) = queue.pop_front() {
        println!("running {job}"); // trailing spaces follow   
    }
    let raw = r#"a "raw" string with a \t tab"#;
    assert!(!raw.is_empty());
}
