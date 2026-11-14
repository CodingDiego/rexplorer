//! Deterministic fuzz input for the parser tests: a seeded xorshift generator, no dependency. A failing case
//! reproduces from its seed alone. The checks both the seeded tests and fuzz/'s cargo-fuzz targets run:
pub use crate::archive::fuzz_check as archive_member;
pub use crate::cli::fuzz_check as command_line;
pub use crate::dragdrop::parse_dropfiles as drop_files;
pub use crate::i18n::fuzz_check as language_file;
pub use crate::query::fuzz_check as search_query;
pub use crate::settings::fuzz_check as ini_file;
pub use crate::update::fuzz_check as update_response;

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

    /// Hostile shapes the seeded junk rarely builds: huge lines, NUL, a lone surrogate (lossy-decoded, as every
    /// file read does), deep nesting and numbers past every integer width.
    fn hostile() -> Vec<String> {
        let surrogate = String::from_utf16_lossy(&[0xD800, b'a' as u16, 0xDC00, 0xD83D]);
        let big = "99999999999999999999999999999999";
        let mut v = vec![
            "a".repeat(1 << 20),
            "[".repeat(100_000),
            "=".repeat(100_000),
            format!("k={}", "v".repeat(1 << 20)),
            format!("[{}]\nk=v", "s".repeat(1 << 16)),
            "\n".repeat(100_000),
            "\0".into(),
            format!("[\0]\n\0=\0\n{surrogate}={surrogate}"),
            surrogate.clone(),
            "{".repeat(100_000) + "\"tag_name\":\"v1.2.3\"",
            format!("{{\"tag_name\": \"{big}.{big}.{big}\"}}"),
            format!("{{\"tag_name\": \"{}\"}}", "9.".repeat(100_000)),
            "\"tag_name\":\"".into(),
            format!("size:>{big}kb size:{big}..{big} size:18446744073709551615gb size:-{big} size:1e999 size:{}", "9".repeat(5000)),
            format!("date:{big}-1-1 date:>{big} date:{big} days ago date:last {big} date:9223372036854775807 date:-{big}"),
            format!("ext:{} kind:\0 \"{}", ";".repeat(100_000), "x".repeat(1 << 16)),
            "\"".repeat(100_001),
            format!("#{big} #FFFFF\0 {surrogate}"),
            format!("0:{big}"),
        ];
        v.push(v[0].replace('a', "\u{202e}\u{1F600}"));
        v
    }

    #[test]
    fn hostile_inputs_through_every_text_parser() {
        for t in hostile() {
            ini_file(&t);
            language_file(&t);
            archive_member(&t);
            search_query(&t);
            update_response(t.as_bytes());
            drop_files(t.as_bytes());
            let parts: Vec<String> = t.split('\n').take(6).map(String::from).collect();
            command_line(&t, &parts, &parts, true);
        }
    }

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
