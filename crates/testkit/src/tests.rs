use super::*;

#[test]
fn a_stub_reports_the_request_it_was_given_and_answers_the_canned_response() {
    let (base, thread) = serve("200 OK", r#"{"ok":true}"#);
    let body = std::thread::spawn(move || {
        let mut stream = std::net::TcpStream::connect(base.trim_start_matches("http://")).unwrap();
        stream.write_all(b"GET /thing HTTP/1.1\r\nhost: fixture\r\n\r\n").unwrap();
        let mut answer = String::new();
        stream.read_to_string(&mut answer).unwrap();
        answer
    })
    .join()
    .unwrap();

    assert!(body.contains(r#"{"ok":true}"#), "the canned body must come back: {body}");
    let request = thread.join().expect("fixture thread").expect("a client arrived");
    assert!(request.starts_with("GET /thing "), "{request}");
    assert!(request.contains("host: fixture"), "the head must be reported: {request}");
}

#[test]
fn a_request_body_reaches_the_recorded_request() {
    let (base, thread) = serve("200 OK", "{}");
    std::thread::spawn(move || {
        let mut s = std::net::TcpStream::connect(base.trim_start_matches("http://")).unwrap();
        s.write_all(b"POST /x HTTP/1.1\r\ncontent-length: 5\r\n\r\nhello").unwrap();
        let mut answer = String::new();
        let _ = s.read_to_string(&mut answer);
    })
    .join()
    .unwrap();
    let request = thread.join().unwrap().expect("a client arrived");
    assert!(request.ends_with("hello"), "the body must be read, not just the head: {request}");
}

/// **`None`, not an empty string.** A stub nobody connected to measured nothing, and it must not
/// read as a stub that measured agreement — an assertion checking a prefix would pass on `""`.
#[test]
fn a_stub_nobody_connects_to_reports_that_nobody_came() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    assert!(accept_within(&listener, Duration::from_millis(50)).is_none());
}

#[test]
fn a_scratch_directory_is_unique_and_removed_on_drop() {
    let (a, b) = (Scratch::new("kit"), Scratch::new("kit"));
    assert_ne!(a.path(), b.path(), "two scratches in one process must not collide");
    let path = a.path().to_owned();
    assert!(path.is_dir());
    drop(a);
    assert!(!path.exists(), "the directory must go when the guard does");
}

/// The property the `let _ = remove_dir_all(..)`-at-the-end form did not have.
#[test]
fn a_scratch_directory_is_removed_even_when_the_test_panics() {
    let path = {
        let dir = Scratch::new("kit-panic");
        let path = dir.path().to_owned();
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _held = dir;
            panic!("as a failing test would");
        }));
        path
    };
    assert!(!path.exists(), "a panicking test must not leak its directory");
}
