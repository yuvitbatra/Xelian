//! `xelian gateway` — one local MCP endpoint for every installed MCP server.
//!
//! Instead of wiring N MCP servers into an IDE or agent framework config,
//! the client connects to a single Streamable-HTTP endpoint
//! (`http://127.0.0.1:11432/mcp` by default). The gateway spawns each
//! configured Xelian MCP package as a stdio child, merges their tool lists
//! under `<package>__<tool>` names, routes `tools/call` to the right backend,
//! respawns backends that die, and writes every backend's stderr to one log
//! directory (`~/.xelian/logs/gateway/`).
//!
//! Protocol scope (MVP): `initialize`, `ping`, `tools/list`, `tools/call`.
//! Client notifications are accepted and dropped; other methods get a
//! JSON-RPC -32601. The gateway declares only the `tools` capability, so
//! conforming clients will not ask for resources/prompts.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};
use xelian_core::cache::XelianHome;
use xelian_core::manifest::{Manifest, PackageType};

pub const DEFAULT_PORT: u16 = 11432;
const PROTOCOL_VERSION: &str = "2025-06-18";
const BACKEND_CALL_TIMEOUT: Duration = Duration::from_secs(120);
const BACKEND_INIT_TIMEOUT: Duration = Duration::from_secs(30);

// ---------------------------------------------------------------------------
// Config (~/.xelian/gateway.toml)
// ---------------------------------------------------------------------------

#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct GatewayConfig {
    #[serde(default)]
    pub packages: Vec<String>,
    pub port: Option<u16>,
}

pub fn config_path(home: &XelianHome) -> PathBuf {
    home.root().join("gateway.toml")
}

pub fn load_config(home: &XelianHome) -> anyhow::Result<GatewayConfig> {
    let path = config_path(home);
    if !path.is_file() {
        return Ok(GatewayConfig::default());
    }
    let text = std::fs::read_to_string(&path)?;
    toml::from_str(&text).map_err(|e| anyhow::anyhow!("malformed {}: {e}", path.display()))
}

fn save_config(home: &XelianHome, config: &GatewayConfig) -> anyhow::Result<()> {
    let path = config_path(home);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, toml::to_string_pretty(config)?)?;
    Ok(())
}

fn parse_ref(target: &str) -> anyhow::Result<(String, String)> {
    match xelian_core::run::parse_run_target(target) {
        Ok(xelian_core::run::RunTarget::RegistryRef { owner, name }) => Ok((owner, name)),
        _ => anyhow::bail!("gateway backends must be registry refs (owner/name), got {target:?}"),
    }
}

pub fn cmd_add(target: &str) -> anyhow::Result<()> {
    parse_ref(target)?;
    let home = XelianHome::resolve()?;
    let mut config = load_config(&home)?;
    if config.packages.iter().any(|p| p == target) {
        println!("{target} is already in the gateway");
        return Ok(());
    }
    config.packages.push(target.to_string());
    save_config(&home, &config)?;
    println!(
        "Added {target}. {} package(s) configured — start with `xelian gateway serve`.",
        config.packages.len()
    );
    Ok(())
}

pub fn cmd_remove(target: &str) -> anyhow::Result<()> {
    let home = XelianHome::resolve()?;
    let mut config = load_config(&home)?;
    let before = config.packages.len();
    config.packages.retain(|p| p != target);
    if config.packages.len() == before {
        anyhow::bail!("{target} is not in the gateway config");
    }
    save_config(&home, &config)?;
    println!("Removed {target}.");
    Ok(())
}

pub fn cmd_list() -> anyhow::Result<()> {
    let home = XelianHome::resolve()?;
    let config = load_config(&home)?;
    if config.packages.is_empty() {
        println!("No gateway backends configured. Add one with `xelian gateway add owner/name`.");
        return Ok(());
    }
    for pkg in &config.packages {
        println!("{pkg}");
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Backend: a spawned MCP-server package with a reader thread
// ---------------------------------------------------------------------------

/// Everything needed to (re)spawn a backend without re-running the pipeline.
struct PreparedBackend {
    owner: String,
    name: String,
    /// The `<package>` half of the exposed `<package>__<tool>` names.
    alias: String,
    version: String,
    manifest: Manifest,
    package_dir: PathBuf,
    env_dir: PathBuf,
    bin_dir: PathBuf,
    env_pairs: Vec<(String, String)>,
    log_path: PathBuf,
}

struct Running {
    child: Child,
    stdin: std::process::ChildStdin,
    /// Messages from the backend's stdout, forwarded by the reader thread.
    incoming: Receiver<Value>,
    next_id: i64,
}

struct Backend {
    prepared: PreparedBackend,
    running: Option<Running>,
    restarts: u32,
}

impl Backend {
    /// Spawn the backend process, redirect stderr to its log file, and run
    /// the MCP initialize handshake.
    fn spawn(&mut self) -> anyhow::Result<()> {
        let p = &self.prepared;
        let (program, args, work_dir) =
            build_command(&p.manifest, &p.package_dir, &p.env_dir, &p.bin_dir)?;

        if let Some(parent) = p.log_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let log_file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&p.log_path)?;

        let mut cmd = Command::new(&program);
        cmd.args(&args)
            .current_dir(&work_dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::from(log_file));
        for (k, v) in &p.env_pairs {
            cmd.env(k, v);
        }

        let mut child = cmd
            .spawn()
            .map_err(|e| anyhow::anyhow!("failed to spawn {}/{}: {e}", p.owner, p.name))?;
        let stdin = child.stdin.take().expect("piped stdin");
        let stdout = child.stdout.take().expect("piped stdout");

        // Reader thread: one per spawn; forwards every JSON line. It exits
        // when the child's stdout closes; a stale thread's sends just fail.
        let (tx, rx): (Sender<Value>, Receiver<Value>) = std::sync::mpsc::channel();
        let tag = format!("{}/{}", p.owner, p.name);
        std::thread::spawn(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines() {
                let Ok(line) = line else { break };
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                match serde_json::from_str::<Value>(trimmed) {
                    Ok(msg) => {
                        if tx.send(msg).is_err() {
                            break;
                        }
                    }
                    Err(_) => {
                        eprintln!("gateway: {tag}: ignoring non-JSON stdout line");
                    }
                }
            }
        });

        let mut running = Running {
            child,
            stdin,
            incoming: rx,
            next_id: 1,
        };

        // MCP initialize handshake (stdio transport, newline-delimited JSON).
        let init = json!({
            "jsonrpc": "2.0",
            "id": 0,
            "method": "initialize",
            "params": {
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": {"name": "xelian-gateway", "version": env!("CARGO_PKG_VERSION")},
            },
        });
        write_message(&mut running.stdin, &init)?;
        wait_for_response(&running.incoming, 0, BACKEND_INIT_TIMEOUT)
            .map_err(|e| anyhow::anyhow!("{}/{} failed MCP initialize: {e}", p.owner, p.name))?;
        write_message(
            &mut running.stdin,
            &json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
        )?;

        self.running = Some(running);
        Ok(())
    }

    fn alive(&mut self) -> bool {
        match &mut self.running {
            Some(r) => r.child.try_wait().map(|s| s.is_none()).unwrap_or(false),
            None => false,
        }
    }

    /// Send `method`/`params` to the backend and wait for its response,
    /// respawning once if the process has died.
    fn request(&mut self, method: &str, params: Value) -> anyhow::Result<Value> {
        if !self.alive() {
            if let Some(mut r) = self.running.take() {
                let _ = r.child.kill();
                let _ = r.child.wait();
            }
            eprintln!(
                "gateway: {}/{} is down — respawning",
                self.prepared.owner, self.prepared.name
            );
            self.restarts += 1;
            self.spawn()?;
        }
        let running = self.running.as_mut().expect("spawned above");
        let id = running.next_id;
        running.next_id += 1;
        let msg = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        write_message(&mut running.stdin, &msg)?;
        wait_for_response(&running.incoming, id, BACKEND_CALL_TIMEOUT)
    }
}

fn write_message(stdin: &mut std::process::ChildStdin, msg: &Value) -> anyhow::Result<()> {
    let mut line = serde_json::to_string(msg)?;
    line.push('\n');
    stdin.write_all(line.as_bytes())?;
    stdin.flush()?;
    Ok(())
}

/// Drain incoming messages until the response with `id` arrives, skipping
/// backend notifications and rejecting backend->client requests (the gateway
/// declares no client capabilities, so none are legitimate).
fn wait_for_response(
    incoming: &Receiver<Value>,
    id: i64,
    timeout: Duration,
) -> anyhow::Result<Value> {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        let remaining = deadline
            .checked_duration_since(std::time::Instant::now())
            .ok_or_else(|| anyhow::anyhow!("timed out after {timeout:?} waiting for response"))?;
        let msg = incoming
            .recv_timeout(remaining)
            .map_err(|_| anyhow::anyhow!("backend closed or timed out"))?;
        if msg.get("id").and_then(Value::as_i64) == Some(id)
            && (msg.get("result").is_some() || msg.get("error").is_some())
        {
            return Ok(msg);
        }
        // Anything else: a notification (drop) or an unexpected request from
        // the backend (drop — replying would require write access we don't
        // hold here; a tools-only server has no business asking).
    }
}

/// Mirror of launch.rs's `build_launch_command` for a piped (non-inherited)
/// spawn. Kept here so the gateway can own its child processes.
fn build_command(
    manifest: &Manifest,
    package_dir: &Path,
    env_dir: &Path,
    bin_dir: &Path,
) -> anyhow::Result<(String, Vec<String>, PathBuf)> {
    use xelian_core::manifest::Language;
    match manifest.language {
        Language::Python => {
            let python_bin = env_dir.join("bin").join("python");
            anyhow::ensure!(
                python_bin.is_file(),
                "Python binary not found at {}",
                python_bin.display()
            );
            Ok((
                python_bin.to_string_lossy().to_string(),
                vec![package_dir
                    .join(&manifest.entrypoint)
                    .to_string_lossy()
                    .to_string()],
                package_dir.to_path_buf(),
            ))
        }
        Language::Node => {
            let node_bin = bin_dir.join("node");
            anyhow::ensure!(
                node_bin.is_file(),
                "Node binary not found at {}",
                node_bin.display()
            );
            let script = env_dir.join(&manifest.entrypoint);
            anyhow::ensure!(
                script.is_file(),
                "entrypoint missing at {}",
                script.display()
            );
            Ok((
                node_bin.to_string_lossy().to_string(),
                vec![
                    "--preserve-symlinks".to_string(),
                    "--preserve-symlinks-main".to_string(),
                    script.to_string_lossy().to_string(),
                ],
                env_dir.to_path_buf(),
            ))
        }
    }
}

// ---------------------------------------------------------------------------
// Preparation: run the standard pipeline for each configured package
// ---------------------------------------------------------------------------

/// Prepare a registry MCP package for gateway use: resolve, download/cache,
/// extract, provision the environment, check permissions, resolve env vars.
/// Same pipeline as `xelian run owner/name --prepare`, minus the launch.
fn prepare_backend(home: &XelianHome, owner: &str, name: &str) -> anyhow::Result<PreparedBackend> {
    let registry_url = xelian_core::auth::resolve_registry_url(home);
    let client = xelian_core::registry_client::RegistryClient::new(&registry_url);
    let info = client
        .fetch_metadata(owner, name)
        .map_err(|e| anyhow::anyhow!("failed to resolve {owner}/{name}: {e}"))?;
    let version = info
        .latest_version
        .ok_or_else(|| anyhow::anyhow!("no resolvable version of {owner}/{name}"))?;

    let pkg_dir = home.registry_package_dir(owner, name, &version);
    let (manifest, package_dir) = if pkg_dir.join("xelian.toml").is_file() {
        let (manifest, _warnings) =
            xelian_core::run::validate_extracted(&pkg_dir).map_err(|e| anyhow::anyhow!(e))?;
        (manifest, pkg_dir)
    } else {
        eprintln!("gateway: downloading {owner}/{name} v{version} ...");
        let archive_bytes = client
            .download_archive(owner, name, &version)
            .map_err(|e| anyhow::anyhow!("failed to download {owner}/{name} v{version}: {e}"))?;
        let tmp_dir = home.tmp();
        std::fs::create_dir_all(&tmp_dir)?;
        let archive_path = tmp_dir.join(format!("{owner}-{name}-{version}.xelian"));
        std::fs::write(&archive_path, &archive_bytes)?;
        let prepared = xelian_core::run::run_registry_archive(&archive_path, owner, name, home)
            .map_err(|e| anyhow::anyhow!(e))?;
        let _ = std::fs::remove_file(&archive_path);
        let manifest_str = std::fs::read_to_string(prepared.package_dir.join("xelian.toml"))?;
        let manifest = Manifest::from_toml_str(&manifest_str)
            .map_err(|e| anyhow::anyhow!("failed to parse cached xelian.toml: {e}"))?;
        (manifest, prepared.package_dir)
    };

    anyhow::ensure!(
        manifest.package_type == PackageType::Mcp,
        "{owner}/{name} is package-type {:?} — the gateway serves MCP servers only",
        manifest.package_type
    );

    let env_dir = home.registry_env_dir(owner, name, &version);
    let prepared_env =
        xelian_core::run::prepare_environment(&package_dir, &manifest, home, env_dir)
            .map_err(|e| anyhow::anyhow!(e))?;

    let grants_path = home.registry_grants_path(owner, name, &version);
    xelian_core::permissions::check_and_prompt(name, &version, &manifest.permissions, &grants_path)
        .map_err(|e| anyhow::anyhow!("permission error: {e}"))?;
    xelian_core::run::model::ensure_model(manifest.primary_model.as_deref(), home)
        .map_err(|e| anyhow::anyhow!("model error: {e}"))?;
    let env_pairs = xelian_core::run::env_vars::resolve_env_vars(&manifest.environment)
        .map_err(|e| anyhow::anyhow!(e))?;

    let log_path = home
        .logs()
        .join("gateway")
        .join(format!("{owner}-{name}.log"));

    Ok(PreparedBackend {
        owner: owner.to_string(),
        name: name.to_string(),
        alias: sanitize_alias(name),
        version,
        env_dir: prepared_env.env_dir.clone(),
        bin_dir: prepared_env.bin_dir.clone(),
        manifest,
        package_dir,
        env_pairs,
        log_path,
    })
}

/// Tool names must match `[a-zA-Z0-9_-]`; a package name becomes the alias
/// half of `<alias>__<tool>` with any other character mapped to `-`.
fn sanitize_alias(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// HTTP server
// ---------------------------------------------------------------------------

type Backends = Arc<Vec<Mutex<Backend>>>;

pub fn cmd_serve(port_flag: Option<u16>) -> anyhow::Result<()> {
    let home = XelianHome::resolve()?;
    home.ensure_layout()?;
    let config = load_config(&home)?;
    anyhow::ensure!(
        !config.packages.is_empty(),
        "no gateway backends configured — add one with `xelian gateway add owner/name`"
    );
    let port = port_flag.or(config.port).unwrap_or(DEFAULT_PORT);

    // Prepare + spawn every backend up front so startup failures are loud.
    let mut backends: Vec<Mutex<Backend>> = Vec::new();
    let mut aliases: BTreeMap<String, String> = BTreeMap::new();
    for target in &config.packages {
        let (owner, name) = parse_ref(target)?;
        let prepared = prepare_backend(&home, &owner, &name)?;
        if let Some(other) = aliases.get(&prepared.alias) {
            anyhow::bail!(
                "gateway alias collision: {target} and {other} both expose tools as \
                 `{}__*` — remove one (`xelian gateway remove ...`)",
                prepared.alias
            );
        }
        aliases.insert(prepared.alias.clone(), target.clone());
        let mut backend = Backend {
            prepared,
            running: None,
            restarts: 0,
        };
        backend.spawn()?;
        eprintln!(
            "gateway: {} v{} up (tools exposed as {}__*, log: {})",
            target,
            backend.prepared.version,
            backend.prepared.alias,
            backend.prepared.log_path.display()
        );
        backends.push(Mutex::new(backend));
    }
    let backends: Backends = Arc::new(backends);

    let addr = format!("127.0.0.1:{port}");
    let server = tiny_http::Server::http(&addr)
        .map_err(|e| anyhow::anyhow!("failed to bind http://{addr}: {e}"))?;
    eprintln!("gateway: serving MCP at http://{addr}/mcp (status: http://{addr}/status)");
    eprintln!("gateway: press Ctrl-C to stop");

    // A small worker pool: enough for an IDE plus an agent framework talking
    // at once, without async machinery.
    let server = Arc::new(server);
    let mut workers = Vec::new();
    for _ in 0..4 {
        let server = Arc::clone(&server);
        let backends = Arc::clone(&backends);
        workers.push(std::thread::spawn(move || {
            for request in server.incoming_requests() {
                handle_request(request, &backends);
            }
        }));
    }
    for w in workers {
        let _ = w.join();
    }
    Ok(())
}

fn handle_request(mut request: tiny_http::Request, backends: &Backends) {
    let url = request.url().to_string();
    let method = request.method().clone();
    let response = match (method.as_str(), url.as_str()) {
        ("GET", "/health") => json_response(200, &json!({"ok": true})),
        ("GET", "/status") => json_response(200, &status_json(backends)),
        ("POST", "/mcp") => {
            let mut body = String::new();
            if request.as_reader().read_to_string(&mut body).is_err() {
                json_response(400, &json!({"error": "unreadable body"}))
            } else {
                match serde_json::from_str::<Value>(&body) {
                    Ok(msg) => match handle_mcp_message(msg, backends) {
                        Some(reply) => json_response(200, &reply),
                        // Notification: acknowledged, no body.
                        None => tiny_http::Response::from_string("")
                            .with_status_code(202)
                            .boxed(),
                    },
                    Err(e) => json_response(
                        400,
                        &json!({
                            "jsonrpc": "2.0", "id": null,
                            "error": {"code": -32700, "message": format!("parse error: {e}")},
                        }),
                    ),
                }
            }
        }
        _ => json_response(
            404,
            &json!({"error": "not found", "endpoints": ["/mcp", "/status", "/health"]}),
        ),
    };
    let _ = request.respond(response);
}

fn json_response(status: u16, body: &Value) -> tiny_http::ResponseBox {
    tiny_http::Response::from_string(body.to_string())
        .with_status_code(status)
        .with_header(
            tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).unwrap(),
        )
        .boxed()
}

fn status_json(backends: &Backends) -> Value {
    let rows: Vec<Value> = backends
        .iter()
        .map(|b| {
            let mut b = b.lock().unwrap();
            let alive = b.alive();
            json!({
                "package": format!("{}/{}", b.prepared.owner, b.prepared.name),
                "version": b.prepared.version,
                "alias": b.prepared.alias,
                "alive": alive,
                "restarts": b.restarts,
                "log": b.prepared.log_path.display().to_string(),
            })
        })
        .collect();
    json!({"gateway": env!("CARGO_PKG_VERSION"), "backends": rows})
}

/// Handle one client->gateway JSON-RPC message. Returns None for
/// notifications (no response is due).
fn handle_mcp_message(msg: Value, backends: &Backends) -> Option<Value> {
    let id = msg.get("id").cloned();
    let method = msg.get("method").and_then(Value::as_str).unwrap_or("");

    // Notifications (no id) are acknowledged and dropped.
    id.as_ref().filter(|v| !v.is_null())?;
    let id = id.unwrap();

    let reply = |body: Value| -> Option<Value> {
        let mut out = json!({"jsonrpc": "2.0", "id": id});
        out.as_object_mut()
            .unwrap()
            .extend(body.as_object().unwrap().clone());
        Some(out)
    };

    match method {
        "initialize" => {
            let client_proto = msg
                .pointer("/params/protocolVersion")
                .and_then(Value::as_str)
                .unwrap_or(PROTOCOL_VERSION);
            reply(json!({"result": {
                "protocolVersion": client_proto,
                "capabilities": {"tools": {"listChanged": false}},
                "serverInfo": {"name": "xelian-gateway", "version": env!("CARGO_PKG_VERSION")},
                "instructions": "Tools are namespaced <package>__<tool>. GET /status lists backends.",
            }}))
        }
        "ping" => reply(json!({"result": {}})),
        "tools/list" => {
            let mut tools = Vec::new();
            let mut errors = Vec::new();
            for backend in backends.iter() {
                let mut b = backend.lock().unwrap();
                let alias = b.prepared.alias.clone();
                match b.request("tools/list", json!({})) {
                    Ok(resp) => {
                        if let Some(list) = resp.pointer("/result/tools").and_then(Value::as_array)
                        {
                            for tool in list {
                                let mut tool = tool.clone();
                                if let Some(name) = tool.get("name").and_then(Value::as_str) {
                                    tool["name"] = json!(format!("{alias}__{name}"));
                                }
                                tools.push(tool);
                            }
                        }
                    }
                    Err(e) => errors.push(format!("{alias}: {e}")),
                }
            }
            if tools.is_empty() && !errors.is_empty() {
                reply(
                    json!({"error": {"code": -32000, "message": format!("all backends failed: {}", errors.join("; "))}}),
                )
            } else {
                reply(json!({"result": {"tools": tools}}))
            }
        }
        "tools/call" => {
            let full_name = msg
                .pointer("/params/name")
                .and_then(Value::as_str)
                .unwrap_or("");
            let Some((alias, tool)) = full_name.split_once("__") else {
                return reply(json!({"error": {"code": -32602, "message":
                    format!("unknown tool {full_name:?} — gateway tools are named <package>__<tool>")}}));
            };
            let backend = backends
                .iter()
                .find(|b| b.lock().unwrap().prepared.alias == alias);
            let Some(backend) = backend else {
                return reply(json!({"error": {"code": -32602, "message":
                    format!("no gateway backend named {alias:?} (see GET /status)")}}));
            };
            let mut params = msg.get("params").cloned().unwrap_or_else(|| json!({}));
            params["name"] = json!(tool);
            let mut b = backend.lock().unwrap();
            match b.request("tools/call", params) {
                Ok(resp) => {
                    let mut body = serde_json::Map::new();
                    if let Some(result) = resp.get("result") {
                        body.insert("result".into(), result.clone());
                    } else if let Some(error) = resp.get("error") {
                        body.insert("error".into(), error.clone());
                    }
                    reply(Value::Object(body))
                }
                Err(e) => reply(json!({"error": {"code": -32000, "message":
                    format!("backend {alias} failed: {e}")}})),
            }
        }
        other => reply(json!({"error": {"code": -32601, "message":
            format!("method {other:?} is not supported by the gateway (tools only)")}})),
    }
}

// ---------------------------------------------------------------------------
// Status / logs client commands
// ---------------------------------------------------------------------------

pub fn cmd_status(port_flag: Option<u16>) -> anyhow::Result<()> {
    let home = XelianHome::resolve()?;
    let config = load_config(&home)?;
    let port = port_flag.or(config.port).unwrap_or(DEFAULT_PORT);
    let url = format!("http://127.0.0.1:{port}/status");
    let resp = ureq::get(&url)
        .timeout(Duration::from_secs(3))
        .call()
        .map_err(|_| {
            anyhow::anyhow!(
                "gateway is not running on port {port} — start it with `xelian gateway serve`"
            )
        })?;
    let status: Value = resp.into_json()?;
    println!(
        "gateway v{} on port {port}",
        status["gateway"].as_str().unwrap_or("?")
    );
    let Some(backends) = status["backends"].as_array() else {
        return Ok(());
    };
    for b in backends {
        println!(
            "  {} v{}  {}  restarts:{}  tools:{}__*  log:{}",
            b["package"].as_str().unwrap_or("?"),
            b["version"].as_str().unwrap_or("?"),
            if b["alive"].as_bool().unwrap_or(false) {
                "up"
            } else {
                "DOWN"
            },
            b["restarts"],
            b["alias"].as_str().unwrap_or("?"),
            b["log"].as_str().unwrap_or("?"),
        );
    }
    Ok(())
}

pub fn cmd_logs(target: Option<&str>, lines: usize) -> anyhow::Result<()> {
    let home = XelianHome::resolve()?;
    let log_dir = home.logs().join("gateway");
    let paths: Vec<PathBuf> = match target {
        Some(t) => {
            let (owner, name) = parse_ref(t)?;
            vec![log_dir.join(format!("{owner}-{name}.log"))]
        }
        None => {
            let mut all = Vec::new();
            if log_dir.is_dir() {
                for entry in std::fs::read_dir(&log_dir)? {
                    let path = entry?.path();
                    if path.extension().is_some_and(|e| e == "log") {
                        all.push(path);
                    }
                }
            }
            all.sort();
            all
        }
    };
    anyhow::ensure!(
        !paths.is_empty(),
        "no gateway logs found under {}",
        log_dir.display()
    );
    for path in paths {
        if !path.is_file() {
            anyhow::bail!("no log at {} — has this backend ever run?", path.display());
        }
        let content = std::fs::read_to_string(&path)?;
        let tail: Vec<&str> = content.lines().rev().take(lines).collect();
        println!("==> {} <==", path.display());
        for line in tail.into_iter().rev() {
            println!("{line}");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_ref_accepts_owner_name_only() {
        assert!(parse_ref("demo/echo").is_ok());
        assert!(parse_ref("not-a-ref").is_err());
        assert!(parse_ref("https://github.com/a/b").is_err());
        assert!(parse_ref("../evil/pkg").is_err());
    }

    #[test]
    fn sanitize_alias_maps_unsafe_chars() {
        assert_eq!(sanitize_alias("my-server"), "my-server");
        assert_eq!(sanitize_alias("weird.name+x"), "weird-name-x");
    }

    #[test]
    fn mcp_initialize_and_unknown_method() {
        let backends: Backends = Arc::new(Vec::new());
        let resp = handle_mcp_message(
            serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
                "params": {"protocolVersion": "2025-03-26"}}),
            &backends,
        )
        .unwrap();
        assert_eq!(resp["result"]["protocolVersion"], "2025-03-26");
        assert_eq!(resp["result"]["serverInfo"]["name"], "xelian-gateway");

        let resp = handle_mcp_message(
            serde_json::json!({"jsonrpc": "2.0", "id": 2, "method": "resources/list"}),
            &backends,
        )
        .unwrap();
        assert_eq!(resp["error"]["code"], -32601);
    }

    #[test]
    fn notifications_get_no_response() {
        let backends: Backends = Arc::new(Vec::new());
        let resp = handle_mcp_message(
            serde_json::json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
            &backends,
        );
        assert!(resp.is_none());
    }

    #[test]
    fn tools_call_with_unprefixed_name_is_a_clear_error() {
        let backends: Backends = Arc::new(Vec::new());
        let resp = handle_mcp_message(
            serde_json::json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call",
                "params": {"name": "plain-tool"}}),
            &backends,
        )
        .unwrap();
        let msg = resp["error"]["message"].as_str().unwrap();
        assert!(msg.contains("<package>__<tool>"), "{msg}");
    }
}
