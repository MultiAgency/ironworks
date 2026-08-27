//! Which tenants this process serves, and which conversation belongs to each.
//!
//! [`route`](crate::route) answers *whose conversation is this?* given a table of claims. This is
//! where the table comes from — and, more importantly, where a table that cannot be trusted is
//! refused rather than served.
//!
//! # The whole registry refuses, or none of it does
//!
//! Every failure here rejects the **set**, never a row. That is the property worth having and it
//! is easy to lose: a loader that skipped a malformed tenant would serve fewer organizations than
//! the operator believes it does, silently, with no signal anywhere — and the tenant that vanished
//! is exactly the one whose configuration someone just edited. A registry is a claim about who is
//! being served; a partial one is a false claim.
//!
//! # It holds references, never content and never secrets
//!
//! Guidance is a [`GuidanceRef`] and credentials are [`CredentialRef`]s, so this type names where
//! things live and reads none of them. That keeps `domain` pure, and it means an operator can
//! validate the *shape* of a registry with no filesystem, no record store and no network — which
//! is what makes the refusal tests cheap enough to be exhaustive.
//!
//! What it therefore cannot check: that a guidance file exists, that a service is embedded,
//! that a credential resolves. Those need IO and belong to the binary, which must apply the same
//! all-or-nothing rule when it checks them. Two layers, one property.

use crate::address::Addressing;
use crate::ids::{
    ActorRef, ContributorId, CredentialPurpose, CredentialRef, GuidanceRef, ServiceId, Slug,
};
use crate::route::ChannelRef;
use serde::Deserialize;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum RegistryError {
    #[error("the registry is not valid JSON: {0}")]
    Malformed(String),
    #[error("a registry with no tenants serves nobody; remove it or add one")]
    Empty,
    #[error("tenant {index}: {field} is missing or empty")]
    MissingField { index: usize, field: &'static str },
    #[error("tenant {index}: {0}", .source)]
    InvalidField {
        index: usize,
        #[source]
        source: crate::ids::IdError,
    },
    /// The defect `route::resolve` refuses to resolve, caught earlier and with a better message:
    /// two tenants claiming one conversation would serve it with one of their credentials, and
    /// which one is an accident of ordering.
    #[error(
        "channel {channel} is claimed by both {first} and {second} — one conversation belongs \
             to exactly one tenant"
    )]
    DuplicateChannel { channel: String, first: String, second: String },
    #[error("tenant {slug} appears twice; a slug is the unit of isolation and must be unique")]
    DuplicateSlug { slug: String },
    /// One sender naming two contributors inside one tenant. Attribution is the whole point of a
    /// promotion, so it may not be decided by ordering — or by which room the person used.
    ///
    /// Two shapes, one defect. A repeated **pair** would attribute by whichever line came last.
    /// The same actor in two **rooms** under two different contributors would attribute by which
    /// conversation they happened to type in — the same ambiguity [`ActorInTwoTenants`] refuses
    /// across tenants, and no more acceptable inside one. The pair is what *authorizes*; who a
    /// person *is* is not a property of the room they are standing in.
    ///
    /// One actor in several rooms under the same contributor is not this: it is a person
    /// reachable in more than one conversation, which is the shape bindings exist to express.
    ///
    /// The actor key is deliberately not named: it is the sender's channel identity, refused
    /// through a message an operator reads on a terminal. The tenant and the contributors are
    /// enough to find the two lines.
    ///
    /// [`ActorInTwoTenants`]: Self::ActorInTwoTenants
    #[error(
        "tenant {slug} binds one sender to both {first} and {second} — one person carries exactly \
             one name inside one organization, in every room they are bound in"
    )]
    DuplicateActor { slug: String, first: String, second: String },
    /// One actor bound in two tenants. `/share` would have two organizations to promote into and
    /// no way to choose, so the set is refused rather than resolved by ordering.
    #[error(
        "an actor is bound in both {first} and {second} — one actor belongs to exactly one \
             tenant, or a promotion has two organizations to choose from"
    )]
    ActorInTwoTenants { first: String, second: String },
}

/// One tenant this process serves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TenantEntry {
    slug: Slug,
    channel: ChannelRef,
    service: ServiceId,
    guidance: GuidanceRef,
    records: CredentialRef,
    runtime: CredentialRef,
    addressing: Addressing,
    contributors: Vec<ContributorBinding>,
    append: Option<CredentialRef>,
}

/// One person, in one of this tenant's conversations.
///
/// # Why the pair, and not the actor alone
///
/// Telegram gives every person their own private chat id, so one tenant's team arrives on several
/// conversations. The **conversation decides the tenant** and the **pair decides the person**: an
/// actor id lifted from one chat and replayed in another matches no binding and authorizes
/// nothing, which is a property of the key rather than a check someone remembered to write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContributorBinding {
    channel: ChannelRef,
    actor: ActorRef,
    contributor: ContributorId,
}

impl ContributorBinding {
    /// The conversation this person reaches the tenant through.
    pub fn channel(&self) -> &ChannelRef {
        &self.channel
    }
    pub fn contributor(&self) -> &ContributorId {
        &self.contributor
    }
}

impl TenantEntry {
    pub fn slug(&self) -> &Slug {
        &self.slug
    }
    pub fn channel(&self) -> &ChannelRef {
        &self.channel
    }
    pub fn service(&self) -> &ServiceId {
        &self.service
    }
    pub fn guidance(&self) -> &GuidanceRef {
        &self.guidance
    }
    /// The handle for this tenant's record-store credential. Per tenant, deliberately: a shared
    /// one would make isolation a property of the routing table rather than of verified evidence.
    pub fn records(&self) -> &CredentialRef {
        &self.records
    }
    pub fn runtime(&self) -> &CredentialRef {
        &self.runtime
    }

    /// Where this tenant's **append** credential lives, if it has one.
    ///
    /// Optional, and its absence is the whole authorization story for promotion: a tenant with no
    /// append handle cannot promote, and that is a configuration an operator chooses rather than
    /// a failure. It never weakens anything — a missing handle makes `/confirm` unavailable, it
    /// does not make it unauthenticated.
    ///
    /// Separate from [`records`](Self::records) by purpose, so the read handle can never stand in
    /// for it and it can never stand in for the read handle.
    pub fn append(&self) -> Option<&CredentialRef> {
        self.append.as_ref()
    }
    /// How this room decides a message is for us. Absent from the registry means every message
    /// is — correct for a direct chat, and wrong in a group, which is why a group must say so.
    pub fn addressing(&self) -> &Addressing {
        &self.addressing
    }

    /// Who this sender promotes records as **in this conversation**, or `None`.
    ///
    /// # This is operator-recorded authorization, not proof of anything
    ///
    /// The binding says an operator wrote down that this channel actor is that contributor. It is
    /// **not** evidence of an IronClaw pairing: both bots receive the same Telegram `from.id`, and
    /// nothing here can inspect the other's pairing registry or prove which runtime member it
    /// resolved to. Reading it as a canonical member binding would claim a verified cross-system
    /// identity that does not exist. A POC limitation, named where the value is produced.
    ///
    /// Absent means absent: a tenant that lists no contributors authorizes nobody, which is why
    /// the field is optional in the registry and empty is the default.
    ///
    /// Both halves must match. A sender who is bound in their own private chat and sends the same
    /// id into a colleague's conversation matches nothing — the cross-wire refuses because the
    /// key never described it, not because a rule caught it.
    pub fn contributor(&self, channel: &ChannelRef, actor: &ActorRef) -> Option<&ContributorId> {
        self.contributors
            .iter()
            .find(|b| &b.channel == channel && &b.actor == actor)
            .map(ContributorBinding::contributor)
    }

    /// The bindings, in the order the registry declared them. For operator inspection.
    pub fn contributors(&self) -> &[ContributorBinding] {
        &self.contributors
    }

    /// Every conversation that reaches this tenant: the entry's own, plus each contributor's.
    ///
    /// One tenant, many rooms, **one set of credentials** — the entry is not duplicated per
    /// person, so the organization, the record scope and the runtime member stay single-valued
    /// however many of a team are bound.
    pub fn channels(&self) -> Vec<ChannelRef> {
        let mut out = vec![self.channel.clone()];
        for binding in &self.contributors {
            if !out.contains(&binding.channel) {
                out.push(binding.channel.clone());
            }
        }
        out
    }
}

/// Every tenant this process serves. Constructible only by [`TenantRegistry::parse`], so holding
/// one means the whole set validated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TenantRegistry {
    entries: Vec<TenantEntry>,
}

#[derive(Deserialize)]
struct WireEntry {
    #[serde(default)]
    slug: String,
    #[serde(default)]
    channel: String,
    #[serde(default)]
    service: String,
    #[serde(default)]
    guidance: String,
    #[serde(default)]
    records_credential: String,
    #[serde(default)]
    runtime_credential: String,
    /// The handle that summons the bot in this room. Absent means a direct chat.
    #[serde(default)]
    address: Option<String>,
    /// Who may promote records for this tenant, and as whom. Absent authorizes nobody.
    #[serde(default)]
    contributors: Vec<WireContributor>,
    /// Where the append credential lives. Absent means this tenant cannot promote.
    #[serde(default)]
    append_credential: Option<String>,
}

#[derive(Deserialize)]
struct WireContributor {
    /// The private conversation this person reaches the tenant through. Absent means the
    /// tenant's own `channel`, which keeps a one-person registry exactly as it was written.
    #[serde(default)]
    channel: Option<String>,
    #[serde(default)]
    actor: String,
    #[serde(default)]
    contributor: String,
}

#[derive(Deserialize)]
struct WireRegistry {
    #[serde(default)]
    tenants: Vec<WireEntry>,
}

impl TenantRegistry {
    /// Parse and validate a whole registry, or refuse it.
    pub fn parse(text: &str) -> Result<Self, RegistryError> {
        let wire: WireRegistry =
            serde_json::from_str(text).map_err(|e| RegistryError::Malformed(e.to_string()))?;
        if wire.tenants.is_empty() {
            return Err(RegistryError::Empty);
        }

        let mut entries: Vec<TenantEntry> = Vec::with_capacity(wire.tenants.len());
        for (index, t) in wire.tenants.iter().enumerate() {
            let required = |value: &str, field: &'static str| -> Result<(), RegistryError> {
                if value.trim().is_empty() {
                    return Err(RegistryError::MissingField { index, field });
                }
                Ok(())
            };
            required(&t.slug, "slug")?;
            required(&t.channel, "channel")?;
            required(&t.service, "service")?;
            required(&t.guidance, "guidance")?;
            required(&t.records_credential, "records_credential")?;
            required(&t.runtime_credential, "runtime_credential")?;

            // Bindings are built before the entry so a malformed one refuses the set here,
            // beside every other field, rather than at the first `/share` months later.
            let own_channel = ChannelRef::new(t.channel.trim());
            let mut contributors: Vec<ContributorBinding> =
                Vec::with_capacity(t.contributors.len());
            for c in &t.contributors {
                required(&c.actor, "contributors[].actor")?;
                required(&c.contributor, "contributors[].contributor")?;
                // An omitted channel is the tenant's own, so a registry written when a tenant
                // meant one person keeps working unchanged. An empty string is a mistake rather
                // than a default: someone meant to write a conversation id.
                let channel = match c.channel.as_deref().map(str::trim) {
                    None => own_channel.clone(),
                    Some("") => {
                        return Err(RegistryError::MissingField {
                            index,
                            field: "contributors[].channel",
                        });
                    }
                    Some(channel) => ChannelRef::new(channel),
                };
                let actor = ActorRef::parse(c.actor.clone())
                    .map_err(|source| RegistryError::InvalidField { index, source })?;
                let contributor = ContributorId::parse(c.contributor.clone())
                    .map_err(|source| RegistryError::InvalidField { index, source })?;
                // Two rules, and the second is the one that was missing.
                //
                // A pair may not repeat: a conversation-and-sender naming two contributors would
                // attribute by ordering.
                //
                // And one actor may not name two contributors ANYWHERE in this tenant, however
                // many rooms they are bound in. The pair is what authorizes, but an actor id is
                // one person's channel account, and one person carrying two names inside one
                // organization is the same ambiguity the cross-tenant rule already refuses —
                // there, resolved by whichever conversation they happened to use. Attribution is
                // the entire product of a promotion; it must not be a property of the room.
                //
                // One actor bound in several rooms to the SAME contributor stays legal, and has
                // to: that is a person reachable in more than one conversation, which is the
                // shape contributors were added to express.
                if let Some(prior) = contributors.iter().find(|b| {
                    b.actor == actor && (b.channel == channel || b.contributor != contributor)
                }) {
                    return Err(RegistryError::DuplicateActor {
                        slug: t.slug.trim().to_owned(),
                        first: prior.contributor.as_str().to_owned(),
                        second: contributor.as_str().to_owned(),
                    });
                }
                contributors.push(ContributorBinding { channel, actor, contributor });
            }

            let entry = TenantEntry {
                slug: Slug::parse(t.slug.clone())
                    .map_err(|source| RegistryError::InvalidField { index, source })?,
                channel: ChannelRef::new(t.channel.trim()),
                service: ServiceId::parse(t.service.clone())
                    .map_err(|source| RegistryError::InvalidField { index, source })?,
                guidance: GuidanceRef::new(t.guidance.trim()),
                records: CredentialRef::new(
                    CredentialPurpose::RecordScope,
                    t.records_credential.trim(),
                ),
                runtime: CredentialRef::new(
                    CredentialPurpose::RuntimeMember,
                    t.runtime_credential.trim(),
                ),
                // An empty string is a mistake rather than a direct chat: someone meant to write
                // a handle. Refusing it beats answering every message in a group.
                addressing: match t.address.as_deref().map(str::trim) {
                    None => Addressing::Always,
                    Some("") => {
                        return Err(RegistryError::MissingField { index, field: "address" });
                    }
                    Some(handle) => Addressing::Mention(handle.to_owned()),
                },
                contributors,
                // Absent is the default and stays the default: an existing registry gains no
                // append authority by being read by a newer binary. An empty string is a
                // mistake rather than an absence — someone meant to write a handle.
                append: match t.append_credential.as_deref().map(str::trim) {
                    None => None,
                    Some("") => {
                        return Err(RegistryError::MissingField {
                            index,
                            field: "append_credential",
                        });
                    }
                    Some(handle) => {
                        Some(CredentialRef::new(CredentialPurpose::RecordAppend, handle))
                    }
                },
            };

            // Collisions are checked against what is already accepted, so the error can name both
            // sides. A post-hoc scan could only say "there is a duplicate somewhere".
            //
            // Across EVERY conversation a tenant answers on, not just its own: a tenant now has
            // one room per bound person, and a room claimed by two tenants would be served with
            // one of their credentials by an accident of ordering — the same defect the
            // single-channel check has always refused.
            let mine = entry.channels();
            if let Some((prior, channel)) = entries.iter().find_map(|e| {
                let theirs = e.channels();
                mine.iter().find(|c| theirs.contains(c)).map(|c| (e, c))
            }) {
                return Err(RegistryError::DuplicateChannel {
                    channel: channel.as_str().to_owned(),
                    first: prior.slug.as_str().to_owned(),
                    second: entry.slug.as_str().to_owned(),
                });
            }
            if entries.iter().any(|e| e.slug == entry.slug) {
                return Err(RegistryError::DuplicateSlug { slug: entry.slug.as_str().to_owned() });
            }
            // Cross-tenant, and checked against what is already accepted for the same reason the
            // channel collision is: the error can name both sides. An actor bound twice gives a
            // promotion two organizations and no way to choose between them.
            //
            // On the ACTOR alone, deliberately — not the pair. The pair is what authorizes, but
            // one person reachable in two organizations is an ambiguity about who they are, and
            // resolving it by whichever conversation they happened to use is exactly the accident
            // this refuses everywhere else.
            if let Some(prior) = entries.iter().find(|e| {
                entry
                    .contributors
                    .iter()
                    .any(|b| e.contributors.iter().any(|theirs| theirs.actor == b.actor))
            }) {
                return Err(RegistryError::ActorInTwoTenants {
                    first: prior.slug.as_str().to_owned(),
                    second: entry.slug.as_str().to_owned(),
                });
            }
            entries.push(entry);
        }
        Ok(Self { entries })
    }

    /// The claims table [`crate::route::resolve`] takes.
    ///
    /// A slice of pairs rather than a map, matching what `resolve` wants: a map would have lost a
    /// collision before anyone could look it up. This registry refuses collisions at parse time,
    /// so the table it produces cannot contain one — but the shape stays honest about the fact
    /// that resolution, not lookup, is what decides.
    pub fn claims(&self) -> Vec<(ChannelRef, Slug)> {
        self.entries
            .iter()
            .flat_map(|e| e.channels().into_iter().map(|c| (c, e.slug.clone())))
            .collect()
    }

    pub fn tenant(&self, slug: &Slug) -> Option<&TenantEntry> {
        self.entries.iter().find(|e| &e.slug == slug)
    }

    pub fn entries(&self) -> &[TenantEntry] {
        &self.entries
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A valid tenant, so every refusal test can put a *good* entry first and a bad one second.
    /// That is what makes them tests of the set rather than of a row: if parsing returned a
    /// registry of one, the operator would be serving half of who they think they are.
    fn good(slug: &str, channel: &str) -> String {
        format!(
            r#"{{"slug":"{slug}","channel":"{channel}","service":"account-analysis",
                "guidance":"/etc/ironworks/{slug}.guidance.md",
                "records_credential":"env:{slug}_RECORDS","runtime_credential":"env:{slug}_RUNTIME"}}"#
        )
    }
    fn registry(entries: &[String]) -> String {
        format!(r#"{{"tenants":[{}]}}"#, entries.join(","))
    }

    #[test]
    fn a_valid_two_tenant_set_loads() {
        // The positive control. Without it every refusal below could pass on a parser that
        // refuses everything.
        let r = TenantRegistry::parse(&registry(&[good("acme", "-1001"), good("globex", "-1002")]))
            .expect("a valid two-tenant registry loads");
        assert_eq!(r.len(), 2);
        let claims = r.claims();
        assert_eq!(claims.len(), 2);
        assert_eq!(
            crate::route::resolve(&claims, &ChannelRef::new("-1002")).tenant().map(Slug::as_str),
            Some("globex")
        );
        assert_eq!(
            r.tenant(&Slug::parse("acme").unwrap()).map(|t| t.guidance().as_str()),
            Some("/etc/ironworks/acme.guidance.md")
        );
    }

    #[test]
    fn two_tenants_claiming_one_channel_refuse_the_whole_registry() {
        // The defect this exists to prevent: one conversation served with the wrong tenant's
        // credentials and records, decided by which entry happened to be last.
        let err =
            TenantRegistry::parse(&registry(&[good("acme", "-1001"), good("globex", "-1001")]))
                .unwrap_err();
        assert!(matches!(err, RegistryError::DuplicateChannel { .. }), "{err}");
        // Both sides named: "there is a duplicate somewhere" is not an actionable message.
        let text = err.to_string();
        assert!(text.contains("acme") && text.contains("globex"), "{text}");
    }

    #[test]
    fn a_repeated_slug_refuses_the_whole_registry() {
        let err = TenantRegistry::parse(&registry(&[good("acme", "-1001"), good("acme", "-1002")]))
            .unwrap_err();
        assert!(matches!(err, RegistryError::DuplicateSlug { .. }), "{err}");
    }

    #[test]
    fn an_unparseable_slug_refuses_the_whole_registry() {
        let err =
            TenantRegistry::parse(&registry(&[good("acme", "-1001"), good("Not A Slug", "-1002")]))
                .unwrap_err();
        assert!(matches!(err, RegistryError::InvalidField { index: 1, .. }), "{err}");
    }

    #[test]
    fn every_required_field_refuses_the_whole_registry_when_absent() {
        for field in
            ["slug", "channel", "service", "guidance", "records_credential", "runtime_credential"]
        {
            let broken = good("globex", "-1002")
                .replace(&format!(r#""{field}":"#), &format!(r#""{field}":"" ,"_{field}":"#));
            let err = TenantRegistry::parse(&registry(&[good("acme", "-1001"), broken]))
                .unwrap_err_or_panic(field);
            assert!(
                matches!(err, RegistryError::MissingField { index: 1, .. }),
                "empty {field} must refuse the set, got {err}"
            );
        }
    }

    #[test]
    fn a_room_with_no_address_answers_everything_and_a_group_must_say_its_handle() {
        let direct = TenantRegistry::parse(&registry(&[good("acme", "-1001")])).unwrap();
        assert_eq!(direct.entries()[0].addressing(), &crate::Addressing::Always);

        let with_handle = good("acme", "-1001")
            .replace(r#""slug":"acme""#, r#""slug":"acme","address":"@Multiferroic_bot""#);
        let grouped = TenantRegistry::parse(&registry(&[with_handle])).unwrap();
        assert_eq!(
            grouped.entries()[0].addressing(),
            &crate::Addressing::Mention("@Multiferroic_bot".into())
        );
    }

    #[test]
    fn an_empty_address_refuses_rather_than_answering_every_message_in_a_group() {
        // Someone meant to write a handle. Reading it as "a direct chat" would make the bot
        // answer every sentence anyone typed, one billed turn at a time.
        let blank = good("globex", "-1002")
            .replace(r#""slug":"globex""#, r#""slug":"globex","address":"   ""#);
        let err = TenantRegistry::parse(&registry(&[good("acme", "-1001"), blank])).unwrap_err();
        assert!(matches!(err, RegistryError::MissingField { index: 1, field: "address" }), "{err}");
    }

    #[test]
    fn a_registry_with_no_tenants_refuses() {
        // Not an empty registry object: a process that serves nobody is a configuration mistake,
        // and starting successfully would hide it until someone noticed the silence.
        assert_eq!(TenantRegistry::parse(r#"{"tenants":[]}"#).unwrap_err(), RegistryError::Empty);
        assert_eq!(TenantRegistry::parse("{}").unwrap_err(), RegistryError::Empty);
    }

    #[test]
    fn a_malformed_registry_refuses_rather_than_loading_what_parsed() {
        let err = TenantRegistry::parse(r#"{"tenants":[{"slug":"acme","#).unwrap_err();
        assert!(matches!(err, RegistryError::Malformed(_)), "{err}");
    }

    /// A tenant plus contributor bindings, each on its own conversation.
    ///
    /// `(channel, actor, contributor)`; an empty channel means "the tenant's own", which is the
    /// shape a one-person registry was written in before a tenant could have several.
    fn with_bindings(slug: &str, channel: &str, bindings: &[(&str, &str, &str)]) -> String {
        let rendered: Vec<String> = bindings
            .iter()
            .map(|(ch, a, c)| {
                if ch.is_empty() {
                    format!(r#"{{"actor":"{a}","contributor":"{c}"}}"#)
                } else {
                    format!(r#"{{"channel":"{ch}","actor":"{a}","contributor":"{c}"}}"#)
                }
            })
            .collect();
        good(slug, channel).replace(
            &format!(r#""slug":"{slug}""#),
            &format!(r#""slug":"{slug}","contributors":[{}]"#, rendered.join(",")),
        )
    }

    /// A tenant plus one contributor binding on the tenant's own channel.
    fn with_contributors(slug: &str, channel: &str, pairs: &[(&str, &str)]) -> String {
        let bindings: Vec<(&str, &str, &str)> = pairs.iter().map(|(a, c)| ("", *a, *c)).collect();
        with_bindings(slug, channel, &bindings)
    }

    /// The moment this patch exists for: two people, two private conversations, one tenant.
    #[test]
    fn two_people_in_their_own_private_chats_reach_one_tenant_and_one_organization() {
        // Telegram gives every person their own private chat id, so a team arrives on several
        // conversations. They must reach ONE tenant — one org, one record scope, one runtime
        // member — or the shared account store they are promoting into is not shared.
        let r = TenantRegistry::parse(&registry(&[with_bindings(
            "multiagency",
            "184223311",
            &[("184223311", "184223311", "alice"), ("713300284", "713300284", "bob")],
        )]))
        .expect("two people on one tenant");
        assert_eq!(r.len(), 1, "one tenant, not one per person");

        let claims = r.claims();
        let alice_chat = ChannelRef::new("184223311");
        let bob_chat = ChannelRef::new("713300284");
        for chat in [&alice_chat, &bob_chat] {
            assert_eq!(
                crate::route::resolve(&claims, chat).tenant().map(Slug::as_str),
                Some("multiagency"),
                "{chat} must route to the shared tenant"
            );
        }

        // One entry, so one set of credentials however many people are bound. Duplicating the
        // tenant per person would have given each their own organization.
        let t = r.tenant(&Slug::parse("multiagency").unwrap()).unwrap();
        assert_eq!(t.records().handle(), "env:multiagency_RECORDS");
        assert_eq!(t.runtime().handle(), "env:multiagency_RUNTIME");

        // And each conversation resolves to its own person.
        let alice = ActorRef::parse("184223311").unwrap();
        let bob = ActorRef::parse("713300284").unwrap();
        assert_eq!(t.contributor(&alice_chat, &alice).map(ContributorId::as_str), Some("alice"));
        assert_eq!(t.contributor(&bob_chat, &bob).map(ContributorId::as_str), Some("bob"));
    }

    #[test]
    fn a_sender_replayed_into_a_colleagues_conversation_authorizes_nothing() {
        // The cross-wire. Authorization is the pair, so Alice's id arriving in Bob's chat matches
        // no binding — and so does Bob's id in Alice's. Nothing catches this; the key never
        // described it.
        let r = TenantRegistry::parse(&registry(&[with_bindings(
            "multiagency",
            "184223311",
            &[("184223311", "184223311", "alice"), ("713300284", "713300284", "bob")],
        )]))
        .unwrap();
        let t = r.tenant(&Slug::parse("multiagency").unwrap()).unwrap();
        let alice = ActorRef::parse("184223311").unwrap();
        let bob = ActorRef::parse("713300284").unwrap();
        assert_eq!(t.contributor(&ChannelRef::new("713300284"), &alice), None);
        assert_eq!(t.contributor(&ChannelRef::new("184223311"), &bob), None);
        // And a conversation this tenant does not answer on binds nobody at all.
        assert_eq!(t.contributor(&ChannelRef::new("-100999"), &alice), None);
    }

    #[test]
    fn a_conversation_claimed_by_two_tenants_still_refuses_the_whole_registry() {
        // The check now spans every room a tenant answers on, so a contributor's chat colliding
        // with another tenant's is caught by the same rule that always caught the main one.
        let err = TenantRegistry::parse(&registry(&[
            with_bindings("acme", "-1001", &[("-2001", "184223311", "alice")]),
            with_bindings("globex", "-1002", &[("-2001", "713300284", "bob")]),
        ]))
        .unwrap_err();
        assert!(matches!(err, RegistryError::DuplicateChannel { .. }), "{err}");
        let text = err.to_string();
        assert!(text.contains("acme") && text.contains("globex"), "{text}");

        // And a contributor's chat colliding with another tenant's OWN channel, which is the
        // same defect wearing a different shape.
        let err = TenantRegistry::parse(&registry(&[
            good("acme", "-1001"),
            with_bindings("globex", "-1002", &[("-1001", "713300284", "bob")]),
        ]))
        .unwrap_err();
        assert!(matches!(err, RegistryError::DuplicateChannel { .. }), "{err}");
    }

    #[test]
    fn a_tenant_that_lists_no_contributors_authorizes_nobody() {
        // The default, and it is the fail-closed one: an existing registry gains no authority
        // from this field existing, so no deployment starts permitting promotion by upgrading.
        let r = TenantRegistry::parse(&registry(&[good("acme", "-1001")])).unwrap();
        let t = r.tenant(&Slug::parse("acme").unwrap()).unwrap();
        assert!(t.contributors().is_empty());
        assert_eq!(
            t.contributor(&ChannelRef::new("-1001"), &ActorRef::parse("184223311").unwrap()),
            None
        );
    }

    #[test]
    fn a_bound_actor_names_its_contributor_and_an_unbound_one_names_nobody() {
        let r = TenantRegistry::parse(&registry(&[with_contributors(
            "acme",
            "-1001",
            &[("184223311", "alice"), ("713300284", "bob")],
        )]))
        .unwrap();
        let t = r.tenant(&Slug::parse("acme").unwrap()).unwrap();
        let room = ChannelRef::new("-1001");
        assert_eq!(
            t.contributor(&room, &ActorRef::parse("184223311").unwrap()).map(ContributorId::as_str),
            Some("alice")
        );
        assert_eq!(
            t.contributor(&room, &ActorRef::parse("713300284").unwrap()).map(ContributorId::as_str),
            Some("bob")
        );
        // The negative control: an actor nobody wrote down is nobody, not the first entry.
        assert_eq!(t.contributor(&room, &ActorRef::parse("999999999").unwrap()), None);
    }

    #[test]
    fn one_actor_bound_twice_in_a_tenant_refuses_the_whole_registry() {
        // Which contributor a promoted record is attributed to would be decided by ordering, and
        // attribution is the entire product of the operation.
        let err = TenantRegistry::parse(&registry(&[with_contributors(
            "acme",
            "-1001",
            &[("184223311", "alice"), ("184223311", "bob")],
        )]))
        .unwrap_err();
        assert!(matches!(err, RegistryError::DuplicateActor { .. }), "{err}");
        let text = err.to_string();
        assert!(text.contains("alice") && text.contains("bob"), "{text}");
        // The actor key is the sender's identity and stays out of the operator's terminal.
        assert!(!text.contains("184223311"), "{text}");
    }

    #[test]
    fn one_actor_naming_two_contributors_in_two_rooms_refuses_the_whole_registry() {
        // The room is not part of who someone is. Authorization is the pair, so this WAS accepted
        // — and a promotion's attribution then depended on which of her own conversations Alice
        // happened to type in. That is the ambiguity the cross-tenant rule refuses; a tenant
        // boundary does not make it acceptable.
        let err = TenantRegistry::parse(&registry(&[with_bindings(
            "acme",
            "-1001",
            &[("-2001", "184223311", "alice"), ("-2002", "184223311", "bob")],
        )]))
        .unwrap_err();
        assert!(matches!(err, RegistryError::DuplicateActor { .. }), "{err}");
        let text = err.to_string();
        assert!(text.contains("alice") && text.contains("bob"), "{text}");
        assert!(!text.contains("184223311"), "{text}");
    }

    #[test]
    fn one_person_reachable_in_several_rooms_is_the_shape_bindings_exist_for() {
        // The positive control, and it is the one that matters: the rule above must refuse two
        // NAMES, never two rooms. Without this, tightening the check to the actor alone would
        // have refused exactly the configuration contributors were added to express — one person
        // who reaches the tenant through a direct chat and a shared room.
        let r = TenantRegistry::parse(&registry(&[with_bindings(
            "acme",
            "-1001",
            &[("-2001", "184223311", "alice"), ("-2002", "184223311", "alice")],
        )]))
        .expect("one person, two rooms, one name");
        let t = r.tenant(&Slug::parse("acme").unwrap()).unwrap();
        let alice = ActorRef::parse("184223311").unwrap();
        for room in ["-2001", "-2002"] {
            assert_eq!(
                t.contributor(&ChannelRef::new(room), &alice).map(ContributorId::as_str),
                Some("alice"),
                "{room} must reach the same person"
            );
        }
    }

    #[test]
    fn one_actor_bound_in_two_tenants_refuses_the_whole_registry() {
        // A promotion would have two organizations to append to. Refusing the set is the same
        // rule the duplicate-channel check applies, for the same reason.
        let err = TenantRegistry::parse(&registry(&[
            with_contributors("acme", "-1001", &[("184223311", "alice")]),
            with_contributors("globex", "-1002", &[("184223311", "alice")]),
        ]))
        .unwrap_err();
        assert!(matches!(err, RegistryError::ActorInTwoTenants { .. }), "{err}");
        let text = err.to_string();
        assert!(text.contains("acme") && text.contains("globex"), "{text}");
        assert!(!text.contains("184223311"), "{text}");
    }

    #[test]
    fn two_tenants_may_each_bind_their_own_actors() {
        // The positive control for the check above: without it, refusing every second tenant
        // that has any binding at all would pass the test that matters.
        let r = TenantRegistry::parse(&registry(&[
            with_contributors("acme", "-1001", &[("184223311", "alice")]),
            with_contributors("globex", "-1002", &[("713300284", "bob")]),
        ]))
        .unwrap();
        assert_eq!(r.len(), 2);
    }

    #[test]
    fn a_malformed_binding_refuses_the_whole_registry() {
        for broken in [
            r#""contributors":[{"actor":"","contributor":"alice"}]"#,
            r#""contributors":[{"actor":"184223311","contributor":""}]"#,
            r#""contributors":[{"actor":"184223311","contributor":"Alice"}]"#,
            r#""contributors":[{"actor":" 184223311","contributor":"alice"}]"#,
            r#""contributors":[{"contributor":"alice"}]"#,
        ] {
            let bad = good("globex", "-1002")
                .replace(r#""slug":"globex""#, &format!(r#""slug":"globex",{broken}"#));
            let err =
                TenantRegistry::parse(&registry(&[good("acme", "-1001"), bad])).expect_err(broken);
            assert!(
                matches!(
                    err,
                    RegistryError::MissingField { index: 1, .. }
                        | RegistryError::InvalidField { index: 1, .. }
                ),
                "{broken} gave {err}"
            );
        }
    }

    #[test]
    fn a_tenant_without_an_append_credential_simply_cannot_promote() {
        // The default, and the one that keeps every existing registry exactly as it was: a
        // deployment gains no write authority by running a newer binary.
        let r = TenantRegistry::parse(&registry(&[good("acme", "-1001")])).unwrap();
        assert_eq!(r.entries()[0].append(), None);
    }

    #[test]
    fn an_append_credential_is_its_own_purpose_and_not_the_read_one() {
        let with_append = good("acme", "-1001")
            .replace(r#""slug":"acme""#, r#""slug":"acme","append_credential":"env:acme_APPEND""#);
        let r = TenantRegistry::parse(&registry(&[with_append])).unwrap();
        let t = &r.entries()[0];
        let append = t.append().expect("declared");
        assert_eq!(append.purpose(), CredentialPurpose::RecordAppend);
        assert_eq!(append.handle(), "env:acme_APPEND");
        // Three handles, three purposes, no overlap. Reading and appending are different
        // authorities and neither can be passed where the other belongs.
        assert_eq!(t.records().purpose(), CredentialPurpose::RecordScope);
        assert_eq!(t.runtime().purpose(), CredentialPurpose::RuntimeMember);
        assert_ne!(append.handle(), t.records().handle());
    }

    #[test]
    fn an_empty_append_credential_refuses_the_whole_registry() {
        // Someone meant to write a handle. Reading it as "this tenant cannot promote" would
        // disable a feature silently rather than saying the configuration is wrong.
        let blank = good("globex", "-1002")
            .replace(r#""slug":"globex""#, r#""slug":"globex","append_credential":"  ""#);
        let err = TenantRegistry::parse(&registry(&[good("acme", "-1001"), blank])).unwrap_err();
        assert!(
            matches!(err, RegistryError::MissingField { index: 1, field: "append_credential" }),
            "{err}"
        );
    }

    #[test]
    fn credentials_carry_their_purpose_so_one_cannot_stand_in_for_the_other() {
        let r = TenantRegistry::parse(&registry(&[good("acme", "-1001")])).unwrap();
        let t = r.tenant(&Slug::parse("acme").unwrap()).unwrap();
        assert_eq!(t.records().purpose(), CredentialPurpose::RecordScope);
        assert_eq!(t.runtime().purpose(), CredentialPurpose::RuntimeMember);
        // The handle is a location, never a value: a careless print leaks a variable name.
        assert_eq!(t.records().handle(), "env:acme_RECORDS");
    }

    trait UnwrapErrOrPanic<T> {
        fn unwrap_err_or_panic(self, what: &str) -> RegistryError;
    }
    impl UnwrapErrOrPanic<TenantRegistry> for Result<TenantRegistry, RegistryError> {
        fn unwrap_err_or_panic(self, what: &str) -> RegistryError {
            match self {
                Ok(r) => panic!("empty {what} loaded a registry of {} tenants", r.len()),
                Err(e) => e,
            }
        }
    }
}
