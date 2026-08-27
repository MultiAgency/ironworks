//! The version of record for the model and the runtime.
//!
//! # The rule
//!
//! **The first line, then everything before the first `#`, trimmed.** A pin file carries a
//! trailing comment explaining the choice; that comment is not the value.
//!
//! # Why there is no `Default`
//!
//! A default would be the one value that can silently outrank the configuration. The model pin
//! is a privacy claim — it records that the named model is TEE-hosted and was tested against a
//! real book on a stated date — so a stale literal could move a tenant's private records onto a
//! model with weaker guarantees, with nothing in the reply saying so. Neither [`ModelPin`] nor
//! [`IronclawPin`] implements `Default`, and neither has an infallible constructor.
//!
//! # A divergence in the prior art, resolved
//!
//! ironworks reads its pins twice, and the two readers do not agree. `multi/seam/pins.py` does
//! `raw.split("#", 1)[0].strip()` over the *whole file*; `deploy/lib/fleet.sh` does
//! `cut -d'#' -f1 | head -n1 | tr -d '[:space:]'`, which is the *first line*. Given
//! `"abc\ndef # x"` the Python yields `"abc\ndef"` and the shell yields `"abc"`. Its
//! `test_pins.py` asserts the two agree on the current pin, which carries a `#` on line one, so
//! the divergence never shows.
//!
//! We take the stricter reading: first line, then before `#`. A pin is one line by construction,
//! so a value can never contain a newline.

use std::fmt;

/// A pin file names nothing usable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PinError {
    /// The file is empty, or its first line is blank or comment-only.
    NamesNothing,
    /// The value carries a character a pin may not contain.
    Malformed { found: char },
}

impl fmt::Display for PinError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NamesNothing => f.write_str(
                "pin names nothing on its first line — a pin is tracked, so an unreadable or \
                 empty one means a broken checkout, not a reason to substitute a literal",
            ),
            Self::Malformed { found } => {
                write!(f, "pin contains {found:?}, which a pin value may not carry")
            }
        }
    }
}

impl std::error::Error for PinError {}

/// The parse rule, defined once: first line, then everything before the first `#`, trimmed.
///
/// Returns a borrowed slice so callers that only want to look do not allocate.
pub fn parse(raw: &str) -> Result<&str, PinError> {
    let first_line = raw.split('\n').next().unwrap_or("");
    let value = first_line.split('#').next().unwrap_or("").trim();
    if value.is_empty() {
        return Err(PinError::NamesNothing);
    }
    if let Some(found) = value.chars().find(|c| c.is_control()) {
        return Err(PinError::Malformed { found });
    }
    Ok(value)
}

macro_rules! pin_newtype {
    ($name:ident, $doc:literal) => {
        #[doc = $doc]
        ///
        /// Deliberately implements neither `Default` nor `From<&str>`: the only way in is
        /// [`parse`](Self::parse), which can fail.
        #[derive(Debug, Clone, PartialEq, Eq, Hash)]
        pub struct $name(String);

        impl $name {
            /// Parse from the contents of a pin file.
            pub fn parse(raw: &str) -> Result<Self, PinError> {
                parse(raw).map(|v| Self(v.to_owned()))
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }
    };
}

pin_newtype!(ModelPin, "The model of record. A behavioural and privacy claim, not a version.");
pin_newtype!(IronclawPin, "The runtime revision of record — what every image is built from.");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn takes_everything_before_the_first_hash() {
        assert_eq!(
            parse("Qwen/Qwen3.5-122B-A10B  # TEE-hosted").unwrap(),
            "Qwen/Qwen3.5-122B-A10B"
        );
    }

    #[test]
    fn the_delimiter_is_hash_not_whitespace() {
        // A rev followed immediately by a comment, no space. `cut -d' '` would keep the tag.
        assert_eq!(parse("deadbeef# ironclaw-v1.3.0").unwrap(), "deadbeef");
    }

    #[test]
    fn trailing_comment_lines_are_not_the_value() {
        let raw = "Qwen/Qwen3.5-122B-A10B  # TEE-hosted\n# PRIVACY: ...\n# COST: ...\n";
        assert_eq!(parse(raw).unwrap(), "Qwen/Qwen3.5-122B-A10B");
    }

    #[test]
    fn a_bare_value_with_no_comment_is_fine() {
        assert_eq!(
            parse("70795c16ed0cec21eb8cba16d2dcf851d25dc83d\n").unwrap(),
            "70795c16ed0cec21eb8cba16d2dcf851d25dc83d"
        );
    }

    #[test]
    fn empty_and_comment_only_files_name_nothing() {
        assert_eq!(parse(""), Err(PinError::NamesNothing));
        assert_eq!(parse("\n\n"), Err(PinError::NamesNothing));
        assert_eq!(parse("# just a comment\nvalue"), Err(PinError::NamesNothing));
        assert_eq!(parse("   \n"), Err(PinError::NamesNothing));
    }

    /// The case where ironworks' two readers disagree. We take the shell's answer.
    ///
    /// `multi/seam/pins.py` splits the whole file and would yield `"abc\ndef"`, a value with a
    /// newline in it. `deploy/lib/fleet.sh` takes the first line and yields `"abc"`. Neither
    /// repo's test covers this, because the real pin files carry a `#` on line one.
    #[test]
    fn a_pin_is_one_line() {
        assert_eq!(parse("abc\ndef # x").unwrap(), "abc");
        assert!(!parse("abc\ndef # x").unwrap().contains('\n'));
    }

    #[test]
    fn newtypes_have_no_default_and_no_infallible_constructor() {
        // This test is documentation: the assertions that matter are the ones that do not
        // compile. `ModelPin::default()` and `ModelPin::from("...")` are both absent by design,
        // so a fallback literal has no syntax to be written in.
        assert!(ModelPin::parse("# comment only").is_err());
        assert_eq!(ModelPin::parse("m # c").unwrap().as_str(), "m");
        assert_eq!(IronclawPin::parse("abc123 # v1").unwrap().to_string(), "abc123");
    }

    /// The real files, so a checkout that breaks them fails here rather than at a tenant turn.
    ///
    /// The literal is the point: a pin bump has to be a deliberate edit in two places, and this
    /// test is where "we upgraded" stops being a silent consequence of editing one file. It did
    /// its job on the 1.3.0 -> 1.4.0 move, which is the only reason this comment exists.
    #[test]
    fn the_committed_pins_parse() {
        let model = include_str!("../../../MODEL_PIN");
        let rev = include_str!("../../../IRONCLAW_PIN");
        assert_eq!(ModelPin::parse(model).unwrap().as_str(), "qwen/qwen3.5-397b-a17b");
        assert_eq!(
            IronclawPin::parse(rev).unwrap().as_str(),
            "4cb47cfaf3fbacd3a2f575a8da654a6542d5fdfe"
        );
    }
}
