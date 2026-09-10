use super::*;
use std::io::BufReader;

fn read_str(raw: &str) -> Result<Request, Refusal> {
    read(&mut BufReader::new(raw.as_bytes()))
}

#[test]
fn a_request_line_splits_into_method_path_and_query() {
    let r = read_str("GET /poll?offset=7&x=1 HTTP/1.1\r\nhost: fixture\r\n\r\n").expect("read");
    assert_eq!(r.method, "GET");
    assert_eq!(r.path, "/poll");
    assert_eq!(r.query, "offset=7&x=1");
    assert!(r.body.is_empty());
}

/// The defect `devstore` fixed and `devchannel` never received.
#[test]
fn a_header_name_folds_and_its_value_does_not() {
    let r = read_str(
        "POST /send HTTP/1.1\r\nX-Service-Token: Mixed-Case-SECRET\r\ncontent-length: 0\r\n\r\n",
    )
    .expect("read");
    assert_eq!(r.header("x-service-token"), Some("Mixed-Case-SECRET"));
    assert_eq!(r.header("X-SERVICE-TOKEN"), Some("Mixed-Case-SECRET"), "the name is folded");
    // The positive control: a value that *is* lowercase reads back unchanged too, so the assertion
    // above is not passing because everything is being returned verbatim by accident.
    assert_eq!(read_str("GET / HTTP/1.1\r\nA: b\r\n\r\n").unwrap().header("a"), Some("b"));
}

/// Verbatim includes a value that is not ASCII. Each byte used to be pushed as its own `char`,
/// which re-encoded UTF-8 as Latin-1: `tök` read back as `tÃ¶k`.
#[test]
fn a_non_ascii_header_value_reads_back_verbatim() {
    let r = read_str("GET /x HTTP/1.1\r\nX-Name: tök\r\n\r\n").expect("read");
    assert_eq!(r.header("x-name"), Some("tök"));
}

/// Refused, not repaired: a lossy decode answers a request carrying a byte nobody sent.
#[test]
fn a_request_line_or_header_that_is_not_utf8_is_refused_as_malformed() {
    let read_bytes = |raw: &[u8]| read(&mut BufReader::new(raw));
    assert_eq!(
        read_bytes(b"GET /x HTTP/1.1\r\nX-Name: t\xffk\r\n\r\n").unwrap_err(),
        Refusal::Malformed
    );
    assert_eq!(read_bytes(b"GET /t\xffk HTTP/1.1\r\n\r\n").unwrap_err(), Refusal::Malformed);
    // The positive control: the same request with the byte valid is read, not refused.
    assert!(read_bytes("GET /tök HTTP/1.1\r\nX-Name: tök\r\n\r\n".as_bytes()).is_ok());
}

#[test]
fn a_body_is_read_to_exactly_its_content_length() {
    let r = read_str("POST /x HTTP/1.1\r\ncontent-length: 5\r\n\r\nhelloTRAILING").expect("read");
    assert_eq!(r.body, b"hello");
}

/// `parse().unwrap_or(0)` answered a request nobody sent: a malformed length became an empty body.
#[test]
fn an_unreadable_content_length_is_refused_rather_than_read_as_empty() {
    assert_eq!(
        read_str("POST /x HTTP/1.1\r\ncontent-length: soon\r\n\r\n").unwrap_err(),
        Refusal::UnreadableContentLength
    );
    // The positive control: a readable one is not refused.
    assert!(read_str("POST /x HTTP/1.1\r\ncontent-length: 0\r\n\r\n").is_ok());
}

#[test]
fn an_unbounded_request_line_or_header_is_refused_rather_than_buffered() {
    let long = "x".repeat(MAX_REQUEST_LINE_BYTES + 10);
    assert_eq!(
        read_str(&format!("GET /{long} HTTP/1.1\r\n\r\n")).unwrap_err(),
        Refusal::RequestLineTooLarge
    );

    let value = "y".repeat(MAX_HEADER_LINE_BYTES + 10);
    assert_eq!(
        read_str(&format!("GET / HTTP/1.1\r\nbig: {value}\r\n\r\n")).unwrap_err(),
        Refusal::HeadersTooLarge
    );

    // Many headers, each small enough on its own, are refused on the total.
    let many: String =
        (0..600).map(|i| format!("h{i}: {}\r\n", "z".repeat(100))).collect::<Vec<_>>().concat();
    assert_eq!(
        read_str(&format!("GET / HTTP/1.1\r\n{many}\r\n")).unwrap_err(),
        Refusal::HeadersTooLarge
    );
}

#[test]
fn a_declared_body_larger_than_the_ceiling_is_refused_before_it_is_allocated() {
    assert_eq!(
        read_str(&format!("POST /x HTTP/1.1\r\ncontent-length: {}\r\n\r\n", MAX_BODY_BYTES + 1))
            .unwrap_err(),
        Refusal::BodyTooLarge
    );
}

#[test]
fn a_body_shorter_than_it_claimed_is_incomplete_rather_than_silently_short() {
    assert_eq!(
        read_str("POST /x HTTP/1.1\r\ncontent-length: 10\r\n\r\nshort").unwrap_err(),
        Refusal::IncompleteBody
    );
}

#[test]
fn a_peer_that_sent_nothing_is_closed_and_gets_no_answer() {
    assert_eq!(read_str("").unwrap_err(), Refusal::Closed);
    assert_eq!(Refusal::Closed.response(), None);
    // Every other refusal does answer, and says which it is.
    for why in [
        Refusal::RequestLineTooLarge,
        Refusal::HeadersTooLarge,
        Refusal::Malformed,
        Refusal::UnreadableContentLength,
        Refusal::BodyTooLarge,
        Refusal::IncompleteBody,
    ] {
        let (status, body) = why.response().expect("a refusal with a peer must answer");
        assert!((400..500).contains(&status), "{why:?} answered {status}");
        assert!(body.starts_with("{\"error\":"), "{why:?} answered {body}");
    }
}
