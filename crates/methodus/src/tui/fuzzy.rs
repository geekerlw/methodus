//! Tiny subsequence fuzzy matcher for overlay lists. No extra crate.

/// Case-insensitive subsequence match. Empty needle matches everything.
pub fn fuzzy_match(needle: &str, haystack: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    let n = needle.to_ascii_lowercase();
    let h = haystack.to_ascii_lowercase();
    let mut it = h.chars();
    for ch in n.chars() {
        loop {
            match it.next() {
                Some(c) if c == ch => break,
                Some(_) => continue,
                None => return false,
            }
        }
    }
    true
}

/// Rank: earlier contiguous hits score higher. `None` if no match.
pub fn fuzzy_score(needle: &str, haystack: &str) -> Option<i32> {
    if needle.is_empty() {
        return Some(0);
    }
    if !fuzzy_match(needle, haystack) {
        return None;
    }
    let n = needle.to_ascii_lowercase();
    let h = haystack.to_ascii_lowercase();
    if let Some(i) = h.find(&n) {
        return Some(1000 - i as i32);
    }
    Some(h.find(n.chars().next()?)? as i32 * -1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_matches_all() {
        assert!(fuzzy_match("", "anything"));
    }

    #[test]
    fn subsequence() {
        assert!(fuzzy_match("ssn", "sessions"));
        assert!(fuzzy_match("INB", "inbox"));
        assert!(!fuzzy_match("xyz", "inbox"));
    }

    #[test]
    fn contiguous_ranks_higher() {
        let a = fuzzy_score("run", "running task").unwrap();
        let b = fuzzy_score("run", "r u n scattered").unwrap();
        assert!(a > b);
    }
}
