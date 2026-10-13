//! Cryptographically secure password generator.

use rand::rngs::OsRng;
use rand::seq::SliceRandom;
use zeroize::Zeroizing;

use crate::error::{Error, Result};

const LOWER: &str = "abcdefghijklmnopqrstuvwxyz";
const UPPER: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZ";
const DIGITS: &str = "0123456789";
const SYMBOLS: &str = "!@#$%^&*()-_=+[]{};:,.<>?/~";
const AMBIGUOUS: &str = "Il1O0o|`'\"";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GeneratorOptions {
    pub length: usize,
    pub lowercase: bool,
    pub uppercase: bool,
    pub digits: bool,
    pub symbols: bool,
    pub avoid_ambiguous: bool,
}

impl Default for GeneratorOptions {
    fn default() -> Self {
        GeneratorOptions {
            length: 20,
            lowercase: true,
            uppercase: true,
            digits: true,
            symbols: true,
            avoid_ambiguous: true,
        }
    }
}

pub const MIN_LENGTH: usize = 8;
pub const MAX_LENGTH: usize = 256;

/// Generate a password with at least one character from every enabled class,
/// drawn uniformly from the OS CSPRNG.
pub fn generate_password(opts: &GeneratorOptions) -> Result<Zeroizing<String>> {
    if !(MIN_LENGTH..=MAX_LENGTH).contains(&opts.length) {
        return Err(Error::InvalidInput(format!(
            "length must be between {MIN_LENGTH} and {MAX_LENGTH}"
        )));
    }
    let classes: Vec<Vec<char>> = [
        (opts.lowercase, LOWER),
        (opts.uppercase, UPPER),
        (opts.digits, DIGITS),
        (opts.symbols, SYMBOLS),
    ]
    .iter()
    .filter(|(on, _)| *on)
    .map(|(_, set)| {
        set.chars()
            .filter(|c| !opts.avoid_ambiguous || !AMBIGUOUS.contains(*c))
            .collect()
    })
    .collect();
    if classes.is_empty() {
        return Err(Error::InvalidInput("enable at least one character class".into()));
    }
    let all: Vec<char> = classes.iter().flatten().copied().collect();
    let mut rng = OsRng;
    let mut chars: Zeroizing<Vec<char>> = Zeroizing::new(Vec::with_capacity(opts.length));
    for class in &classes {
        chars.push(*class.choose(&mut rng).expect("class not empty"));
    }
    while chars.len() < opts.length {
        chars.push(*all.choose(&mut rng).expect("alphabet not empty"));
    }
    chars.shuffle(&mut rng);
    Ok(Zeroizing::new(chars.iter().collect()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn respects_options() {
        for _ in 0..50 {
            let p = generate_password(&GeneratorOptions::default()).unwrap();
            assert_eq!(p.chars().count(), 20);
            assert!(p.chars().any(|c| c.is_ascii_lowercase()));
            assert!(p.chars().any(|c| c.is_ascii_uppercase()));
            assert!(p.chars().any(|c| c.is_ascii_digit()));
            assert!(p.chars().any(|c| SYMBOLS.contains(c)));
            assert!(!p.chars().any(|c| AMBIGUOUS.contains(c)));
        }
        let digits = generate_password(&GeneratorOptions {
            length: 12,
            lowercase: false,
            uppercase: false,
            digits: true,
            symbols: false,
            avoid_ambiguous: false,
        })
        .unwrap();
        assert!(digits.chars().all(|c| c.is_ascii_digit()));
    }

    #[test]
    fn rejects_bad_options() {
        let o = GeneratorOptions {
            length: 4,
            ..Default::default()
        };
        assert!(generate_password(&o).is_err());
        let o = GeneratorOptions {
            lowercase: false,
            uppercase: false,
            digits: false,
            symbols: false,
            ..Default::default()
        };
        assert!(generate_password(&o).is_err());
    }

    #[test]
    fn outputs_differ() {
        let a = generate_password(&GeneratorOptions::default()).unwrap();
        let b = generate_password(&GeneratorOptions::default()).unwrap();
        assert_ne!(*a, *b);
    }
}
