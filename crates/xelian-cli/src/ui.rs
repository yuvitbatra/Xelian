//! `xelian ui` — a small local control panel (design
//! `2026-07-25-smart-run-and-local-ui`).
//!
//! Serves a single self-contained page (embedded in this binary, no Next.js,
//! no network) from `127.0.0.1:<port>` with a tiny localhost JSON API: list the
//! packages you've cached, launch one in a terminal with a click, and
//! set/switch the API keys your agents use. This keeps the single-static-binary
//! and local-first principles intact — the panel ships inside `xelian` itself.

use serde_json::{json, Value};
use tiny_http::{Header, Request, Response, ResponseBox, Server};

use xelian_core::cache::{list_cached_packages, CachedPackage, PackageSource, XelianHome};
use xelian_core::manifest::Manifest;
use xelian_core::run::provider::Provider;
use xelian_core::secrets::SecretStore;

/// Default port — deliberately memorable and unlikely to clash (design 2026-07-25).
const DEFAULT_PORT: u16 = 2106;

/// `xelian ui [--port N]`: start the local control panel and serve until Ctrl-C.
pub fn cmd_ui(port: Option<u16>) -> anyhow::Result<()> {
    let port = port.unwrap_or(DEFAULT_PORT);
    let addr = format!("127.0.0.1:{port}");
    let server =
        Server::http(&addr).map_err(|e| anyhow::anyhow!("failed to bind http://{addr}: {e}"))?;
    let url = format!("http://{addr}");
    eprintln!("Xelian control panel → {url}  (Ctrl-C to stop)");
    open_browser(&url);

    // A personal, single-user panel: one request at a time is plenty.
    for request in server.incoming_requests() {
        let response = route(request);
        // `route` consumes the request body; it returns the response to send.
        if let Some((req, resp)) = response {
            let _ = req.respond(resp);
        }
    }
    Ok(())
}

/// Route a request, returning the originating request paired with its response
/// (the request is threaded back so the caller owns the `respond` call).
fn route(mut request: Request) -> Option<(Request, ResponseBox)> {
    let method = request.method().as_str().to_string();
    let url = request.url().to_string();
    // Strip any query string for matching.
    let path = url.split('?').next().unwrap_or(&url).to_string();

    let response = match (method.as_str(), path.as_str()) {
        ("GET", "/") => html_response(INDEX_HTML),
        ("GET", "/api/packages") => match packages_json() {
            Ok(v) => json_response(200, &v),
            Err(e) => json_response(500, &json!({ "error": e })),
        },
        ("GET", "/api/keys") => match keys_json() {
            Ok(v) => json_response(200, &v),
            Err(e) => json_response(500, &json!({ "error": e })),
        },
        ("POST", "/api/keys/set") => {
            let body = read_body(&mut request);
            handle_set_key(&body)
        }
        ("POST", "/api/keys/remove") => {
            let body = read_body(&mut request);
            handle_remove_key(&body)
        }
        ("POST", "/api/run") => {
            let body = read_body(&mut request);
            handle_run(&body)
        }
        _ => json_response(404, &json!({ "error": "not found" })),
    };
    Some((request, response))
}

fn read_body(request: &mut Request) -> Value {
    let mut body = String::new();
    if request.as_reader().read_to_string(&mut body).is_err() {
        return Value::Null;
    }
    serde_json::from_str(&body).unwrap_or(Value::Null)
}

/// Build the JSON list of cached packages, enriched from each manifest.
fn packages_json() -> Result<Value, String> {
    let home = XelianHome::resolve().map_err(|e| e.to_string())?;
    let packages = list_cached_packages(&home).map_err(|e| e.to_string())?;
    let items: Vec<Value> = packages.iter().map(package_to_json).collect();
    Ok(json!({ "packages": items }))
}

fn package_to_json(pkg: &CachedPackage) -> Value {
    let (source, run_command) = match &pkg.source {
        PackageSource::Registry { owner } => {
            ("registry", format!("xelian run {owner}/{}", pkg.name))
        }
        PackageSource::Github { owner, repo } => (
            "github",
            format!("xelian run https://github.com/{owner}/{repo}"),
        ),
        PackageSource::Local => ("local", format!("xelian run {}", pkg.name)),
    };

    // Best-effort manifest enrichment (type + description) — never fatal.
    let (ptype, description) = std::fs::read_to_string(pkg.path.join("xelian.toml"))
        .ok()
        .and_then(|s| Manifest::from_toml_str(&s).ok())
        .map(|m| (m.package_type.to_string(), m.description))
        .unwrap_or_default();

    json!({
        "name": pkg.name,
        "version": pkg.version,
        "source": source,
        "type": ptype,
        "description": description,
        "run_command": run_command,
    })
}

/// Stored key names plus per-provider configured/free status.
fn keys_json() -> Result<Value, String> {
    let home = XelianHome::resolve().map_err(|e| e.to_string())?;
    let store = SecretStore::load(&home.secrets_path()).map_err(|e| e.to_string())?;
    let providers: Vec<Value> = Provider::all()
        .iter()
        .map(|p| {
            let configured = match p.key_var() {
                Some(kv) => store.get(kv).is_some(),
                None => true, // Ollama is always available (local, free).
            };
            json!({
                "id": p.display_name().to_lowercase(),
                "name": p.display_name(),
                "free": p.is_local_free(),
                "key_var": p.key_var(),
                "configured": configured,
            })
        })
        .collect();
    let names: Vec<&str> = store.names();
    Ok(json!({ "keys": names, "providers": providers }))
}

fn handle_set_key(body: &Value) -> ResponseBox {
    let name = body
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    let value = body.get("value").and_then(Value::as_str).unwrap_or("");
    if name.is_empty() || value.is_empty() {
        return json_response(400, &json!({ "error": "name and value are required" }));
    }
    let home = match XelianHome::resolve() {
        Ok(h) => h,
        Err(e) => return json_response(500, &json!({ "error": e.to_string() })),
    };
    let path = home.secrets_path();
    let mut store = match SecretStore::load(&path) {
        Ok(s) => s,
        Err(e) => return json_response(500, &json!({ "error": e.to_string() })),
    };
    store.set(name, value);
    if let Err(e) = store.save(&path) {
        return json_response(500, &json!({ "error": e.to_string() }));
    }
    json_response(200, &json!({ "ok": true }))
}

fn handle_remove_key(body: &Value) -> ResponseBox {
    let name = body
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    if name.is_empty() {
        return json_response(400, &json!({ "error": "name is required" }));
    }
    let home = match XelianHome::resolve() {
        Ok(h) => h,
        Err(e) => return json_response(500, &json!({ "error": e.to_string() })),
    };
    let path = home.secrets_path();
    let mut store = match SecretStore::load(&path) {
        Ok(s) => s,
        Err(e) => return json_response(500, &json!({ "error": e.to_string() })),
    };
    let removed = store.remove(name);
    if removed {
        if let Err(e) = store.save(&path) {
            return json_response(500, &json!({ "error": e.to_string() }));
        }
    }
    json_response(200, &json!({ "ok": true, "removed": removed }))
}

/// Launch a cached package in a new terminal window.
fn handle_run(body: &Value) -> ResponseBox {
    let name = body
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    if name.is_empty() {
        return json_response(400, &json!({ "error": "name is required" }));
    }
    let home = match XelianHome::resolve() {
        Ok(h) => h,
        Err(e) => return json_response(500, &json!({ "error": e.to_string() })),
    };
    let packages = match list_cached_packages(&home) {
        Ok(p) => p,
        Err(e) => return json_response(500, &json!({ "error": e.to_string() })),
    };
    let Some(pkg) = packages.iter().find(|p| p.name == name) else {
        return json_response(
            404,
            &json!({ "error": format!("no cached package named {name}") }),
        );
    };
    let command = package_to_json(pkg)["run_command"]
        .as_str()
        .unwrap_or("")
        .to_string();

    let launched = spawn_in_terminal(&command);
    json_response(
        200,
        &json!({
            "ok": true,
            "command": command,
            "launched": launched,
        }),
    )
}

/// Open a new terminal window running `command`. Returns whether it launched;
/// the UI always shows the command so a copy-paste fallback is available.
#[cfg(target_os = "macos")]
fn spawn_in_terminal(command: &str) -> bool {
    let escaped = command.replace('\\', "\\\\").replace('"', "\\\"");
    std::process::Command::new("osascript")
        .arg("-e")
        .arg(format!(
            "tell application \"Terminal\" to do script \"{escaped}\""
        ))
        .arg("-e")
        .arg("tell application \"Terminal\" to activate")
        .spawn()
        .is_ok()
}

#[cfg(not(target_os = "macos"))]
fn spawn_in_terminal(_command: &str) -> bool {
    // Other platforms: the UI shows the command for copy-paste. A portable
    // terminal-launch is out of scope for v1.
    false
}

/// Best-effort: open the panel in the user's default browser.
fn open_browser(url: &str) {
    #[cfg(target_os = "macos")]
    let program = "open";
    #[cfg(target_os = "linux")]
    let program = "xdg-open";
    #[cfg(target_os = "windows")]
    let program = "explorer";
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    let program = "";
    if program.is_empty() {
        return;
    }
    let _ = std::process::Command::new(program).arg(url).spawn();
}

fn json_response(status: u16, body: &Value) -> ResponseBox {
    Response::from_string(body.to_string())
        .with_status_code(status)
        .with_header(Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).unwrap())
        .boxed()
}

fn html_response(body: &str) -> ResponseBox {
    Response::from_string(body)
        .with_header(
            Header::from_bytes(&b"Content-Type"[..], &b"text/html; charset=utf-8"[..]).unwrap(),
        )
        .boxed()
}

/// The whole control panel: one self-contained page (inline CSS + JS, no
/// external requests), theme-aware.
const INDEX_HTML: &str = include_str!("ui/index.html");
