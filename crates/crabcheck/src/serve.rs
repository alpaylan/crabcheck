//! `crabcheck serve`: a local page over what test runs leave behind.
//!
//! Point it at the directory the runs write to (`.crabcheck` by default) and
//! it serves the coverage grid pages found in `coverage/`, the latest
//! report per property from `reports.jsonl`, and the targets in
//! `targets.jsonl`. A grid page served this way gains an "Add to
//! targets.jsonl" button, and every page reloads when the files change, so
//! the loop is: pick a region in the grid, add it as a target, rerun the
//! tests (or let an agent watching `targets.jsonl` rewrite the generator
//! first), and watch the target's outcome appear.
//!
//! ```text
//! CRABCHECK_COVERAGE_HTML=.crabcheck/coverage CRABCHECK_REPORT_JSON=.crabcheck/reports.jsonl \
//!   RUSTFLAGS="-C instrument-coverage" RUST_TEST_THREADS=1 cargo test --features coverage
//! cargo run --features serve --bin crabcheck -- serve      # then open http://127.0.0.1:7878/
//! ```
//!
//! The server binds to `127.0.0.1` only, speaks plain HTTP/1.1 from the
//! standard library, and never runs anything: it reads and writes files.

use std::{
    collections::{
        BTreeMap,
        hash_map::DefaultHasher,
    },
    fs,
    hash::{
        Hash,
        Hasher,
    },
    io::{
        self,
        BufRead,
        BufReader,
        Read,
        Write,
    },
    net::{
        SocketAddr,
        TcpListener,
        TcpStream,
    },
    path::{
        Path,
        PathBuf,
    },
    sync::{
        Arc,
        atomic::{
            AtomicBool,
            Ordering,
        },
    },
    thread::{
        self,
        JoinHandle,
    },
    time::Duration,
};

use crate::config::{
    CoverageTarget,
    load_targets,
};

/// Where to look and where to listen.
#[derive(Clone, Debug)]
pub struct Options {
    /// The directory test runs write to. Default `.crabcheck`.
    pub dir: PathBuf,
    /// TCP port on 127.0.0.1; 0 picks a free one. Default 7878.
    pub port: u16,
}

impl Default for Options {
    fn default() -> Self {
        Options { dir: PathBuf::from(".crabcheck"), port: 7878 }
    }
}

/// A running server; dropping it asks the accept loop to stop.
pub struct Server {
    addr: SocketAddr,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Server {
    /// The bound address.
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// The page's URL.
    pub fn url(&self) -> String {
        format!("http://{}/", self.addr)
    }

    /// Stop accepting and wait for the accept loop.
    pub fn stop(mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

struct State {
    dir: PathBuf,
    stop: Arc<AtomicBool>,
}

/// Start serving in a background thread.
pub fn start(options: Options) -> io::Result<Server> {
    let listener = TcpListener::bind(("127.0.0.1", options.port))?;
    listener.set_nonblocking(true)?;
    let addr = listener.local_addr()?;
    let stop = Arc::new(AtomicBool::new(false));
    let state = Arc::new(State { dir: options.dir, stop: stop.clone() });
    let thread = thread::spawn(move || {
        while !state.stop.load(Ordering::Relaxed) {
            match listener.accept() {
                Ok((stream, _)) => {
                    let state = state.clone();
                    thread::spawn(move || {
                        let _ = stream.set_nonblocking(false);
                        let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
                        let _ = handle(stream, &state);
                    });
                },
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(20));
                },
                Err(_) => break,
            }
        }
    });
    Ok(Server { addr, stop, thread: Some(thread) })
}

struct Request {
    method: String,
    path: String,
    body: Vec<u8>,
}

fn read_request(reader: &mut BufReader<TcpStream>) -> io::Result<Option<Request>> {
    let mut line = String::new();
    if reader.read_line(&mut line)? == 0 {
        return Ok(None);
    }
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let target = parts.next().unwrap_or("/");
    let path = target.split('?').next().unwrap_or("/").to_string();
    let mut content_length = 0usize;
    loop {
        line.clear();
        if reader.read_line(&mut line)? == 0 || line.trim().is_empty() {
            break;
        }
        if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
            content_length = value.trim().parse().unwrap_or(0);
        }
    }
    if content_length > 1 << 20 {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "body too large"));
    }
    let mut body = vec![0; content_length];
    reader.read_exact(&mut body)?;
    Ok(Some(Request { method, path, body }))
}

fn respond(stream: &mut TcpStream, status: u16, content_type: &str, body: &[u8]) -> io::Result<()> {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        405 => "Method Not Allowed",
        _ => "Error",
    };
    write!(
        stream,
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
        body.len()
    )?;
    stream.write_all(body)?;
    stream.flush()
}

fn handle(stream: TcpStream, state: &State) -> io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut stream = stream;
    let Some(request) = read_request(&mut reader)? else {
        return Ok(());
    };
    match (request.method.as_str(), request.path.as_str()) {
        ("GET", "/") => {
            respond(&mut stream, 200, "text/html; charset=utf-8", index(&state.dir).as_bytes())
        },
        ("GET", "/api/state") => {
            respond(
                &mut stream,
                200,
                "application/json",
                snapshot(&state.dir).to_string().as_bytes(),
            )
        },
        ("GET", "/api/events") => events(stream, state),
        ("GET", path) if path.starts_with("/grid/") => {
            let name = &path["/grid/".len()..];
            let safe = !name.is_empty()
                && !name.contains(['/', '\\'])
                && !name.contains("..")
                && name.ends_with(".html");
            match safe.then(|| fs::read(state.dir.join("coverage").join(name))) {
                Some(Ok(page)) => respond(&mut stream, 200, "text/html; charset=utf-8", &page),
                _ => respond(&mut stream, 404, "text/plain", b"no such page"),
            }
        },
        ("POST", "/api/targets") => {
            match parse_target(&request.body) {
                Ok(target) => {
                    let path = state.dir.join("targets.jsonl");
                    fs::create_dir_all(&state.dir)?;
                    let mut file = fs::OpenOptions::new().create(true).append(true).open(&path)?;
                    writeln!(file, "{}", serde_json::to_string(&target).unwrap_or_default())?;
                    let count = load_targets(&path).map(|t| t.len()).unwrap_or(0);
                    respond(
                        &mut stream,
                        200,
                        "application/json",
                        format!("{{\"targets\":{count}}}").as_bytes(),
                    )
                },
                Err(e) => respond(&mut stream, 400, "text/plain", e.as_bytes()),
            }
        },
        ("POST", "/api/targets/clear") => {
            fs::create_dir_all(&state.dir)?;
            fs::write(state.dir.join("targets.jsonl"), "")?;
            respond(&mut stream, 200, "application/json", b"{\"targets\":0}")
        },
        ("GET", _) => respond(&mut stream, 404, "text/plain", b"not found"),
        _ => respond(&mut stream, 405, "text/plain", b"method not allowed"),
    }
}

fn parse_target(body: &[u8]) -> Result<CoverageTarget, String> {
    let target: CoverageTarget = serde_json::from_slice(body)
        .map_err(|e| format!("expected {{\"file\",\"begin_line\",\"end_line\"}}: {e}"))?;
    if target.file.trim().is_empty() || target.file.contains(['\n', '\r']) {
        return Err("file must be a single non-empty line".to_string());
    }
    if target.begin_line == 0 || target.begin_line > target.end_line {
        return Err("lines must satisfy 1 <= begin_line <= end_line".to_string());
    }
    Ok(target)
}

/// Server-sent events: one line naming what changed (`pages`, `targets`,
/// `reports`) whenever the watched files change, with keepalives.
fn events(mut stream: TcpStream, state: &State) -> io::Result<()> {
    stream.write_all(
        b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-store\r\nConnection: keep-alive\r\n\r\n",
    )?;
    stream.write_all(b": connected\n\n")?;
    stream.flush()?;
    let mut last = fingerprints(&state.dir);
    let mut ticks = 0u32;
    while !state.stop.load(Ordering::Relaxed) {
        thread::sleep(Duration::from_millis(300));
        let now = fingerprints(&state.dir);
        for (what, digest) in &now {
            if last.get(what) != Some(digest) {
                stream.write_all(format!("data: {what}\n\n").as_bytes())?;
                stream.flush()?;
            }
        }
        last = now;
        ticks += 1;
        if ticks.is_multiple_of(50) {
            stream.write_all(b": keepalive\n\n")?;
            stream.flush()?;
        }
    }
    Ok(())
}

/// A digest per watched group: `pages` (coverage/*.html), `targets`, `reports`.
fn fingerprints(dir: &Path) -> BTreeMap<&'static str, u64> {
    fn stamp(hasher: &mut DefaultHasher, path: &Path) {
        if let Ok(meta) = fs::metadata(path) {
            path.hash(hasher);
            meta.len().hash(hasher);
            if let Ok(modified) = meta.modified() {
                modified.hash(hasher);
            }
        }
    }
    let mut out = BTreeMap::new();
    let mut pages = DefaultHasher::new();
    for page in grid_pages(dir) {
        stamp(&mut pages, &page.path);
    }
    out.insert("pages", pages.finish());
    let mut targets = DefaultHasher::new();
    stamp(&mut targets, &dir.join("targets.jsonl"));
    out.insert("targets", targets.finish());
    let mut reports = DefaultHasher::new();
    stamp(&mut reports, &dir.join("reports.jsonl"));
    out.insert("reports", reports.finish());
    out
}

struct Page {
    name: String,
    path: PathBuf,
    modified: u64,
}

fn grid_pages(dir: &Path) -> Vec<Page> {
    let mut pages: Vec<Page> = fs::read_dir(dir.join("coverage"))
        .map(|entries| {
            entries
                .flatten()
                .filter_map(|entry| {
                    let path = entry.path();
                    let name = path.file_name()?.to_str()?.strip_suffix(".html")?.to_string();
                    let modified = entry
                        .metadata()
                        .ok()?
                        .modified()
                        .ok()?
                        .duration_since(std::time::UNIX_EPOCH)
                        .ok()?
                        .as_secs();
                    Some(Page { name, path, modified })
                })
                .collect()
        })
        .unwrap_or_default();
    pages.sort_by(|a, b| b.modified.cmp(&a.modified).then_with(|| a.name.cmp(&b.name)));
    pages
}

/// The latest report per property from `reports.jsonl`.
fn latest_reports(dir: &Path) -> BTreeMap<String, serde_json::Value> {
    let mut out = BTreeMap::new();
    let Ok(text) = fs::read_to_string(dir.join("reports.jsonl")) else {
        return out;
    };
    for line in text.lines() {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(line)
            && let Some(name) = value.get("name").and_then(|n| n.as_str())
        {
            out.insert(name.to_string(), value);
        }
    }
    out
}

/// Everything the pages need, as JSON.
fn snapshot(dir: &Path) -> serde_json::Value {
    use serde_json::json;
    let reports = latest_reports(dir);
    let pages: Vec<serde_json::Value> = grid_pages(dir)
        .iter()
        .map(|page| {
            let report = reports.get(&page.name);
            json!({
                "name": page.name,
                "page": format!("/grid/{}.html", page.name),
                "modified": page.modified,
                "report": report,
            })
        })
        .collect();
    let targets = load_targets(&dir.join("targets.jsonl")).unwrap_or_default();
    json!({
        "dir": dir.display().to_string(),
        "pages": pages,
        "targets": targets,
        "reports": reports.values().collect::<Vec<_>>(),
    })
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// The index page.
fn index(dir: &Path) -> String {
    let reports = latest_reports(dir);
    let pages = grid_pages(dir);
    let targets = load_targets(&dir.join("targets.jsonl")).unwrap_or_default();
    let mut rows = String::new();
    for page in &pages {
        let report = reports.get(&page.name);
        let field =
            |key: &str| report.and_then(|r| r.get(key)).cloned().unwrap_or(serde_json::Value::Null);
        let status = field("status").as_str().unwrap_or("–").to_string();
        let tests = field("tests").as_u64().map_or("–".to_string(), |t| t.to_string());
        let unique =
            field("unique_tests").as_u64().map_or(String::new(), |u| format!(" ({u} distinct)"));
        let warnings = field("health").as_array().map_or(0, Vec::len);
        let coverage = report
            .and_then(|r| r.get("coverage"))
            .and_then(|c| {
                Some(format!(
                    "{} of {} functions",
                    c.get("functions_reached")?.as_u64()?,
                    c.get("functions_total")?.as_u64()?
                ))
            })
            .unwrap_or_else(|| "–".to_string());
        rows.push_str(&format!(
            "<tr><td><a href=\"/grid/{n}.html\">{n}</a></td><td class=\"s-{}\">{}</td><td>{tests}{unique}</td><td>{coverage}</td><td>{warnings}</td><td>{}</td></tr>",
            status.to_ascii_lowercase(),
            escape(&status),
            crate::report::civil_time(page.modified),
            n = escape(&page.name),
        ));
    }
    if rows.is_empty() {
        rows = format!(
            "<tr><td colspan=\"6\" class=\"muted\">No coverage pages under {}/coverage yet. Run the tests with <code>CRABCHECK_COVERAGE_HTML={}/coverage</code> on an instrumented build.</td></tr>",
            escape(&dir.display().to_string()),
            escape(&dir.display().to_string())
        );
    }
    let mut target_items = String::new();
    for target in &targets {
        target_items.push_str(&format!("<li><code>{}</code></li>", escape(&target.to_string())));
    }
    if target_items.is_empty() {
        target_items =
            "<li class=\"muted\">none; open a page and select lines to add one.</li>".to_string();
    }
    format!(
        r#"<!doctype html>
<html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1">
<title>crabcheck serve</title>
<style>
:root {{ --bg:#f4f5f8; --panel:#fff; --ink:#1b2330; --muted:#69737f; --line:#dfe3ea; --accent:#2f6fed; --bad:#c62828; --mono: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace; }}
@media (prefers-color-scheme: dark) {{ :root {{ --bg:#11151b; --panel:#181d25; --ink:#e4e8ee; --muted:#96a1b0; --line:#2a3240; --accent:#7ea3ff; --bad:#ef6b6b; }} }}
body {{ margin:0; font-family: system-ui, -apple-system, "Segoe UI", Roboto, sans-serif; background: var(--bg); color: var(--ink); font-size: 14px; }}
header {{ padding: 12px 20px; border-bottom: 1px solid var(--line); background: var(--panel); display: flex; gap: 16px; align-items: baseline; }}
h1 {{ font-size: 15px; margin: 0; font-weight: 650; }} h1 code, td code, li code {{ font-family: var(--mono); font-weight: 500; }}
main {{ max-width: 1100px; margin: 0 auto; padding: 20px; display: grid; gap: 20px; }}
section {{ background: var(--panel); border: 1px solid var(--line); border-radius: 6px; padding: 14px 16px; }}
h2 {{ font-size: 12px; text-transform: uppercase; letter-spacing: .05em; color: var(--muted); margin: 0 0 10px; }}
table {{ border-collapse: collapse; width: 100%; }} th, td {{ text-align: left; padding: 6px 8px; border-bottom: 1px solid var(--line); font-variant-numeric: tabular-nums; }} th {{ color: var(--muted); font-weight: 500; font-size: 12px; }}
a {{ color: var(--accent); }} .muted {{ color: var(--muted); }} .s-failed {{ color: var(--bad); font-weight: 600; }} .s-passed {{ color: var(--accent); }}
ul {{ margin: 0; padding-left: 18px; }} li {{ margin: 4px 0; }}
button {{ font: inherit; font-size: 13px; padding: 5px 10px; border: 1px solid var(--line); background: var(--panel); color: var(--ink); border-radius: 4px; cursor: pointer; }}
p.how {{ color: var(--muted); font-size: 13px; margin: 8px 0 0; }} p.how code {{ font-family: var(--mono); }}
</style></head>
<body>
<header><h1>crabcheck serve · <code>{dir}</code></h1><span class="muted">pages reload when the files change</span></header>
<main>
<section><h2>Properties</h2>
<table><thead><tr><th>property</th><th>status</th><th>tests</th><th>coverage</th><th>warnings</th><th>page written</th></tr></thead><tbody>{rows}</tbody></table>
<p class="how">Reports come from <code>{dir}/reports.jsonl</code> (<code>CRABCHECK_REPORT_JSON</code>); pages from <code>{dir}/coverage</code> (<code>CRABCHECK_COVERAGE_HTML</code>).</p>
</section>
<section><h2>Targets in <code>{dir}/targets.jsonl</code></h2><ul>{target_items}</ul>
<p class="how">The next run steers toward these and reports whether each was reached. An agent watching the file can rewrite the generator first.</p>
<p><button id="clear">Clear targets</button></p>
</section>
</main>
<script>
document.getElementById("clear").onclick = function () {{ fetch("/api/targets/clear", {{method: "POST"}}).then(function () {{ location.reload(); }}); }};
try {{ new EventSource("/api/events").onmessage = function () {{ location.reload(); }}; }} catch (e) {{}}
</script>
</body></html>
"#,
        dir = escape(&dir.display().to_string()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(addr: SocketAddr, raw: &str) -> (u16, String) {
        let mut stream = TcpStream::connect(addr).unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        stream.write_all(raw.as_bytes()).unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        let status = response.split_whitespace().nth(1).unwrap().parse().unwrap();
        let body = response.split("\r\n\r\n").nth(1).unwrap_or("").to_string();
        (status, body)
    }

    #[test]
    fn serves_pages_state_targets_and_events() {
        let dir = std::env::temp_dir().join(format!("crabcheck-serve-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("coverage")).unwrap();
        fs::write(dir.join("coverage/demo.html"), "<html>demo page</html>").unwrap();
        fs::write(
            dir.join("reports.jsonl"),
            "{\"name\":\"demo\",\"status\":\"Passed\",\"tests\":5,\"unique_tests\":4,\"health\":[],\"coverage\":{\"functions_reached\":1,\"functions_total\":2}}\n",
        )
        .unwrap();
        let server = start(Options { dir: dir.clone(), port: 0 }).unwrap();
        let addr = server.addr();

        let (status, body) = request(addr, "GET / HTTP/1.1\r\nHost: x\r\n\r\n");
        assert_eq!(status, 200);
        assert!(body.contains("/grid/demo.html") && body.contains("1 of 2 functions"), "{body}");
        assert!(body.contains("none; open a page"), "{body}");

        let (status, body) = request(addr, "GET /grid/demo.html HTTP/1.1\r\n\r\n");
        assert_eq!((status, body.as_str()), (200, "<html>demo page</html>"));
        assert_eq!(request(addr, "GET /grid/../Cargo.toml HTTP/1.1\r\n\r\n").0, 404);
        assert_eq!(request(addr, "GET /grid/nope.html HTTP/1.1\r\n\r\n").0, 404);
        assert_eq!(request(addr, "GET /nowhere HTTP/1.1\r\n\r\n").0, 404);
        assert_eq!(request(addr, "DELETE / HTTP/1.1\r\n\r\n").0, 405);

        let body = "{\"file\":\"src/lib.rs\",\"begin_line\":3,\"end_line\":4}";
        let (status, reply) = request(
            addr,
            &format!("POST /api/targets HTTP/1.1\r\nContent-Length: {}\r\n\r\n{body}", body.len()),
        );
        assert_eq!((status, reply.as_str()), (200, "{\"targets\":1}"));
        assert_eq!(
            load_targets(&dir.join("targets.jsonl")).unwrap(),
            vec![CoverageTarget::new("src/lib.rs", 3..=4)]
        );
        let bad = "{\"file\":\"\",\"begin_line\":9,\"end_line\":4}";
        assert_eq!(
            request(
                addr,
                &format!(
                    "POST /api/targets HTTP/1.1\r\nContent-Length: {}\r\n\r\n{bad}",
                    bad.len()
                )
            )
            .0,
            400
        );
        let (_, state) = request(addr, "GET /api/state HTTP/1.1\r\n\r\n");
        let state: serde_json::Value = serde_json::from_str(&state).unwrap();
        assert_eq!(state["targets"][0]["file"], "src/lib.rs");
        assert_eq!(state["pages"][0]["report"]["tests"], 5);
        assert!(request(addr, "GET / HTTP/1.1\r\n\r\n").1.contains("src/lib.rs:3-4"));
        assert_eq!(request(addr, "POST /api/targets/clear HTTP/1.1\r\n\r\n").0, 200);
        assert_eq!(fs::read_to_string(dir.join("targets.jsonl")).unwrap(), "");

        // Events: connect, change a page, expect a `pages` event.
        let mut stream = TcpStream::connect(addr).unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        stream.write_all(b"GET /api/events HTTP/1.1\r\n\r\n").unwrap();
        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        assert!(line.starts_with("HTTP/1.1 200"), "{line}");
        thread::sleep(Duration::from_millis(400));
        fs::write(dir.join("coverage/other.html"), "<html>other</html>").unwrap();
        let mut saw = String::new();
        for _ in 0..40 {
            line.clear();
            if reader.read_line(&mut line).is_err() {
                break;
            }
            if line.starts_with("data:") {
                saw = line.trim().to_string();
                break;
            }
        }
        assert_eq!(saw, "data: pages");
        drop(reader);
        server.stop();
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn targets_are_validated() {
        assert!(parse_target(b"{\"file\":\"a.rs\",\"begin_line\":1,\"end_line\":1}").is_ok());
        assert!(parse_target(b"{\"file\":\"a.rs\",\"begin_line\":0,\"end_line\":1}").is_err());
        assert!(parse_target(b"{\"file\":\"a\\nb\",\"begin_line\":1,\"end_line\":1}").is_err());
        assert!(parse_target(b"nope").is_err());
    }
}
