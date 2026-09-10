//! Shared page furniture for the generated pages.
//!
//! One palette, one type scale, one set of panel and table rules, so a second page cannot quietly
//! become a second design. Each generator adds only what is its own — the composition matrix
//! belongs to the arc page, the annotated answer to the reader.
//!
//! # Colour is tokenised for three theme states, not two
//!
//! Bare `:root` carries the light palette; the `prefers-color-scheme` block is guarded so an
//! explicit light choice beats a dark OS; and the `[data-theme="dark"]` block lets a toggle win
//! the other way. Nothing declares a colour outside the token set.

/// The shared stylesheet, kept as a file so it reads as CSS rather than as a Rust string.
pub const BASE: &str = include_str!("base.css");

/// Typeface links. Two preconnects, then one stylesheet.
pub const FONTS: &str = "<link rel=\"preconnect\" href=\"https://fonts.googleapis.com\">\n\
<link rel=\"preconnect\" href=\"https://fonts.gstatic.com\" crossorigin>\n\
<link rel=\"stylesheet\" href=\"https://fonts.googleapis.com/css2?\
family=IBM+Plex+Mono:wght@400;500;600&family=IBM+Plex+Sans:wght@400;500;600&display=swap\">";

/// The subject is fingerprints, so the mark is a whorl.
pub const FAVICON: &str = "<link rel=\"icon\" href=\"data:image/svg+xml,\
%3Csvg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 32 32'%3E\
%3Cg fill='none' stroke='%232c6086' stroke-width='2.5' stroke-linecap='round'%3E\
%3Cpath d='M16 26a10 10 0 0 1 0-20'/%3E%3Cpath d='M16 21a5 5 0 0 1 0-10'/%3E\
%3Cpath d='M16 16h10'/%3E%3C/g%3E%3C/svg%3E\">";

/// Escape for HTML text and attributes alike.
///
/// **This is a security control, not politeness.** Every generated page renders text nobody here
/// wrote — a model's answer, and record bodies that are attacker-reachable the moment anyone can
/// write into a tenant's book (`docs/INVARIANTS.md` §A-1). Quoting them is the difference between
/// showing evidence and executing it.
///
/// Both quote forms are escaped, not just the one a text node needs, because the same function is
/// used inside attributes and a reader who has to remember which is which will eventually forget.
///
/// The single quote is the half that was missing, and it was missing while this paragraph claimed
/// it was not. Every attribute in the generators happens to be double-quoted today, so nothing was
/// reachable — but "no page uses a single-quoted attribute" is a property of the pages, not of the
/// escaper, and the escaper is what the paragraph promises. `&#39;` rather than `&apos;`: the
/// named form is XHTML's and is not in the HTML 4 entity set, so the numeric one is the spelling
/// every parser agrees on.
pub fn esc(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

/// One page: the shared furniture, this page's own rules, and its content.
pub fn shell(title: &str, body: &str, css: &str) -> String {
    format!(
        "<!doctype html>\n\
<html lang=\"en\">\n\
<head>\n\
<meta charset=\"utf-8\">\n\
<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
<title>{}</title>\n\
{FAVICON}\n\
{FONTS}\n\
<style>{BASE}{css}</style>\n\
</head>\n\
<body>\n\
<div class=\"wrap\">\n\
{body}\n\
</div>\n\
</body>\n\
</html>\n",
        esc(title)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_dangerous_character_is_escaped() {
        assert_eq!(
            esc(r#"<script>alert("x" & 'y')</script>"#),
            "&lt;script&gt;alert(&quot;x&quot; &amp; &#39;y&#39;)&lt;/script&gt;"
        );
    }

    #[test]
    fn a_single_quoted_attribute_cannot_be_broken_out_of() {
        // The control for the half that was missing. A page that writes `class='…'` is not written
        // today, and this is what keeps the escaper safe for the one that is written tomorrow —
        // the guarantee has to belong to this function rather than to a habit in its callers.
        let attacker = "x' onerror='alert(1)";
        let escaped = esc(attacker);
        assert!(!escaped.contains('\''), "{escaped}");
        assert_eq!(escaped, "x&#39; onerror=&#39;alert(1)");
    }

    #[test]
    fn the_ampersand_is_escaped_first() {
        // Escaping `<` before `&` would turn `&lt;` into `&amp;lt;` and show the entity rather
        // than the character. One pass over the input is what makes the order unarguable.
        assert_eq!(esc("&lt;"), "&amp;lt;");
    }

    #[test]
    fn a_record_body_cannot_reach_the_page_as_markup() {
        // The property §A-1 depends on: an activity body is evidence to display, never markup to
        // execute.
        //
        // **This tests the escaper, not the generators.** The sentence that used to end this
        // comment — "this is the whole reason the generators never interpolate raw text" — is a
        // claim about `crates/site`, and `esc` is opt-in at every interpolation there, so a
        // `format!` that forgot one would leave this test green.
        //
        // `crates/site/src/read/render.rs::tests::no_field_of_a_book_or_an_answer_reaches_the_page_as_markup`
        // is where that claim is actually checked: it renders a real case with the payload in
        // every field a book and a transcript carry, and fails if any `esc` is dropped.
        let attacker = "<img src=x onerror=alert(1)>";
        assert!(!esc(attacker).contains('<'));
    }

    #[test]
    fn a_title_is_escaped_in_the_shell() {
        let html = shell("a <b> title", "body", "");
        assert!(html.contains("<title>a &lt;b&gt; title</title>"), "{html}");
    }

    #[test]
    fn the_shell_carries_the_shared_stylesheet_and_the_pages_own() {
        let html = shell("t", "body", ".mine { color: var(--ink); }");
        assert!(html.contains(":root"), "the shared palette must be present");
        assert!(html.contains(".mine"), "the page's own rules must follow it");
        assert!(html.find(":root") < html.find(".mine"), "shared first, so a page can override");
    }

    #[test]
    fn the_palette_defines_light_before_any_media_query() {
        // Three theme states, not two: bare `:root` is the light palette, and both the OS query
        // and the explicit toggle override it. A colour defined only inside a media block would
        // be undefined in the third state.
        let root_at = BASE.find(":root").expect("a bare :root");
        let media_at = BASE.find("@media").unwrap_or(BASE.len());
        assert!(root_at < media_at, "the light palette must not live inside a media query");
    }
}
