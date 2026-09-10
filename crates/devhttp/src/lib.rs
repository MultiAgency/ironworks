//! The HTTP a proof fixture needs, once.
//!
//! # What was here twice, and had drifted
//!
//! `ironworks-devstore` and `ironworks-devchannel` each read a request by hand — request line,
//! header loop, `content-length`, body — and each wrote a response by hand. The two were the same
//! code, and they had already come apart in three places:
//!
//! - **Header case folding.** `devstore` carries a fix whose comment records the defect: *"Both
//!   used to come off the lowercased copy, which silently downcased every credential before the
//!   lookup in `cfg.identities`, working only because each fixture token happens to be lowercase."*
//!   `devchannel` still matched on `h.to_ascii_lowercase().strip_prefix("content-length:")`, which
//!   has the same shape and also fails on any leading whitespace.
//! - **The status line.** `devstore` emitted `HTTP/1.1 {status} X`; `devchannel` emitted `OK` or
//!   `ERR`. One of those reason phrases is wrong for every status.
//! - **Bounds.** Neither had any. `crates/telegram/src/main.rs` grew bounded line reads, a header
//!   budget, IO timeouts and a refusal on an unparseable `content-length` — and the fixtures are
//!   what the live proofs actually run against, so that hardening was measured somewhere it was
//!   not applied.
//!
//! The bounds below are telegram's, ported. A fixture is not exposed to the internet, and that is
//! not the argument: an unbounded `read_line` is how a fixture stops being a fixture and becomes a
//! thing under test.
//!
//! # What this deliberately is not
//!
//! Not a server, not a router, not a framework. Both fixtures state the same reason for staying
//! minimal — anything they grow makes the live gate less trustworthy, because the thing under test
//! would be less like production and more like them. This reads a request and writes a response.

use std::collections::BTreeMap;
use std::io::{BufRead, Write};
use std::net::TcpStream;
use std::time::Duration;

/// A request line longer than this is refused rather than buffered.
pub const MAX_REQUEST_LINE_BYTES: usize = 8 * 1024;
/// One header line's ceiling.
pub const MAX_HEADER_LINE_BYTES: usize = 8 * 1024;
/// Every header line together.
pub const MAX_HEADER_BYTES: usize = 32 * 1024;
/// A body larger than this is refused rather than allocated.
pub const MAX_BODY_BYTES: usize = 4 * 1024 * 1024;
/// Neither read nor write may block a single-threaded accept loop forever.
pub const IO_TIMEOUT: Duration = Duration::from_secs(30);

/// One parsed request.
///
/// `query` is the raw string after `?`, undecoded: the two fixtures decode differently on purpose
/// and this layer does not choose for them.
#[derive(Debug)]
pub struct Request {
    pub method: String,
    pub path: String,
    pub query: String,
    pub body: Vec<u8>,
    /// Names folded to lowercase; **values verbatim**. See [`Request::header`].
    headers: BTreeMap<String, String>,
}

impl Request {
    /// A header's value, by case-insensitive name.
    ///
    /// **The name is folded, the value is not.** A header name is case-insensitive per HTTP and a
    /// bearer token is not, so reading a value off a lowercased copy of the line silently downcases
    /// every credential — which works exactly as long as every fixture token happens to be
    /// lowercase, a property of the fixtures and not of this parser.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(&name.to_ascii_lowercase()).map(String::as_str)
    }
}

/// Why a request was not read, and what to answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// The peer went away, or sent nothing. Answer nothing.
    Closed,
    RequestLineTooLarge,
    HeadersTooLarge,
    /// The request line or a header is not UTF-8. Refused rather than decoded lossily: a
    /// replacement character is a byte nobody sent.
    Malformed,
    UnreadableContentLength,
    BodyTooLarge,
    IncompleteBody,
}

impl Refusal {
    /// The status and JSON body to answer with, or `None` when there is no one to answer.
    pub fn response(self) -> Option<(u16, &'static str)> {
        Some(match self {
            Self::Closed => return None,
            Self::RequestLineTooLarge => (414, r#"{"error":"request too large"}"#),
            Self::HeadersTooLarge => (431, r#"{"error":"headers too large"}"#),
            Self::Malformed => (400, r#"{"error":"malformed request"}"#),
            Self::UnreadableContentLength => (400, r#"{"error":"invalid content length"}"#),
            Self::BodyTooLarge => (413, r#"{"error":"body too large"}"#),
            Self::IncompleteBody => (400, r#"{"error":"incomplete body"}"#),
        })
    }
}

/// Apply the IO deadlines a single-threaded accept loop needs to survive a peer that stops talking.
pub fn set_timeouts(stream: &TcpStream) {
    let _ = stream.set_read_timeout(Some(IO_TIMEOUT));
    let _ = stream.set_write_timeout(Some(IO_TIMEOUT));
}

/// Read one request: the line, the headers, and exactly `content-length` bytes of body.
pub fn read(reader: &mut impl BufRead) -> Result<Request, Refusal> {
    let line = match bounded_line(reader, MAX_REQUEST_LINE_BYTES) {
        Ok(Some(line)) => line,
        Ok(None) => return Err(Refusal::Closed),
        Err(Line::TooLong) => return Err(Refusal::RequestLineTooLarge),
        Err(Line::NotUtf8) => return Err(Refusal::Malformed),
    };

    let mut headers = BTreeMap::new();
    let mut length = 0usize;
    let mut header_bytes = 0usize;
    loop {
        let header = match bounded_line(reader, MAX_HEADER_LINE_BYTES) {
            Ok(Some(header)) => header,
            // A request whose headers never end is the same refusal as headers that are too big.
            Ok(None) | Err(Line::TooLong) => return Err(Refusal::HeadersTooLarge),
            Err(Line::NotUtf8) => return Err(Refusal::Malformed),
        };
        if header == "\r\n" || header == "\n" {
            break;
        }
        header_bytes = header_bytes.saturating_add(header.len());
        if header_bytes > MAX_HEADER_BYTES {
            return Err(Refusal::HeadersTooLarge);
        }
        let Some((name, value)) = header.split_once(':') else { continue };
        let name = name.trim().to_ascii_lowercase();
        let value = value.trim().to_owned();
        if name == "content-length" {
            // Refused, not defaulted. `parse().unwrap_or(0)` reads a malformed length as an empty
            // body, so a request that could not be understood is answered as though it carried
            // nothing — which is a different request from the one that arrived.
            length = value.parse().map_err(|_| Refusal::UnreadableContentLength)?;
        }
        headers.insert(name, value);
    }

    if length > MAX_BODY_BYTES {
        return Err(Refusal::BodyTooLarge);
    }
    let mut body = vec![0u8; length];
    if length > 0 {
        reader.read_exact(&mut body).map_err(|_| Refusal::IncompleteBody)?;
    }

    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or("").to_owned();
    let target = parts.next().unwrap_or("/");
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    Ok(Request { method, path: path.to_owned(), query: query.to_owned(), body, headers })
}

/// Why [`bounded_line`] did not return a line.
enum Line {
    /// It would exceed its bound before ending, or the read failed.
    TooLong,
    /// Its bytes are not UTF-8.
    NotUtf8,
}

/// One line, or why not.
///
/// The bound is the point: `BufRead::read_line` on a peer that never sends `\n` grows a `String`
/// until the process dies. The line is collected as bytes and decoded once, whole. Pushing each
/// byte as its own `char` re-encoded UTF-8 as Latin-1, so a header value read back as bytes nobody
/// sent — `tök` as `tÃ¶k` — under a doc that promises values verbatim.
fn bounded_line(reader: &mut impl BufRead, maximum: usize) -> Result<Option<String>, Line> {
    let mut line = Vec::new();
    loop {
        let mut byte = [0u8; 1];
        match reader.read(&mut byte) {
            Ok(0) if line.is_empty() => return Ok(None),
            Ok(0) => break,
            Ok(_) => {
                line.push(byte[0]);
                if byte[0] == b'\n' {
                    break;
                }
                if line.len() > maximum {
                    return Err(Line::TooLong);
                }
            }
            Err(_) => return Err(Line::TooLong),
        }
    }
    String::from_utf8(line).map(Some).map_err(|_| Line::NotUtf8)
}

/// A JSON response.
pub fn respond(stream: &mut TcpStream, status: u16, body: &str) {
    write_response(stream, status, "application/json", body);
}

/// A plain-text response, for the routes whose production counterpart answers in text.
pub fn respond_text(stream: &mut TcpStream, status: u16, body: &str) {
    write_response(stream, status, "text/plain; charset=utf-8", body);
}

/// Answer a [`Refusal`], if there is anyone left to answer.
pub fn respond_refusal(stream: &mut TcpStream, why: Refusal) {
    if let Some((status, body)) = why.response() {
        respond(stream, status, body);
    }
}

fn write_response(stream: &mut TcpStream, status: u16, content_type: &str, body: &str) {
    // A real reason phrase. One copy of this emitted the literal `X` for every status and the
    // other emitted `OK` or `ERR`, so between them no status was described correctly.
    let reason = match status {
        200 => "OK",
        201 => "Created",
        400 => "Bad Request",
        401 => "Unauthorized",
        404 => "Not Found",
        409 => "Conflict",
        413 => "Payload Too Large",
        414 => "URI Too Long",
        431 => "Request Header Fields Too Large",
        500 => "Internal Server Error",
        _ => "Unknown",
    };
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(body.as_bytes());
    let _ = stream.flush();
}

#[cfg(test)]
mod tests;
