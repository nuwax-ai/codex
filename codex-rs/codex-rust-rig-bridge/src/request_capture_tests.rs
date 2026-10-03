use super::*;
use pretty_assertions::assert_eq;
use serde_json::Value;
use serde_json::json;

fn recorder(file: Option<PathBuf>) -> Arc<Mutex<FinalRequestCapture>> {
    Arc::new(Mutex::new(FinalRequestCapture {
        file,
        ..Default::default()
    }))
}

fn request(url: &str, body: impl Into<bytes::Bytes>) -> Request<bytes::Bytes> {
    Request::builder()
        .method("POST")
        .uri(url)
        .header("authorization", "Bearer header-secret")
        .header("x-api-key", "api-header-secret")
        .body(body.into())
        .expect("request")
}

fn capture_snapshot(capture: &Arc<Mutex<FinalRequestCapture>>) -> (Value, usize) {
    let capture = capture.lock().expect("capture");
    (
        serde_json::to_value(&*capture).expect("capture JSON"),
        capture.bytes,
    )
}

fn lines(path: &std::path::Path) -> Vec<Value> {
    std::fs::read_to_string(path)
        .expect("capture file")
        .lines()
        .map(|line| serde_json::from_str(line).expect("capture line"))
        .collect()
}

#[test]
fn record_redacts_userinfo_query_and_headers_but_preserves_raw_json_bytes() {
    let temp = tempfile::tempdir().expect("temporary capture directory");
    let path = temp.path().join("requests.jsonl");
    let capture = recorder(Some(path.clone()));
    let raw = r#"{ "tools" : [ { "parameters" : { "z" : 18446744073709551617, "a" : 1e+02 } } ], "input" : "tool output" }"#;
    record(&Some(capture.clone()), &request(
        "https://url-user:url-password@example.com/v1/responses?api_key=query-secret&existing=a%26b&api_key=another-secret", raw,
    )).expect("record raw attempt");
    let expected = json!({
        "method":"POST", "url":"https://example.com/v1/responses?api_key=REDACTED&existing=REDACTED&api_key=REDACTED",
        "body_raw":raw, "body":serde_json::from_str::<Value>(raw).expect("convenience JSON"),
    });
    assert_eq!(capture_snapshot(&capture).0, json!({"requests":[expected]}));
    assert_eq!(lines(&path), vec![expected]);
    let bytes = std::fs::read(&path).expect("serialized capture bytes");
    for secret in [
        "url-user",
        "url-password",
        "query-secret",
        "another-secret",
        "header-secret",
        "api-header-secret",
    ] {
        assert!(
            !String::from_utf8_lossy(&bytes).contains(secret),
            "credential leaked: {secret}"
        );
    }
    assert_eq!(
        capture.lock().expect("capture").requests[0]
            .body_raw
            .as_bytes(),
        raw.as_bytes()
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(path)
                .expect("capture metadata")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
}

#[test]
fn record_keeps_every_attempt_in_memory_and_across_file_recorder_instances() {
    let temp = tempfile::tempdir().expect("directory");
    let path = temp.path().join("attempts.jsonl");
    let first = recorder(Some(path.clone()));
    let second = recorder(Some(path.clone()));
    let bodies = ["{\"attempt\":1}", "{ \"attempt\" : 2 }", "{\"attempt\":3}"];
    for body in &bodies[..2] {
        record(
            &Some(first.clone()),
            &request("https://example.com/responses", *body),
        )
        .expect("first recorder attempt");
    }
    record(
        &Some(second.clone()),
        &request("https://example.com/responses", bodies[2]),
    )
    .expect("second recorder attempt");
    let expected: Vec<_> = bodies
        .iter()
        .map(|raw| {
            json!({"method":"POST", "url":"https://example.com/responses",
        "body_raw":raw, "body":serde_json::from_str::<Value>(raw).expect("JSON body")})
        })
        .collect();
    assert_eq!(lines(&path), expected);
    assert_eq!(
        capture_snapshot(&first).0,
        json!({"requests":expected[..2]})
    );
    assert_eq!(
        capture_snapshot(&second).0,
        json!({"requests":expected[2..]})
    );
}

#[test]
fn poisoned_recorder_lock_fails_instead_of_dropping_an_attempt() {
    let capture = recorder(None);
    let poisoned = capture.clone();
    assert!(
        std::thread::spawn(move || {
            let _guard = poisoned.lock().expect("capture lock");
            panic!("poison only this recorder");
        })
        .join()
        .is_err()
    );
    let error = record(
        &Some(capture),
        &request("https://example.com/responses", "{}"),
    )
    .expect_err("poison must fail fast");
    assert!(
        error.to_string().contains("request capture lock poisoned"),
        "{error}"
    );
}

#[test]
fn body_byte_budget_accepts_exact_limit_and_rejects_one_more_byte_without_mutation() {
    let capture = recorder(None);
    let exact = format!("\"{}\"", "x".repeat(MAX_BODY_BYTES - 2));
    record(
        &Some(capture.clone()),
        &request("https://example.com/responses", exact.clone()),
    )
    .expect("exact body limit");
    let before = capture_snapshot(&capture);
    let too_large = format!("\"{}\"", "x".repeat(MAX_BODY_BYTES - 1));
    let error = record(
        &Some(capture.clone()),
        &request("https://example.com/responses", too_large),
    )
    .expect_err("body over budget");
    assert!(error.to_string().contains("body exceeds 2 MiB"), "{error}");
    assert_eq!(capture_snapshot(&capture), before);
    assert_eq!(capture.lock().expect("capture").requests[0].body_raw, exact);
}

#[test]
fn attempt_budget_keeps_first_64_attempts_and_rejects_the_65th_without_mutation() {
    let capture = recorder(None);
    let mut expected = Vec::new();
    for attempt in 0..MAX_ATTEMPTS {
        let raw = format!("{{\"attempt\":{attempt}}}");
        record(
            &Some(capture.clone()),
            &request("https://example.com/responses", raw.clone()),
        )
        .expect("bounded attempt");
        expected.push(
            json!({"method":"POST", "url":"https://example.com/responses",
            "body_raw":raw, "body":{"attempt":attempt}}),
        );
    }
    let before = capture_snapshot(&capture);
    let error = record(
        &Some(capture.clone()),
        &request("https://example.com/responses", "{\"attempt\":64}"),
    )
    .expect_err("65th attempt fails");
    assert!(
        error.to_string().contains("attempt or total byte limit"),
        "{error}"
    );
    assert_eq!(capture_snapshot(&capture), before);
    assert_eq!(before.0, json!({"requests":expected}));
}

#[test]
fn aggregate_byte_budget_rejects_complete_attempt_without_truncating_prior_bodies() {
    let capture = recorder(None);
    let raw = format!("\"{}\"", "x".repeat(1024 * 1024 - 2));
    let request = request("https://example.com/responses", raw.clone());
    record(&Some(capture.clone()), &request).expect("first large attempt");
    let encoded_len = capture.lock().expect("capture").bytes;
    let accepted = MAX_CAPTURE_BYTES / encoded_len;
    for _ in 1..accepted {
        record(&Some(capture.clone()), &request).expect("attempt inside aggregate budget");
    }
    let minimal = recorder(None);
    let empty = Request::builder()
        .method("POST")
        .uri("https://example.com/responses")
        .body(bytes::Bytes::from_static(b"\"\""))
        .expect("empty string request");
    record(&Some(minimal.clone()), &empty).expect("minimal encoded entry");
    let overhead = minimal.lock().expect("minimal capture").bytes;
    let remaining = MAX_CAPTURE_BYTES - capture.lock().expect("capture").bytes;
    // An ASCII string byte occurs once in body_raw and once in body; trailing
    // JSON whitespace occurs only in body_raw and fills either byte parity.
    let extra = remaining - overhead;
    let boundary_body = format!("\"{}\"{}", "x".repeat(extra / 2), " ".repeat(extra % 2));
    let boundary = Request::builder()
        .method("POST")
        .uri("https://example.com/responses")
        .body(bytes::Bytes::from(boundary_body.clone()))
        .expect("boundary request");
    record(&Some(capture.clone()), &boundary).expect("exact aggregate byte limit");
    let before = capture_snapshot(&capture);
    assert_eq!(before.1, MAX_CAPTURE_BYTES);
    let error = record(&Some(capture.clone()), &request).expect_err("aggregate over budget");
    assert!(
        error.to_string().contains("attempt or total byte limit"),
        "{error}"
    );
    assert_eq!(capture_snapshot(&capture), before);
    let captured = capture.lock().expect("capture");
    assert!(
        captured.requests[..accepted]
            .iter()
            .all(|entry| entry.body_raw == raw)
    );
    assert_eq!(
        captured.requests.last().expect("boundary entry").body_raw,
        boundary_body
    );
}

#[test]
fn file_attempt_budget_survives_recorder_reinitialization() {
    let temp = tempfile::tempdir().expect("directory");
    let path = temp.path().join("attempts.jsonl");
    for attempt in 0..MAX_ATTEMPTS {
        let capture = recorder(Some(path.clone()));
        record(
            &Some(capture),
            &request(
                "https://example.com/responses",
                format!("{{\"attempt\":{attempt}}}"),
            ),
        )
        .expect("file budget includes earlier recorder instances");
    }
    let before = std::fs::read(&path).expect("64 captured attempts");
    let capture = recorder(Some(path.clone()));
    let error = record(
        &Some(capture.clone()),
        &request("https://example.com/responses", "{}"),
    )
    .expect_err("file attempt budget exceeded");
    assert!(
        error
            .to_string()
            .contains("file exceeds attempt or total byte limit"),
        "{error}"
    );
    assert_eq!(std::fs::read(path).expect("unchanged file"), before);
    assert_eq!(capture_snapshot(&capture), (json!({"requests":[]}), 0));
}

#[test]
fn file_byte_budget_accepts_exact_limit_then_rejects_next_attempt() {
    use std::io::Read;
    use std::io::Seek;
    use std::io::SeekFrom;
    let temp = tempfile::tempdir().expect("directory");
    let path = temp.path().join("bytes.jsonl");
    let request = request("https://example.com/responses", "{}");
    let memory = recorder(None);
    record(&Some(memory.clone()), &request).expect("measure complete encoded attempt");
    let encoded =
        serde_json::to_vec(&memory.lock().expect("capture").requests[0]).expect("encoded entry");
    let prefix_len = MAX_CAPTURE_BYTES - encoded.len() - 1;
    std::fs::File::create(&path)
        .expect("file")
        .set_len(prefix_len as u64)
        .expect("near-budget file");
    let capture = recorder(Some(path.clone()));
    record(&Some(capture.clone()), &request).expect("exact file byte limit");
    assert_eq!(
        std::fs::metadata(&path).expect("metadata").len(),
        MAX_CAPTURE_BYTES as u64
    );
    let before = capture_snapshot(&capture);
    let error = record(&Some(capture.clone()), &request)
        .expect_err("next complete attempt exceeds file limit");
    assert!(
        error
            .to_string()
            .contains("file exceeds attempt or total byte limit"),
        "{error}"
    );
    assert_eq!(capture_snapshot(&capture), before);
    let mut file = std::fs::File::open(path).expect("capture file");
    file.seek(SeekFrom::Start(prefix_len as u64))
        .expect("seek appended attempt");
    let mut tail = Vec::new();
    file.read_to_end(&mut tail).expect("read appended bytes");
    assert_eq!(tail, [encoded, vec![b'\n']].concat());
}

#[test]
fn file_io_failure_and_oversized_existing_file_leave_recorder_unchanged() {
    let temp = tempfile::tempdir().expect("directory");
    let missing_parent = temp.path().join("missing-parent/requests.jsonl");
    let directory = temp.path().join("directory");
    std::fs::create_dir(&directory).expect("non-file path");
    let oversized = temp.path().join("oversized.jsonl");
    std::fs::File::create(&oversized)
        .expect("existing file")
        .set_len(MAX_CAPTURE_BYTES as u64 + 1)
        .expect("oversized file");
    for (path, expected) in [
        (missing_parent, "cannot open"),
        (directory, "must be a regular file"),
        (oversized, "file exceeds byte limit"),
    ] {
        let capture = recorder(Some(path));
        let error = record(
            &Some(capture.clone()),
            &request("https://example.com/responses", "{}"),
        )
        .expect_err("file failure must propagate");
        assert!(error.to_string().contains(expected), "{error}");
        assert_eq!(capture_snapshot(&capture), (json!({"requests":[]}), 0));
    }
}

#[cfg(unix)]
#[test]
fn existing_symbolic_capture_path_is_rejected_without_touching_its_target() {
    let temp = tempfile::tempdir().expect("directory");
    let target = temp.path().join("target.jsonl");
    std::fs::write(&target, "existing private evidence\n").expect("target");
    let link = temp.path().join("capture-link.jsonl");
    std::os::unix::fs::symlink(&target, &link).expect("capture symlink");
    let capture = recorder(Some(link));
    let error = record(
        &Some(capture.clone()),
        &request("https://example.com/responses", "{}"),
    )
    .expect_err("symbolic path rejected");
    assert!(
        error.to_string().contains("must be a regular file"),
        "{error}"
    );
    assert_eq!(
        std::fs::read(target).expect("target unchanged"),
        b"existing private evidence\n"
    );
    assert_eq!(capture_snapshot(&capture), (json!({"requests":[]}), 0));
}

#[cfg(unix)]
#[test]
fn appending_to_existing_public_file_restricts_permissions_before_sensitive_capture() {
    use std::os::unix::fs::PermissionsExt;
    let temp = tempfile::tempdir().expect("private test directory");
    let path = temp.path().join("existing.jsonl");
    let prior = json!({"method":"POST","url":"https://example.com/responses",
        "body_raw":"{\"prior\":1}","body":{"prior":1}});
    std::fs::write(
        &path,
        format!("{}\n", serde_json::to_string(&prior).expect("prior entry")),
    )
    .expect("existing capture file");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644))
        .expect("public existing mode");
    assert_eq!(
        std::fs::metadata(&path)
            .expect("prior mode")
            .permissions()
            .mode()
            & 0o777,
        0o644
    );
    let capture = recorder(Some(path.clone()));
    let raw = r#"{"input":"sensitive prompt","output":"private tool output"}"#;
    record(
        &Some(capture.clone()),
        &request("https://example.com/responses", raw),
    )
    .expect("append private capture");
    let expected = json!({"method":"POST","url":"https://example.com/responses",
        "body_raw":raw,"body":{"input":"sensitive prompt","output":"private tool output"}});
    assert_eq!(
        std::fs::metadata(&path)
            .expect("restricted mode")
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    assert_eq!(lines(&path), vec![prior, expected.clone()]);
    assert_eq!(capture_snapshot(&capture).0, json!({"requests":[expected]}));
}

#[test]
fn capture_limit_and_file_errors_remain_non_retryable_through_http_and_completion_wrappers() {
    let temp = tempfile::tempdir().expect("test directory");
    let capture = recorder(None);
    let oversized = format!("\"{}\"", "x".repeat(MAX_BODY_BYTES));
    let cap_error = record(
        &Some(capture.clone()),
        &request("https://example.com/responses", oversized),
    )
    .expect_err("capture body limit");
    let file_capture = recorder(Some(temp.path().join("missing-parent/requests.jsonl")));
    let file_error = record(
        &Some(file_capture.clone()),
        &request("https://example.com/responses", "{}"),
    )
    .expect_err("capture file failure");
    for (error, expected_message) in [
        (cap_error, "request capture body exceeds 2 MiB limit"),
        (file_error, "cannot open request capture file"),
    ] {
        assert_eq!(
            error.to_string(),
            format!("Http client error: {expected_message}")
        );
        assert!(
            matches!(api_error(&error), Some(codex_api::ApiError::InvalidRequest { message })
            if message == expected_message),
            "HTTP capture error must fail fast: {error}"
        );
        let completion =
            rig_core::completion::request::CompletionError::RequestError(Box::new(error));
        assert!(
            matches!(api_error(&completion), Some(codex_api::ApiError::InvalidRequest { message })
            if message == expected_message),
            "wrapped capture error must fail fast: {completion}"
        );
    }
    assert_eq!(capture_snapshot(&capture), (json!({"requests":[]}), 0));
    assert_eq!(capture_snapshot(&file_capture), (json!({"requests":[]}), 0));
    let ordinary_network_failure = std::io::Error::other("unrelated transport failure");
    assert!(
        api_error(&ordinary_network_failure).is_none(),
        "ordinary errors cannot be relabeled as capture failures"
    );
}

#[test]
fn huge_raw_number_preserves_complete_request_bytes_when_convenience_view_cannot_parse_it() {
    let temp = tempfile::tempdir().expect("capture directory");
    let path = temp.path().join("huge-number.jsonl");
    let capture = recorder(Some(path.clone()));
    let raw = r#"{ "tools" : [{ "parameters" : { "limit" : 1e999, "count" : 18446744073709551617 } }], "input" : "retained" }"#;
    // RawValue accepts the exact literal independently of Value's numeric
    // representation; an audit convenience view cannot block such a request.
    let raw_value =
        serde_json::value::RawValue::from_string(raw.to_string()).expect("valid raw JSON");
    assert_eq!(raw_value.get().as_bytes(), raw.as_bytes());
    record(
        &Some(capture.clone()),
        &request("https://example.com/responses", raw),
    )
    .expect("capture accepts raw number outside f64 range");
    let captured = capture.lock().expect("capture");
    assert_eq!(captured.requests.len(), 1);
    assert_eq!(captured.requests[0].body_raw.as_bytes(), raw.as_bytes());
    // The full workspace can unify serde_json/arbitrary_precision through
    // exec-server-protocol; the scoped bridge build normally has no such flag.
    match serde_json::from_str::<Value>(raw) {
        Err(_) => assert_eq!(captured.requests[0].body, Value::Null),
        Ok(exact_view) => assert_eq!(captured.requests[0].body, exact_view),
    }
    let encoded = serde_json::to_vec(&captured.requests[0]).expect("encoded capture");
    assert_eq!(
        captured.bytes,
        encoded.len() + 1,
        "budget counts the complete encoded record and newline"
    );
    drop(captured);
    assert_eq!(
        std::fs::read(&path).expect("file bytes"),
        [encoded, vec![b'\n']].concat()
    );
    let records = lines(&path);
    assert_eq!(
        records[0]["body_raw"]
            .as_str()
            .expect("raw file field")
            .as_bytes(),
        raw.as_bytes()
    );
}
