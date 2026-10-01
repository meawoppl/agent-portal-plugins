//! Agent-facing commands: MCP, live KiCad IPC discovery, reasoning, REPL and
//! declarative automation. These deliberately contain no Python bridge.

use std::ffi::OsString;
use std::fs;
use std::io::{self, BufRead, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Command;

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
        "tools/list" => json!({"tools":[
            {"name":"kct_commands","description":"List native kct commands and port status","inputSchema":{"type":"object","properties":{}}},
            {"name":"kct_run","description":"Run a native kct command","inputSchema":{"type":"object","required":["command"],"properties":{"command":{"type":"string"},"args":{"type":"array","items":{"type":"string"}}}}}
        ]}),
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
    if name == "kct_commands" {
        let commands: Vec<_> = COMMANDS
            .iter()
            .map(|c| json!({"name":c.name,"description":c.about,"native":c.run.is_some()}))
            .collect();
        return Ok(
            json!({"content":[{"type":"text","text":serde_json::to_string_pretty(&commands)?}]}),
        );
    }
    if name != "kct_run" {
        bail!("unknown tool {name}");
    }
    let a = params.get("arguments").context("missing arguments")?;
    let command = a
        .get("command")
        .and_then(Value::as_str)
        .context("missing command")?;
    let mut cmd = Command::new(std::env::current_exe()?);
    cmd.args(["kct", "--", command]);
    if let Some(args) = a.get("args").and_then(Value::as_array) {
        for arg in args {
            cmd.arg(arg.as_str().context("args must be strings")?);
        }
    }
    let out = cmd.output()?;
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    Ok(json!({"content":[{"type":"text","text":text}],"isError":!out.status.success()}))
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
    #[arg(long)]
    dry_run: bool,
    #[arg(long, value_enum, default_value = "text")]
    format: Format,
}
pub fn reason(args: Vec<OsString>, _: &Globals) -> Result<i32> {
    let a = parse_args::<ReasonArgs>("reason", args);
    let text =
        fs::read_to_string(&a.pcb).with_context(|| format!("loading {}", a.pcb.display()))?;
    let counts = count_named_forms(&text);
    let digest = format!("{:x}", Sha256::digest(text.as_bytes()));
    let state = json!({"pcb_file":a.pcb,"sha256":digest,"components":counts.get("footprint").copied().unwrap_or(0),"segments":counts.get("segment").copied().unwrap_or(0),"vias":counts.get("via").copied().unwrap_or(0),"zones":counts.get("zone").copied().unwrap_or(0),"nets":counts.get("net").copied().unwrap_or(0),"drc_report":a.drc});
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
    let prompt = format!("Review {}: {} footprints, {} nets, {} segments, {} vias, {} zones. Preserve connectivity and run native DRC after any authorized edit.", a.pcb.display(), state["components"], state["nets"], state["segments"], state["vias"], state["zones"]);
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
    let report = json!({"command":"reason","mode":mode,"board":state,"prompt":prompt,"max_nets":a.max_nets,"dry_run":a.dry_run,"output":a.output,"success":true});
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
        emit(a.format, if a.export_state { &state } else { &report });
    }
    Ok(0)
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
    eprintln!("kicadmium kct interactive — type `help` or `quit`");
    for line in io::stdin().lock().lines() {
        let line = line?;
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
    Ok(0)
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
fn count_named_forms(text: &str) -> std::collections::HashMap<String, usize> {
    let mut map = std::collections::HashMap::new();
    let b = text.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'(' {
            i += 1;
            let start = i;
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b"_-".contains(&b[i])) {
                i += 1
            }
            if i > start {
                *map.entry(text[start..i].to_string()).or_insert(0) += 1
            }
        } else {
            i += 1
        }
    }
    map
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
    fn counts_board_forms() {
        let c = count_named_forms("(kicad_pcb (footprint x)(segment)(via)(via))");
        assert_eq!(c["via"], 2);
        assert_eq!(c["footprint"], 1)
    }
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
