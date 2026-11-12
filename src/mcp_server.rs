//! Optional, loopback-only MCP server that reports the monitor's cached quota.
//!
//! Tool calls only read the snapshot the widget already holds. They never
//! start a provider request, read credentials or call a model, so asking an
//! agent "how much quota is left?" costs no quota.

use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, SocketAddrV4, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use crate::diagnose;
use crate::models::{UsageData, UsageSection};
use crate::poller::{remaining_percentage, PollError};
use crate::provider_poll::Service;
use crate::quota_refresh::History;

pub const DEFAULT_PORT: u16 = 46827;
const MCP_PATH: &str = "/mcp";
const MAX_HEADER_BYTES: usize = 16 * 1024;
const MAX_BODY_BYTES: usize = 64 * 1024;
const READ_TIMEOUT: Duration = Duration::from_secs(2);
/// A whole request must arrive within this time, so a slow client cannot
/// hold the single listener thread (or delay shutdown) indefinitely.
const REQUEST_DEADLINE: Duration = Duration::from_secs(3);
const PROTOCOL_VERSIONS: [&str; 3] = ["2025-11-25", "2025-06-18", "2025-03-26"];

/// What the server may know about one provider: a copy of the widget's state.
#[derive(Clone, Debug, Default)]
pub struct ProviderRecord {
    pub enabled: bool,
    pub data: Option<UsageData>,
    pub last_success: Option<SystemTime>,
    pub expected_interval_ms: u32,
    pub error: Option<PollError>,
}

impl ProviderRecord {
    pub fn from_service(service: &Service) -> Self {
        Self {
            enabled: service.enabled(),
            data: service.data.clone(),
            last_success: service.history.last_success,
            expected_interval_ms: service.history.expected_interval_ms,
            error: service.error,
        }
    }
}

/// Claude Code and Codex, in tool order. `None` means the app state is gone.
pub type Snapshot = Option<[ProviderRecord; 2]>;
type Source = Arc<dyn Fn() -> Snapshot + Send + Sync>;

#[derive(Debug)]
pub enum StartError {
    PortUnavailable(u16),
    Failed(String),
}

impl std::fmt::Display for StartError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PortUnavailable(port) => write!(f, "127.0.0.1:{port} is in use or reserved"),
            Self::Failed(message) => f.write_str(message),
        }
    }
}

struct ServerHandle {
    port: u16,
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

impl ServerHandle {
    fn shutdown(mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

static SERVER: Mutex<Option<ServerHandle>> = Mutex::new(None);

/// Start (or keep) the listener on `127.0.0.1:port`. `source` must only copy
/// already-held state; it runs on the listener thread for each tool call.
pub fn start(
    port: u16,
    source: impl Fn() -> Snapshot + Send + Sync + 'static,
) -> Result<(), StartError> {
    let mut server = SERVER.lock().unwrap_or_else(|e| e.into_inner());
    if server.as_ref().is_some_and(|handle| handle.port == port) {
        return Ok(());
    }
    if let Some(previous) = server.take() {
        previous.shutdown();
    }
    let listener = TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port)).map_err(
        |error| match error.kind() {
            io::ErrorKind::AddrInUse | io::ErrorKind::PermissionDenied => {
                StartError::PortUnavailable(port)
            }
            _ => StartError::Failed(format!("could not bind 127.0.0.1:{port}: {error}")),
        },
    )?;
    *server = Some(spawn_listener(listener, port, Arc::new(source))?);
    diagnose::log(format!(
        "MCP server listening at http://127.0.0.1:{port}{MCP_PATH}"
    ));
    Ok(())
}

/// Stop the listener and wait for its thread. Safe to call when not running.
pub fn stop() {
    let handle = SERVER.lock().unwrap_or_else(|e| e.into_inner()).take();
    if let Some(handle) = handle {
        handle.shutdown();
        diagnose::log("MCP server stopped");
    }
}

fn spawn_listener(
    listener: TcpListener,
    port: u16,
    source: Source,
) -> Result<ServerHandle, StartError> {
    listener
        .set_nonblocking(true)
        .map_err(|error| StartError::Failed(format!("could not configure listener: {error}")))?;
    let stop = Arc::new(AtomicBool::new(false));
    let thread_stop = Arc::clone(&stop);
    let join = thread::Builder::new()
        .name("mcp-server".into())
        .spawn(move || {
            while !thread_stop.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((stream, _)) => handle_connection(stream, port, &*source),
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(50));
                    }
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                    Err(error) => {
                        diagnose::log_error("MCP listener stopped after accept error", error);
                        break;
                    }
                }
            }
        })
        .map_err(|error| StartError::Failed(format!("could not start MCP thread: {error}")))?;
    Ok(ServerHandle {
        port,
        stop,
        join: Some(join),
    })
}

struct HttpRequest {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl HttpRequest {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }
}

struct HttpResponse {
    status: u16,
    reason: &'static str,
    content_type: &'static str,
    body: Vec<u8>,
}

impl HttpResponse {
    fn text(status: u16, reason: &'static str, body: &str) -> Self {
        Self {
            status,
            reason,
            content_type: "text/plain; charset=utf-8",
            body: body.as_bytes().to_vec(),
        }
    }
}

fn handle_connection(mut stream: TcpStream, port: u16, source: &dyn Fn() -> Snapshot) {
    // Accepted sockets inherit the listener's non-blocking mode on Windows.
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_write_timeout(Some(READ_TIMEOUT));
    let response = match read_http_request(&mut stream, Instant::now() + REQUEST_DEADLINE) {
        Ok(request) => respond(&request, port, source, SystemTime::now()),
        Err(response) => response,
    };
    let header = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\n\r\n",
        response.status,
        response.reason,
        response.content_type,
        response.body.len()
    );
    let _ = stream.write_all(header.as_bytes());
    let _ = stream.write_all(&response.body);
    let _ = stream.flush();
}

fn read_http_request(
    stream: &mut TcpStream,
    deadline: Instant,
) -> Result<HttpRequest, HttpResponse> {
    let bad = || HttpResponse::text(400, "Bad Request", "bad request");
    let mut bytes = Vec::with_capacity(4096);
    let header_end = loop {
        if let Some(index) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
            break index + 4;
        }
        if bytes.len() >= MAX_HEADER_BYTES {
            return Err(HttpResponse::text(
                431,
                "Request Header Fields Too Large",
                "headers too large",
            ));
        }
        read_socket(stream, deadline, &mut bytes).map_err(|()| bad())?;
    };
    if header_end > MAX_HEADER_BYTES {
        return Err(HttpResponse::text(
            431,
            "Request Header Fields Too Large",
            "headers too large",
        ));
    }
    let (method, path, headers, content_length) =
        parse_head(&bytes[..header_end]).ok_or_else(bad)?;
    if content_length > MAX_BODY_BYTES {
        return Err(HttpResponse::text(
            413,
            "Payload Too Large",
            "body too large",
        ));
    }
    let target = header_end + content_length;
    while bytes.len() < target {
        read_socket(stream, deadline, &mut bytes).map_err(|()| bad())?;
    }
    Ok(HttpRequest {
        method,
        path,
        headers,
        body: bytes[header_end..target].to_vec(),
    })
}

fn read_socket(stream: &mut TcpStream, deadline: Instant, bytes: &mut Vec<u8>) -> Result<(), ()> {
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        return Err(());
    }
    stream
        .set_read_timeout(Some(remaining.min(READ_TIMEOUT)))
        .map_err(|_| ())?;
    let mut buffer = [0u8; 4096];
    match stream.read(&mut buffer) {
        Ok(0) | Err(_) => Err(()),
        Ok(count) => {
            bytes.extend_from_slice(&buffer[..count]);
            Ok(())
        }
    }
}

type Head = (String, String, Vec<(String, String)>, usize);

/// Parse the request line and headers. Chunked bodies are not accepted.
fn parse_head(head: &[u8]) -> Option<Head> {
    let text = std::str::from_utf8(head).ok()?;
    let mut lines = text.split("\r\n");
    let mut request_line = lines.next()?.split(' ');
    let method = request_line.next()?.to_string();
    let path = request_line.next()?.to_string();
    if !request_line.next()?.starts_with("HTTP/1.") || request_line.next().is_some() {
        return None;
    }
    let mut headers = Vec::new();
    let mut content_length = None;
    for line in lines.take_while(|line| !line.is_empty()) {
        let (name, value) = line.split_once(':')?;
        let name = name.trim().to_ascii_lowercase();
        let value = value.trim().to_string();
        match name.as_str() {
            "transfer-encoding" => return None,
            "content-length" => {
                let length = value.parse::<usize>().ok()?;
                if content_length.is_some_and(|previous| previous != length) {
                    return None;
                }
                content_length = Some(length);
            }
            _ => {}
        }
        headers.push((name, value));
    }
    Some((method, path, headers, content_length.unwrap_or(0)))
}

fn respond(
    request: &HttpRequest,
    port: u16,
    source: &dyn Fn() -> Snapshot,
    now: SystemTime,
) -> HttpResponse {
    // Host and Origin checks defeat DNS rebinding and cross-site browser posts.
    if !request
        .header("host")
        .is_some_and(|host| loopback_authority(host, port))
    {
        return HttpResponse::text(403, "Forbidden", "invalid host");
    }
    if let Some(origin) = request.header("origin") {
        let authority = origin.strip_prefix("http://").unwrap_or("");
        if !loopback_authority(authority, port) {
            return HttpResponse::text(403, "Forbidden", "invalid origin");
        }
    }
    if request.path != MCP_PATH {
        return HttpResponse::text(404, "Not Found", "not found");
    }
    if request.method != "POST" {
        // No server-to-client stream is offered (GET) and no sessions exist (DELETE).
        return HttpResponse::text(405, "Method Not Allowed", "POST required");
    }
    if !request
        .header("content-type")
        .is_some_and(|value| value.to_ascii_lowercase().starts_with("application/json"))
    {
        return HttpResponse::text(415, "Unsupported Media Type", "application/json required");
    }
    if !request.header("accept").is_some_and(|value| {
        let value = value.to_ascii_lowercase();
        value.contains("application/json") || value.contains("*/*")
    }) {
        return HttpResponse::text(406, "Not Acceptable", "application/json must be accepted");
    }
    if request
        .header("mcp-protocol-version")
        .is_some_and(|version| !PROTOCOL_VERSIONS.contains(&version))
    {
        return HttpResponse::text(400, "Bad Request", "unsupported MCP-Protocol-Version");
    }
    let Ok(message) = serde_json::from_slice::<Value>(&request.body) else {
        return json_response(&rpc_error(Value::Null, -32700, "Parse error"));
    };
    match handle_rpc(&message, source, now) {
        None => HttpResponse {
            status: 202,
            reason: "Accepted",
            content_type: "application/json",
            body: Vec::new(),
        },
        Some(response) => json_response(&response),
    }
}

fn loopback_authority(authority: &str, port: u16) -> bool {
    ["127.0.0.1", "localhost"]
        .iter()
        .any(|host| authority.eq_ignore_ascii_case(&format!("{host}:{port}")))
}

fn json_response(value: &Value) -> HttpResponse {
    HttpResponse {
        status: 200,
        reason: "OK",
        content_type: "application/json",
        body: serde_json::to_vec(value).unwrap_or_default(),
    }
}

fn rpc_error(id: Value, code: i32, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

const TOOLS: [(&str, &str, &str, &str); 2] = [
    (
        "get_claude_usage",
        "claude_code",
        "Get Claude Code usage",
        "Read the Claude Code quota the monitor last fetched: 5-hour and weekly used/remaining percent, reset times, data age and status. Reads the monitor's cache only; never makes a request or uses quota.",
    ),
    (
        "get_codex_usage",
        "codex",
        "Get Codex usage",
        "Read the Codex quota the monitor last fetched: 5-hour and weekly used/remaining percent, reset times, data age and status. Reads the monitor's cache only; never makes a request or uses quota.",
    ),
];

/// Answer one JSON-RPC message. `None` means no response body (a notification).
/// `source` is consulted only by `tools/call`.
fn handle_rpc(message: &Value, source: &dyn Fn() -> Snapshot, now: SystemTime) -> Option<Value> {
    let Some(object) = message.as_object() else {
        // Batches are not part of current MCP revisions.
        return Some(rpc_error(Value::Null, -32600, "Invalid Request"));
    };
    let id = object.get("id")?.clone();
    if object.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return Some(rpc_error(id, -32600, "Invalid Request"));
    }
    let params = object.get("params").unwrap_or(&Value::Null);
    let result = match object.get("method").and_then(Value::as_str).unwrap_or("") {
        "initialize" => {
            let requested = params.get("protocolVersion").and_then(Value::as_str);
            let protocol = requested
                .filter(|version| PROTOCOL_VERSIONS.contains(version))
                .unwrap_or(PROTOCOL_VERSIONS[0]);
            json!({
                "protocolVersion": protocol,
                "capabilities": {"tools": {"listChanged": false}},
                "serverInfo": {"name": "codex-usage-monitor", "version": env!("CARGO_PKG_VERSION")}
            })
        }
        "ping" => json!({}),
        "tools/list" => json!({"tools": TOOLS.iter().map(|(name, _, title, description)| json!({
            "name": name,
            "title": title,
            "description": description,
            "inputSchema": {"type": "object", "properties": {}, "additionalProperties": false},
            "annotations": {
                "title": title,
                "readOnlyHint": true,
                "destructiveHint": false,
                "idempotentHint": true,
                "openWorldHint": false
            }
        })).collect::<Vec<_>>()}),
        "tools/call" => {
            let index = params
                .get("name")
                .and_then(Value::as_str)
                .and_then(|name| TOOLS.iter().position(|tool| tool.0 == name));
            let no_arguments = params
                .get("arguments")
                .is_none_or(|arguments| arguments.as_object().is_some_and(|map| map.is_empty()));
            let Some(index) = index.filter(|_| no_arguments) else {
                return Some(rpc_error(
                    id,
                    -32602,
                    "Unknown tool or unsupported arguments",
                ));
            };
            let record = source().map(|records| records[index].clone());
            let data = usage_result(TOOLS[index].1, record.as_ref(), now);
            json!({
                "content": [{"type": "text", "text": data.to_string()}],
                "structuredContent": data,
                "isError": false
            })
        }
        _ => return Some(rpc_error(id, -32601, "Method not found")),
    };
    Some(json!({"jsonrpc": "2.0", "id": id, "result": result}))
}

fn unix_seconds(time: SystemTime) -> Option<u64> {
    time.duration_since(UNIX_EPOCH).ok().map(|d| d.as_secs())
}

/// Status is `ok`, `stale` (an error since the last success, or older than the
/// provider's own refresh interval plus grace), `unavailable` (no successful
/// reading yet) or `disabled` (the provider is not monitored).
fn usage_result(provider: &str, record: Option<&ProviderRecord>, now: SystemTime) -> Value {
    let record = record.cloned().unwrap_or_default();
    let error = record.error.filter(|_| record.enabled);
    let authentication_required = matches!(
        error,
        Some(PollError::AuthRequired | PollError::TokenExpired | PollError::NoCredentials)
    );
    let mut result = json!({
        "provider": provider,
        "status": "unavailable",
        "five_hour": null,
        "weekly": null,
        "expired_windows": [],
        "lowest_remaining_percent": null,
        "limiting_window": null,
        "updated_at": null,
        "age_seconds": null,
        "refresh_interval_seconds": null,
        "data_source": null,
        "error_category": error.map(PollError::category),
        "authentication_required": authentication_required
    });
    if !record.enabled {
        result["status"] = json!("disabled");
        return result;
    }
    let (Some(data), Some(updated_at)) = (record.data.as_ref(), record.last_success) else {
        return result;
    };
    let history = History {
        last_success: Some(updated_at),
        expected_interval_ms: record.expected_interval_ms,
    };
    let stale = error.is_some() || history.is_stale(now);
    let mut expired = Vec::new();
    let mut lowest: Option<(f64, &str)> = None;
    for (name, section) in [("five_hour", &data.session), ("weekly", &data.weekly)] {
        // Once a reset has passed, the stored value describes the old window.
        if section.resets_at.is_some_and(|reset| reset <= now) {
            expired.push(name);
            continue;
        }
        let Some(window) = window_json(section) else {
            continue;
        };
        let remaining = remaining_percentage(section.percentage);
        if lowest.is_none_or(|(current, _)| remaining < current) {
            lowest = Some((remaining, name));
        }
        result[name] = window;
    }
    result["status"] = json!(if stale { "stale" } else { "ok" });
    result["expired_windows"] = json!(expired);
    result["lowest_remaining_percent"] = json!(lowest.map(|(value, _)| value));
    result["limiting_window"] = json!(lowest.map(|(_, name)| name));
    result["updated_at"] = json!(unix_seconds(updated_at));
    result["age_seconds"] = json!(now.duration_since(updated_at).unwrap_or_default().as_secs());
    result["refresh_interval_seconds"] = json!(record.expected_interval_ms.div_ceil(1000));
    result["data_source"] = json!("last_successful_poll");
    result
}

fn window_json(section: &UsageSection) -> Option<Value> {
    if !section.percentage.is_finite() {
        return None;
    }
    let used = section.percentage.clamp(0.0, 100.0);
    Some(json!({
        "used_percent": used,
        "remaining_percent": remaining_percentage(used),
        "resets_at": section.resets_at.and_then(unix_seconds)
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    const NOW: u64 = 1_800_000_000;

    fn at(seconds: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(seconds)
    }

    fn usage(session: f64, weekly: f64) -> UsageData {
        UsageData {
            session: UsageSection {
                percentage: session,
                resets_at: Some(at(NOW + 3_600)),
            },
            weekly: UsageSection {
                percentage: weekly,
                resets_at: Some(at(NOW + 86_400)),
            },
        }
    }

    fn fresh(data: UsageData) -> ProviderRecord {
        ProviderRecord {
            enabled: true,
            data: Some(data),
            last_success: Some(at(NOW - 60)),
            expected_interval_ms: 300_000,
            error: None,
        }
    }

    fn call(tool: &str, records: Snapshot) -> Value {
        let request = json!({"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":tool}});
        let response = handle_rpc(&request, &|| records.clone(), at(NOW)).unwrap();
        assert_eq!(response["result"]["isError"], false);
        let structured = response["result"]["structuredContent"].clone();
        let text = response["result"]["content"][0]["text"].as_str().unwrap();
        assert_eq!(serde_json::from_str::<Value>(text).unwrap(), structured);
        structured
    }

    fn rpc(message: Value) -> Option<Value> {
        handle_rpc(
            &message,
            &|| panic!("only tools/call may read the snapshot"),
            at(NOW),
        )
    }

    #[test]
    fn lists_exactly_two_read_only_tools_without_reading_the_snapshot() {
        let response = rpc(json!({"jsonrpc":"2.0","id":1,"method":"tools/list"})).unwrap();
        let tools = response["result"]["tools"].as_array().unwrap();
        let names: Vec<_> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(names, ["get_claude_usage", "get_codex_usage"]);
        for tool in tools {
            assert_eq!(tool["annotations"]["readOnlyHint"], true);
            assert_eq!(tool["annotations"]["destructiveHint"], false);
            assert_eq!(tool["annotations"]["openWorldHint"], false);
            assert_eq!(tool["inputSchema"]["additionalProperties"], false);
            assert_eq!(tool["inputSchema"]["properties"], json!({}));
        }
    }

    #[test]
    fn negotiates_supported_protocol_versions() {
        for (requested, expected) in [
            ("2025-06-18", "2025-06-18"),
            ("2025-03-26", "2025-03-26"),
            ("2099-01-01", PROTOCOL_VERSIONS[0]),
        ] {
            let response = rpc(json!({"jsonrpc":"2.0","id":1,"method":"initialize",
                "params":{"protocolVersion":requested,"capabilities":{},
                "clientInfo":{"name":"test","version":"1"}}}))
            .unwrap();
            assert_eq!(response["result"]["protocolVersion"], expected);
            assert_eq!(
                response["result"]["capabilities"]["tools"]["listChanged"],
                false
            );
        }
        let ping = rpc(json!({"jsonrpc":"2.0","id":2,"method":"ping"})).unwrap();
        assert_eq!(ping["result"], json!({}));
    }

    #[test]
    fn rejects_unknown_tools_arguments_and_malformed_messages() {
        let error = |message: Value| rpc(message).unwrap()["error"]["code"].clone();
        for params in [
            json!({"name":"read_file"}),
            json!({"name":"get_codex_usage","arguments":{"path":"C:\\"}}),
            json!({"name":"get_codex_usage","arguments":[]}),
        ] {
            let message = json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":params});
            assert_eq!(error(message), -32602);
        }
        let unknown = json!({"jsonrpc":"2.0","id":1,"method":"resources/list"});
        assert_eq!(error(unknown), -32601);
        assert_eq!(error(json!({"id":1,"method":"ping"})), -32600);
        let batch = json!([{"jsonrpc":"2.0","id":1,"method":"ping"}]);
        assert_eq!(error(batch), -32600);
        // Notifications get no response body.
        let note = json!({"jsonrpc":"2.0","method":"notifications/initialized"});
        assert!(rpc(note).is_none());
    }

    #[test]
    fn tool_calls_copy_the_snapshot_once_and_never_more() {
        let calls = Cell::new(0);
        let source = || {
            calls.set(calls.get() + 1);
            Some([fresh(usage(10.0, 20.0)), fresh(usage(30.0, 40.0))])
        };
        let request = json!({"jsonrpc":"2.0","id":1,"method":"tools/call",
            "params":{"name":"get_codex_usage","arguments":{}}});
        handle_rpc(&request, &source, at(NOW)).unwrap();
        assert_eq!(calls.get(), 1);
    }

    #[test]
    fn reports_fresh_values_with_full_precision() {
        let records = Some([fresh(usage(55.0, 5.0)), fresh(usage(12.345, 87.5))]);
        let codex = call("get_codex_usage", records.clone());
        assert_eq!(codex["provider"], "codex");
        assert_eq!(codex["status"], "ok");
        assert_eq!(codex["five_hour"]["used_percent"], 12.345);
        assert_eq!(codex["five_hour"]["remaining_percent"], 100.0 - 12.345);
        assert_eq!(codex["five_hour"]["resets_at"], NOW + 3_600);
        assert_eq!(codex["weekly"]["remaining_percent"], 12.5);
        assert_eq!(codex["lowest_remaining_percent"], 12.5);
        assert_eq!(codex["limiting_window"], "weekly");
        assert_eq!(codex["updated_at"], NOW - 60);
        assert_eq!(codex["age_seconds"], 60);
        assert_eq!(codex["refresh_interval_seconds"], 300);
        assert_eq!(codex["data_source"], "last_successful_poll");
        assert_eq!(codex["error_category"], Value::Null);
        assert_eq!(codex["authentication_required"], false);
        assert_eq!(codex["expired_windows"], json!([]));

        let claude = call("get_claude_usage", records);
        assert_eq!(claude["provider"], "claude_code");
        assert_eq!(claude["five_hour"]["used_percent"], 55.0);
        assert_eq!(claude["limiting_window"], "five_hour");
    }

    #[test]
    fn distinguishes_missing_disabled_and_absent_state() {
        let never = ProviderRecord {
            enabled: true,
            ..ProviderRecord::default()
        };
        let disabled = ProviderRecord {
            enabled: false,
            error: Some(PollError::ServerError),
            ..fresh(usage(10.0, 10.0))
        };
        let claude = call("get_claude_usage", Some([never, disabled.clone()]));
        assert_eq!(claude["status"], "unavailable");
        assert_eq!(claude["five_hour"], Value::Null);
        assert_eq!(claude["updated_at"], Value::Null);
        let codex = call(
            "get_codex_usage",
            Some([ProviderRecord::default(), disabled]),
        );
        assert_eq!(codex["status"], "disabled");
        assert_eq!(codex["five_hour"], Value::Null);
        assert_eq!(codex["error_category"], Value::Null);
        assert_eq!(call("get_codex_usage", None)["status"], "disabled");
        // Same keys in every state, so clients can rely on the shape.
        let keys = |value: &Value| {
            let mut keys: Vec<_> = value.as_object().unwrap().keys().cloned().collect();
            keys.sort();
            keys
        };
        assert_eq!(keys(&claude), keys(&codex));
        let ok = call(
            "get_codex_usage",
            Some([fresh(usage(1.0, 1.0)), fresh(usage(1.0, 1.0))]),
        );
        assert_eq!(keys(&ok), keys(&codex));
    }

    #[test]
    fn a_failing_provider_is_stale_and_never_hides_the_other() {
        let failed = ProviderRecord {
            error: Some(PollError::TokenExpired),
            ..fresh(usage(40.0, 50.0))
        };
        let records = Some([failed, fresh(usage(10.0, 20.0))]);
        let claude = call("get_claude_usage", records.clone());
        assert_eq!(claude["status"], "stale");
        assert_eq!(claude["five_hour"]["used_percent"], 40.0);
        assert_eq!(claude["error_category"], "token_expired");
        assert_eq!(claude["authentication_required"], true);
        let codex = call("get_codex_usage", records);
        assert_eq!(codex["status"], "ok");
        assert_eq!(codex["error_category"], Value::Null);

        let never_succeeded = ProviderRecord {
            enabled: true,
            error: Some(PollError::RateLimited(Some(60_000))),
            ..ProviderRecord::default()
        };
        let records = Some([never_succeeded, fresh(usage(1.0, 1.0))]);
        let claude = call("get_claude_usage", records);
        assert_eq!(claude["status"], "unavailable");
        assert_eq!(claude["error_category"], "rate_limited");
        assert_eq!(claude["authentication_required"], false);
    }

    #[test]
    fn old_readings_become_stale_on_the_providers_own_interval() {
        // Upstream rule: stale after the interval plus max(interval / 10, 30 s).
        let mut record = fresh(usage(10.0, 10.0));
        record.last_success = Some(at(NOW - 330));
        let codex = call(
            "get_codex_usage",
            Some([ProviderRecord::default(), record.clone()]),
        );
        assert_eq!(codex["status"], "ok");
        record.last_success = Some(at(NOW - 331));
        let codex = call("get_codex_usage", Some([ProviderRecord::default(), record]));
        assert_eq!(codex["status"], "stale");
        assert_eq!(codex["age_seconds"], 331);
    }

    #[test]
    fn a_window_whose_reset_passed_reports_no_old_value() {
        let mut data = usage(90.0, 30.0);
        data.session.resets_at = Some(at(NOW - 1));
        let codex = call(
            "get_codex_usage",
            Some([ProviderRecord::default(), fresh(data)]),
        );
        assert_eq!(codex["five_hour"], Value::Null);
        assert_eq!(codex["expired_windows"], json!(["five_hour"]));
        assert_eq!(codex["weekly"]["used_percent"], 30.0);
        assert_eq!(codex["lowest_remaining_percent"], 70.0);
        assert_eq!(codex["limiting_window"], "weekly");
    }

    #[test]
    fn windows_without_a_reset_time_are_reported_as_given() {
        let mut data = usage(0.0, 25.0);
        data.session.resets_at = None;
        let codex = call(
            "get_codex_usage",
            Some([ProviderRecord::default(), fresh(data)]),
        );
        assert_eq!(codex["five_hour"]["used_percent"], 0.0);
        assert_eq!(codex["five_hour"]["resets_at"], Value::Null);
        let codex = call(
            "get_codex_usage",
            Some([ProviderRecord::default(), fresh(usage(f64::NAN, 25.0))]),
        );
        assert_eq!(codex["five_hour"], Value::Null);
        assert_eq!(codex["limiting_window"], "weekly");
    }

    fn request(method: &str, path: &str, headers: &[(&str, &str)], body: &str) -> HttpRequest {
        HttpRequest {
            method: method.into(),
            path: path.into(),
            headers: headers
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            body: body.as_bytes().to_vec(),
        }
    }

    const PING: &str = r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#;
    const GOOD: [(&str, &str); 3] = [
        ("host", "127.0.0.1:46827"),
        ("content-type", "application/json"),
        ("accept", "application/json, text/event-stream"),
    ];

    fn send(method: &str, path: &str, headers: &[(&str, &str)], body: &str) -> HttpResponse {
        respond(
            &request(method, path, headers, body),
            46827,
            &|| None,
            at(NOW),
        )
    }

    fn status(method: &str, path: &str, headers: &[(&str, &str)], body: &str) -> u16 {
        send(method, path, headers, body).status
    }

    /// The good headers with one replaced (or removed, when `value` is empty).
    fn with<'a>(name: &'a str, value: &'a str) -> Vec<(&'a str, &'a str)> {
        let mut headers: Vec<_> = GOOD.iter().copied().filter(|h| h.0 != name).collect();
        if !value.is_empty() {
            headers.push((name, value));
        }
        headers
    }

    fn plus<'a>(name: &'a str, value: &'a str) -> Vec<(&'a str, &'a str)> {
        let mut headers = GOOD.to_vec();
        headers.push((name, value));
        headers
    }

    #[test]
    fn http_layer_accepts_only_loopback_json_posts_to_the_endpoint() {
        assert_eq!(status("POST", "/mcp", &GOOD, PING), 200);
        let localhost = with("host", "localhost:46827");
        assert_eq!(status("POST", "/mcp", &localhost, PING), 200);
        for host in [
            "",
            "evil.example:46827",
            "127.0.0.1:1234",
            "127.0.0.1",
            "0.0.0.0:46827",
        ] {
            let headers = with("host", host);
            assert_eq!(status("POST", "/mcp", &headers, PING), 403, "{host}");
        }
        let local_origin = plus("origin", "http://localhost:46827");
        assert_eq!(status("POST", "/mcp", &local_origin, PING), 200);
        for origin in ["https://evil.example", "null", "http://127.0.0.1:1"] {
            let headers = plus("origin", origin);
            assert_eq!(status("POST", "/mcp", &headers, PING), 403, "{origin}");
        }
        assert_eq!(status("GET", "/mcp", &GOOD, ""), 405);
        assert_eq!(status("DELETE", "/mcp", &GOOD, ""), 405);
        assert_eq!(status("POST", "/", &GOOD, PING), 404);
        let text = with("content-type", "text/plain");
        assert_eq!(status("POST", "/mcp", &text, PING), 415);
        let html = with("accept", "text/html");
        assert_eq!(status("POST", "/mcp", &html, PING), 406);
        let version = plus("mcp-protocol-version", "1999-01-01");
        assert_eq!(status("POST", "/mcp", &version, PING), 400);
        let version = plus("mcp-protocol-version", "2025-06-18");
        assert_eq!(status("POST", "/mcp", &version, PING), 200);
        let parse: Value = serde_json::from_slice(&send("POST", "/mcp", &GOOD, "{").body).unwrap();
        assert_eq!(parse["error"]["code"], -32700);
        let note = r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#;
        assert_eq!(status("POST", "/mcp", &GOOD, note), 202);
    }

    #[test]
    fn request_heads_reject_chunked_and_ambiguous_framing() {
        let head = |text: &str| parse_head(text.as_bytes());
        let ok = head("POST /mcp HTTP/1.1\r\nHost: a\r\nContent-Length: 5\r\n\r\n").unwrap();
        assert_eq!((ok.0.as_str(), ok.1.as_str(), ok.3), ("POST", "/mcp", 5));
        for bad in [
            "POST /mcp HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n",
            "POST /mcp HTTP/1.1\r\nContent-Length: 1\r\nContent-Length: 2\r\n\r\n",
            "POST /mcp HTTP/1.1\r\nContent-Length: -1\r\n\r\n",
            "POST /mcp\r\n\r\n",
            "POST /mcp SPDY/3\r\n\r\n",
            "POST /mcp HTTP/1.1\r\nbroken header\r\n\r\n",
        ] {
            assert!(head(bad).is_none(), "{bad:?}");
        }
    }

    fn free_port() -> u16 {
        TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
    }

    fn exchange(port: u16, raw: &[u8]) -> String {
        let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        // The server may answer and close before an oversized request is sent.
        let _ = stream.write_all(raw);
        let mut response = Vec::new();
        let _ = stream.read_to_end(&mut response);
        String::from_utf8_lossy(&response).into_owned()
    }

    fn post(port: u16, body: &str) -> String {
        let request = format!(
            "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\nAccept: application/json, text/event-stream\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        );
        exchange(port, request.as_bytes())
    }

    /// Records come from upstream's real per-provider monitor state.
    #[test]
    fn records_follow_the_provider_monitor() {
        use crate::provider_poll::{execute, Monitor};
        let clock = Instant::now();
        let wall = at(NOW - 10);
        let mut monitor = Monitor::new([true, true, false], clock);
        let records =
            |m: &Monitor| Some([0, 1].map(|id| ProviderRecord::from_service(&m.services[id])));
        assert_eq!(
            call("get_codex_usage", records(&monitor))["status"],
            "unavailable"
        );

        let jobs = monitor.plan(clock);
        let credentials = |_| vec!["credential-v1".to_string()];
        let claude = execute(&jobs[0], || Err(PollError::TokenExpired), credentials);
        monitor.finish(&jobs[0], claude, clock, wall, 300_000, false);
        let codex = execute(&jobs[1], || Ok(usage(25.0, 50.0)), credentials);
        monitor.finish(&jobs[1], codex, clock, wall, 300_000, false);

        let claude = call("get_claude_usage", records(&monitor));
        assert_eq!(claude["status"], "unavailable");
        assert_eq!(claude["authentication_required"], true);
        let codex = call("get_codex_usage", records(&monitor));
        assert_eq!(codex["status"], "ok");
        assert_eq!(codex["updated_at"], NOW - 10);
        assert_eq!(codex["five_hour"]["used_percent"], 25.0);
        assert_eq!(codex["refresh_interval_seconds"], 300);

        // Turning a provider off hides its cached values at once.
        monitor.configure([true, false, false], clock);
        assert_eq!(
            call("get_codex_usage", records(&monitor))["status"],
            "disabled"
        );
    }

    /// One test drives the real listener, since the server slot is process-wide.
    #[test]
    fn live_listener_serves_limits_and_stops_cleanly() {
        let port = free_port();
        start(port, || {
            Some([fresh(usage(10.0, 20.0)), fresh(usage(30.0, 40.0))])
        })
        .unwrap();
        start(port, || None).expect("starting again on the same port is a no-op");

        let list = post(port, r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#);
        assert!(list.starts_with("HTTP/1.1 200 OK"), "{list}");
        assert!(list.contains("get_codex_usage") && list.contains("get_claude_usage"));
        let codex = post(
            port,
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"get_codex_usage"}}"#,
        );
        assert!(codex.contains(r#""used_percent":30.0"#), "{codex}");

        // Oversized requests are refused from their headers alone. (Bytes the
        // server never reads could reset the connection before the reply.)
        let big = format!(
            "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Length: {}\r\n\r\n",
            MAX_BODY_BYTES + 1
        );
        assert!(exchange(port, big.as_bytes()).starts_with("HTTP/1.1 413"));
        let mut headers = b"POST /mcp HTTP/1.1\r\nX: ".to_vec();
        headers.resize(MAX_HEADER_BYTES, b'y');
        assert!(exchange(port, &headers).starts_with("HTTP/1.1 431"));

        // A client that stalls mid-request is cut off and does not block others.
        let started = Instant::now();
        let mut slow = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
        slow.write_all(b"POST /mcp HTTP/1.1\r\n").unwrap();
        assert!(post(port, PING).starts_with("HTTP/1.1 200"));
        assert!(started.elapsed() < REQUEST_DEADLINE + Duration::from_secs(2));
        drop(slow);

        // Moving to a port that is taken reports the conflict and leaves
        // nothing listening on the old one.
        let other = free_port();
        let blocker = TcpListener::bind((Ipv4Addr::LOCALHOST, other)).unwrap();
        assert!(matches!(start(other, || None), Err(StartError::PortUnavailable(p)) if p == other));
        drop(blocker);
        assert!(TcpStream::connect((Ipv4Addr::LOCALHOST, port)).is_err());

        start(port, || None).unwrap();
        stop();
        stop();
        TcpListener::bind((Ipv4Addr::LOCALHOST, port)).expect("port released after stop");
    }

    /// Manual end-to-end check with a real MCP client against synthetic data:
    /// set `MCP_TEST_PORT` (default 46900) and optionally `MCP_TEST_SECONDS`,
    /// then run `cargo test serve_synthetic_snapshot -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn serve_synthetic_snapshot_for_client_testing() {
        let env = |name: &str, default: u64| {
            std::env::var(name)
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(default)
        };
        let port = env("MCP_TEST_PORT", 46900) as u16;
        let seconds = env("MCP_TEST_SECONDS", 120);
        let started = SystemTime::now();
        start(port, move || {
            let record = |session: f64, weekly: f64, error: Option<PollError>| {
                let window = |used: f64, hours: u64| UsageSection {
                    percentage: used,
                    resets_at: Some(started + Duration::from_secs(hours * 3_600)),
                };
                ProviderRecord {
                    enabled: true,
                    data: Some(UsageData {
                        session: window(session, 3),
                        weekly: window(weekly, 100),
                    }),
                    last_success: Some(started),
                    expected_interval_ms: 300_000,
                    error,
                }
            };
            Some([
                record(42.5, 17.25, Some(PollError::RateLimited(None))),
                record(12.5, 61.0, None),
            ])
        })
        .unwrap();
        println!("serving synthetic MCP data at http://127.0.0.1:{port}/mcp for {seconds} s");
        thread::sleep(Duration::from_secs(seconds));
        stop();
    }
}
