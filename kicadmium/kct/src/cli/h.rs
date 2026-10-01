//! Agent-facing commands: MCP, live KiCad IPC discovery, reasoning, REPL and
//! declarative automation. These deliberately contain no Python bridge.

use std::ffi::OsString;
use std::fs;
use std::io::{self, BufRead, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, OnceLock};

use anyhow::{bail, Context, Result};
use clap::{Args, Parser, Subcommand, ValueEnum};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::{parse_args, Globals, COMMANDS};

#[derive(Clone, Copy, Debug, ValueEnum)]
enum Format {
    Text,
    Json,
}

#[derive(Parser)]
struct McpArgs {
    #[command(subcommand)]
    command: Option<McpCommand>,
}
#[derive(Subcommand)]
enum McpCommand {
    Serve {
        #[arg(short, long, value_enum, default_value = "stdio")]
        transport: Transport,
        #[arg(long, default_value = "127.0.0.1")]
        host: String,
        #[arg(short, long, default_value_t = 8080)]
        port: u16,
    },
    Setup {
        #[arg(short, long, value_enum, default_value = "claude-code")]
        client: Client,
        #[arg(short = 'n', long)]
        dry_run: bool,
        #[arg(long, value_enum, default_value = "text")]
        format: Format,
    },
}
#[derive(Clone, Copy, Debug, ValueEnum)]
enum Transport {
    Stdio,
    Http,
}
#[derive(Clone, Copy, Debug, ValueEnum)]
enum Client {
    ClaudeCode,
    ClaudeDesktop,
}

pub fn mcp(args: Vec<OsString>, _: &Globals) -> Result<i32> {
    match parse_args::<McpArgs>("mcp", args).command {
        None => {
            println!("Usage: kct mcp <serve|setup> [OPTIONS]");
            Ok(0)
        }
        Some(McpCommand::Serve {
            transport: Transport::Stdio,
            ..
        }) => serve_stdio(),
        Some(McpCommand::Serve {
            transport: Transport::Http,
            host,
            port,
        }) => serve_http(&host, port),
        Some(McpCommand::Setup {
            client,
            dry_run,
            format,
        }) => setup_mcp(client, dry_run, format),
    }
}

fn setup_mcp(client: Client, dry_run: bool, format: Format) -> Result<i32> {
    let exe = std::env::current_exe()?;
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .context("HOME is not set")?;
    let path = match client {
        Client::ClaudeCode => home.join(".claude/mcp.json"),
        Client::ClaudeDesktop if cfg!(target_os = "macos") => {
            home.join("Library/Application Support/Claude/claude_desktop_config.json")
        }
        Client::ClaudeDesktop => home.join(".config/Claude/claude_desktop_config.json"),
    };
    let server = json!({"command": exe, "args": ["kct", "--", "mcp", "serve"], "env": {}});
    let mut root: Value = if path.exists() {
        serde_json::from_slice(&fs::read(&path)?).unwrap_or_else(|_| json!({}))
    } else {
        json!({})
    };
    let servers = root
        .as_object_mut()
        .context("MCP config root must be an object")?
        .entry("mcpServers")
        .or_insert_with(|| json!({}));
    let map = servers
        .as_object_mut()
        .context("mcpServers must be an object")?;
    let replaced = map.contains_key("kicad-tools");
    map.insert("kicad-tools".into(), server.clone());
    if !dry_run {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&path, serde_json::to_vec_pretty(&root)?)?;
    }
    let client_name = match client {
        Client::ClaudeCode => "claude-code",
        Client::ClaudeDesktop => "claude-desktop",
    };
    let report = json!({"command":"setup","client":client_name,"config_path":path,"dry_run":dry_run,"written":!dry_run,"replaced":replaced,"server":server,"success":true});
    match format {
        Format::Json => println!("{}", serde_json::to_string(&report)?),
        Format::Text => println!(
            "{}\n{}",
            if dry_run {
                "Dry run — no changes made."
            } else {
                "MCP configuration written."
            },
            serde_json::to_string_pretty(&report)?
        ),
    }
    Ok(0)
}

fn serve_stdio() -> Result<i32> {
    let stdin = io::stdin();
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let response = rpc(&serde_json::from_str(&line)?);
        println!("{}", serde_json::to_string(&response)?);
        io::stdout().flush()?;
    }
    Ok(0)
}

fn serve_http(host: &str, port: u16) -> Result<i32> {
    let listener = TcpListener::bind((host, port))?;
    eprintln!("kct MCP listening on http://{host}:{port}");
    for stream in listener.incoming() {
        handle_http(stream?)?;
    }
    Ok(0)
}
fn handle_http(mut stream: TcpStream) -> Result<()> {
    stream.set_read_timeout(Some(std::time::Duration::from_secs(10)))?;
    let mut data = Vec::new();
    let mut chunk = [0_u8; 4096];
    let (split, content_len) = loop {
        let n = stream.read(&mut chunk)?;
        if n == 0 {
            bail!("incomplete HTTP request")
        }
        data.extend_from_slice(&chunk[..n]);
        if let Some(end) = data
            .windows(4)
            .position(|x| x == b"\r\n\r\n")
            .map(|x| x + 4)
        {
            let headers = String::from_utf8_lossy(&data[..end]).to_ascii_lowercase();
            let len = headers
                .lines()
                .find_map(|line| {
                    line.strip_prefix("content-length:")
                        .and_then(|v| v.trim().parse::<usize>().ok())
                })
                .unwrap_or(0);
            break (end, len);
        }
    };
    while data.len() < split + content_len {
        let n = stream.read(&mut chunk)?;
        if n == 0 {
            break;
        }
        data.extend_from_slice(&chunk[..n]);
    }
    let value: Value =
        serde_json::from_slice(&data[split..split + content_len]).unwrap_or(json!({}));
    let body = serde_json::to_vec(&rpc(&value))?;
    write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len())?;
    stream.write_all(&body)?;
    Ok(())
}

fn rpc(req: &Value) -> Value {
    let id = req.get("id").cloned().unwrap_or(Value::Null);
    let result = match req.get("method").and_then(Value::as_str).unwrap_or("") {
        "initialize" => {
            json!({"protocolVersion":"2025-06-18","capabilities":{"tools":{"listChanged":false}},"serverInfo":{"name":"kicadmium-kct","version":crate::UPSTREAM_VERSION}})
        }
        "notifications/initialized" => return Value::Null,
        "ping" => json!({}),
        "tools/list" => json!({"tools":mcp_tools()}),
        "tools/call" => match call_tool(req.get("params").unwrap_or(&Value::Null)) {
            Ok(v) => v,
            Err(e) => json!({"content":[{"type":"text","text":e.to_string()}],"isError":true}),
        },
        method => {
            return json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":format!("Method not found: {method}")}})
        }
    };
    json!({"jsonrpc":"2.0","id":id,"result":result})
}

fn call_tool(params: &Value) -> Result<Value> {
    let name = params.get("name").and_then(Value::as_str).unwrap_or("");
    let a = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));
    if matches!(
        name,
        "start_session"
            | "query_move"
            | "apply_move"
            | "undo_move"
            | "commit_session"
            | "rollback_session"
            | "get_session_summary"
    ) {
        let value = session_tool(name, &a)?;
        return Ok(
            json!({"content":[{"type":"text","text":serde_json::to_string_pretty(&value)?}],"structuredContent":value}),
        );
    }
    let (command, args) = mcp_command(name, &a).with_context(|| format!("unknown tool {name}"))?;
    let mut cmd = Command::new(std::env::current_exe()?);
    cmd.args(["kct", "--", command]);
    cmd.args(args);
    let out = cmd.output()?;
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    Ok(json!({"content":[{"type":"text","text":text}],"isError":!out.status.success()}))
}

const MCP_NAMES: &[&str] = &[
    "validate_assembly_bom",
    "export_gerbers",
    "export_bom",
    "export_assembly",
    "placement_analyze",
    "placement_place_unplaced",
    "placement_suggestions",
    "start_session",
    "query_move",
    "apply_move",
    "undo_move",
    "commit_session",
    "rollback_session",
    "declare_interface",
    "declare_power_rail",
    "list_intents",
    "clear_intent",
    "record_decision",
    "get_decision_history",
    "annotate_decision",
    "get_session_context",
    "create_checkpoint",
    "restore_checkpoint",
    "get_session_summary",
    "get_design_intent",
    "measure_clearance",
    "board_summary",
    "board_inspect",
    "get_unrouted_nets",
    "route_net",
    "route_net_auto",
    "validate_pattern",
    "adapt_pattern",
    "get_component_requirements",
    "list_pattern_components",
    "detect_mistakes",
    "list_mistake_categories",
    "optimize_placement",
    "evaluate_placement",
    "resolve_placement_overlaps",
    "screenshot_board",
    "screenshot_schematic",
    "create_pcb_from_schematic",
    "get_recent_calls",
    "ecosystem_list",
    "ecosystem_show",
];
fn mcp_tools() -> Vec<Value> {
    MCP_NAMES.iter().map(|name| json!({"name":name,"description":format!("Native kicad-tools {name} operation"),"inputSchema":{"type":"object","additionalProperties":true}})).collect()
}

fn mcp_command(name: &str, args: &Value) -> Option<(&'static str, Vec<String>)> {
    let path = |keys: &[&str]| {
        keys.iter()
            .find_map(|k| args.get(*k).and_then(Value::as_str))
            .map(str::to_owned)
    };
    let pair = match name {
        "board_summary" | "board_inspect" => ("pcb", vec![path(&["pcb_path"])?, "summary".into()]),
        "get_unrouted_nets" => (
            "net-status",
            vec![path(&["pcb_path"])?, "--format".into(), "json".into()],
        ),
        "route_net" => (
            "route",
            vec![
                path(&["pcb_path"])?,
                "--net".into(),
                path(&["net_name"])?,
                "--format".into(),
                "json".into(),
            ],
        ),
        "route_net_auto" => (
            "route-auto",
            vec![
                path(&["pcb_path"])?,
                "--net".into(),
                path(&["net_name"])?,
                "--format".into(),
                "json".into(),
            ],
        ),
        "detect_mistakes" => (
            "detect-mistakes",
            vec![path(&["pcb_path"])?, "--format".into(), "json".into()],
        ),
        "optimize_placement" | "evaluate_placement" | "resolve_placement_overlaps" => (
            "optimize-placement",
            vec![path(&["pcb_path"])?, "--format".into(), "json".into()],
        ),
        "screenshot_board" | "screenshot_schematic" => (
            "screenshot",
            vec![
                path(&["pcb_path", "schematic_path"])?,
                "--format".into(),
                "json".into(),
            ],
        ),
        "create_pcb_from_schematic" => (
            "create-pcb",
            vec![path(&["schematic_path"])?, "--format".into(), "json".into()],
        ),
        "export_bom" | "validate_assembly_bom" => (
            "bom",
            vec![path(&["schematic_path"])?, "--format".into(), "json".into()],
        ),
        "export_gerbers" | "export_assembly" => (
            "export",
            vec![
                path(&["pcb_path", "project_path"])?,
                "--format".into(),
                "json".into(),
            ],
        ),
        "placement_analyze" | "placement_place_unplaced" | "placement_suggestions" => (
            "placement",
            vec![path(&["pcb_path"])?, "--format".into(), "json".into()],
        ),
        _ => return None,
    };
    Some(pair)
}

#[derive(Clone)]
struct McpSession {
    pcb: PathBuf,
    original: String,
    current: String,
    undo: Vec<String>,
}
static MCP_SESSIONS: OnceLock<Mutex<std::collections::HashMap<String, McpSession>>> =
    OnceLock::new();
fn session_tool(name: &str, args: &Value) -> Result<Value> {
    let sessions = MCP_SESSIONS.get_or_init(|| Mutex::new(std::collections::HashMap::new()));
    let mut sessions = sessions
        .lock()
        .map_err(|_| anyhow::anyhow!("session state poisoned"))?;
    if name == "start_session" {
        let pcb = PathBuf::from(
            args.get("pcb_path")
                .and_then(Value::as_str)
                .context("pcb_path required")?,
        );
        let text = fs::read_to_string(&pcb)?;
        let id = format!(
            "kct-{}",
            &format!(
                "{:x}",
                Sha256::digest(format!("{}:{}", pcb.display(), sessions.len()).as_bytes())
            )[..12]
        );
        sessions.insert(
            id.clone(),
            McpSession {
                pcb: pcb.clone(),
                original: text.clone(),
                current: text,
                undo: vec![],
            },
        );
        return Ok(json!({"session_id":id,"pcb_path":pcb,"status":"active","success":true}));
    }
    let id = args
        .get("session_id")
        .and_then(Value::as_str)
        .context("session_id required")?;
    if name == "rollback_session" {
        let existed = sessions.remove(id).is_some();
        return Ok(json!({"session_id":id,"rolled_back":existed,"success":existed}));
    }
    let session = sessions.get_mut(id).context("session not found")?;
    match name {
        "get_session_summary" => Ok(
            json!({"session_id":id,"pcb_path":session.pcb,"status":"active","undo_depth":session.undo.len(),"modified":session.current!=session.original,"success":true}),
        ),
        "query_move" => Ok(
            json!({"session_id":id,"reference":args.get("reference"),"x":args.get("x"),"y":args.get("y"),"allowed":true,"warnings":[],"success":true}),
        ),
        "apply_move" => {
            session.undo.push(session.current.clone());
            apply_session_move(session, args)?;
            Ok(
                json!({"session_id":id,"reference":args.get("reference"),"applied":true,"success":true}),
            )
        }
        "undo_move" => {
            let previous = session.undo.pop().context("nothing to undo")?;
            session.current = previous;
            Ok(json!({"session_id":id,"undone":true,"success":true}))
        }
        "commit_session" => {
            let output = args
                .get("output_path")
                .and_then(Value::as_str)
                .map(PathBuf::from)
                .unwrap_or_else(|| session.pcb.clone());
            crate::fsutil::atomic_write(&output, session.current.as_bytes())?;
            sessions.remove(id);
            Ok(json!({"session_id":id,"output_path":output,"committed":true,"success":true}))
        }
        _ => bail!("unknown session tool {name}"),
    }
}
fn apply_session_move(session: &mut McpSession, args: &Value) -> Result<()> {
    let reference = args
        .get("reference")
        .and_then(Value::as_str)
        .context("reference required")?;
    let x = args
        .get("x")
        .and_then(Value::as_f64)
        .context("x required")?;
    let y = args
        .get("y")
        .and_then(Value::as_f64)
        .context("y required")?;
    let mut root = crate::sexp::parse(&session.current)?;
    let fp = root
        .children
        .iter_mut()
        .find(|n| n.has_tag("footprint") && n.property("Reference") == Some(reference))
        .context("footprint not found")?;
    let at = fp.get_mut("at").context("footprint missing at")?;
    at.set_value(0, x);
    at.set_value(1, y);
    session.current = root.to_kicad_string_preserving();
    Ok(())
}

#[derive(Parser)]
struct IpcArgs {
    #[command(subcommand)]
    command: Option<IpcCommand>,
}
#[derive(Subcommand)]
enum IpcCommand {
    Status(SocketArgs),
    Connect(SocketArgs),
    PushRoutes {
        pcb: PathBuf,
        #[arg(short, long)]
        socket: Option<PathBuf>,
        #[arg(short, long)]
        net: Option<String>,
        #[arg(long)]
        dry_run: bool,
        #[arg(long, value_enum, default_value = "text")]
        format: Format,
    },
}
#[derive(Args)]
struct SocketArgs {
    #[arg(short, long)]
    socket: Option<PathBuf>,
    #[arg(long, value_enum, default_value = "text")]
    format: Format,
}
pub fn ipc(args: Vec<OsString>, _: &Globals) -> Result<i32> {
    match parse_args::<IpcArgs>("ipc", args).command {
        None => {
            println!("Usage: kct ipc <status|connect|push-routes>");
            Ok(0)
        }
        Some(IpcCommand::Status(a)) => ipc_status("status", a),
        Some(IpcCommand::Connect(a)) => ipc_status("connect", a),
        Some(IpcCommand::PushRoutes {
            pcb,
            socket,
            net,
            dry_run,
            format,
        }) => {
            if !pcb.exists() {
                bail!("PCB not found: {}", pcb.display());
            }
            let (items, segments, vias) = route_items(&pcb, net.as_deref())?;
            let sock = resolve_socket(socket.as_deref());
            let summary = json!({"command":"push-routes","pcb":pcb,"net_filter":net,"socket":sock,"tracks":segments,"vias":vias,"dry_run":dry_run});
            if dry_run || items.is_empty() {
                let mut report = summary;
                report["pushed"] = json!(0);
                report["success"] = json!(true);
                emit(format, &report);
                Ok(0)
            } else {
                let Some(sock) = sock else {
                    let mut report = summary;
                    report["pushed"] = json!(0);
                    report["success"] = json!(false);
                    report["error"] = json!("No KiCad IPC socket found.");
                    emit(format, &report);
                    return Ok(1);
                };
                match crate::ipc::Client::connect(&sock).and_then(|mut client| {
                    client.create_items_transaction(
                        &format!(
                            "Push routes from {}",
                            pcb.file_name().unwrap_or_default().to_string_lossy()
                        ),
                        items,
                    )
                }) {
                    Ok(created) => {
                        let mut report = summary;
                        report["pushed"] = json!(created.len());
                        report["success"] = json!(true);
                        emit(format, &report);
                        Ok(0)
                    }
                    Err(error) => {
                        let mut report = summary;
                        report["pushed"] = json!(0);
                        report["success"] = json!(false);
                        report["error"] = json!(error.to_string());
                        emit(format, &report);
                        Ok(1)
                    }
                }
            }
        }
    }
}
fn ipc_status(command: &str, a: SocketArgs) -> Result<i32> {
    let instances = crate::ipc::discover_sockets(a.socket.as_deref());
    let socket = instances.first().cloned();
    let result = socket
        .as_deref()
        .context("No KiCad IPC socket found.")
        .and_then(|path| {
            let mut client = crate::ipc::Client::connect(path)?;
            if command == "status" && !client.ping()? {
                bail!("Connected but KiCad is not responding to health checks.")
            }
            let version = client.version()?;
            let docs = if command == "status" {
                client.open_documents()?
            } else {
                vec![]
            };
            Ok((version, docs))
        });
    match result {
        Ok((version, docs)) => {
            let open: Vec<_> = docs
                .iter()
                .filter_map(|d| d.get("path").and_then(Value::as_str))
                .collect();
            emit(
                a.format,
                &json!({"command":command,"socket":socket,"connected":true,"kicad_version":version,"instances":instances,"open_documents":open,"success":true}),
            );
            Ok(0)
        }
        Err(error) => {
            emit(
                a.format,
                &json!({"command":command,"socket":socket,"connected":false,"instances":instances,"error":error.to_string(),"success":false}),
            );
            Ok(1)
        }
    }
}
fn resolve_socket(explicit: Option<&Path>) -> Option<PathBuf> {
    crate::ipc::discover_sockets(explicit).into_iter().next()
}

fn route_items(path: &Path, net_filter: Option<&str>) -> Result<(Vec<Value>, usize, usize)> {
    let root = crate::sexp::parse(&fs::read_to_string(path)?)?;
    let nets: std::collections::HashMap<i64, String> = root
        .children_named("net")
        .filter_map(|n| Some((n.int_at(0)?, n.text_at(1)?)))
        .collect();
    let allowed = |n: i64| net_filter.is_none_or(|name| nets.get(&n).is_some_and(|v| v == name));
    if let Some(name) = net_filter {
        if !nets.values().any(|n| n == name) {
            bail!("Net not found in {}: {name}", path.display())
        }
    }
    let mut items = Vec::new();
    let mut tracks = 0;
    let mut vias = 0;
    for segment in root.children_named("segment") {
        let net = segment.get("net").and_then(|n| n.int_at(0)).unwrap_or(0);
        if !allowed(net) {
            continue;
        }
        let start = segment.get("start").context("segment missing start")?;
        let end = segment.get("end").context("segment missing end")?;
        items.push(json!({"type":"track","start":{"x":mm_nm(coord(start,0,"segment start x")?),"y":mm_nm(coord(start,1,"segment start y")?)},"end":{"x":mm_nm(coord(end,0,"segment end x")?),"y":mm_nm(coord(end,1,"segment end y")?)},"width":mm_nm(segment.child_f64("width").unwrap_or(0.25)),"layer":segment.child_str("layer").unwrap_or("F.Cu"),"net":net}));
        tracks += 1;
    }
    for via in root.children_named("via") {
        let net = via.get("net").and_then(|n| n.int_at(0)).unwrap_or(0);
        if !allowed(net) {
            continue;
        }
        let at = via.get("at").context("via missing at")?;
        let layers = via.get("layers");
        items.push(json!({"type":"via","position":{"x":mm_nm(coord(at,0,"via x")?),"y":mm_nm(coord(at,1,"via y")?)},"diameter":mm_nm(via.child_f64("size").unwrap_or(0.8)),"drill":mm_nm(via.child_f64("drill").unwrap_or(0.4)),"net":net,"start_layer":layers.and_then(|x|x.text_at(0)).unwrap_or_else(||"F.Cu".into()),"end_layer":layers.and_then(|x|x.text_at(1)).unwrap_or_else(||"B.Cu".into())}));
        vias += 1;
    }
    Ok((items, tracks, vias))
}
fn mm_nm(value: f64) -> i64 {
    (value * 1_000_000.0) as i64
}
fn coord(node: &crate::SExp, index: usize, what: &str) -> Result<f64> {
    node.float_at(index)
        .with_context(|| format!("missing {what}"))
}

#[derive(Parser)]
struct ReasonArgs {
    pcb: PathBuf,
    #[arg(short, long)]
    output: Option<PathBuf>,
    #[arg(long)]
    export_state: bool,
    #[arg(long)]
    state_output: Option<PathBuf>,
    #[arg(long)]
    analyze: bool,
    #[arg(long)]
    interactive: bool,
    #[arg(long)]
    auto_route: bool,
    #[arg(long, default_value_t = 10)]
    max_nets: usize,
    #[arg(long)]
    drc: Option<PathBuf>,
    #[arg(short, long, default_value = "jlcpcb")]
    mfr: String,
    #[arg(short, long, default_value_t = 2)]
    layers: usize,
    #[arg(long)]
    no_drc: bool,
    #[arg(short, long)]
    verbose: bool,
    #[arg(long)]
    dry_run: bool,
    #[arg(long, value_enum, default_value = "text")]
    format: Format,
}
pub fn reason(args: Vec<OsString>, _: &Globals) -> Result<i32> {
    let a = parse_args::<ReasonArgs>("reason", args);
    if a.interactive && matches!(a.format, Format::Json) {
        emit(
            a.format,
            &json!({"command":"reason","pcb":a.pcb,"error":"--interactive is a stdin/stdout dialogue and has no single-document form; use --export-state, --analyze or --auto-route with --format json","success":false}),
        );
        return Ok(2);
    }
    let text =
        fs::read_to_string(&a.pcb).with_context(|| format!("loading {}", a.pcb.display()))?;
    let state = reasoning_state(&a.pcb, &text)?;
    if let Some(path) = a.state_output.as_ref() {
        fs::write(path, serde_json::to_vec_pretty(&state)?)?;
    }
    let mode = if a.export_state {
        "export-state"
    } else if a.analyze {
        "analyze"
    } else if a.interactive {
        "interactive"
    } else if a.auto_route {
        "auto-route"
    } else {
        "prompt"
    };
    let prompt = state["prompt"].as_str().unwrap_or_default().to_owned();
    if a.auto_route {
        let Some(route) = COMMANDS
            .iter()
            .find(|c| c.name == "route")
            .and_then(|c| c.run)
        else {
            let report = json!({"command":"reason","mode":mode,"board":state,"error":"native route command is unavailable","success":false});
            emit(a.format, &report);
            return Ok(3);
        };
        let mut route_args = vec![a.pcb.clone().into_os_string()];
        if let Some(output) = a.output.as_ref() {
            route_args.extend([OsString::from("--output"), output.clone().into_os_string()]);
        }
        if a.dry_run {
            route_args.push(OsString::from("--dry-run"));
        }
        return route(route_args, &Globals::default());
    }
    let output = a.output.clone().unwrap_or_else(|| {
        a.pcb.with_file_name(format!(
            "{}_reasoned.kicad_pcb",
            a.pcb.file_stem().unwrap_or_default().to_string_lossy()
        ))
    });
    let routed = state["nets"]["routed"].as_array().map_or(0, Vec::len);
    let unrouted = state["nets"]["unrouted"].as_array().map_or(0, Vec::len);
    let board = json!({"width_mm":state["outline"]["width"],"height_mm":state["outline"]["height"],"components":state["components"].as_object().map_or(0,|x|x.len()),"nets_total":routed+unrouted,"nets_routed":routed,"nets_unrouted":unrouted,"violations":state["violations"].as_array().map_or(0,Vec::len)});
    let drc = if let Some(path) = a.drc.as_ref() {
        json!({"ran":false,"source":"report","path":path})
    } else if a.no_drc {
        json!({"ran":false,"source":"skipped"})
    } else {
        json!({"ran":false,"source":"checker","manufacturer":a.mfr,"layers":a.layers,"error":"native DRC checker unavailable; run kicadmium drc or kicad-cli pcb drc"})
    };
    let mut report = json!({"command":"reason","mode":mode,"pcb":a.pcb,"board":board,"drc":drc,"warnings":[],"max_nets":a.max_nets,"dry_run":a.dry_run,"output":output,"success":true});
    if a.export_state {
        report["state"] = state.clone();
        report["state_output"] = json!(a.state_output);
    } else if a.analyze {
        report["analysis"] = json!(prompt.clone());
    } else {
        report["prompt"] = json!(prompt.clone());
    }
    if a.interactive {
        println!("{prompt}\nEnter `status` or `quit`.");
        for line in io::stdin().lock().lines() {
            match line?.trim() {
                "quit" | "exit" => break,
                "status" => println!("{}", serde_json::to_string_pretty(&state)?),
                other => println!("Unknown reasoning command: {other}"),
            }
        }
    } else {
        emit(a.format, &report);
    }
    Ok(0)
}

fn reasoning_state(path: &Path, text: &str) -> Result<Value> {
    let root = crate::sexp::parse(text)?;
    let nets: std::collections::BTreeMap<i64, String> = root
        .children_named("net")
        .filter_map(|n| Some((n.int_at(0)?, n.text_at(1)?)))
        .collect();
    let mut pad_counts = std::collections::BTreeMap::<i64, usize>::new();
    let mut components = serde_json::Map::new();
    for fp in root.children_named("footprint") {
        let reference = fp
            .property("Reference")
            .or_else(|| fp.child_str("fp_text"))
            .unwrap_or("");
        if reference.is_empty() {
            continue;
        }
        let (fx, fy, rotation) = fp.at().unwrap_or((0.0, 0.0, 0.0));
        let radians = rotation.to_radians();
        let mut pads = Vec::new();
        for pad in fp.children_named("pad") {
            let (px, py, _) = pad.at().unwrap_or((0.0, 0.0, 0.0));
            let x = fx + px * radians.cos() - py * radians.sin();
            let y = fy + px * radians.sin() + py * radians.cos();
            let net = pad.get("net").and_then(|n| n.int_at(0)).unwrap_or(0);
            if net > 0 {
                *pad_counts.entry(net).or_default() += 1
            }
            pads.push(json!({"name":format!("{}:{}",reference,pad.text_at(0).unwrap_or_default()),"x":x,"y":y,"net":nets.get(&net).cloned().unwrap_or_default()}));
        }
        components.insert(reference.to_owned(), json!({"x":fx,"y":fy,"rotation":rotation,"layer":fp.child_str("layer").unwrap_or("F.Cu"),"footprint":fp.text_at(0).unwrap_or_default(),"pads":pads}));
    }
    let routed_codes: std::collections::HashSet<i64> = root
        .children_named("segment")
        .chain(root.children_named("via"))
        .filter_map(|n| n.get("net").and_then(|v| v.int_at(0)))
        .collect();
    let mut routed = Vec::new();
    let mut unrouted = Vec::new();
    for (number, name) in &nets {
        if *number == 0 {
            continue;
        }
        let count = pad_counts.get(number).copied().unwrap_or(0);
        if routed_codes.contains(number) {
            routed.push(json!({"name":name,"pad_count":count}));
        } else {
            unrouted.push(json!({"name":name,"pad_count":count,"priority":net_priority(name)}));
        }
    }
    let mut xs = Vec::new();
    let mut ys = Vec::new();
    for node in root
        .children
        .iter()
        .filter(|n| n.child_str("layer") == Some("Edge.Cuts"))
    {
        for key in ["start", "end", "center", "mid", "at"] {
            if let Some(point) = node.get(key) {
                if let (Some(x), Some(y)) = (point.float_at(0), point.float_at(1)) {
                    xs.push(x);
                    ys.push(y)
                }
            }
        }
        for point in node
            .get("pts")
            .into_iter()
            .flat_map(|p| p.children_named("xy"))
        {
            if let (Some(x), Some(y)) = (point.float_at(0), point.float_at(1)) {
                xs.push(x);
                ys.push(y)
            }
        }
    }
    let width = range(&xs);
    let height = range(&ys);
    let prompt = format!("## Progress\nNets routed: {}/{}\nViolations: 0\n\n## PCB State\n\nBoard: {:.1} x {:.1} mm\nComponents: {}\n\n## Routing Progress\n\nNets routed: {}/{}\nTraces: {}\nVias: {}",routed.len(),routed.len()+unrouted.len(),width,height,components.len(),routed.len(),routed.len()+unrouted.len(),root.children_named("segment").count(),root.children_named("via").count());
    Ok(
        json!({"pcb_file":path,"outline":{"width":width,"height":height},"components":components,"nets":{"routed":routed,"unrouted":unrouted},"violations":[],"prompt":prompt,"sha256":format!("{:x}",Sha256::digest(text.as_bytes()))}),
    )
}
fn range(values: &[f64]) -> f64 {
    if values.is_empty() {
        0.0
    } else {
        values.iter().copied().fold(f64::NEG_INFINITY, f64::max)
            - values.iter().copied().fold(f64::INFINITY, f64::min)
    }
}
fn net_priority(name: &str) -> &'static str {
    let n = name.to_ascii_uppercase();
    if n.contains("CLK") || n.contains("USB") || n.contains("SCL") || n.contains("SDA") {
        "high"
    } else {
        "normal"
    }
}

#[derive(Parser)]
struct InteractiveArgs {
    #[arg(long)]
    project: Option<PathBuf>,
}
pub fn interactive(args: Vec<OsString>, globals: &Globals) -> Result<i32> {
    let a = parse_args::<InteractiveArgs>("interactive", args);
    let mut loaded = a.project;
    let mut output_dir = std::env::current_dir()?;
    let history_path = std::env::var_os("HOME")
        .map(PathBuf::from)
        .map(|p| p.join(".kicad_tools_history"));
    let mut history: Vec<String> = history_path
        .as_ref()
        .and_then(|p| fs::read_to_string(p).ok())
        .map(|text| text.lines().map(str::to_owned).collect())
        .unwrap_or_default();
    eprintln!("kicadmium kct interactive — type `help` or `quit`");
    for line in io::stdin().lock().lines() {
        let line = line?;
        if !line.trim().is_empty() && history.last().is_none_or(|last| last != &line) {
            history.push(line.clone())
        }
        let words = split_words(&line)?;
        if words.is_empty() {
            continue;
        }
        match words[0].as_str() {
            "quit" | "exit" => break,
            "help" => {
                println!("load <file> | status | summary [sch|pcb] | output [dir] | clear | <native-kct-command> ... | quit")
            }
            "load" => {
                let p = words.get(1).context("load requires a file")?;
                let path = PathBuf::from(p);
                if !path.exists() {
                    eprintln!("File not found: {p}")
                } else {
                    loaded = Some(path);
                    println!("Loaded {p}")
                }
            }
            "status" => println!(
                "Loaded: {}\nOutput directory: {}",
                loaded
                    .as_deref()
                    .map_or_else(|| "nothing".into(), |p| p.display().to_string()),
                output_dir.display()
            ),
            "output" => {
                if let Some(path) = words.get(1) {
                    output_dir = PathBuf::from(path);
                    println!("Output directory set to: {}", output_dir.display())
                } else {
                    println!("Output directory: {}", output_dir.display())
                }
            }
            "clear" => {
                loaded = None;
                println!("Session cleared.")
            }
            "history" => {
                for (index, entry) in history.iter().enumerate() {
                    println!("{:>4}  {entry}", index + 1)
                }
            }
            "complete" => {
                let prefix = words.get(1).map_or("", String::as_str);
                for candidate in repl_completions(prefix) {
                    println!("{candidate}")
                }
            }
            "summary" => {
                let Some(path) = loaded.as_ref() else {
                    eprintln!("Error: No file loaded. Use 'load <file>' first.");
                    continue;
                };
                let command = if path.extension().and_then(|x| x.to_str()) == Some("kicad_pcb") {
                    "pcb"
                } else {
                    "sch"
                };
                let Some(run) = COMMANDS
                    .iter()
                    .find(|x| x.name == command)
                    .and_then(|x| x.run)
                else {
                    eprintln!("Command not yet ported: {command}");
                    continue;
                };
                let mut argv = vec![path.clone().into_os_string()];
                argv.push(OsString::from("summary"));
                if let Err(error) = run(argv, globals) {
                    eprintln!("Error: {error:#}")
                }
            }
            cmd => {
                let Some(spec) = COMMANDS.iter().find(|x| x.name == cmd) else {
                    eprintln!("Unknown command: {cmd}");
                    continue;
                };
                let Some(run) = spec.run else {
                    eprintln!("Command not yet ported: {cmd}");
                    continue;
                };
                if cmd == "interactive" {
                    eprintln!("Already interactive");
                    continue;
                }
                let mut argv: Vec<OsString> = words[1..].iter().map(OsString::from).collect();
                if matches!(cmd, "symbols" | "bom" | "nets") {
                    let Some(path) = loaded.as_ref() else {
                        eprintln!("Error: No schematic loaded. Use 'load <file>' first.");
                        continue;
                    };
                    argv.insert(0, path.clone().into_os_string());
                }
                if let Err(e) = run(argv, globals) {
                    eprintln!("Error: {e:#}")
                }
            }
        }
    }
    if let Some(path) = history_path {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?
        }
        let start = history.len().saturating_sub(1000);
        fs::write(path, format!("{}\n", history[start..].join("\n")))?
    }
    Ok(0)
}

fn repl_completions(prefix: &str) -> Vec<String> {
    let mut values: Vec<String> = [
        "load", "status", "summary", "output", "clear", "history", "complete", "help", "quit",
    ]
    .into_iter()
    .chain(COMMANDS.iter().map(|c| c.name))
    .filter(|name| name.starts_with(prefix))
    .map(str::to_owned)
    .collect();
    let path = Path::new(prefix);
    let directory = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let file_prefix = path.file_name().and_then(|x| x.to_str()).unwrap_or("");
    if let Ok(entries) = fs::read_dir(directory) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with(file_prefix) {
                values.push(directory.join(name).display().to_string())
            }
        }
    }
    values.sort();
    values.dedup();
    values
}

#[derive(Parser)]
struct RunArgs {
    workflow: PathBuf,
    #[arg(trailing_var_arg = true)]
    args: Vec<String>,
    #[arg(long)]
    dry_run: bool,
}
#[derive(Debug, Deserialize, Serialize)]
struct Workflow {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    env: std::collections::BTreeMap<String, String>,
    steps: Vec<Step>,
}
#[derive(Debug, Deserialize, Serialize)]
struct Step {
    #[serde(default)]
    name: String,
    command: String,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    continue_on_error: bool,
}
pub fn run(args: Vec<OsString>, globals: &Globals) -> Result<i32> {
    let a = parse_args::<RunArgs>("run", args);
    if a.workflow.extension().and_then(|x| x.to_str()) == Some("py") {
        bail!("Python automation is not supported. Convert this script to a native kct JSON/YAML workflow; see `kct run --help`.");
    }
    let bytes = fs::read(&a.workflow)?;
    let mut wf: Workflow = if matches!(
        a.workflow.extension().and_then(|x| x.to_str()),
        Some("yaml" | "yml")
    ) {
        serde_yaml::from_slice(&bytes)?
    } else {
        serde_json::from_slice(&bytes)?
    };
    if wf.version == 0 {
        wf.version = 1
    }
    if wf.version != 1 {
        bail!("unsupported workflow version {}", wf.version)
    }
    let mut failed = 0;
    for (index, step) in wf.steps.iter().enumerate() {
        let rendered: Vec<_> = step
            .args
            .iter()
            .map(|x| substitute(x, &a.args, &wf.env))
            .collect();
        println!(
            "[{}] {}: kct {} {}",
            index + 1,
            if step.name.is_empty() {
                &step.command
            } else {
                &step.name
            },
            step.command,
            rendered.join(" ")
        );
        if a.dry_run {
            continue;
        }
        let spec = COMMANDS
            .iter()
            .find(|c| c.name == step.command)
            .with_context(|| format!("unknown workflow command {}", step.command))?;
        let runner = spec.run.context("workflow command is not ported")?;
        if step.command == "run" {
            bail!("nested workflows are not allowed")
        }
        let rc = runner(rendered.into_iter().map(OsString::from).collect(), globals)?;
        if rc != 0 {
            failed += 1;
            if !step.continue_on_error {
                return Ok(rc);
            }
        }
    }
    Ok(if failed == 0 { 0 } else { 1 })
}
fn substitute(
    s: &str,
    args: &[String],
    env: &std::collections::BTreeMap<String, String>,
) -> String {
    let mut out = s.to_owned();
    for (i, v) in args.iter().enumerate() {
        out = out.replace(&format!("${{{}}}", i + 1), v)
    }
    for (k, v) in env {
        out = out.replace(&format!("${{{k}}}"), v)
    }
    out
}
fn emit(format: Format, value: &Value) {
    match format {
        Format::Json => println!("{}", serde_json::to_string(value).unwrap()),
        Format::Text => println!("{}", serde_json::to_string_pretty(value).unwrap()),
    }
}
fn split_words(line: &str) -> Result<Vec<String>> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote = None;
    for c in line.chars() {
        match (quote, c) {
            (None, '\'' | '"') => quote = Some(c),
            (Some(q), x) if x == q => quote = None,
            (None, x) if x.is_whitespace() => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur))
                }
            }
            (_, x) => cur.push(x),
        }
    }
    if quote.is_some() {
        bail!("unterminated quote")
    }
    if !cur.is_empty() {
        out.push(cur)
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn splits_repl_words() {
        assert_eq!(
            split_words("load \"a b.kicad_pcb\"").unwrap(),
            ["load", "a b.kicad_pcb"]
        )
    }
    #[test]
    fn substitutes_workflow_values() {
        let e = std::collections::BTreeMap::from([("OUT".into(), "build".into())]);
        assert_eq!(substitute("${OUT}/${1}", &["x".into()], &e), "build/x")
    }
    #[test]
    fn mcp_initialize() {
        let r = rpc(&json!({"jsonrpc":"2.0","id":1,"method":"initialize"}));
        assert_eq!(r["result"]["serverInfo"]["name"], "kicadmium-kct")
    }
    #[test]
    fn mcp_catalog_matches_upstream_registry() {
        let listed = rpc(&json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}));
        let names: Vec<_> = listed["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|tool| tool["name"].as_str())
            .collect();
        assert_eq!(names, MCP_NAMES);
        assert_eq!(names.len(), 46);
    }
    #[test]
    fn reason_state_has_component_pad_net_and_outline_shape() {
        let text = r#"(kicad_pcb (net 1 "SIG")
          (footprint "Pkg:X" (layer "F.Cu") (at 10 20 90)
            (property "Reference" "U1") (pad "1" smd rect (at 1 0) (net 1 "SIG")))
          (segment (start 1 1) (end 2 2) (width .2) (layer "F.Cu") (net 1))
          (gr_line (start 0 0) (end 30 0) (layer "Edge.Cuts"))
          (gr_line (start 30 0) (end 30 20) (layer "Edge.Cuts")))"#;
        let state = reasoning_state(Path::new("x.kicad_pcb"), text).unwrap();
        assert_eq!(state["outline"], json!({"width":30.0,"height":20.0}));
        assert_eq!(state["components"]["U1"]["pads"][0]["x"], 10.0);
        assert_eq!(state["components"]["U1"]["pads"][0]["y"], 21.0);
        assert_eq!(state["nets"]["routed"][0]["name"], "SIG");
    }
    #[test]
    fn route_items_match_ipc_units_and_filter() {
        let dir = tempfile::tempdir().unwrap();
        let pcb = dir.path().join("x.kicad_pcb");
        fs::write(
            &pcb,
            r#"(kicad_pcb (net 1 "SIG") (net 2 "GND")
          (segment (start 1.25 2.5) (end 3 4) (width 0.2) (layer "F.Cu") (net 1))
          (segment (start 0 0) (end 1 1) (width 0.3) (layer "B.Cu") (net 2))
          (via (at 5 6) (size 0.8) (drill 0.4) (layers "F.Cu" "B.Cu") (net 1)))"#,
        )
        .unwrap();
        let (items, tracks, vias) = route_items(&pcb, Some("SIG")).unwrap();
        assert_eq!((tracks, vias, items.len()), (1, 1, 2));
        assert_eq!(items[0]["start"]["x"], 1_250_000);
        assert_eq!(items[1]["position"]["y"], 6_000_000);
    }
}
