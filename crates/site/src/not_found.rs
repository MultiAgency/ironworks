//! The page an unmatched path gets.
//!
//! **Not decoration.** Without one, Cloudflare Pages answers every unknown path with 200 and the
//! site's index — so a mistyped or dead link reports that it exists and serves something else
//! entirely. A site whose every URL claims to be there is the same defect class as a status page
//! with only two colours.

use ironworks_page::shell;

const CSS: &str = "
.wrap { max-width: 60ch; padding-top: 96px; }
.four { font-size: 13px; letter-spacing: 0.14em; }
h1 { font-size: 30px; font-weight: 600; margin: 14px 0 12px; letter-spacing: -0.015em; }
p { color: var(--muted); margin: 0 0 26px; }
nav { display: flex; gap: 22px; }
nav a { color: var(--steel); text-decoration: none; font-weight: 500; }
nav a:hover { text-decoration: underline; }
nav a:focus-visible { outline: 2px solid var(--steel); outline-offset: 3px; border-radius: 2px; }
";

pub fn render() -> String {
    shell(
        "Not Found",
        r#"  <span class="label mono four">404</span>
  <h1>Nothing is published here.</h1>
  <p>The path does not match a page on this site. Nothing was moved and nothing broke —
  there was never anything at this address.</p>
  <nav>
    <a href="/">Account coordination</a>
    <a href="/arc/">Composition arc</a>
    <a href="/read/">Reading the answers</a>
  </nav>"#,
        CSS,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_404_says_nothing_was_ever_here() {
        // The distinction the page exists to draw: not "moved", not "broken" — never present.
        let html = render();
        assert!(html.contains("Nothing is published here."));
        assert!(html.contains("there was never anything at this address"));
    }

    #[test]
    fn it_carries_the_shared_furniture() {
        assert!(render().contains(":root"), "a second page must not become a second design");
    }
}
