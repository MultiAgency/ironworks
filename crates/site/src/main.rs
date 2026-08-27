//! `ironworks-site` — generate the published pages.
//!
//! ```text
//! ironworks-site demo      --html site/index.html
//! ironworks-site not-found --html site/404.html
//! ```
//!
//! # The 404 is not decoration
//!
//! Without one, Cloudflare Pages answers every unmatched path with 200 and the site index — so a
//! dead or mistyped link reports that it exists and serves something else.
//!
//! # Nothing under `site/` is tracked
//!
//! A generated page that slips into a commit is a page that can disagree with the composition it
//! claims to describe.

mod arc;
mod demo;
mod markdown;
mod not_found;
mod operator;
mod read;
mod transcript;

const USAGE: &str = "usage: ironworks-site <page> --html <path>\n\n\
     \x20  demo        pick a question, watch it answered against its records\n\
     \x20  operator    the composition arc, and the reading of the answers beneath it\n\
     \x20  not-found   the 404\n";

fn main() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(page) = args.first() else {
        eprint!("{USAGE}");
        return std::process::ExitCode::from(64);
    };
    let out = match flag(&args, "--html") {
        Some(p) => std::path::PathBuf::from(p),
        None => {
            eprintln!("ironworks-site: --html <path> is required\n\n{USAGE}");
            return std::process::ExitCode::from(64);
        }
    };

    let root = ironworks_evidence::root();
    let rendered = match page.as_str() {
        "demo" => demo::build(&root),
        "operator" => operator::build(
            &root,
            "account-coordination",
            flag(&args, "--since").unwrap_or("HEAD"),
            flag(&args, "--transcript"),
            flag(&args, "--case"),
        ),
        "not-found" => Ok(not_found::render()),
        other => {
            eprintln!("ironworks-site: no page {other:?}\n\n{USAGE}");
            return std::process::ExitCode::from(64);
        }
    };

    match rendered {
        // BLOCKED, not a failure: the generator worked and the evidence did not permit publishing.
        Err(why) => {
            println!("BLOCKED  {why}");
            std::process::ExitCode::from(3)
        }
        Ok(html) => {
            if let Some(parent) = out.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            match std::fs::write(&out, html) {
                Ok(()) => {
                    println!("page written to {}", out.display());
                    std::process::ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("ironworks-site: writing {}: {e}", out.display());
                    std::process::ExitCode::from(2)
                }
            }
        }
    }
}

fn flag<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    let at = args.iter().position(|a| a == name)?;
    args.get(at + 1).map(String::as_str)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_flag_reads_the_value_after_it() {
        let args: Vec<String> =
            ["demo", "--html", "site/index.html"].iter().map(|s| (*s).to_owned()).collect();
        assert_eq!(flag(&args, "--html"), Some("site/index.html"));
        assert_eq!(flag(&args, "--missing"), None);
    }

    #[test]
    fn a_flag_with_no_value_is_absent_rather_than_empty() {
        let args = vec!["demo".to_owned(), "--html".to_owned()];
        assert_eq!(flag(&args, "--html"), None);
    }
}
