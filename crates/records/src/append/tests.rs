use super::*;
use crate::credential::EnvCredentials;

fn org(id: &str) -> OrgId {
    OrgId::parse(id).expect("org")
}
fn account() -> AccountId {
    AccountId::parse("ACME-001").expect("account")
}
fn who() -> ContributorId {
    ContributorId::parse("alice").expect("contributor")
}

/// One append, in the organization the caller expects.
fn request<'a>(expected: &'a OrgId, account: &'a AccountId, who: &'a ContributorId) -> Append<'a> {
    Append {
        activity_id: "share-abc123",
        account,
        occurred_at: "2026-09-01",
        body: "Customer requested a September renewal review.",
        contributor: who,
        expected_org: expected,
    }
}

type Read = Result<Appended, AppendFailure>;

fn read(status: u16, body: &str) -> Read {
    let expected = org("multiagency-org");
    let account = account();
    let who = who();
    AppendClient::<EnvCredentials>::read(status, body, &request(&expected, &account, &who))
}

#[test]
fn a_created_row_and_a_replayed_one_are_both_success_and_are_distinguishable() {
    // Both mean "the confirmed words are in the store". They are still two facts: an operator
    // reading a log needs to know whether this attempt wrote, and a retry after an ambiguous
    // failure lands on the second.
    let created =
        read(201, r#"{"status":"created","org":"multiagency-org","activity_id":"share-abc123"}"#)
            .expect("created");
    assert!(matches!(created, Appended::Created { .. }));
    assert_eq!(created.activity_id(), "share-abc123");
    assert_eq!(created.org().as_str(), "multiagency-org");

    let replayed =
        read(200, r#"{"status":"replayed","org":"multiagency-org","activity_id":"share-abc123"}"#)
            .expect("replayed");
    assert!(matches!(replayed, Appended::Replayed { .. }));
    assert_eq!(replayed.activity_id(), "share-abc123");
}

#[test]
fn a_conflict_is_permanent_and_names_what_differed() {
    // The one refusal that must never become a retry: two different notes cannot both be the
    // promotion of one proposal, and overwriting either is what an immutable record forbids.
    let err = read(
        409,
        r#"{"error":"conflict","org":"multiagency-org","activity_id":"share-abc123",
            "differing_field":"body"}"#,
    )
    .unwrap_err();
    assert!(
        matches!(&err, AppendFailure::Conflict { differing_field, .. } if differing_field == "body")
    );
    assert!(!err.may_retry_identical());
    assert!(err.to_string().contains("never overwritten"), "{err}");
}

#[test]
fn an_organization_the_credential_does_not_authorise_is_scope_drift_not_a_conflict() {
    // The guard's own answer. It reads as drift rather than as a content conflict because
    // nothing about the note is wrong — the credential is pointed somewhere else.
    let err = read(409, r#"{"error":"org_mismatch","org":"somebody-else"}"#).unwrap_err();
    assert!(
        matches!(&err, AppendFailure::ScopeDrift { answered, .. } if answered.as_str() == "somebody-else")
    );
    assert!(!err.may_retry_identical());
}

#[test]
fn a_success_answering_for_another_organization_is_refused() {
    // Belt and braces over the server's own comparison. A store that reported success while
    // naming a different org is one this must not believe, whatever it says it did.
    let err = read(201, r#"{"status":"created","org":"somebody-else","activity_id":"share-abc"}"#)
        .unwrap_err();
    assert!(matches!(err, AppendFailure::ScopeDrift { .. }));
}

#[test]
fn an_unknown_account_is_named_and_is_not_retryable() {
    let err = read(404, r#"{"error":"unknown_account","org":"multiagency-org"}"#).unwrap_err();
    assert!(matches!(&err, AppendFailure::UnknownAccount { account, .. } if account == "ACME-001"));
    assert!(!err.may_retry_identical());
}

#[test]
fn a_refused_credential_is_never_read_as_something_to_retry() {
    for status in [401, 403] {
        let err = read(status, r#"{"error":"unauthorized"}"#).unwrap_err();
        assert!(matches!(err, AppendFailure::Unauthorized), "{status}");
        assert!(!err.may_retry_identical());
    }
}

#[test]
fn a_store_that_did_not_decide_leaves_the_request_retryable() {
    // The whole retry contract in one place: exactly the outcomes where the store established
    // nothing may be sent again, and the identical body is what makes that a replay rather than
    // a second write.
    for (status, body) in [
        (500, r#"{"error":"internal"}"#),
        (503, r#"{"error":"append_unresolved","ref":"abc"}"#),
        (502, "<html>gateway</html>"),
    ] {
        let err = read(status, body).unwrap_err();
        assert!(
            err.may_retry_identical(),
            "HTTP {status} established nothing and must stay retryable, got {err:?}"
        );
    }
}

#[test]
fn a_rate_limit_is_the_one_4xx_that_asks_for_the_same_request_later() {
    // It is not a decision about the request. The store is saying "not now", which is precisely
    // the fact `Unreachable` carries — and the identical body is still the right thing to send.
    let err = read(429, r#"{"error":"rate_limited","retry_after":30}"#).unwrap_err();
    assert!(err.may_retry_identical(), "a 429 asks for a retry, got {err:?}");
}

#[test]
fn a_4xx_this_build_has_no_branch_for_is_still_a_decision() {
    // The one that was wrong. These fell through to `Unreachable`, so a `/confirm` against a
    // mistyped endpoint replayed a refused request on every attempt and never terminated. None
    // of them becomes true by being sent again.
    for (status, body, why) in [
        (404, r#"{"error":"not_found"}"#, "a mistyped endpoint is not a transient outage"),
        (405, r#"{"error":"method_not_allowed"}"#, "the route does not accept a POST"),
        (410, r#"{"error":"gone"}"#, "the endpoint was withdrawn"),
        (422, r#"{"error":"unprocessable"}"#, "the store understood it and refused"),
    ] {
        let err = read(status, body).unwrap_err();
        assert!(!err.may_retry_identical(), "HTTP {status}: {why}, got {err:?}");
        assert!(
            matches!(err, AppendFailure::Refused { .. }),
            "HTTP {status} must name the store's refusal, got {err:?}"
        );
    }
    // The store's own code travels when it gave one, and the status stands in when it did not.
    let named = read(422, r#"{"error":"unprocessable"}"#).unwrap_err();
    assert!(named.to_string().contains("unprocessable"), "{named}");
    let unnamed = read(410, "gone").unwrap_err();
    assert!(unnamed.to_string().contains("410"), "{unnamed}");
}

#[test]
fn a_validation_refusal_is_a_decision_and_is_not_retried() {
    let err = read(400, r#"{"error":"invalid_contributor"}"#).unwrap_err();
    assert!(matches!(&err, AppendFailure::Refused { code } if code == "invalid_contributor"));
    assert!(!err.may_retry_identical());
}

#[test]
fn only_an_append_credential_can_append() {
    // The purpose separation, at the boundary where a read handle would otherwise become a
    // write one. The registry stamps the purpose at parse, so this is unreachable in practice —
    // and it is the check that keeps it unreachable.
    let client = AppendClient::new("http://127.0.0.1:1", EnvCredentials);
    let expected = org("multiagency-org");
    let account = account();
    let contributor = who();
    for purpose in [CredentialPurpose::RecordScope, CredentialPurpose::RuntimeMember] {
        let wrong = CredentialRef::new(purpose, "env:SOMETHING");
        let err = client
            .append(&wrong, &request(&expected, &account, &contributor))
            .expect_err("a non-append credential must not append");
        assert!(matches!(err, AppendFailure::WrongPurpose { .. }), "{purpose:?}: {err}");
        // And it refuses before resolving anything, so a wrong-purpose handle is never even read.
        assert!(!err.may_retry_identical());
    }
}

#[test]
fn a_missing_credential_is_named_by_its_location_and_never_its_value() {
    let client = AppendClient::new("http://127.0.0.1:1", EnvCredentials);
    let expected = org("multiagency-org");
    let account = account();
    let contributor = who();
    let absent =
        CredentialRef::new(CredentialPurpose::RecordAppend, "env:IRONWORKS_DEFINITELY_ABSENT");
    let err = client
        .append(&absent, &request(&expected, &account, &contributor))
        .expect_err("no credential, no append");
    assert!(matches!(&err, AppendFailure::NoCredential { handle } if handle.contains("env:")));
    assert!(!err.may_retry_identical());
}

#[test]
fn a_success_that_does_not_acknowledge_the_request_is_ambiguous_not_proven() {
    // A `2xx` naming a different activity, or claiming a status the HTTP code contradicts, is an
    // answer this cannot read. The write may still have landed, so it must stay replayable —
    // treating it as a refusal would strand a committed row, and treating it as success would
    // record a fiction.
    for (status, body, why) in [
        (
            201,
            r#"{"status":"created","org":"multiagency-org","activity_id":"share-somethingelse"}"#,
            "acknowledged a different activity",
        ),
        (
            201,
            r#"{"status":"replayed","org":"multiagency-org","activity_id":"share-abc123"}"#,
            "201 means created, not replayed",
        ),
        (
            200,
            r#"{"status":"created","org":"multiagency-org","activity_id":"share-abc123"}"#,
            "200 means replayed, not created",
        ),
        (200, r#"{"org":"multiagency-org","activity_id":"share-abc123"}"#, "no status at all"),
        (201, r#"{"status":"created","org":"multiagency-org"}"#, "no activity id at all"),
    ] {
        let err = read(status, body).expect_err(why);
        assert!(
            err.may_retry_identical(),
            "{why}: the write may have landed, so it must stay replayable — got {err:?}"
        );
    }
}

#[test]
fn a_truthful_acknowledgement_is_still_accepted() {
    // The positive control for the check above. Without it, a `read` that rejected every 2xx
    // would satisfy the ambiguity test and break the whole path.
    assert!(matches!(
        read(201, r#"{"status":"created","org":"multiagency-org","activity_id":"share-abc123"}"#),
        Ok(Appended::Created { .. })
    ));
    assert!(matches!(
        read(200, r#"{"status":"replayed","org":"multiagency-org","activity_id":"share-abc123"}"#),
        Ok(Appended::Replayed { .. })
    ));
}

#[test]
fn an_unreadable_success_body_is_ambiguous_not_malformed() {
    // A `2xx` means the store believes it wrote. A body this cannot parse therefore leaves a
    // request that may well have landed, and classifying it as permanent would strand a
    // committed row behind an unreadable acknowledgement. Ambiguity is the only safe reading,
    // and it is what permits replaying the byte-identical request.
    for status in [200, 201] {
        for (body, why) in [
            ("<html>502 from a proxy</html>", "not JSON at all"),
            ("", "empty body"),
            (r#"{"status":"created","org":"multiagency-org","#, "truncated JSON"),
            ("[1,2,3]", "JSON that is not an object"),
        ] {
            let err = read(status, body).expect_err(why);
            assert!(
                err.may_retry_identical(),
                "HTTP {status} with {why} must stay replayable, got {err:?}"
            );
        }
    }
}

#[test]
fn a_success_without_a_readable_organization_is_ambiguous_not_malformed() {
    // The organization is part of the acknowledgement. Absent, empty or unparseable, the answer
    // cannot be turned into a truthful `Appended` — and the write may still have landed.
    for status in [200, 201] {
        for (body, why) in [
            (r#"{"status":"created","activity_id":"share-abc123"}"#, "no org at all"),
            (r#"{"status":"created","org":"","activity_id":"share-abc123"}"#, "empty org"),
            (
                r#"{"status":"created","org":"Not An Org","activity_id":"share-abc123"}"#,
                "an org id this cannot parse",
            ),
            (
                r#"{"status":"created","org":42,"activity_id":"share-abc123"}"#,
                "an org that is not a string",
            ),
        ] {
            let err = read(status, body).expect_err(why);
            assert!(
                err.may_retry_identical(),
                "HTTP {status} with {why} must stay replayable, got {err:?}"
            );
        }
    }
}

#[test]
fn a_valid_success_naming_another_organization_stays_scope_drift() {
    // The boundary between the two readings, asserted so the fix above cannot widen into it. A
    // parseable org that is simply not ours is the store telling us plainly where it wrote —
    // a decision, not ambiguity — and replaying into it is exactly what must not happen.
    for status in [200, 201] {
        let ok_status = if status == 201 { "created" } else { "replayed" };
        let body = format!(
            r#"{{"status":"{ok_status}","org":"somebody-else","activity_id":"share-abc123"}}"#
        );
        let err = read(status, &body).expect_err("another org is drift");
        assert!(matches!(err, AppendFailure::ScopeDrift { .. }), "HTTP {status}: {err:?}");
        assert!(!err.may_retry_identical(), "HTTP {status}: drift is never replayed");
    }
}

#[test]
fn an_unreadable_refusal_is_still_a_decision_and_is_not_retried() {
    // The other side of the same rule, so the fix cannot drift into making every unreadable
    // answer retryable: a 4xx is a refusal the store made, and repeating a refused request
    // would loop forever.
    let err = read(400, "<html>bad request</html>").unwrap_err();
    assert!(matches!(err, AppendFailure::Malformed { .. }), "{err:?}");
    assert!(!err.may_retry_identical());
}
