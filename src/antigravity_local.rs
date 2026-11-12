//! Antigravity's quota panel, read from its own language server on loopback.
//!
//! Antigravity's quota panel lists model groups (for example Gemini Models,
//! Claude and GPT Models and Gemini 4 Argon), each with its own 5-hour and
//! weekly windows. The panel's data comes from the language server that
//! Antigravity bundles and starts. That server answers the read-only
//! `RetrieveUserQuotaSummary` method on 127.0.0.1 when the request carries the
//! per-launch CSRF token that Antigravity passes on the server's command line.
//!
//! Scope and safeguards:
//! - Only processes in the current Windows session whose image is
//!   `...\antigravity\resources\bin\language_server*.exe` are considered.
//! - The token is read from that process's command line for each poll. It is
//!   sent only in one request to a 127.0.0.1 port that the same process
//!   listens on, with proxies and redirects disabled. It is never logged,
//!   stored, cached or shown.
//! - One read-only quota request per poll; no other method is ever called.
//!
//! This is an internal, undocumented interface. When it is unavailable or
//! answers unexpectedly, callers fall back to Google's quota service.

use crate::diagnose;
use std::ffi::c_void;
use std::io::Read;
use std::sync::Mutex;
use std::time::Duration;

const METHOD: &str = "exa.language_server_pb.LanguageServerService/RetrieveUserQuotaSummary";
const MAX_RESPONSE_BYTES: u64 = 1 << 20;
const MAX_PORTS_PER_SERVER: usize = 8;
const MAX_TOKEN_LEN: usize = 256;

const TH32CS_SNAPPROCESS: u32 = 0x2;
const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
const PROCESS_COMMAND_LINE_INFORMATION: u32 = 60;
const AF_INET: u32 = 2;
const TCP_TABLE_OWNER_PID_LISTENER: u32 = 3;
const ERROR_INSUFFICIENT_BUFFER: u32 = 122;
const INVALID_HANDLE_VALUE: isize = -1;
/// 127.0.0.1 as `MIB_TCPROW_OWNER_PID::dwLocalAddr` stores it.
const LOOPBACK_V4: u32 = u32::from_ne_bytes([127, 0, 0, 1]);

#[repr(C)]
struct ProcessEntry32W {
    size: u32,
    usage: u32,
    process_id: u32,
    default_heap_id: usize,
    module_id: u32,
    threads: u32,
    parent_process_id: u32,
    priority_class_base: i32,
    flags: u32,
    exe_file: [u16; 260],
}

#[repr(C)]
struct UnicodeString {
    length: u16,
    maximum_length: u16,
    buffer: *const u16,
}

#[link(name = "kernel32")]
extern "system" {
    fn CreateToolhelp32Snapshot(flags: u32, process_id: u32) -> isize;
    fn Process32FirstW(snapshot: isize, entry: *mut ProcessEntry32W) -> i32;
    fn Process32NextW(snapshot: isize, entry: *mut ProcessEntry32W) -> i32;
    fn OpenProcess(access: u32, inherit: i32, process_id: u32) -> isize;
    fn QueryFullProcessImageNameW(
        process: isize,
        flags: u32,
        name: *mut u16,
        size: *mut u32,
    ) -> i32;
    fn ProcessIdToSessionId(process_id: u32, session_id: *mut u32) -> i32;
    fn GetCurrentProcessId() -> u32;
    fn CloseHandle(handle: isize) -> i32;
}

#[link(name = "ntdll")]
extern "system" {
    fn NtQueryInformationProcess(
        process: isize,
        class: u32,
        information: *mut c_void,
        length: u32,
        return_length: *mut u32,
    ) -> i32;
}

#[link(name = "iphlpapi")]
extern "system" {
    fn GetExtendedTcpTable(
        table: *mut c_void,
        size: *mut u32,
        order: i32,
        family: u32,
        class: u32,
        reserved: u32,
    ) -> u32;
}

/// Antigravity's per-launch CSRF token. Its `Debug` output is redacted so it
/// cannot reach a log by accident.
struct CsrfToken(String);

impl std::fmt::Debug for CsrfToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CsrfToken(<redacted>)")
    }
}

/// The (process, port) pair that answered last, tried first next time.
static LAST_PORT: Mutex<Option<(u32, u16)>> = Mutex::new(None);
static LAST_STATE: Mutex<Option<String>> = Mutex::new(None);

/// The raw `RetrieveUserQuotaSummary` response body from a running
/// Antigravity, or `None` when its language server is not available.
pub fn retrieve_user_quota_summary() -> Option<String> {
    let servers = antigravity_server_pids();
    if servers.is_empty() {
        log_state("Antigravity language server not found; using Google's quota service");
        return None;
    }
    // Loopback only: never through a proxy from the environment, never
    // following a redirect elsewhere.
    let agent = ureq::AgentBuilder::new()
        .try_proxy_from_env(false)
        .redirects(0)
        .timeout_connect(Duration::from_secs(1))
        .timeout(Duration::from_secs(4))
        .build();
    let cached = *LAST_PORT.lock().unwrap_or_else(|e| e.into_inner());

    for pid in &servers {
        // Read for this request only; dropped when it is done.
        let Some(token) = server_token(*pid) else {
            continue;
        };
        let mut ports = loopback_listen_ports(*pid);
        if let Some((_, port)) = cached.filter(|(cached_pid, _)| cached_pid == pid) {
            if let Some(index) = ports.iter().position(|p| *p == port) {
                ports[..=index].rotate_right(1);
            }
        }
        for port in ports.into_iter().take(MAX_PORTS_PER_SERVER) {
            let Ok(response) = agent
                .post(&format!("http://127.0.0.1:{port}/{METHOD}"))
                .set("X-Codeium-Csrf-Token", &token.0)
                .set("Connect-Protocol-Version", "1")
                .set("Content-Type", "application/json")
                .send_string("{}")
            else {
                continue;
            };
            if !response
                .content_type()
                .eq_ignore_ascii_case("application/json")
            {
                continue;
            }
            let mut body = String::new();
            if response
                .into_reader()
                .take(MAX_RESPONSE_BYTES)
                .read_to_string(&mut body)
                .is_err()
            {
                continue;
            }
            *LAST_PORT.lock().unwrap_or_else(|e| e.into_inner()) = Some((*pid, port));
            log_state(&format!(
                "Antigravity quota from its language server (loopback port {port})"
            ));
            return Some(body);
        }
    }
    log_state("Antigravity language server did not answer; using Google's quota service");
    None
}

/// Which Antigravity language servers are running (process IDs only), so a
/// paused provider retries when Antigravity starts or restarts.
/// No token is read for this.
pub fn presence_signature() -> String {
    let mut pids = antigravity_server_pids();
    pids.sort_unstable();
    format!("antigravity-language-server|{pids:?}")
}

fn log_state(message: &str) {
    let mut last = LAST_STATE.lock().unwrap_or_else(|e| e.into_inner());
    if last.as_deref() != Some(message) {
        diagnose::log(message);
        *last = Some(message.to_string());
    }
}

/// Process IDs of Antigravity's language servers in this Windows session.
fn antigravity_server_pids() -> Vec<u32> {
    let mut servers = Vec::new();
    let Some(session) = session_of(unsafe { GetCurrentProcessId() }) else {
        return servers;
    };
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snapshot == INVALID_HANDLE_VALUE || snapshot == 0 {
            return servers;
        }
        let mut entry: ProcessEntry32W = std::mem::zeroed();
        entry.size = std::mem::size_of::<ProcessEntry32W>() as u32;
        let mut more = Process32FirstW(snapshot, &mut entry) != 0;
        while more {
            let len = entry.exe_file.iter().position(|c| *c == 0).unwrap_or(260);
            let name = String::from_utf16_lossy(&entry.exe_file[..len]);
            if is_language_server_name(&name)
                && session_of(entry.process_id) == Some(session)
                && with_antigravity_server(entry.process_id, |_| Some(())).is_some()
            {
                servers.push(entry.process_id);
            }
            more = Process32NextW(snapshot, &mut entry) != 0;
        }
        CloseHandle(snapshot);
    }
    servers
}

fn session_of(pid: u32) -> Option<u32> {
    let mut session = 0u32;
    (unsafe { ProcessIdToSessionId(pid, &mut session) } != 0).then_some(session)
}

fn is_language_server_name(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    name.starts_with("language_server") && name.ends_with(".exe")
}

/// Whether an image path is Antigravity's bundled language server, and not,
/// for example, the same server bundled by another editor.
fn is_antigravity_server_path(path: &str) -> bool {
    let path = path.replace('/', "\\").to_ascii_lowercase();
    let Some((dir, file)) = path.rsplit_once('\\') else {
        return false;
    };
    is_language_server_name(file) && dir.ends_with("\\antigravity\\resources\\bin")
}

/// Runs `f` with a query handle to `pid`, only when the process's image is
/// Antigravity's bundled language server.
fn with_antigravity_server<T>(pid: u32, f: impl FnOnce(isize) -> Option<T>) -> Option<T> {
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if process == 0 {
            return None;
        }
        let mut path = [0u16; 1024];
        let mut size = path.len() as u32;
        let result = (QueryFullProcessImageNameW(process, 0, path.as_mut_ptr(), &mut size) != 0
            && is_antigravity_server_path(&String::from_utf16_lossy(&path[..size as usize])))
        .then(|| f(process))
        .flatten();
        CloseHandle(process);
        result
    }
}

/// The CSRF token of `pid`, only when its image is Antigravity's server.
fn server_token(pid: u32) -> Option<CsrfToken> {
    with_antigravity_server(pid, |process| unsafe {
        csrf_token_from_command_line(&command_line(process)?)
    })
}

/// The process's command line (`ProcessCommandLineInformation`, Windows 8.1+).
unsafe fn command_line(process: isize) -> Option<String> {
    let mut needed = 0u32;
    NtQueryInformationProcess(
        process,
        PROCESS_COMMAND_LINE_INFORMATION,
        std::ptr::null_mut(),
        0,
        &mut needed,
    );
    if needed < std::mem::size_of::<UnicodeString>() as u32 || needed > 1 << 20 {
        return None;
    }
    // u64 storage keeps the UNICODE_STRING header aligned.
    let mut buffer = vec![0u64; (needed as usize).div_ceil(8)];
    let status = NtQueryInformationProcess(
        process,
        PROCESS_COMMAND_LINE_INFORMATION,
        buffer.as_mut_ptr().cast(),
        (buffer.len() * 8) as u32,
        &mut needed,
    );
    if status < 0 {
        return None;
    }
    let header = &*(buffer.as_ptr() as *const UnicodeString);
    let start = buffer.as_ptr() as usize;
    let end = start + buffer.len() * 8;
    let text = header.buffer as usize;
    let bytes = header.length as usize;
    if header.buffer.is_null() || text < start || text + bytes > end {
        return None;
    }
    Some(String::from_utf16_lossy(std::slice::from_raw_parts(
        header.buffer,
        bytes / 2,
    )))
}

/// The `--csrf_token` value (`--csrf_token X` or `--csrf_token=X`), accepted
/// only as a short plain identifier. Other tokens on the command line are
/// never read.
fn csrf_token_from_command_line(command_line: &str) -> Option<CsrfToken> {
    let mut parts = command_line.split_whitespace();
    while let Some(part) = parts.next() {
        let value = if part == "--csrf_token" {
            parts.next()?
        } else if let Some(value) = part.strip_prefix("--csrf_token=") {
            value
        } else {
            continue;
        };
        let value = value.trim_matches('"');
        let plain = !value.is_empty()
            && value.len() <= MAX_TOKEN_LEN
            && value
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
        return plain.then(|| CsrfToken(value.to_string()));
    }
    None
}

/// IPv4 loopback ports that the process listens on.
fn loopback_listen_ports(pid: u32) -> Vec<u16> {
    for _ in 0..3 {
        let mut size = 0u32;
        unsafe {
            GetExtendedTcpTable(
                std::ptr::null_mut(),
                &mut size,
                0,
                AF_INET,
                TCP_TABLE_OWNER_PID_LISTENER,
                0,
            );
        }
        if size == 0 || size > 16 << 20 {
            return Vec::new();
        }
        // Room for a few rows added between the two calls.
        let mut table = vec![0u32; (size as usize).div_ceil(4) + 6 * 4];
        let mut size = (table.len() * 4) as u32;
        let result = unsafe {
            GetExtendedTcpTable(
                table.as_mut_ptr().cast(),
                &mut size,
                0,
                AF_INET,
                TCP_TABLE_OWNER_PID_LISTENER,
                0,
            )
        };
        match result {
            0 => return ports_from_tcp_table(&table, pid),
            ERROR_INSUFFICIENT_BUFFER => continue,
            _ => return Vec::new(),
        }
    }
    Vec::new()
}

/// `MIB_TCPTABLE_OWNER_PID`: a row count, then rows of state, local address,
/// local port (network byte order), remote address, remote port and owner PID.
fn ports_from_tcp_table(table: &[u32], pid: u32) -> Vec<u16> {
    let Some((&count, rows)) = table.split_first() else {
        return Vec::new();
    };
    let mut ports: Vec<u16> = rows
        .chunks_exact(6)
        .take(count as usize)
        .filter(|row| row[5] == pid && row[1] == LOOPBACK_V4)
        .map(|row| u16::from_be(row[2] as u16))
        .filter(|port| *port != 0)
        .collect();
    ports.sort_unstable();
    ports.dedup();
    ports
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_csrf_token_is_read_from_either_flag_form_and_must_be_plain() {
        let id = "0a1b2c3d-0000-4000-8000-123456789abc";
        let token = |line: &str| csrf_token_from_command_line(line).map(|t| t.0);
        assert_eq!(
            token(&format!(
                "language_server.exe --standalone --csrf_token {id} --app_data_dir x"
            ))
            .as_deref(),
            Some(id)
        );
        assert_eq!(
            token(&format!("x --csrf_token=\"{id}\"")).as_deref(),
            Some(id)
        );
        assert_eq!(token("x --csrf_token"), None);
        assert_eq!(token("x --csrf_token a;b"), None);
        assert_eq!(token("x --csrf_token http://example.com/"), None);
        assert_eq!(token(&format!("x --csrf_token {}", "a".repeat(257))), None);
        assert_eq!(token("x --csrf_tokens y"), None);
        assert_eq!(
            token("x --host_bridge_token=abc --extension_server_csrf_token=def"),
            None,
            "other tokens on the command line are never used"
        );
    }

    #[test]
    fn the_csrf_token_never_appears_in_debug_output() {
        let token = csrf_token_from_command_line("x --csrf_token secret-value").unwrap();
        let printed = format!("{token:?}");
        let printed = format!("{printed} {:?}", vec![(1u32, token)]);
        assert!(!printed.contains("secret-value"), "{printed}");
    }

    #[test]
    fn only_antigravitys_bundled_language_server_qualifies() {
        assert!(is_antigravity_server_path(
            r"C:\Users\u\AppData\Local\Programs\Antigravity\resources\bin\language_server.exe"
        ));
        assert!(is_antigravity_server_path(
            r"D:\Apps\antigravity\resources\bin\language_server_windows_x64.exe"
        ));
        assert!(is_antigravity_server_path(
            "C:/Apps/Antigravity/resources/bin/LANGUAGE_SERVER.EXE"
        ));
        for other in [
            r"C:\Users\u\AppData\Local\Programs\Windsurf\resources\bin\language_server.exe",
            r"C:\Temp\antigravity\resources\bin\other.exe",
            r"C:\Temp\antigravity\resources\language_server.exe",
            r"C:\Temp\not-antigravity\resources\bin\language_server.exe",
            r"C:\Temp\antigravity\resources\bin\language_server.exe.bak",
            "language_server.exe",
        ] {
            assert!(!is_antigravity_server_path(other), "{other}");
        }
    }

    #[test]
    fn listening_ports_are_the_processs_own_loopback_ones() {
        let port = |p: u16| u32::from(p.to_be());
        let any = u32::from_ne_bytes([0, 0, 0, 0]);
        let lan = u32::from_ne_bytes([192, 168, 1, 5]);
        #[rustfmt::skip]
        let table = [
            5,
            2, LOOPBACK_V4, port(55975), 0, 0, 42,
            2, LOOPBACK_V4, port(55974), 0, 0, 42,
            2, any, port(8080), 0, 0, 42,
            2, lan, port(8081), 0, 0, 42,
            2, LOOPBACK_V4, port(9000), 0, 0, 7,
            // Beyond the row count: ignored.
            2, LOOPBACK_V4, port(1234), 0, 0, 42,
        ];
        assert_eq!(ports_from_tcp_table(&table, 42), vec![55974, 55975]);
        assert_eq!(ports_from_tcp_table(&table, 7), vec![9000]);
        assert!(ports_from_tcp_table(&table, 99).is_empty());
        assert!(ports_from_tcp_table(&[], 42).is_empty());
        // A truncated row is never read.
        assert!(ports_from_tcp_table(&[1, 2, LOOPBACK_V4, port(1)], 42).is_empty());
    }

    #[test]
    fn the_current_process_has_a_session() {
        assert!(session_of(unsafe { GetCurrentProcessId() }).is_some());
    }
}
