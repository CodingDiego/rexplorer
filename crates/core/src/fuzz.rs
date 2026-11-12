//! Deterministic fuzz input for the parser tests: a seeded xorshift generator, no dependency. A failing case
//! reproduces from its seed alone. The checks both the seeded tests and fuzz/'s cargo-fuzz targets run:
pub use crate::archive::fuzz_check as archive_member;
pub use crate::cli::fuzz_check as command_line;
pub use crate::i18n::fuzz_check as language_file;
pub use crate::settings::fuzz_check as ini_file;

pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        // xorshift never leaves state 0; the odd multiplier spreads small seeds.
        Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }

    pub fn step(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    pub fn below(&mut self, n: usize) -> usize {
        (self.step() % n.max(1) as u64) as usize
    }

    /// Up to `max` tokens from `tokens`, concatenated: structured junk that reaches the parser's branches.
    pub fn text(&mut self, tokens: &[&str], max: usize) -> String {
        let n = self.below(max + 1);
        (0..n).map(|_| tokens[self.below(tokens.len())]).collect()
    }
}

/// Pieces of ini, theme, language, session and payload files plus the characters that trip parsers.
pub const TOKENS: &[&str] = &[
    "[", "]", "=", "\n", "\r\n", "\t", ";", "#", " ", "\u{feff}", "\0", "\\", "\\n", "\\\\", "{", "}", "..", "/", ":", "\"",
    "theme", "general", "keys", "profile ", "default", "format", "dark", "mica", "background", "surface", "text", "accent",
    "font_size", "icon_theme", "language", "true", "false", "#FF00AA", "#12345", "#GGGGGG", "0", "1", "7", "-1", "4294967296",
    "99999999999999999999", "tab", "active", "view", "details", "tree_width", "v", "path", "select", "search", "newtab",
    "C:", "C:\\", "C:\\dir\\file.txt", "\u{e9}", "\u{1F600}", "\u{202e}", "-", ",", "0:100",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_seed_same_stream_and_bounded() {
        let (mut a, mut b) = (Rng::new(3), Rng::new(3));
        assert!((0..50).all(|_| a.step() == b.step()));
        assert_ne!(Rng::new(1).step(), Rng::new(2).step());
        let mut r = Rng::new(0);
        assert!((0..1000).all(|_| r.below(7) < 7));
        assert_eq!(r.below(0), 0);
        assert!(r.text(TOKENS, 5).len() < 5 * 32);
    }
}
