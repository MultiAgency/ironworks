use super::*;
use ironworks_domain::{CredentialPurpose, CredentialRef, ModelPin};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

#[derive(Debug)]
struct Request {
    method: String,
    path: String,
    body: String,
}

struct FakePolicy {
    base: String,
    requests: Arc<Mutex<Vec<Request>>>,
}

fn policy_server(replies: Vec<(u16, String)>) -> FakePolicy {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind fixture policy server");
    let base = format!("http://{}", listener.local_addr().unwrap());
    let requests = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&requests);
    std::thread::spawn(move || {
        for (stream, (status, reply)) in listener.incoming().zip(replies) {
            let Ok(mut stream) = stream else { continue };
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut request_line = String::new();
            reader.read_line(&mut request_line).unwrap();
            let mut content_length = 0;
            loop {
                let mut header = String::new();
                reader.read_line(&mut header).unwrap();
                if header == "\r\n" {
                    break;
                }
                if let Some(value) = header.to_ascii_lowercase().strip_prefix("content-length:") {
                    content_length = value.trim().parse().unwrap();
                }
            }
            let mut body = vec![0; content_length];
            reader.read_exact(&mut body).unwrap();
            let mut parts = request_line.split_whitespace();
            log.lock().unwrap().push(Request {
                method: parts.next().unwrap_or("").to_owned(),
                path: parts.next().unwrap_or("").to_owned(),
                body: String::from_utf8(body).unwrap(),
            });
            let head = format!(
                "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                reply.len()
            );
            stream.write_all(head.as_bytes()).unwrap();
            stream.write_all(reply.as_bytes()).unwrap();
        }
    });
    FakePolicy { base, requests }
}

fn catalogue(default: &str, models: &[&str]) -> String {
    serde_json::json!({
        "selection_enabled": true,
        "workspace_default": default,
        "models": models,
    })
    .to_string()
}

fn client(server: &FakePolicy) -> ModelPolicyControl {
    let credential =
        CredentialRef::new(CredentialPurpose::OperatorConfiguration, "env:FIXTURE_OPERATOR");
    let mut client = ModelPolicyControl::new(&server.base, &credential);
    client.set_token_for_test(OperatorToken::new("synthetic-operator-token"));
    client
}

#[test]
fn exact_match_passes_and_a_widened_allowlist_does_not() {
    let pin = ModelPin::parse("qwen/pinned").unwrap();
    let exact = policy_server(vec![(200, catalogue(pin.as_str(), &[pin.as_str()]))]);
    assert!(client(&exact).check(&pin).unwrap().is_exact());

    // Positive widening control: this proves the comparison saw both values rather than passing
    // because it ignored the catalogue or accepted mere inclusion of the pin.
    let widened = policy_server(vec![(
        200,
        catalogue(pin.as_str(), &[pin.as_str(), "provider/other-model"]),
    )]);
    let comparison = client(&widened).check(&pin).unwrap();
    assert!(!comparison.is_exact());
    assert!(comparison.workspace_default_matches);
    assert!(!comparison.allowed_models_exact);
    assert_eq!(comparison.allowed_model_count, 2);
}

#[test]
fn apply_is_put_then_get_and_succeeds_only_after_exact_read_back() {
    let pin = ModelPin::parse("qwen/pinned").unwrap();
    let server =
        policy_server(vec![(200, "{}".into()), (200, catalogue(pin.as_str(), &[pin.as_str()]))]);
    client(&server).apply(&pin).unwrap();

    let requests = server.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests.iter().map(|r| (r.method.as_str(), r.path.as_str())).collect::<Vec<_>>(),
        [("PUT", "/api/webchat/v2/llm/model-policy"), ("GET", "/api/webchat/v2/llm/models"),]
    );
    let body: serde_json::Value = serde_json::from_str(&requests[0].body).unwrap();
    assert_eq!(body["workspace_default"], pin.as_str());
    assert_eq!(body["allowed_models"], serde_json::json!([pin.as_str()]));
}

#[test]
fn a_successful_put_without_matching_read_back_is_not_success() {
    let pin = ModelPin::parse("qwen/pinned").unwrap();
    let widened = catalogue(pin.as_str(), &[pin.as_str(), "provider/other-model"]);
    let server = policy_server(vec![(200, "{}".into()), (200, widened)]);
    let error = client(&server).apply(&pin).unwrap_err();
    assert!(matches!(
        error,
        ModelPolicyError::ReadBackMismatch(comparison) if !comparison.is_exact()
    ));
    assert_eq!(server.requests.lock().unwrap().len(), 2);
}

/// The purpose guard in `OperatorToken::from_env` is the only thing under test, so the handle names
/// a variable every test process has. It used to name one nothing sets, and the lookup refused
/// before the guard mattered — this passed with the guard deleted, which is how it was found.
#[test]
fn a_runtime_member_handle_cannot_be_used_as_operator_configuration() {
    let pin = ModelPin::parse("qwen/pinned").unwrap();
    let member = CredentialRef::new(CredentialPurpose::RuntimeMember, "env:PATH");
    let control = ModelPolicyControl::new("http://127.0.0.1:1", &member);
    assert_eq!(control.check(&pin), Err(ModelPolicyError::NoCredential));

    // Positive control: the same handle under the operator purpose resolves, so the check gets
    // past the credential to the transport. Nothing listens on port 1, so the value is never sent.
    let operator = CredentialRef::new(CredentialPurpose::OperatorConfiguration, "env:PATH");
    let control = ModelPolicyControl::new("http://127.0.0.1:1", &operator);
    assert_eq!(control.check(&pin), Err(ModelPolicyError::Unreachable));
}

#[test]
fn malformed_remote_values_are_not_retained_in_errors() {
    let pin = ModelPin::parse("qwen/pinned").unwrap();
    let server = policy_server(vec![(200, "{\"models\":\"sensitive-sentinel\"}".into())]);
    assert_eq!(client(&server).check(&pin), Err(ModelPolicyError::MalformedResponse));
}
