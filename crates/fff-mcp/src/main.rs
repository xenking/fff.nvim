//! FFF MCP Server — high-performance file finder for AI code assistants.
//!
//! Drop-in replacement for AI code assistant file search tools (Glob/Grep).
//! Provides frecency-ranked, fuzzy-matched, git-aware file finding and
//! code search via the Model Context Protocol (MCP).
//!
//! Uses `fff-core` directly (zero FFI overhead) for all search operations.

mod cursor;
mod healthcheck;
mod output;
mod server;
mod update_check;

use axum::routing::get;
use clap::{Parser, ValueEnum};
use fff::file_picker::FilePicker;
use fff::frecency::FrecencyTracker;
use fff::{FFFMode, SharedFrecency, SharedPicker};
use git2::Repository;
use mimalloc::MiMalloc;
use rmcp::{
    ServiceExt,
    transport::{
        stdio,
        streamable_http_server::{
            StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
        },
    },
};
use server::FffServer;
use std::net::SocketAddr;
use tokio_util::sync::CancellationToken;

#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;

pub const MCP_INSTRUCTIONS: &str = concat!(
    "FFF is a fast file finder with frecency-ranked results (frequent/recent files first, git-dirty files boosted).\n",
    "\n",
    "## Which Tool Should I Use?\n",
    "\n",
    "- **grep**: DEFAULT tool. Searches file CONTENTS -- definitions, usage, patterns. Use when you have a specific name or pattern.\n",
    "- **find_files**: Explores which files/modules exist for a topic. Use when you DON'T have a specific identifier or LOOKING FOR A FILE.\n",
    "- **multi_grep**: OR logic across multiple patterns. Use for case variants (e.g. ['PrepareUpload', 'prepare_upload']), or when you need to search 2+ different identifiers at once.\n",
    "\n",
    "## Core Rules\n",
    "\n",
    "### 1. Search BARE IDENTIFIERS only\n",
    "Grep matches single lines. Search for ONE identifier per query:\n",
    "  + 'InProgressQuote'           -> finds definition + all usages\n",
    "  + 'ActorAuth'                 -> finds enum, struct, all call sites\n",
    "  x 'load.*metadata.*InProgressQuote' -> regex spanning multiple tokens, 0 results\n",
    "  x 'ctx.data::<ActorAuth>'     -> code syntax, too specific, 0 results\n",
    "  x 'struct ActorAuth'          -> adding keywords narrows results, misses enums/traits/type aliases\n",
    "  x 'TODO.*#\\d+'               -> complex regex, use simple 'TODO' then filter visually\n",
    "\n",
    "### 2. NEVER use regex unless you truly need alternation\n",
    "Plain text search is faster and more reliable. Regex patterns like `.*`, `\\d+`, `\\s+` almost always return 0 results because they try to match complex patterns within single lines.\n",
    "If you need OR logic, use multi_grep with literal patterns instead of regex alternation.\n",
    "\n",
    "### 3. Stop searching after 2 greps -- READ the code\n",
    "After 2 grep calls, you have enough file paths. Read the top result to understand the code.\n",
    "Do NOT keep grepping with variations. More greps != better understanding.\n",
    "\n",
    "### 4. Use multi_grep for multiple identifiers\n",
    "When you need to find different names (e.g. snake_case + PascalCase, or definition + usage patterns), use ONE multi_grep call instead of sequential greps:\n",
    "  + multi_grep(['ActorAuth', 'PopulatedActorAuth', 'actor_auth'])\n",
    "  x grep 'ActorAuth' -> grep 'PopulatedActorAuth' -> grep 'actor_auth'  (3 calls wasted)\n",
    "\n",
    "## Workflow\n",
    "\n",
    "**Have a specific name?** -> grep the bare identifier.\n",
    "**Need multiple name variants?** -> multi_grep with all variants in one call.\n",
    "**Exploring a topic / finding files?** -> find_files.\n",
    "**Got results?** -> Read the top file. Don't grep again.\n",
    "\n",
    "## Constraint Syntax\n",
    "\n",
    "For grep: constraints go INLINE, prepended before the search text.\n",
    "For multi_grep: constraints go in the separate 'constraints' parameter.\n",
    "\n",
    "Constraints MUST match one of these formats:\n",
    "  Extension: '*.rs', '*.{ts,tsx}'\n",
    "  Directory: 'src/', 'quotes/'\n",
    "  Filename: 'schema.rs', 'src/main.rs'\n",
    "  Exclude: '!test/', '!*.spec.ts'\n",
    "\n",
    "! Bare words without extensions are NOT constraints. 'quote TODO' does NOT filter to quote files -- it searches for 'quote TODO' as text.\n",
    "  + 'schema.rs TODO'   -> searches for 'TODO' in files schema.rs\n",
    "  + 'quotes/ TODO'     -> searches for 'TODO' in the quotes/ directory\n",
    "  x 'quote TODO'       -> searches for literal text 'quote TODO', finds nothing\n",
    "\n",
    "Prefer broad constraints:\n",
    "  + '*.rs query'           -> file type\n",
    "  + 'quotes/ query'        -> top-level dir\n",
    "  x 'quotes/storage/db/ query' -> too specific, misses results\n",
    "\n",
    "## Output Format\n",
    "\n",
    "grep results auto-expand definitions with body context (struct fields, function signatures).\n",
    "This often provides enough information WITHOUT a follow-up Read call.\n",
    "Lines marked with | are definition body context. [def] marks definition files.\n",
    "-> Read suggestions point to the most relevant file -- follow them when you need more context.\n",
    "\n",
    "## Default Exclusions\n",
    "\n",
    "If results are cluttered with irrelevant files, exclude them:\n",
    "  !tests/ - exclude tests directory\n",
    "  !*.spec.ts - exclude test files\n",
    "  !generated/ - exclude generated code",
);

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub(crate) enum McpTransport {
    /// Serve MCP over stdio. This is the default for compatibility with all MCP clients.
    Stdio,
    /// Serve MCP over Streamable HTTP. Useful for long-lived clients that handle stdio
    /// MCP process restarts poorly.
    StreamableHttp,
}

/// FFF MCP Server — high-performance file finder for AI code assistants.
#[derive(Parser)]
#[command(name = "fff-mcp", version = concat!(env!("CARGO_PKG_VERSION"), " (", env!("FFF_GIT_HASH"), ")"))]
pub(crate) struct Args {
    /// Base directory to index. Defaults to the current working directory.
    #[arg(value_name = "PATH")]
    base_path: Option<String>,

    /// Path to the frecency database.
    #[arg(long = "frecency-db")]
    frecency_db_path: Option<String>,

    /// Path to the query history database.
    #[arg(long = "history-db")]
    #[allow(dead_code)]
    history_db_path: Option<String>,

    /// Path to the log file.
    #[arg(long = "log-file")]
    log_file: Option<String>,

    /// Log level (e.g. trace, debug, info, warn, error).
    #[arg(long = "log-level")]
    log_level: Option<String>,

    /// Disable automatic update checks on startup.
    #[arg(long = "no-update-check")]
    no_update_check: bool,

    /// Disable eager mmap warmup after the initial scan. Grep results will
    /// still work (files are mmap'd lazily on first access), but the first
    /// search may be slightly slower. Useful on very large repos where the
    /// warmup would consume too many kernel resources.
    #[arg(long = "no-warmup")]
    no_warmup: bool,

    /// Disable the content index built after the initial scan.
    /// This makes grep calls slower but consumes less RAM (recommended to not turn off)
    #[arg(long = "no-content-indexing")]
    no_content_indexing: bool,

    /// Explicitly enable content indexing even when `--no-warmup` is set.
    #[arg(long = "content-indexing")]
    content_indexing: bool,

    /// Disable the background file-system watcher. Files are scanned once
    /// at startup but not monitored for changes.
    #[arg(long = "no-watch")]
    no_watch: bool,

    /// Re-scan the indexed tree periodically instead of relying on OS-level
    /// file-system watches. This is useful for broad roots on macOS where
    /// live watchers can consume thousands of file descriptors.
    #[arg(long = "poll-interval-secs", env = "FFF_POLL_INTERVAL_SECS")]
    poll_interval_secs: Option<u64>,

    /// Maximum number of files whose content is kept persistently in memory.
    /// Files beyond this limit are still searchable via temporary mmaps that
    /// are released after each grep. Defaults to 30 000.
    /// Also settable via the FFF_MAX_CACHED_FILES environment variable.
    #[arg(long = "max-cached-files", env = "FFF_MAX_CACHED_FILES")]
    max_cached_files: Option<usize>,

    /// MCP transport to use.
    #[arg(
        long = "transport",
        env = "FFF_MCP_TRANSPORT",
        value_enum,
        default_value_t = McpTransport::Stdio
    )]
    transport: McpTransport,

    /// Address used by Streamable HTTP mode.
    #[arg(
        long = "http-bind",
        env = "FFF_MCP_HTTP_BIND",
        default_value = "127.0.0.1:8761"
    )]
    http_bind: String,

    /// HTTP path for Streamable HTTP mode.
    #[arg(long = "http-path", env = "FFF_MCP_HTTP_PATH", default_value = "/mcp")]
    http_path: String,

    /// Run a health check and print diagnostic information, then exit.
    #[arg(long = "healthcheck")]
    pub(crate) healthcheck: bool,
}

/// Resolve default paths for the log file.
/// Database paths (frecency, history) must be explicitly provided via flags.
fn resolve_defaults(args: &mut Args) {
    // Ensure parent directories exist for database paths when provided
    for path in [&args.frecency_db_path, &args.history_db_path]
        .into_iter()
        .flatten()
    {
        if let Some(parent) = std::path::Path::new(path).parent() {
            let _ = std::fs::create_dir_all(parent);
        }
    }

    if args.log_file.is_none() {
        let home = dirs_home();
        let is_windows = cfg!(target_os = "windows");
        args.log_file = Some(if is_windows {
            format!("{}\\AppData\\Local\\fff_mcp.log", home)
        } else {
            format!("{}/.cache/fff_mcp.log", home)
        });
    }
}

fn dirs_home() -> String {
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| "/tmp".to_string())
}

fn spawn_periodic_rescan(
    shared_picker: SharedPicker,
    shared_frecency: SharedFrecency,
    interval_secs: u64,
) {
    tokio::task::spawn_blocking(move || {
        let interval = std::time::Duration::from_secs(interval_secs);
        tracing::info!(
            interval_secs,
            "Periodic filesystem rescan enabled for fff-mcp"
        );

        loop {
            std::thread::sleep(interval);

            let Ok(mut guard) = shared_picker.write() else {
                tracing::warn!("Failed to acquire picker lock for periodic rescan");
                continue;
            };

            let Some(ref mut picker) = *guard else {
                tracing::warn!("Picker not initialized, skipping periodic rescan");
                continue;
            };

            let started = std::time::Instant::now();
            match picker.trigger_rescan(&shared_frecency) {
                Ok(()) => tracing::info!(
                    elapsed = ?started.elapsed(),
                    "Periodic filesystem rescan completed"
                ),
                Err(error) => tracing::error!(?error, "Periodic filesystem rescan failed"),
            }
        }
    });
}

fn normalize_http_path(path: &str) -> String {
    if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{path}")
    }
}

async fn health() -> &'static str {
    "ok"
}

fn build_streamable_http_router(
    shared_picker: SharedPicker,
    shared_frecency: SharedFrecency,
    path: &str,
    cancellation_token: CancellationToken,
) -> axum::Router {
    let http_path = normalize_http_path(path);
    let picker_for_factory = shared_picker.clone();
    let frecency_for_factory = shared_frecency.clone();
    let service: StreamableHttpService<FffServer, LocalSessionManager> = StreamableHttpService::new(
        move || {
            Ok(FffServer::new(
                picker_for_factory.clone(),
                frecency_for_factory.clone(),
            ))
        },
        Default::default(),
        StreamableHttpServerConfig {
            sse_keep_alive: None,
            cancellation_token,
            ..Default::default()
        },
    );

    axum::Router::new()
        .nest_service(&http_path, service)
        .route("/health", get(health))
}

async fn serve_streamable_http(
    shared_picker: SharedPicker,
    shared_frecency: SharedFrecency,
    bind: &str,
    path: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let addr: SocketAddr = bind.parse()?;
    let http_path = normalize_http_path(path);
    let cancellation_token = CancellationToken::new();
    let router = build_streamable_http_router(
        shared_picker,
        shared_frecency,
        &http_path,
        cancellation_token.child_token(),
    );
    let listener = tokio::net::TcpListener::bind(addr).await?;
    let local_addr = listener.local_addr()?;
    tracing::info!(%local_addr, path = %http_path, "FFF MCP Streamable HTTP server listening");

    axum::serve(listener, router)
        .with_graceful_shutdown(async move {
            tokio::signal::ctrl_c().await.ok();
            cancellation_token.cancel();
        })
        .await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn make_temp_repo() -> std::path::PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock before UNIX_EPOCH")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("fff-mcp-http-test-{unique}"));
        std::fs::create_dir_all(path.join("src")).expect("create temp test repo");
        std::fs::write(path.join("README.md"), "# fff test\n").expect("write README");
        std::fs::write(path.join("src").join("main.rs"), "fn main() {}\n")
            .expect("write source file");
        path
    }

    async fn post_mcp(
        addr: SocketAddr,
        path: &str,
        session_id: Option<&str>,
        body: &str,
        expected_body_fragment: &str,
    ) -> (Vec<(String, String)>, String) {
        let mut stream = tokio::net::TcpStream::connect(addr)
            .await
            .expect("connect test HTTP server");
        let session_header = session_id
            .map(|id| format!("Mcp-Session-Id: {id}\r\n"))
            .unwrap_or_default();
        let request = format!(
            "POST {path} HTTP/1.1\r\n\
             Host: {addr}\r\n\
             Content-Type: application/json\r\n\
             Accept: application/json, text/event-stream\r\n\
             Connection: close\r\n\
             {session_header}\
             Content-Length: {}\r\n\
             \r\n\
             {body}",
            body.len()
        );
        stream
            .write_all(request.as_bytes())
            .await
            .expect("write HTTP request");

        let mut response = Vec::new();
        let mut chunk = [0_u8; 4096];
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                break;
            }
            match tokio::time::timeout(
                remaining.min(Duration::from_millis(100)),
                stream.read(&mut chunk),
            )
            .await
            {
                Ok(Ok(0)) => break,
                Ok(Ok(n)) => {
                    response.extend_from_slice(&chunk[..n]);
                    if !expected_body_fragment.is_empty()
                        && String::from_utf8_lossy(&response).contains(expected_body_fragment)
                    {
                        break;
                    }
                }
                Ok(Err(error)) => panic!("read HTTP response: {error}"),
                Err(_)
                    if !expected_body_fragment.is_empty()
                        && String::from_utf8_lossy(&response).contains(expected_body_fragment) =>
                {
                    break;
                }
                Err(_) => continue,
            }
        }

        let response = String::from_utf8(response).expect("HTTP response is UTF-8");
        assert!(
            response.starts_with("HTTP/1.1 200") || response.starts_with("HTTP/1.1 202"),
            "unexpected HTTP response: {response}"
        );
        if !expected_body_fragment.is_empty() {
            assert!(
                response.contains(expected_body_fragment),
                "response did not contain {expected_body_fragment:?}: {response}"
            );
        }

        let (head, body) = response
            .split_once("\r\n\r\n")
            .map_or((response.as_str(), ""), |(head, body)| (head, body));
        let headers = head
            .lines()
            .skip(1)
            .filter_map(|line| {
                let (name, value) = line.split_once(':')?;
                Some((name.trim().to_ascii_lowercase(), value.trim().to_string()))
            })
            .collect();

        (headers, body.to_string())
    }

    fn create_test_picker(base_path: &std::path::Path) -> (SharedPicker, SharedFrecency) {
        let shared_picker = SharedPicker::default();
        let shared_frecency = SharedFrecency::default();
        FilePicker::new_with_shared_state(
            shared_picker.clone(),
            shared_frecency.clone(),
            fff::FilePickerOptions {
                base_path: base_path.to_string_lossy().to_string(),
                enable_mmap_cache: false,
                enable_content_indexing: false,
                watch: false,
                mode: FFFMode::Ai,
                cache_budget: None,
            },
        )
        .expect("init file picker");
        assert!(
            shared_picker.wait_for_scan(Duration::from_secs(5)),
            "initial test scan did not finish"
        );
        (shared_picker, shared_frecency)
    }

    #[test]
    fn parses_streamable_http_transport_flags() {
        let args = Args::try_parse_from([
            "fff-mcp",
            "--transport",
            "streamable-http",
            "--http-bind",
            "127.0.0.1:9999",
            "--http-path",
            "mcp",
        ])
        .expect("parse streamable HTTP args");

        assert_eq!(args.transport, McpTransport::StreamableHttp);
        assert_eq!(args.http_bind, "127.0.0.1:9999");
        assert_eq!(normalize_http_path(&args.http_path), "/mcp");
    }

    #[tokio::test]
    async fn streamable_http_serves_find_files() {
        let temp_repo = make_temp_repo();
        let (shared_picker, shared_frecency) = create_test_picker(&temp_repo);
        let cancellation_token = CancellationToken::new();
        let router = build_streamable_http_router(
            shared_picker,
            shared_frecency,
            "/mcp",
            cancellation_token.child_token(),
        );

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind test listener");
        let addr = listener.local_addr().expect("get test listener addr");
        let server = tokio::spawn({
            let cancellation_token = cancellation_token.clone();
            async move {
                axum::serve(listener, router)
                    .with_graceful_shutdown(async move {
                        cancellation_token.cancelled_owned().await;
                    })
                    .await
                    .expect("serve streamable HTTP test server");
            }
        });

        let init = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": { "name": "fff-mcp-test", "version": "1.0" }
            }
        })
        .to_string();
        let (headers, init_body) = post_mcp(addr, "/mcp", None, &init, "serverInfo").await;
        assert!(
            init_body.contains("fff"),
            "initialize response did not include fff server info: {init_body}"
        );
        let session_id = headers
            .iter()
            .find_map(|(name, value)| (name == "mcp-session-id").then(|| value.clone()))
            .expect("initialize response includes Mcp-Session-Id");

        let initialized = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized",
            "params": {}
        })
        .to_string();
        let _ = post_mcp(addr, "/mcp", Some(&session_id), &initialized, "").await;

        let call = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {
                "name": "find_files",
                "arguments": {
                    "query": "README",
                    "maxResults": 5
                }
            }
        })
        .to_string();
        let (_headers, body) = post_mcp(addr, "/mcp", Some(&session_id), &call, "README.md").await;
        assert!(
            body.contains("\"isError\":false") || !body.contains("\"isError\":true"),
            "find_files returned an MCP error: {body}"
        );

        cancellation_token.cancel();
        server.await.expect("join test server");
        std::fs::remove_dir_all(temp_repo).ok();
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = Args::parse();
    resolve_defaults(&mut args);

    if args.healthcheck {
        return healthcheck::run_healthcheck(&args);
    }

    let log_file = args.log_file.as_deref().unwrap_or("");
    if let Err(e) = fff::log::init_tracing(log_file, args.log_level.as_deref()) {
        eprintln!("Warning: Failed to init tracing: {}", e);
    }

    let base_path = args.base_path.unwrap_or_else(|| {
        std::env::current_dir()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string()
    });

    let base_path = match Repository::discover(&base_path) {
        Ok(repo) => {
            if let Some(workdir) = repo.workdir() {
                let git_root = workdir.to_string_lossy().to_string();
                tracing::info!("Discovered git root: {}", git_root);
                git_root
            } else {
                tracing::info!("Git repository is bare, using base path: {}", base_path);
                base_path
            }
        }
        Err(_) => {
            tracing::info!(
                "No git repository found, indexing from base path: {}",
                base_path
            );
            base_path
        }
    };

    let shared_picker = SharedPicker::default();
    let shared_frecency = SharedFrecency::default();
    if let Some(frecency_db_path) = args.frecency_db_path {
        match FrecencyTracker::new(&frecency_db_path, false) {
            Ok(tracker) => {
                let _ = shared_frecency.init(tracker);
                let _ = shared_frecency.spawn_gc(frecency_db_path, false);
            }
            Err(e) => {
                eprintln!("Warning: Failed to init frecency db: {}", e);
            }
        }
    }

    // Content indexing follows warmup by default (backward compat), unless
    // the user explicitly opts in via --content-indexing or out via
    // --no-content-indexing.
    let enable_content_indexing = if args.content_indexing {
        true
    } else if args.no_content_indexing {
        false
    } else {
        !args.no_warmup
    };

    // Initialize file picker (spawns background scan + watcher)
    FilePicker::new_with_shared_state(
        shared_picker.clone(),
        shared_frecency.clone(),
        fff::FilePickerOptions {
            base_path,
            enable_mmap_cache: !args.no_warmup,
            enable_content_indexing,
            watch: !args.no_watch,
            mode: FFFMode::Ai,
            cache_budget: args
                .max_cached_files
                .map(fff::ContentCacheBudget::new_for_repo),
        },
    )
    .map_err(|e| format!("Failed to init file picker: {}", e))?;

    if !args.no_update_check {
        update_check::spawn_update_check();
    }

    if let Some(interval_secs) = args.poll_interval_secs.filter(|interval| *interval > 0) {
        spawn_periodic_rescan(
            shared_picker.clone(),
            shared_frecency.clone(),
            interval_secs,
        );
    }

    // Create and start the MCP server
    let server = FffServer::new(shared_picker.clone(), shared_frecency.clone());

    // Wait for initial scan in background — don't block server startup
    let picker_clone_for_scan = shared_picker.clone();
    tokio::task::spawn_blocking(move || {
        let start = std::time::Instant::now();
        loop {
            let is_scanning = picker_clone_for_scan
                .read()
                .ok()
                .and_then(|g| g.as_ref().map(|p| p.is_scan_active()))
                .unwrap_or(true);

            if !is_scanning {
                tracing::info!("Initial scan completed in {:?}", start.elapsed());
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    });

    if args.transport == McpTransport::StreamableHttp {
        serve_streamable_http(
            shared_picker.clone(),
            shared_frecency.clone(),
            &args.http_bind,
            &args.http_path,
        )
        .await?;
    } else {
        let service = server
            .serve(stdio())
            .await
            .map_err(|e| format!("Failed to start MCP server: {}", e))?;

        let picker_for_shutdown = shared_picker.clone();
        tokio::spawn(async move {
            tokio::signal::ctrl_c().await.ok();
            if let Ok(mut guard) = picker_for_shutdown.write()
                && let Some(ref mut picker) = *guard
            {
                picker.stop_background_monitor();
            }
            std::process::exit(0);
        });

        service.waiting().await?;
    }

    if let Ok(mut guard) = shared_picker.write()
        && let Some(ref mut picker) = *guard
    {
        picker.stop_background_monitor();
    }

    Ok(())
}
