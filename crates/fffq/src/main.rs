use clap::{Parser, Subcommand};
use git2::Repository;
use serde_json::json;
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime};

#[cfg(unix)]
use std::os::unix::process::CommandExt;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[derive(Parser)]
#[command(
    name = "fffq",
    version,
    about = "Fast CLI client for a local fff HTTP sidecar"
)]
struct Args {
    /// Project directory. Defaults to the current directory's git root.
    #[arg(short = 'C', long = "cwd", global = true)]
    cwd: Option<PathBuf>,

    /// Path to fff-mcp. Defaults to FFF_MCP_BIN, sibling binary, then PATH.
    #[arg(long = "fff-mcp-bin", env = "FFF_MCP_BIN", global = true)]
    fff_mcp_bin: Option<PathBuf>,

    /// Do not auto-start a missing sidecar.
    #[arg(long = "no-start", global = true)]
    no_start: bool,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Ensure the project sidecar is running and print its registry JSON.
    Ensure,
    /// Print sidecar diagnostics.
    Doctor,
    /// Fuzzy file search by path/name.
    Find {
        query: String,
        #[arg(short = 'n', long = "max-results", default_value_t = 20)]
        max_results: usize,
        #[arg(long)]
        cursor: Option<String>,
    },
    /// Search file contents.
    Grep {
        query: String,
        #[arg(short = 'n', long = "max-results", default_value_t = 20)]
        max_results: usize,
        #[arg(long, default_value = "content")]
        output_mode: String,
        #[arg(long)]
        cursor: Option<String>,
    },
    /// Search file contents for any of several literal patterns.
    MultiGrep {
        patterns: Vec<String>,
        #[arg(long)]
        constraints: Option<String>,
        #[arg(short = 'n', long = "max-results", default_value_t = 20)]
        max_results: usize,
        #[arg(long, default_value = "content")]
        output_mode: String,
        #[arg(long)]
        cursor: Option<String>,
        #[arg(long)]
        context: Option<usize>,
    },
    /// Send a JSON batch request to /fffq/batch. Reads JSON from stdin.
    Batch,
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
struct Registry {
    root: String,
    pid: u32,
    http_url: String,
    mcp_url: String,
    fffq_url: String,
    started_at_ms: u128,
}

#[derive(Debug)]
struct HttpResponse {
    status: u16,
    headers: Vec<(String, String)>,
    body: String,
}

#[derive(Debug)]
struct HttpUrl {
    host: String,
    port: u16,
    path: String,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("fffq: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let args = Args::parse();
    let root = resolve_root(args.cwd.as_deref())?;
    let registry_path = registry_path(&root);

    match args.command {
        Commands::Ensure => {
            let registry = ensure_sidecar(
                &root,
                &registry_path,
                args.fff_mcp_bin.as_deref(),
                args.no_start,
            )?;
            println!("{}", serde_json::to_string_pretty(&registry)?);
        }
        Commands::Doctor => {
            let registry = ensure_sidecar(
                &root,
                &registry_path,
                args.fff_mcp_bin.as_deref(),
                args.no_start,
            )?;
            let health = http_get_json(&format!("{}/health", registry.fffq_url))?;
            println!("root: {}", registry.root);
            println!("pid: {}", registry.pid);
            println!("fffq: {}", registry.fffq_url);
            println!("mcp: {}", registry.mcp_url);
            println!("health: {}", serde_json::to_string_pretty(&health)?);
        }
        Commands::Find {
            query,
            max_results,
            cursor,
        } => {
            let payload = json!({ "query": query, "maxResults": max_results, "cursor": cursor });
            println!(
                "{}",
                call_text(
                    &root,
                    &registry_path,
                    args.fff_mcp_bin.as_deref(),
                    args.no_start,
                    "find",
                    "find_files",
                    payload,
                )?
            );
        }
        Commands::Grep {
            query,
            max_results,
            output_mode,
            cursor,
        } => {
            let payload = json!({
                "query": query,
                "maxResults": max_results,
                "output_mode": output_mode,
                "cursor": cursor
            });
            println!(
                "{}",
                call_text(
                    &root,
                    &registry_path,
                    args.fff_mcp_bin.as_deref(),
                    args.no_start,
                    "grep",
                    "grep",
                    payload,
                )?
            );
        }
        Commands::MultiGrep {
            patterns,
            constraints,
            max_results,
            output_mode,
            cursor,
            context,
        } => {
            if patterns.is_empty() {
                return Err("multi-grep requires at least one pattern".into());
            }
            let payload = json!({
                "patterns": patterns,
                "constraints": constraints,
                "maxResults": max_results,
                "output_mode": output_mode,
                "cursor": cursor,
                "context": context
            });
            println!(
                "{}",
                call_text(
                    &root,
                    &registry_path,
                    args.fff_mcp_bin.as_deref(),
                    args.no_start,
                    "multi-grep",
                    "multi_grep",
                    payload,
                )?
            );
        }
        Commands::Batch => {
            let mut input = String::new();
            std::io::stdin().read_to_string(&mut input)?;
            print!(
                "{}",
                call_batch(
                    &root,
                    &registry_path,
                    args.fff_mcp_bin.as_deref(),
                    args.no_start,
                    &input,
                )?
            );
        }
    }

    Ok(())
}

fn resolve_root(cwd: Option<&Path>) -> Result<String> {
    let cwd = match cwd {
        Some(path) => path.to_path_buf(),
        None => std::env::current_dir()?,
    };
    match Repository::discover(&cwd) {
        Ok(repo) => repo
            .workdir()
            .map(|path| path.to_string_lossy().to_string())
            .ok_or_else(|| "bare git repositories are not supported by fffq".into()),
        Err(_) => Ok(cwd.canonicalize()?.to_string_lossy().to_string()),
    }
}

fn registry_path(root: &str) -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    let hash = blake3::hash(root.as_bytes()).to_hex()[..16].to_string();
    PathBuf::from(home)
        .join(".cache")
        .join("fff")
        .join("sidecars")
        .join(format!("{hash}.json"))
}

fn ensure_sidecar(
    root: &str,
    registry_path: &Path,
    fff_mcp_bin: Option<&Path>,
    no_start: bool,
) -> Result<Registry> {
    if let Some(registry) = read_live_registry(root, registry_path) {
        return Ok(registry);
    }
    if no_start {
        return Err(format!(
            "no live sidecar for {root}; registry {} is missing or stale",
            registry_path.display()
        )
        .into());
    }

    let bin = resolve_fff_mcp_bin(fff_mcp_bin)?;
    if let Some(parent) = registry_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let _ = std::fs::remove_file(registry_path);

    let mut command = Command::new(&bin);
    command
        .current_dir(root)
        .arg("--transport")
        .arg("streamable-http")
        .arg("--http-bind")
        .arg("127.0.0.1:0")
        .arg("--http-path")
        .arg("/mcp")
        .arg("--registry-path")
        .arg(registry_path)
        .arg("--no-warmup")
        .arg("--no-update-check")
        .arg("--max-cached-files")
        .arg("20000")
        .arg("--no-watch")
        .arg("--poll-interval-secs")
        .arg("300")
        .arg(root)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(unix)]
    command.process_group(0);
    command.spawn()?;

    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline {
        if let Some(registry) = read_live_registry(root, registry_path) {
            return Ok(registry);
        }
        std::thread::sleep(Duration::from_millis(75));
    }

    Err(format!(
        "sidecar did not become healthy before timeout; registry {}",
        registry_path.display()
    )
    .into())
}

fn read_live_registry(root: &str, registry_path: &Path) -> Option<Registry> {
    let raw = std::fs::read_to_string(registry_path).ok()?;
    let registry: Registry = serde_json::from_str(&raw).ok()?;
    if registry.root != root {
        return None;
    }
    let health = http_get_json(&format!("{}/health", registry.fffq_url)).ok()?;
    if health.get("ok").and_then(|v| v.as_bool()) != Some(true) {
        return None;
    }
    if health.get("root").and_then(|v| v.as_str()) != Some(root) {
        return None;
    }
    Some(registry)
}

fn resolve_fff_mcp_bin(explicit: Option<&Path>) -> Result<PathBuf> {
    if let Some(path) = explicit {
        return Ok(path.to_path_buf());
    }
    if let Ok(path) = std::env::var("FFF_MCP_BIN") {
        return Ok(PathBuf::from(path));
    }
    if let Ok(current) = std::env::current_exe()
        && let Some(dir) = current.parent()
    {
        let sibling = dir.join("fff-mcp");
        if sibling.exists() {
            return Ok(sibling);
        }
    }
    let local = PathBuf::from("/Users/xenking/.local/bin/fff-mcp");
    if local.exists() {
        return Ok(local);
    }
    Ok(PathBuf::from("fff-mcp"))
}

fn call_text(
    root: &str,
    registry_path: &Path,
    fff_mcp_bin: Option<&Path>,
    no_start: bool,
    direct_tool: &str,
    mcp_tool: &str,
    payload: serde_json::Value,
) -> Result<String> {
    let registry = ensure_sidecar(root, registry_path, fff_mcp_bin, no_start)?;
    match call_text_once(&registry, direct_tool, mcp_tool, payload.clone()) {
        Ok(text) => Ok(text),
        Err(error) if !no_start && is_retryable_sidecar_error(&error.to_string()) => {
            eprintln!("fffq: sidecar request failed ({error}); restarting sidecar once");
            restart_sidecar(root, registry_path, fff_mcp_bin).and_then(|registry| {
                call_text_once(&registry, direct_tool, mcp_tool, payload).map_err(|retry_error| {
                    format!("{error}; retry after restarting sidecar also failed: {retry_error}")
                        .into()
                })
            })
        }
        Err(error) => Err(error),
    }
}

fn call_batch(
    root: &str,
    registry_path: &Path,
    fff_mcp_bin: Option<&Path>,
    no_start: bool,
    input: &str,
) -> Result<String> {
    let registry = ensure_sidecar(root, registry_path, fff_mcp_bin, no_start)?;
    match call_batch_once(&registry, input) {
        Ok(body) => Ok(body),
        Err(error) if !no_start && is_retryable_sidecar_error(&error.to_string()) => {
            eprintln!("fffq: sidecar request failed ({error}); restarting sidecar once");
            restart_sidecar(root, registry_path, fff_mcp_bin).and_then(|registry| {
                call_batch_once(&registry, input).map_err(|retry_error| {
                    format!("{error}; retry after restarting sidecar also failed: {retry_error}")
                        .into()
                })
            })
        }
        Err(error) => Err(error),
    }
}

fn restart_sidecar(
    root: &str,
    registry_path: &Path,
    fff_mcp_bin: Option<&Path>,
) -> Result<Registry> {
    let _ = std::fs::remove_file(registry_path);
    ensure_sidecar(root, registry_path, fff_mcp_bin, false)
}

fn is_retryable_sidecar_error(message: &str) -> bool {
    message.contains("Connection refused")
        || message.contains("connection refused")
        || message.contains("Connection reset")
        || message.contains("connection reset")
        || message.contains("Broken pipe")
        || message.contains("broken pipe")
        || message.contains("timed out")
        || message.contains("empty HTTP response from sidecar")
        || message.contains("malformed HTTP response")
}

fn call_text_once(
    registry: &Registry,
    direct_tool: &str,
    mcp_tool: &str,
    payload: serde_json::Value,
) -> Result<String> {
    let direct_url = format!("{}/{}", registry.fffq_url, direct_tool);
    match http_post_json(&direct_url, &payload.to_string(), &[]) {
        Ok(response) if response.status < 400 => {
            let value: serde_json::Value = serde_json::from_str(&response.body)?;
            if let Some(text) = value.get("text").and_then(|v| v.as_str()) {
                return Ok(text.to_string());
            }
            Err(format!("unexpected fffq response: {}", response.body).into())
        }
        Ok(response) => {
            let fallback_error = format!("direct fffq HTTP returned {}", response.status);
            call_mcp_text(registry, mcp_tool, payload).map_err(|error| {
                format!("{fallback_error}; streamable HTTP MCP fallback also failed: {error}").into()
            })
        }
        Err(error) => call_mcp_text(registry, mcp_tool, payload).map_err(|fallback| {
            format!("direct fffq HTTP failed: {error}; streamable HTTP MCP fallback also failed: {fallback}").into()
        }),
    }
}

fn call_batch_once(registry: &Registry, input: &str) -> Result<String> {
    let response = http_post_json(&format!("{}/batch", registry.fffq_url), input, &[])?;
    ensure_success(&response)?;
    Ok(response.body)
}

fn call_mcp_text(registry: &Registry, tool: &str, arguments: serde_json::Value) -> Result<String> {
    let init = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": { "name": "fffq", "version": env!("CARGO_PKG_VERSION") }
        }
    });
    let init_response = http_post_json(
        &registry.mcp_url,
        &init.to_string(),
        &[("Accept", "application/json, text/event-stream")],
    )?;
    ensure_success(&init_response)?;
    let session_id = init_response
        .headers
        .iter()
        .find_map(|(name, value)| (name.eq_ignore_ascii_case("mcp-session-id")).then_some(value))
        .ok_or("missing Mcp-Session-Id from initialize response")?
        .clone();

    let initialized = json!({
        "jsonrpc": "2.0",
        "method": "notifications/initialized",
        "params": {}
    });
    let initialized_response = http_post_json(
        &registry.mcp_url,
        &initialized.to_string(),
        &[
            ("Accept", "application/json, text/event-stream"),
            ("Mcp-Session-Id", &session_id),
        ],
    )?;
    ensure_success(&initialized_response)?;

    let call = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {
            "name": tool,
            "arguments": arguments
        }
    });
    let call_response = http_post_json(
        &registry.mcp_url,
        &call.to_string(),
        &[
            ("Accept", "application/json, text/event-stream"),
            ("Mcp-Session-Id", &session_id),
        ],
    )?;
    ensure_success(&call_response)?;
    let json_body = extract_json_body(&call_response.body)?;
    let value: serde_json::Value = serde_json::from_str(&json_body)?;
    if let Some(error) = value.get("error") {
        return Err(format!("MCP error: {error}").into());
    }
    let text = value
        .pointer("/result/content/0/text")
        .and_then(|v| v.as_str())
        .ok_or_else(|| format!("MCP response did not contain text content: {value}"))?;
    Ok(text.to_string())
}

fn http_get_json(url: &str) -> Result<serde_json::Value> {
    let response = http_request("GET", url, None, &[])?;
    ensure_success(&response)?;
    Ok(serde_json::from_str(&response.body)?)
}

fn http_post_json(url: &str, body: &str, headers: &[(&str, &str)]) -> Result<HttpResponse> {
    http_request("POST", url, Some(body), headers)
}

fn http_request(
    method: &str,
    url: &str,
    body: Option<&str>,
    extra_headers: &[(&str, &str)],
) -> Result<HttpResponse> {
    let parsed = parse_http_url(url)?;
    let addr = (parsed.host.as_str(), parsed.port)
        .to_socket_addrs()?
        .next()
        .ok_or("could not resolve HTTP host")?;
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(2))?;
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    stream.set_write_timeout(Some(Duration::from_secs(2)))?;

    let body = body.unwrap_or("");
    let mut request = format!(
        "{method} {} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n",
        parsed.path, parsed.host
    );
    if body.is_empty() {
        request.push_str("Content-Length: 0\r\n");
    } else {
        request.push_str("Content-Type: application/json\r\n");
        request.push_str(&format!("Content-Length: {}\r\n", body.len()));
    }
    for (name, value) in extra_headers {
        request.push_str(name);
        request.push_str(": ");
        request.push_str(value);
        request.push_str("\r\n");
    }
    request.push_str("\r\n");
    request.push_str(body);
    stream.write_all(request.as_bytes())?;

    let mut raw = Vec::new();
    stream.read_to_end(&mut raw)?;
    parse_http_response(&raw)
}

fn parse_http_url(url: &str) -> Result<HttpUrl> {
    let rest = url
        .strip_prefix("http://")
        .ok_or("only http:// sidecar URLs are supported")?;
    let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
    let (host, port) = authority
        .rsplit_once(':')
        .ok_or("HTTP URL must include an explicit port")?;
    Ok(HttpUrl {
        host: host.to_string(),
        port: port.parse()?,
        path: format!("/{path}"),
    })
}

fn parse_http_response(raw: &[u8]) -> Result<HttpResponse> {
    if raw.is_empty() {
        return Err("empty HTTP response from sidecar".into());
    }
    let response = String::from_utf8_lossy(raw);
    let (head, body) = response
        .split_once("\r\n\r\n")
        .ok_or("malformed HTTP response")?;
    let mut lines = head.lines();
    let status_line = lines.next().ok_or("missing HTTP status")?;
    let status = status_line
        .split_whitespace()
        .nth(1)
        .ok_or("missing HTTP status code")?
        .parse()?;
    let headers: Vec<(String, String)> = lines
        .filter_map(|line| {
            let (name, value) = line.split_once(':')?;
            Some((name.trim().to_string(), value.trim().to_string()))
        })
        .collect();
    let body = if headers.iter().any(|(name, value)| {
        name.eq_ignore_ascii_case("transfer-encoding") && value.eq_ignore_ascii_case("chunked")
    }) {
        decode_chunked_body(body)?
    } else {
        body.to_string()
    };
    Ok(HttpResponse {
        status,
        headers,
        body,
    })
}

fn decode_chunked_body(body: &str) -> Result<String> {
    let mut rest = body;
    let mut out = String::new();
    loop {
        let (len_hex, after_len) = rest.split_once("\r\n").ok_or("malformed chunked body")?;
        let len = usize::from_str_radix(len_hex.trim(), 16)?;
        if len == 0 {
            break;
        }
        if after_len.len() < len + 2 {
            return Err("truncated chunked body".into());
        }
        out.push_str(&after_len[..len]);
        rest = &after_len[len + 2..];
    }
    Ok(out)
}

fn ensure_success(response: &HttpResponse) -> Result<()> {
    if response.status >= 400 {
        Err(format!("HTTP {}: {}", response.status, response.body).into())
    } else {
        Ok(())
    }
}

fn extract_json_body(body: &str) -> Result<String> {
    let trimmed = body.trim();
    if trimmed.starts_with('{') {
        return Ok(trimmed.to_string());
    }
    for block in trimmed.split("\n\n") {
        for line in block.lines() {
            if let Some(data) = line.strip_prefix("data: ")
                && data.trim().starts_with('{')
            {
                return Ok(data.trim().to_string());
            }
        }
    }
    Err(format!("could not extract JSON from response: {trimmed}").into())
}

#[allow(dead_code)]
fn _system_time_ms(time: SystemTime) -> Result<u128> {
    Ok(time.duration_since(SystemTime::UNIX_EPOCH)?.as_millis())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_hash_is_stable() {
        assert_eq!(
            registry_path("/tmp/example").file_name().unwrap(),
            "f41a30cd85f04889.json"
        );
    }

    #[test]
    fn parses_sidecar_url() {
        let parsed = parse_http_url("http://127.0.0.1:8761/fffq/find").unwrap();
        assert_eq!(parsed.host, "127.0.0.1");
        assert_eq!(parsed.port, 8761);
        assert_eq!(parsed.path, "/fffq/find");
    }

    #[test]
    fn extracts_sse_json() {
        let body = "event: message\ndata: {\"jsonrpc\":\"2.0\",\"id\":2}\n\n";
        assert_eq!(
            extract_json_body(body).unwrap(),
            "{\"jsonrpc\":\"2.0\",\"id\":2}"
        );
    }

    #[test]
    fn empty_http_response_is_reported_as_sidecar_failure() {
        let error = parse_http_response(b"").unwrap_err().to_string();
        assert_eq!(error, "empty HTTP response from sidecar");
        assert!(is_retryable_sidecar_error(&error));
    }

    #[test]
    fn retryable_sidecar_errors_match_observed_failure() {
        let error = "direct fffq HTTP failed: malformed HTTP response; streamable HTTP MCP fallback also failed: Connection refused (os error 61)";
        assert!(is_retryable_sidecar_error(error));
    }

    #[test]
    fn http_status_errors_do_not_force_sidecar_restart() {
        let error = "direct fffq HTTP returned 400; streamable HTTP MCP fallback also failed: MCP error: invalid request";
        assert!(!is_retryable_sidecar_error(error));
    }
}
