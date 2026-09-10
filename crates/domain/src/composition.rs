//! The composition concerns: what an alloy is made of, and how it is fused.
//!
//! A [`Composition`] declares *which* parts, in what order, with which heading and tail. It
//! holds no content — [`PartSource`] resolves that, so the domain can be composed against a fake
//! and the embedded service library is an adapter rather than a dependency.
//!
//! # Order is the product
//!
//! ```text
//! <parts, in declared order>
//! ---
//! <guidance heading>
//!
//! <validated guidance>
//! ---
//! <safety tail>          <- always last
//! ```
//!
//! Joined with `\n\n---\n\n`. These bytes are what the model reads: changing them changes
//! behaviour and means re-earning the evaluation. Do not tidy this.

use sha2::{Digest, Sha256};

/// A path into the embedded service library. A reference, never content.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PartRef(String);

impl PartRef {
    pub fn new(path: impl Into<String>) -> Self {
        Self(path.into())
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for PartRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Resolves a [`PartRef`] to its text. Implemented by the services crate.
pub trait PartSource {
    fn read(&self, part: &PartRef) -> Option<&str>;
}

/// Which parts compose a service's instructions, and how they are arranged.
///
/// The three fields are one concern, so they are one type. Held separately they were three
/// fields of a service definition and the ordering rule lived in whatever function happened to
/// assemble them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Composition {
    parts: Vec<PartRef>,
    guidance_heading: String,
    safety_tail: PartRef,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum CompositionError {
    #[error("a composition needs at least one part")]
    NoParts,
    #[error("a composition needs a guidance heading; it is model-visible and cited back")]
    NoHeading,
}

impl Composition {
    pub fn new(
        parts: Vec<PartRef>,
        guidance_heading: impl Into<String>,
        safety_tail: PartRef,
    ) -> Result<Self, CompositionError> {
        let guidance_heading = guidance_heading.into();
        if parts.is_empty() {
            return Err(CompositionError::NoParts);
        }
        if guidance_heading.trim().is_empty() {
            return Err(CompositionError::NoHeading);
        }
        Ok(Self { parts, guidance_heading, safety_tail })
    }

    pub fn parts(&self) -> &[PartRef] {
        &self.parts
    }
    pub fn guidance_heading(&self) -> &str {
        &self.guidance_heading
    }
    pub fn safety_tail(&self) -> &PartRef {
        &self.safety_tail
    }

    /// Every part this composition names, in composition order, safety tail last.
    pub fn all_parts(&self) -> impl Iterator<Item = &PartRef> {
        self.parts.iter().chain(std::iter::once(&self.safety_tail))
    }

    /// Whether every named part exists in this source. Checked at load, so a composition naming
    /// a missing part fails at startup rather than mid-turn.
    pub fn missing_in(&self, source: &dyn PartSource) -> Vec<&PartRef> {
        self.all_parts().filter(|p| source.read(p).is_none()).collect()
    }

    /// Each part's digest, in composition order, safety tail last.
    ///
    /// An alloy digest says only *that* a composition moved. These say **which ingredient**
    /// moved, which is the difference between a fact an operator can act on and one they cannot
    /// — an answer-quality result is bound to an alloy digest, so when the digest shifts the next
    /// question is always what shifted under it.
    ///
    /// Each digest covers exactly the bytes [`melt`] fuses for that part, frontmatter stripping
    /// included. That mirroring is load-bearing rather than tidy: a digest taken over different
    /// bytes than the alloy would attribute a move to the wrong ingredient, or miss one entirely.
    pub fn part_digests(
        &self,
        source: &dyn PartSource,
    ) -> Result<Vec<(PartRef, String)>, MeltError> {
        let mut out = Vec::with_capacity(self.parts.len() + 1);
        for part in &self.parts {
            let body = source.read(part).ok_or_else(|| MeltError { part: part.clone() })?;
            out.push((part.clone(), digest16(strip_frontmatter(body).as_bytes())));
        }
        let tail = source
            .read(&self.safety_tail)
            .ok_or_else(|| MeltError { part: self.safety_tail.clone() })?;
        out.push((self.safety_tail.clone(), digest16(tail.as_bytes())));
        Ok(out)
    }
}

// ── guidance ─────────────────────────────────────────────────────────────────────────────

/// A tenant's validated business guidance.
///
/// Constructible only by [`Guidance::parse`], so holding one means it was bound to the right
/// instance and the right service and carries real content. **Guidance is mandatory** — there is
/// no unguided composition, and no `Option<Guidance>` anywhere on the serving path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Guidance {
    content: String,
    /// The marker's declared fact keys. Stripped from the model-visible content, so this travels
    /// beside it rather than inside it.
    fact_fields: Vec<String>,
}

/// The first-line binding a guidance file must carry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuidanceMarker {
    pub slug: String,
    /// An absent service pins the default, so files written before service definitions existed
    /// keep validating unchanged.
    pub service: Option<String>,
    /// Fact keys this tenant expects, in declared order. Empty when the marker declared none —
    /// and equally when it declared `[]`, which is the same instruction here.
    pub facts: Vec<String>,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum GuidanceError {
    #[error(
        "instance {slug:?}: guidance lacks the required first-line marker \
         '<!-- tenant-guidance v1 slug: {slug} -->'"
    )]
    NoMarker { slug: String },

    #[error(
        "instance {slug:?}: guidance is bound to slug {found:?} — refusing to cross-wire one \
         tenant's guidance into another's composition"
    )]
    WrongSlug { slug: String, found: String },

    #[error(
        "instance {slug:?}: guidance is bound to service {declared:?} but this instance runs \
         {configured:?} — refusing to run on a composition its own guidance was not written for. \
         Fix whichever is wrong; do not just make them match."
    )]
    WrongService { slug: String, declared: String, configured: String },

    #[error(
        "instance {slug:?}: guidance declares no service — write 'service: {configured}' into the \
         marker. There is no default to mean, and a file that names none is not a file that names \
         this one."
    )]
    NoService { slug: String, configured: String },

    #[error(
        "instance {slug:?}: guidance holds {len} characters of content, below the {min} minimum \
         — fill in the template rather than serving an empty brief"
    )]
    TooShort { slug: String, len: usize, min: usize },

    #[error(
        "instance {slug:?}: guidance contains a stray HTML comment terminator after {bytes} \
         model-visible bytes — operator notes escaped their comment"
    )]
    LeakedComment { slug: String, bytes: usize },

    #[error("instance {slug:?}: guidance contains {count} unwritten template stub(s)")]
    UnwrittenStubs { slug: String, count: usize },
}

/// Below this, a guidance file is a template nobody filled in.
const MIN_CONTENT: usize = 400;
const STUB_MARKER: &str = "TO BE WRITTEN";

impl Guidance {
    /// This guidance's digest, in the same shape as [`Alloy::digest`].
    ///
    /// An alloy fuses scrap *and* guidance, so a digest can move with no repo diff at all. This
    /// is how that case is told apart from a scrap change — without printing a tenant's own
    /// words to a terminal, a log or a pasted report.
    pub fn digest(&self) -> String {
        digest16(self.content.as_bytes())
    }

    pub fn as_str(&self) -> &str {
        &self.content
    }

    /// The fact keys this tenant declared, in declared order.
    ///
    /// Empty covers both "the marker declared none" and "the marker declared `[]`". They are one
    /// instruction in this domain: assert no gaps. The upstream registry separates them because
    /// an undeclared key there selects a legacy sales-shaped fallback; nothing here has one, and
    /// [`crate::record::AccountRecord::declared_gaps`] reports nothing for either.
    pub fn fact_fields(&self) -> &[String] {
        &self.fact_fields
    }

    /// Read the marker without validating anything else.
    pub fn marker(text: &str) -> Option<GuidanceMarker> {
        let line = text.trim_start().lines().next()?;
        let body = line.trim().strip_prefix("<!--")?.strip_suffix("-->")?.trim();
        let rest = body.strip_prefix("tenant-guidance")?.trim_start();
        let rest = rest.strip_prefix("v1")?.trim_start();
        let rest = rest.strip_prefix("slug:")?.trim_start();

        let (slug, rest) = match rest.split_once(char::is_whitespace) {
            Some((s, r)) => (s, r.trim_start()),
            None => (rest, ""),
        };
        if !is_ident(slug) {
            return None;
        }
        let (service, rest) = match rest.strip_prefix("service:") {
            Some(s) => {
                let s = s.trim_start();
                let (name, rest) = match s.split_once(char::is_whitespace) {
                    Some((n, r)) => (n, r.trim_start()),
                    None => (s, ""),
                };
                if !is_ident(name) {
                    return None;
                }
                (Some(name.to_owned()), rest)
            }
            None => (None, rest),
        };
        let facts = match rest.strip_prefix("facts:") {
            Some(raw) => parse_fact_fields(raw.trim())?,
            None if rest.is_empty() => Vec::new(),
            None => return None,
        };
        Some(GuidanceMarker { slug: slug.to_owned(), service, facts })
    }

    /// Validate one instance's guidance and strip it for injection.
    ///
    /// A **control**, so it fails closed on every count: a missing marker, a marker bound to
    /// another instance, a marker naming no service, a marker bound to another service, or
    /// content too thin to be real.
    ///
    /// The marker's `service:` used to be optional, and omitting it meant the corpus default.
    /// There is no default now (`ironworks_services::service`), so omitting it means nothing at all
    /// and is refused — the alternative would be to read the configured service back into the
    /// silence, which is a check that always passes.
    pub fn parse(text: &str, slug: &str, configured_service: &str) -> Result<Self, GuidanceError> {
        let marker =
            Self::marker(text).ok_or_else(|| GuidanceError::NoMarker { slug: slug.to_owned() })?;
        if marker.slug != slug {
            return Err(GuidanceError::WrongSlug { slug: slug.to_owned(), found: marker.slug });
        }
        let declared = marker.service.as_deref().ok_or_else(|| GuidanceError::NoService {
            slug: slug.to_owned(),
            configured: configured_service.to_owned(),
        })?;
        if declared != configured_service {
            return Err(GuidanceError::WrongService {
                slug: slug.to_owned(),
                declared: declared.to_owned(),
                configured: configured_service.to_owned(),
            });
        }
        // Strip HTML comments AFTER validating the marker: the template's operator instructions
        // must never become model-visible text, and comments must not pad an unfilled template
        // past the minimum.
        let stripped = strip_html_comments(text);
        let content = stripped.trim();
        if let Some(bytes) = content.find("-->") {
            return Err(GuidanceError::LeakedComment { slug: slug.to_owned(), bytes });
        }
        let unwritten = content.matches(STUB_MARKER).count();
        if unwritten != 0 {
            return Err(GuidanceError::UnwrittenStubs { slug: slug.to_owned(), count: unwritten });
        }
        if content.len() < MIN_CONTENT {
            return Err(GuidanceError::TooShort {
                slug: slug.to_owned(),
                len: content.len(),
                min: MIN_CONTENT,
            });
        }
        Ok(Self { content: content.to_owned(), fact_fields: marker.facts })
    }
}

fn is_ident(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// The marker's optional `facts:` value: a JSON array of strings, kept exactly as written.
///
/// **JSON rather than a delimited list, and deliberately not [`is_ident`].** These are keys into
/// the record store's flexible `facts` object, whose domain is arbitrary JSON strings — a key may
/// carry a comma, a leading space or a character outside ASCII. Any delimiter-based encoding, or
/// any identifier grammar borrowed from slugs, would make some key the store can legitimately
/// hold undeclarable here, which is an invented schema boundary wearing a disclaimer.
/// `serde_json::from_str` refuses malformed input, a non-string member and trailing content in
/// the one call that reads it.
///
/// Two refusals are *not* about what a fact may be called. A declared key carrying a control
/// character or `-->` would end the HTML comment early, and [`strip_html_comments`] would then
/// leave the remainder of this line in the model-visible guidance — operator configuration
/// escaping into the prompt. That is a restriction this marker's transport imposes.
///
/// Duplicates are refused because the rendering order is the declaration, and a key named twice
/// has no single place in it.
fn parse_fact_fields(raw: &str) -> Option<Vec<String>> {
    let fields: Vec<String> = serde_json::from_str(raw).ok()?;
    let mut seen = std::collections::BTreeSet::new();
    for field in &fields {
        if field.contains(|c: char| c.is_control()) || field.contains("-->") {
            return None;
        }
        if !seen.insert(field.as_str()) {
            return None;
        }
    }
    Some(fields)
}

fn strip_html_comments(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("<!--") {
        out.push_str(&rest[..start]);
        match rest[start..].find("-->") {
            Some(end) => rest = &rest[start + end + "-->".len()..],
            None => return out,
        }
    }
    out.push_str(rest);
    out
}

// ── the melt ─────────────────────────────────────────────────────────────────────────────

/// The composed instructions for one instance, supplied to the runtime every turn.
///
/// Constructible only by [`melt`], and in practice only through a verified service instance —
/// so an alloy can never belong to a service other than the one that produced it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Alloy(String);

impl Alloy {
    pub fn as_str(&self) -> &str {
        &self.0
    }
    pub fn len(&self) -> usize {
        self.0.len()
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// First 16 hex characters of a SHA-256. Non-secret, comparable, and it never prints the
    /// guidance the alloy carries — which is the tenant's own data.
    pub fn digest(&self) -> String {
        digest16(self.0.as_bytes())
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("the composition names part {part}, which the corpus does not hold")]
pub struct MeltError {
    pub part: PartRef,
}

/// The model-visible separator between sections.
const JOIN: &str = "\n\n---\n\n";

/// The one digest shape in this repository: the first 16 hex characters of a SHA-256.
///
/// Short and non-secret, so an alloy, a part and a tenant's guidance are all comparable in a
/// terminal without any of them being printed.
///
/// **Public because a book fingerprint is the same shape and must be the same bytes.** A
/// transcript binds an answer to the book it was shown by recording this digest over a manifest of
/// per-file hashes, and every recorded transcript in `docs/experiments` carries a value produced
/// by it. A second implementation would be a second answer to a question that must have exactly
/// one — which is the defect a fingerprint exists to rule out.
pub fn digest16(bytes: &[u8]) -> String {
    digest16_of(Sha256::new().chain_update(bytes))
}

/// [`digest16`] for a digest accumulated over many pieces rather than one slice.
///
/// The same sixteen characters of the same hash — a caller that must feed a hasher incrementally
/// gets the shape from here rather than re-authoring the fold, so there stays exactly one answer.
pub fn digest16_of(hasher: Sha256) -> String {
    hasher.finalize().iter().take(8).map(|b| format!("{b:02x}")).collect()
}

/// Fuse a composition's parts with validated guidance into an alloy.
///
/// Takes only the composition concerns — not a whole service definition. Everything else about a
/// service (its capabilities, its evaluation, its identity) is irrelevant to what the model reads
/// and has no business being reachable from here.
pub fn melt(
    composition: &Composition,
    guidance: &Guidance,
    parts: &dyn PartSource,
) -> Result<Alloy, MeltError> {
    let mut sections: Vec<String> = Vec::with_capacity(composition.parts.len() + 2);
    for part in &composition.parts {
        let body = parts.read(part).ok_or_else(|| MeltError { part: part.clone() })?;
        sections.push(strip_frontmatter(body).to_owned());
    }
    sections.push(format!("{}\n\n{}", composition.guidance_heading, guidance.as_str()));
    let tail = parts
        .read(&composition.safety_tail)
        .ok_or_else(|| MeltError { part: composition.safety_tail.clone() })?;
    sections.push(tail.to_owned());
    Ok(Alloy(sections.join(JOIN)))
}

/// Drop a leading YAML frontmatter block.
///
/// Conservative by construction: strips only when the text opens `---\n` **and** a closing
/// `\n---\n` exists. An unterminated block is returned untouched — a malformed header is worth
/// seeing in the prompt, where it is obvious, rather than silently eating a whole section.
pub fn strip_frontmatter(text: &str) -> &str {
    let Some(rest) = text.strip_prefix("---\n") else {
        return text;
    };
    match rest.find("\n---\n") {
        Some(end) => rest[end + "\n---\n".len()..].trim_start_matches('\n'),
        None => text,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    struct Fake(BTreeMap<String, String>);
    impl PartSource for Fake {
        fn read(&self, p: &PartRef) -> Option<&str> {
            self.0.get(p.as_str()).map(String::as_str)
        }
    }
    fn fake() -> Fake {
        Fake(BTreeMap::from([
            ("personas/A.md".into(), "# A\n\nrole".into()),
            ("skills/s/SKILL.md".into(), "---\nname: s\n---\n\n# S\n\nsteps".into()),
            ("personas/_tail.md".into(), "## Safety\n\nrules".into()),
        ]))
    }
    fn composition() -> Composition {
        Composition::new(
            vec![PartRef::new("personas/A.md"), PartRef::new("skills/s/SKILL.md")],
            "# ORGANIZATION GUIDANCE",
            PartRef::new("personas/_tail.md"),
        )
        .unwrap()
    }
    fn guidance(slug: &str, service: Option<&str>) -> String {
        let svc = service.map(|s| format!(" service: {s}")).unwrap_or_default();
        format!(
            "<!-- tenant-guidance v1 slug: {slug}{svc} -->\n\n# Company\n\n{}\n",
            "Real content about the organization. ".repeat(20)
        )
    }
    fn g() -> Guidance {
        Guidance::parse(&guidance("acme", Some("account-analysis")), "acme", "account-analysis")
            .unwrap()
    }

    /// A marker with a `facts:` declaration, and enough body to clear `MIN_CONTENT`.
    fn declaring(facts: &str) -> String {
        format!(
            "<!-- tenant-guidance v1 slug: acme service: account-analysis facts: {facts} -->\n\n             # Company\n\n{}\n",
            "Real content about the organization. ".repeat(20)
        )
    }
    fn declared(facts: &str) -> Option<Vec<String>> {
        Guidance::marker(&declaring(facts)).map(|m| m.facts)
    }

    #[test]
    fn order_is_parts_then_guidance_then_safety_last() {
        let a = melt(&composition(), &g(), &fake()).unwrap();
        let t = a.as_str();
        assert!(t.starts_with("# A"), "first declared part composes first");
        assert!(t.ends_with("## Safety\n\nrules"), "the safety tail composes last");
        let heading = t.find("# ORGANIZATION GUIDANCE").unwrap();
        assert!(t.find("# S").unwrap() < heading, "parts precede guidance");
        assert!(heading < t.rfind("## Safety").unwrap(), "guidance precedes the tail");
        assert_eq!(t.matches(JOIN).count(), 3, "parts + guidance + tail = 3 separators");
    }

    #[test]
    fn skill_frontmatter_never_reaches_the_prompt() {
        let a = melt(&composition(), &g(), &fake()).unwrap();
        assert!(!a.as_str().contains("name: s"));
        assert!(!a.as_str().starts_with("---"));
    }

    #[test]
    fn a_missing_part_fails_the_melt_by_name() {
        let empty = Fake(BTreeMap::new());
        let err = melt(&composition(), &g(), &empty).unwrap_err();
        assert_eq!(err.part, PartRef::new("personas/A.md"));
    }

    #[test]
    fn missing_parts_are_reportable_before_a_turn() {
        assert!(composition().missing_in(&fake()).is_empty());
        assert_eq!(composition().missing_in(&Fake(BTreeMap::new())).len(), 3);
    }

    #[test]
    fn an_unterminated_frontmatter_block_is_left_visible() {
        let t = "---\nname: x\nnever closed\n";
        assert_eq!(strip_frontmatter(t), t);
    }

    #[test]
    fn a_horizontal_rule_is_not_frontmatter() {
        let t = "# Heading\n\n---\n\nmore\n";
        assert_eq!(strip_frontmatter(t), t);
    }

    // ── guidance is a control: it fails closed ───────────────────────────────────────

    #[test]
    fn guidance_bound_to_another_instance_is_refused() {
        let err = Guidance::parse(
            &guidance("other", Some("account-analysis")),
            "acme",
            "account-analysis",
        )
        .unwrap_err();
        assert!(matches!(err, GuidanceError::WrongSlug { .. }));
    }

    #[test]
    fn guidance_bound_to_another_service_is_refused() {
        let err = Guidance::parse(
            &guidance("acme", Some("relationship-intelligence")),
            "acme",
            "account-analysis",
        )
        .unwrap_err();
        assert!(matches!(err, GuidanceError::WrongService { .. }));
    }

    #[test]
    fn a_marker_without_a_declaration_declares_no_facts() {
        // Every guidance file written before this existed must keep validating unchanged, and
        // must not acquire a gap shape it never asked for.
        assert_eq!(Guidance::marker(&guidance("acme", None)).unwrap().facts, Vec::<String>::new());
        assert_eq!(
            Guidance::marker(&guidance("acme", Some("relationship-intelligence"))).unwrap().facts,
            Vec::<String>::new()
        );
        assert!(g().fact_fields().is_empty());
    }

    #[test]
    fn a_declaration_keeps_its_order_and_its_exact_spelling() {
        // Order is the whole instruction: the envelope renders declared keys in this sequence,
        // and `supply::fingerprint` digests that rendering.
        assert_eq!(
            declared(r#"["work_order","commitment"]"#).unwrap(),
            ["work_order", "commitment"]
        );
        assert_eq!(
            declared(r#"["commitment","work_order"]"#).unwrap(),
            ["commitment", "work_order"]
        );
    }

    #[test]
    fn a_declaration_accepts_every_key_the_record_store_can_hold() {
        // These are keys into the store's flexible `facts` object, whose domain is arbitrary JSON
        // strings. A comma-delimited encoding could not express the first of these, and the slug
        // grammar would reject all four — either would make a fact the store legitimately holds
        // undeclarable, which is the invented schema this deliberately does not have.
        let odd = r#"["renewal, phase 2"," padded ","work_order","budsjett_kroner","課題"]"#;
        assert_eq!(
            declared(odd).unwrap(),
            ["renewal, phase 2", " padded ", "work_order", "budsjett_kroner", "課題"]
        );
    }

    #[test]
    fn an_empty_declaration_is_the_same_instruction_as_no_declaration() {
        // Tri-state upstream, two states here. `None` and `[]` both assert no gaps, because this
        // implementation has no legacy sales-shaped fallback for an absent one to select.
        assert_eq!(declared("[]").unwrap(), Vec::<String>::new());
        let empty = Guidance::parse(&declaring("[]"), "acme", "account-analysis").unwrap();
        assert_eq!(empty.fact_fields(), g().fact_fields());
    }

    #[test]
    fn a_malformed_declaration_fails_closed_rather_than_declaring_something_else() {
        for bad in [
            r#"["a","a"]"#,        // a key named twice has no single place in the order
            r#"["a",]"#,           // malformed JSON
            r#"["a""#,             // unterminated
            r#"[1,2]"#,            // non-string members
            r#"["a",null]"#,       // a null is not a key
            r#""a""#,              // a bare string is not the declaration shape
            r#"{"a":1}"#,          // nor an object
            r#"["a"] service: x"#, // trailing marker content after the array
            "",                    // `facts:` present with nothing after it
        ] {
            assert!(declared(bad).is_none(), "{bad:?} must be refused, not reinterpreted");
        }
    }

    #[test]
    fn a_declaration_cannot_escape_the_comment_it_is_written_in() {
        // Not a statement about what a fact may be called — a transport restriction. Marker
        // stripping ends the comment at the first `-->`, so a key carrying one would leave the
        // rest of this line in the model-visible guidance: operator configuration reaching the
        // prompt as instruction. A control character does the same to the one-line marker.
        assert!(declared(r#"["a-->b"]"#).is_none());
        assert!(declared(r#"["a\u0000b"]"#).is_none());
        assert!(declared(r#"["a\nb"]"#).is_none());
    }

    #[test]
    fn a_declaration_is_configuration_and_never_reaches_the_model() {
        let parsed =
            Guidance::parse(&declaring(r#"["work_order"]"#), "acme", "account-analysis").unwrap();
        assert_eq!(parsed.fact_fields(), ["work_order"]);
        assert!(
            !parsed.as_str().contains("work_order"),
            "the declaration is stripped with its comment"
        );
        assert!(!parsed.as_str().contains("facts:"));
    }

    #[test]
    fn declaring_facts_moves_no_alloy_byte() {
        // The property that keeps every transcript-bound composition digest valid: this policy
        // changes how records are rendered, never what the model is told. If this fails, the
        // recorded results in `docs/experiments/` no longer name the composition that produced
        // them.
        let none = Guidance::parse(
            &guidance("acme", Some("account-analysis")),
            "acme",
            "account-analysis",
        )
        .unwrap();
        let some = Guidance::parse(
            &declaring(r#"["work_order","commitment"]"#),
            "acme",
            "account-analysis",
        )
        .unwrap();
        let other =
            Guidance::parse(&declaring(r#"["commitment"]"#), "acme", "account-analysis").unwrap();
        assert_eq!(none.as_str(), some.as_str(), "only the stripped comment differs");
        assert_eq!(none.digest(), some.digest());
        assert_eq!(some.digest(), other.digest(), "reordering the policy is not a guidance change");
        let a = melt(&composition(), &none, &fake()).unwrap();
        let b = melt(&composition(), &some, &fake()).unwrap();
        assert_eq!(a.digest(), b.digest());
        assert_eq!(a.len(), b.len());
    }

    /// A marker that names no service used to inherit the corpus default. There is no default
    /// now, so it names nothing — and nothing is not a match for the configured service. Reading
    /// the configured name back into the silence would have made the binding check unable to fail,
    /// which is the one outcome a control must not have.
    #[test]
    fn an_absent_service_in_the_marker_is_refused_rather_than_defaulted() {
        let err = Guidance::parse(&guidance("acme", None), "acme", "account-analysis").unwrap_err();
        assert!(matches!(err, GuidanceError::NoService { .. }), "got {err}");
        // Still refused against any other configured service: the refusal is about the file, not
        // about which service happened to be asked for.
        assert!(Guidance::parse(&guidance("acme", None), "acme", "other-service").is_err());
    }

    #[test]
    fn an_unfilled_template_is_refused() {
        let err = Guidance::parse(
            "<!-- tenant-guidance v1 slug: acme service: account-analysis -->\n\ntbd\n",
            "acme",
            "account-analysis",
        )
        .unwrap_err();
        assert!(matches!(err, GuidanceError::TooShort { .. }));
    }

    #[test]
    fn legacy_leaked_notes_and_unwritten_stubs_are_not_guidance() {
        let marker = "<!-- tenant-guidance v1 slug: acme service: account-analysis -->\n\n";
        let leaked =
            format!("{marker}operator-only bytes-->\n{}", "Real tenant guidance. ".repeat(30));
        assert!(matches!(
            Guidance::parse(&leaked, "acme", "account-analysis"),
            Err(GuidanceError::LeakedComment { bytes: 19, .. })
        ));

        let stubbed = format!("{marker}{STUB_MARKER}\n{}", "Shared template prose. ".repeat(30));
        assert!(matches!(
            Guidance::parse(&stubbed, "acme", "account-analysis"),
            Err(GuidanceError::UnwrittenStubs { count: 1, .. })
        ));
    }

    #[test]
    fn operator_comments_are_stripped_and_do_not_pad_the_length() {
        let padded = format!(
            "<!-- tenant-guidance v1 slug: acme service: account-analysis -->\n\n\
             <!-- OPERATOR: {} -->\n\nshort\n",
            "y".repeat(500)
        );
        assert!(matches!(
            Guidance::parse(&padded, "acme", "account-analysis"),
            Err(GuidanceError::TooShort { .. })
        ));
        assert!(!g().as_str().contains("<!--"), "no comment survives into the alloy");
    }

    #[test]
    fn a_composition_refuses_to_be_empty_or_unheaded() {
        assert_eq!(
            Composition::new(vec![], "h", PartRef::new("t")).unwrap_err(),
            CompositionError::NoParts
        );
        assert_eq!(
            Composition::new(vec![PartRef::new("p")], "  ", PartRef::new("t")).unwrap_err(),
            CompositionError::NoHeading
        );
    }

    #[test]
    fn part_digests_name_every_ingredient_in_composition_order_with_the_tail_last() {
        let d = composition().part_digests(&fake()).unwrap();
        let named: Vec<&str> = d.iter().map(|(p, _)| p.as_str()).collect();
        assert_eq!(named, ["personas/A.md", "skills/s/SKILL.md", "personas/_tail.md"]);
        assert!(d.iter().all(|(_, h)| h.len() == 16), "same digest shape as an alloy");
    }

    #[test]
    fn a_part_digest_covers_exactly_the_bytes_the_melt_fuses() {
        // Frontmatter is stripped before fusing, so editing it changes no model-visible byte.
        // A digest that moved here would send an operator looking for a behaviour change that
        // cannot exist.
        let mut edited = fake();
        edited.0.insert(
            "skills/s/SKILL.md".into(),
            "---\nname: s\nowner: someone-else\n---\n\n# S\n\nsteps".into(),
        );
        assert_eq!(
            melt(&composition(), &g(), &fake()).unwrap().digest(),
            melt(&composition(), &g(), &edited).unwrap().digest(),
            "positive control: frontmatter is not model-visible"
        );
        assert_eq!(
            composition().part_digests(&fake()).unwrap(),
            composition().part_digests(&edited).unwrap(),
        );
    }

    #[test]
    fn a_changed_part_body_moves_that_part_digest_and_no_other() {
        let mut edited = fake();
        edited.0.insert("skills/s/SKILL.md".into(), "---\nname: s\n---\n\n# S\n\nnew steps".into());
        let before = composition().part_digests(&fake()).unwrap();
        let after = composition().part_digests(&edited).unwrap();
        let moved: Vec<&str> = before
            .iter()
            .zip(&after)
            .filter(|((_, a), (_, b))| a != b)
            .map(|((p, _), _)| p.as_str())
            .collect();
        assert_eq!(moved, ["skills/s/SKILL.md"]);
        assert_ne!(
            melt(&composition(), &g(), &fake()).unwrap().digest(),
            melt(&composition(), &g(), &edited).unwrap().digest(),
        );
    }

    #[test]
    fn guidance_can_move_the_alloy_with_every_part_digest_unchanged() {
        // The case a repo diff cannot see, and the reason part digests and a guidance digest are
        // reported side by side rather than one standing in for the other.
        let other = Guidance::parse(
            &guidance("acme", Some("account-analysis"))
                .replace("Real content", "Different content"),
            "acme",
            "account-analysis",
        )
        .unwrap();
        assert_ne!(g().digest(), other.digest());
        assert_ne!(
            melt(&composition(), &g(), &fake()).unwrap().digest(),
            melt(&composition(), &other, &fake()).unwrap().digest(),
        );
        assert_eq!(
            composition().part_digests(&fake()).unwrap(),
            composition().part_digests(&fake()).unwrap(),
            "no part moved"
        );
    }

    #[test]
    fn part_digests_fail_by_name_on_a_part_the_source_does_not_hold() {
        let err = composition().part_digests(&Fake(BTreeMap::new())).unwrap_err();
        assert_eq!(err.part.as_str(), "personas/A.md");
    }
}
