//! Fuzzy matching for the folder palette, plus the recent-folders list it ranks. Pure Rust, no Win32.

use std::path::PathBuf;

const RECENT_CAP: usize = 200;

fn lower(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
}

fn is_sep(c: char) -> bool {
    matches!(c, '\\' | '/' | '_' | '-' | '.' | ' ')
}

/// fzf-like subsequence score; `None` unless every query char appears in order.
/// Leftmost greedy match (shortcut: not optimal alignment; upgrade if ranking feels off).
pub fn score(candidate: &str, query: &str) -> Option<i32> {
    let mut q = query.chars().map(lower).peekable();
    let mut total = 0i32;
    let mut prev: Option<usize> = None; // index of previous match
    let mut prev_char: Option<char> = None;
    for (i, c) in candidate.chars().enumerate() {
        if let Some(&want) = q.peek() {
            if lower(c) == want {
                q.next();
                total += 16;
                if prev_char.map_or(true, is_sep) {
                    total += 8;
                }
                match prev {
                    Some(p) if p + 1 == i => total += 8,
                    Some(p) => total -= (i - p - 1) as i32,
                    None => {}
                }
                prev = Some(i);
            }
        }
        prev_char = Some(c);
    }
    if q.peek().is_some() {
        return None;
    }
    if query.is_empty() {
        return Some(0);
    }
    Some(total - (candidate.len() / 8) as i32)
}

pub struct Candidate {
    pub text: String,
    pub target: String,
}

pub fn rank<'a>(cands: &'a [Candidate], query: &str, limit: usize) -> Vec<(&'a Candidate, i32)> {
    let mut v: Vec<_> = cands.iter().filter_map(|c| score(&c.text, query).map(|s| (c, s))).collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.text.cmp(&b.0.text)));
    v.truncate(limit);
    v
}

pub struct Recent {
    path: PathBuf,
    items: Vec<String>,
}

impl Recent {
    pub fn load(path: PathBuf) -> Recent {
        let items = std::fs::read_to_string(&path)
            .map(|s| s.lines().filter(|l| !l.is_empty()).map(String::from).collect())
            .unwrap_or_default();
        Recent { path, items }
    }

    pub fn push(&mut self, target: &str) {
        self.remove(target);
        self.items.insert(0, target.to_string());
        self.items.truncate(RECENT_CAP);
    }

    pub fn items(&self) -> &[String] {
        &self.items
    }

    pub fn save(&self) {
        let _ = std::fs::write(&self.path, self.items.join("\n"));
    }

    pub fn clear(&mut self) {
        self.items.clear();
    }

    pub fn remove(&mut self, target: &str) {
        let t = target.to_lowercase();
        self.items.retain(|i| i.to_lowercase() != t);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn score_prefers_word_starts() {
        assert!(score("Documents", "doc") > score("dot config", "doc"));
        assert_eq!(score("xyz", "a"), None);
        let wr = score("WORK-RELATED", "wr");
        assert!(wr.is_some() && wr > score("awareness", "wr"));
    }

    #[test]
    fn rank_sorts_and_limits() {
        let c = |t: &str| Candidate { text: t.into(), target: t.into() };
        let cands = [c("a_dot_config"), c("Documents"), c("docs")];
        let r = rank(&cands, "doc", 2);
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].0.text, "docs");
        assert_eq!(r[1].0.text, "Documents");
    }

    #[test]
    fn recent_round_trips() {
        let dir = std::env::temp_dir().join(format!("rexplorer-fuzzy-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("recent.txt");
        let mut r = Recent::load(f.clone());
        assert!(r.items().is_empty());
        r.push("a");
        r.push("b");
        r.push("A");
        assert_eq!(r.items(), ["A", "b"]);
        r.save();
        let mut r = Recent::load(f);
        assert_eq!(r.items(), ["A", "b"]);
        r.remove("B");
        assert_eq!(r.items(), ["A"]);
        for i in 0..250 {
            r.push(&format!("p{i}"));
        }
        assert_eq!(r.items().len(), 200);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
