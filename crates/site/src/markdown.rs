//! The one piece of markdown the rendered answers need.
//!
//! Not a markdown implementation. The reader marks each sentence itself and needs only `**bold**`
//! restored inside it, with the evidence-tier words rendered as labels instead.
//!
//! **This was larger.** It also rendered paragraphs, bullet lists and `*emphasis*` for the narrated
//! page, which is gone — the demo shows answers through the reader, which does its own block
//! structure so that a bullet naming nothing reads differently from a heading naming nothing.
//! What remains is what has a caller.
//!
//! # Escaping comes first, always
//!
//! The input is a model's answer, and the records it quotes are attacker-reachable
//! (`docs/INVARIANTS.md` §A-1). Every caller escapes before calling here, and this only ever
//! *adds* markup to text that is already inert — adding markup first and escaping after would be
//! the injection these pages exist to disprove.

/// `**bold**` over already-escaped text, with the named tiers rendered as labels instead.
///
/// The tier set is a parameter because it is not universal: the published page recognised
/// `OUTSTANDING` and the reader does not. One rule, two vocabularies — a second copy of the rule
/// would be a second thing to disagree with the first.
pub fn replace_bold(escaped: &str, tiers: &[&str]) -> String {
    replace_delimited(escaped, "**", |inner| {
        if tiers.contains(&inner) {
            format!("<span class=\"lbl {inner}\">{inner}</span>")
        } else {
            format!("<strong>{inner}</strong>")
        }
    })
}

/// Replace every `delim…delim` span, leaving unmatched delimiters alone.
///
/// An unterminated delimiter is literal text, not the start of a span that swallows the rest of
/// the answer — the same rule the quote stripper in `crates/graders` follows, for the same reason.
fn replace_delimited(text: &str, delim: &str, wrap: impl Fn(&str) -> String) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find(delim) {
        let after = &rest[open + delim.len()..];
        let Some(close) = after.find(delim) else {
            break;
        };
        let inner = &after[..close];
        // `****` and `**` with nothing between are not emphasis.
        if inner.is_empty() {
            out.push_str(&rest[..open + delim.len()]);
            rest = after;
            continue;
        }
        out.push_str(&rest[..open]);
        out.push_str(&wrap(inner));
        rest = &after[close + delim.len()..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const TIERS: &[&str] = &["FACT", "STATED", "UNKNOWN"];

    #[test]
    fn evidence_tiers_render_as_labels_not_as_bold() {
        assert_eq!(replace_bold("**FACT**", TIERS), r#"<span class="lbl FACT">FACT</span>"#);
        assert_eq!(replace_bold("**other**", TIERS), "<strong>other</strong>");
    }

    #[test]
    fn the_tier_vocabulary_is_the_callers_to_choose() {
        // `OUTSTANDING` is a tier to one caller and ordinary emphasis to another.
        assert!(replace_bold("**OUTSTANDING**", TIERS).contains("<strong>"));
        assert!(replace_bold("**OUTSTANDING**", &["OUTSTANDING"]).contains("lbl OUTSTANDING"));
    }

    #[test]
    fn markup_is_only_ever_added_to_text_that_is_already_inert() {
        // Callers escape first. This must not undo that — it introduces its own tags and copies
        // everything else through untouched.
        let escaped = "&lt;img src=x onerror=alert(1)&gt; and **bold**";
        let html = replace_bold(escaped, TIERS);
        assert!(html.contains("&lt;img"), "{html}");
        assert!(!html.contains("<img"), "{html}");
        assert!(html.contains("<strong>bold</strong>"), "{html}");
    }

    #[test]
    fn an_unterminated_delimiter_is_literal_text() {
        // Otherwise one stray pair of asterisks swallows the rest of the answer.
        assert_eq!(replace_bold("a ** dangling", TIERS), "a ** dangling");
    }

    #[test]
    fn an_empty_span_is_not_emphasis() {
        assert_eq!(replace_bold("****", TIERS), "****");
    }
}
