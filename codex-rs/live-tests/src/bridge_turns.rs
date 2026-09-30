//! Bridge-level (L1) turn execution: bridge/wire selection, cassette
//! replay dispatch and the per-bridge run helpers.

use super::*;

// ================================================================
// Bridge-level (L1) streaming
// ================================================================

/// Drains a codex `ResponseStream`, printing and persisting every event
/// under `<repo>/logs/live-mimo/bridge/<tag>-<nonce>.log`.
pub async fn drain_stream(stream: ResponseStream, vendor: &str, tag: &str) -> Vec<ResponseEvent> {
    let mut stream = stream;
    let mut events = Vec::new();
    let request_id = format!(
        "[{tag}] upstream_request_id={:?}",
        stream.upstream_request_id
    );
    println!("{request_id}");
    let mut log_lines = vec![request_id];
    loop {
        let event = timeout(TURN_TIMEOUT, stream.rx_event.recv())
            .await
            .expect("next event within timeout");
        match event {
            Some(Ok(event)) => {
                let line = format!("[{tag}] {event:?}");
                println!("{line}");
                log_lines.push(line);
                events.push(event);
            }
            Some(Err(err)) => {
                let line = format!("[{tag}] stream error: {err:#}");
                println!("{line}");
                log_lines.push(line);
                panic!("stream error from bridge: {err:#}");
            }
            None => break,
        }
    }
    persist_lines(vendor, "bridge", tag, &log_lines);
    events
}

/// Which bridge a suite exercises.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Bridge {
    Genai,
    Rig,
}

/// The wire a scenario drives: explicit, mirroring `wire_api` in provider
/// config. `Chat` keeps the URL heuristic as fallback (MiMo-style
/// `/anthropic` gateways); `Anthropic` is the explicit protocol for
/// gateways like StepFun whose URL carries no marker; `Responses` is the
/// same-protocol passthrough (explicit endpoint required).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum LiveWire {
    Chat,
    Anthropic,
    Responses,
}

impl Bridge {
    pub fn name(self) -> &'static str {
        match self {
            Self::Genai => "genai",
            Self::Rig => "rig",
        }
    }
}

/// Whether the shelved genai bridge participates in this run. Rig is the
/// fork default for every wire; genai live/replay tests are opt-in via
/// `LIVE_INCLUDE_GENAI=1` (set on the command line like `LIVE_CASSETTE`),
/// e.g. to re-validate genai before resurrecting it or for A/B comparison
/// runs. Default runs exercise rig only, halving vendor-quota usage.
pub fn genai_bridge_enabled() -> bool {
    std::env::var("LIVE_INCLUDE_GENAI").as_deref() == Ok("1")
}

fn replay_rig_turn(cfg: &LiveConfig, tag: &str) -> Vec<ResponseEvent> {
    let fixture = load_rig_event_fixture(&cfg.vendor, tag)
        .unwrap_or_else(|error| panic!("[cassette-rig] {error}"));
    println!(
        "[cassette-rig] replaying {}/{} through current bridge conversion ({} rig events, offline)",
        cfg.vendor,
        tag,
        fixture.rig_events.len()
    );
    codex_rust_rig_bridge::replay_fixture_events(&fixture)
        .unwrap_or_else(|error| panic!("[cassette-rig] {}/{}: {error}", cfg.vendor, tag))
}

/// One bridge-level turn through the selected bridge (the vendor's chat URL;
/// Anthropic gateways are passed by the anthropic scenarios).
pub async fn run_turn(
    cfg: &LiveConfig,
    base_url: &str,
    wire: LiveWire,
    bridge: Bridge,
    request: &ResponsesApiRequest,
    tag: &str,
) -> Vec<ResponseEvent> {
    if cassette_mode() == CassetteMode::Replay {
        // Responses replay sends the current request through the real bridge
        // to a loopback server serving the recorded raw SSE bytes.
        if bridge == Bridge::Rig && wire == LiveWire::Responses {
            return run_responses_turn_rig(cfg, base_url, request, tag).await;
        }
        // Rig replay must execute current conversion. Never fall back to the
        // already-converted ResponseEvent cassette when parsing fails.
        if bridge == Bridge::Rig {
            return replay_rig_turn(cfg, tag);
        }
        let fixture = load_fixture(&cfg.vendor, bridge.name(), tag).unwrap_or_else(|| {
            // Fail fast: silently falling back to live would consume vendor
            // quota in what the operator explicitly declared an offline run,
            // and a green suite could then be evidence of a live call rather
            // than of the recorded fixture.
            panic!(
                "[cassette] replay: no fixture for {}/{}-{} \
                 (record with LIVE_CASSETTE=record)",
                cfg.vendor,
                bridge.name(),
                tag
            );
        });
        println!(
            "[cassette] replaying {}/{}-{} ({} events, offline)",
            cfg.vendor,
            bridge.name(),
            tag,
            fixture.events.len()
        );
        return fixture.events;
    }
    match bridge {
        Bridge::Genai => {
            let adapter_kind = match wire {
                LiveWire::Anthropic => genai::adapter::AdapterKind::Anthropic,
                // Genai speaks Chat/Anthropic only; the Responses wire is rig
                // passthrough (or native). Responses scenarios never register
                // a genai variant.
                LiveWire::Responses => panic!("genai does not speak the Responses wire"),
                LiveWire::Chat
                    if codex_rust_rig_bridge::RigProtocol::from_base_url(base_url)
                        == codex_rust_rig_bridge::RigProtocol::Anthropic =>
                {
                    genai::adapter::AdapterKind::Anthropic
                }
                LiveWire::Chat => genai::adapter::AdapterKind::OpenAI,
            };
            run_turn_genai(cfg, base_url, request, adapter_kind, tag).await
        }
        Bridge::Rig => {
            let protocol = match wire {
                LiveWire::Anthropic => codex_rust_rig_bridge::RigProtocol::Anthropic,
                LiveWire::Responses => codex_rust_rig_bridge::RigProtocol::Responses,
                LiveWire::Chat => codex_rust_rig_bridge::RigProtocol::from_base_url(base_url),
            };
            if protocol == codex_rust_rig_bridge::RigProtocol::Responses {
                run_responses_turn_rig(cfg, base_url, request, tag).await
            } else {
                run_turn_rig(cfg, base_url, protocol, request, tag).await
            }
        }
    }
}

/// One bridge-level turn on the Responses wire (same-protocol passthrough).
/// Cassette replay exercises request projection and HTTP dispatch against a
/// loopback server serving the recorded raw SSE bytes, then the live decoder.
pub async fn run_responses_turn_rig(
    cfg: &LiveConfig,
    base_url: &str,
    request: &ResponsesApiRequest,
    tag: &str,
) -> Vec<ResponseEvent> {
    if cassette_mode() == CassetteMode::Replay {
        return responses_replay::replay_turn(cfg, request, tag).await;
    }
    let provider = vendor_provider(&cfg.vendor, base_url);
    let recorder: codex_rust_rig_bridge::RigSseRecorder = if cassette_mode() == CassetteMode::Record
    {
        Some(Arc::new(std::sync::Mutex::new(Vec::new())))
    } else {
        None
    };
    let stream = timeout(
        TURN_TIMEOUT,
        codex_rust_rig_bridge::stream_responses_via_rig_with_sse_recording(
            request,
            &provider,
            &shared_auth(&cfg.api_key),
            HeaderMap::new(),
            provider.stream_idle_timeout,
            recorder.clone(),
        ),
    )
    .await
    .expect("responses stream_via_rig started within timeout")
    .expect("responses stream_via_rig succeeded");
    let events = drain_stream(stream, &cfg.vendor, tag).await;
    record_turn(cfg, Bridge::Rig, tag, request, &events).expect("record Rig responses turn");
    if let Some(recorder) = recorder {
        let bytes = recorder.lock().expect("SSE recorder lock").clone();
        let sse = String::from_utf8(bytes)
            .unwrap_or_else(|error| panic!("[cassette-responses] non-UTF8 SSE body: {error}"));
        save_responses_sse_fixture(&cfg.vendor, tag, &sse).expect("record responses SSE");
    }
    events
}

/// One bridge-level turn through the genai bridge.
async fn run_turn_genai(
    cfg: &LiveConfig,
    base_url: &str,
    request: &ResponsesApiRequest,
    adapter_kind: genai::adapter::AdapterKind,
    tag: &str,
) -> Vec<ResponseEvent> {
    let provider = vendor_provider(&cfg.vendor, base_url);
    let stream: ResponseStream = timeout(
        TURN_TIMEOUT,
        codex_rust_genai_bridge::stream_via_genai(
            request,
            &provider,
            &shared_auth(&cfg.api_key),
            HeaderMap::new(),
            adapter_kind,
            provider.stream_idle_timeout,
        ),
    )
    .await
    .expect("stream_via_genai started within timeout")
    .expect("stream_via_genai succeeded");
    let events = drain_stream(stream, &cfg.vendor, tag).await;
    record_turn(cfg, Bridge::Genai, tag, request, &events).expect("record GenAI turn");
    events
}

/// One bridge-level turn through the rig bridge (protocol picked from the
/// provider base URL inside the bridge).
pub async fn run_turn_rig(
    cfg: &LiveConfig,
    base_url: &str,
    protocol: codex_rust_rig_bridge::RigProtocol,
    request: &ResponsesApiRequest,
    tag: &str,
) -> Vec<ResponseEvent> {
    // Replay recorded Rig events through the current conversion. Missing or
    // malformed fixtures must never fall through to a real provider request.
    if cassette_mode() == CassetteMode::Replay {
        return replay_rig_turn(cfg, tag);
    }

    let provider = vendor_provider(&cfg.vendor, base_url);
    let recorder: codex_rust_rig_bridge::RigEventRecorder =
        if cassette_mode() == CassetteMode::Record {
            Some(Arc::new(std::sync::Mutex::new(Vec::new())))
        } else {
            None
        };
    let (stream, recorder): (ResponseStream, codex_rust_rig_bridge::RigEventRecorder) = timeout(
        TURN_TIMEOUT,
        codex_rust_rig_bridge::stream_via_rig_with_recording(
            request,
            &provider,
            &shared_auth(&cfg.api_key),
            HeaderMap::new(),
            protocol,
            provider.stream_idle_timeout,
            recorder,
        ),
    )
    .await
    .expect("stream_via_rig started within timeout")
    .expect("stream_via_rig succeeded");
    let events = drain_stream(stream, &cfg.vendor, tag).await;
    record_turn(cfg, Bridge::Rig, tag, request, &events).expect("record Rig turn");
    // Save the rig-event fixture alongside the event-level one.
    if let Some(rec) = recorder {
        let rig_events = rec.lock().expect("Rig recorder lock");
        let custom_tools = codex_rust_rig_bridge::extract_custom_tool_names(request);
        save_rig_event_fixture(&cfg.vendor, tag, &rig_events, &custom_tools)
            .expect("record Rig events");
    }
    events
}
