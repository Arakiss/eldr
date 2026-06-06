//! `eldr mcp` — a Model Context Protocol server over stdio (newline-delimited JSON-RPC
//! 2.0), hand-rolled, zero crates. It lets an agent harness (Claude Code, Codex) use eldr
//! as a native tool: query the machine and its disks without knowing the CLI.
//!
//! Scope is deliberately read-only — `get_status`, `get_disk_health`, `get_system`,
//! `get_sensors`. The mutating actions (suspend/resume/checkpoint) stay on the CLI, where
//! the human runs them, rather than being handed to a model.
//!
//! Transport: one JSON message per line on stdin; one JSON reply per line on stdout.
//! Nothing else may be written to stdout while serving, or it corrupts the stream.

use crate::json::Json;
use crate::sensors::snapshot::{Snapshot, json_escape};
use crate::sensors::system::SystemInfo;
use std::io::{BufRead, Write};

const SAMPLE_MS: u64 = 500;
/// Fallback when the client doesn't state one; we otherwise echo the client's version.
const DEFAULT_PROTOCOL: &str = "2024-11-05";

/// Serve until stdin closes.
pub fn run() -> i32 {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let Some(req) = Json::parse(&line) else {
            continue; // ignore unparseable lines rather than crash the stream
        };
        let method = req.get("method").and_then(Json::as_str).unwrap_or("");
        // No id ⇒ a notification ⇒ no response (e.g. notifications/initialized).
        let Some(id) = req.get("id") else { continue };
        let reply = handle(method, &req, &id.to_compact());
        if writeln!(stdout, "{reply}").is_err() || stdout.flush().is_err() {
            break;
        }
    }
    0
}

fn handle(method: &str, req: &Json, id: &str) -> String {
    match method {
        "initialize" => {
            let proto = req
                .get("params")
                .and_then(|p| p.get("protocolVersion"))
                .and_then(Json::as_str)
                .unwrap_or(DEFAULT_PROTOCOL);
            result(
                id,
                &format!(
                    "{{\"protocolVersion\":\"{}\",\"capabilities\":{{\"tools\":{{}}}},\"serverInfo\":{{\"name\":\"eldr\",\"version\":\"{}\"}}}}",
                    json_escape(proto),
                    env!("CARGO_PKG_VERSION"),
                ),
            )
        }
        "tools/list" => result(id, &tools_list()),
        "tools/call" => tools_call(req, id),
        "ping" => result(id, "{}"),
        _ => error(id, -32601, "method not found"),
    }
}

fn tools_list() -> String {
    let empty = r#"{"type":"object","properties":{}}"#;
    format!(
        "{{\"tools\":[\
{{\"name\":\"get_status\",\"description\":\"Full machine snapshot (CPU/GPU/power/temps/fans/memory/volumes/disks) as JSON.\",\"inputSchema\":{empty}}},\
{{\"name\":\"get_disk_health\",\"description\":\"Per-volume usage and per-physical-disk health: SMART verdict, I/O errors/retries/latency, and NVMe wear (temp, percentage used, spare, TB written). JSON.\",\"inputSchema\":{empty}}},\
{{\"name\":\"get_system\",\"description\":\"Static machine identity: model, chip, macOS, RAM, internal SSD. JSON.\",\"inputSchema\":{empty}}},\
{{\"name\":\"get_sensors\",\"description\":\"Every SMC sensor (temperatures, fans, power, current, voltage) as JSON.\",\"inputSchema\":{empty}}}\
]}}"
    )
}

fn tools_call(req: &Json, id: &str) -> String {
    let name = req
        .get("params")
        .and_then(|p| p.get("name"))
        .and_then(Json::as_str)
        .unwrap_or("");
    let text = match name {
        "get_status" => Snapshot::gather(SAMPLE_MS).to_json(),
        "get_disk_health" => {
            let mut s = Snapshot::gather(SAMPLE_MS);
            s.read_smart();
            s.to_json()
        }
        "get_system" => SystemInfo::get().to_json(),
        "get_sensors" => crate::ui::pretty::sensors_json_string(),
        _ => return error(id, -32602, "unknown tool"),
    };
    // MCP tool result: a content array. We return our JSON document as one text item
    // (embedded as a JSON string, hence escaped).
    result(
        id,
        &format!(
            "{{\"content\":[{{\"type\":\"text\",\"text\":\"{}\"}}]}}",
            json_escape(&text)
        ),
    )
}

fn result(id: &str, result_obj: &str) -> String {
    format!("{{\"jsonrpc\":\"2.0\",\"id\":{id},\"result\":{result_obj}}}")
}

fn error(id: &str, code: i32, msg: &str) -> String {
    format!(
        "{{\"jsonrpc\":\"2.0\",\"id\":{id},\"error\":{{\"code\":{code},\"message\":\"{}\"}}}}",
        json_escape(msg)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(s: &str) -> Json {
        Json::parse(s).expect("server reply must be valid JSON")
    }

    #[test]
    fn initialize_advertises_server_and_protocol() {
        let req = Json::parse(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}"#).unwrap();
        let r = parse(&handle("initialize", &req, "1"));
        assert_eq!(r.get("id").and_then(Json::as_i64), Some(1));
        let res = r.get("result").unwrap();
        // Echoes the client's protocol version and names the server.
        assert_eq!(
            res.get("protocolVersion").and_then(Json::as_str),
            Some("2025-06-18")
        );
        assert_eq!(
            res.get("serverInfo")
                .and_then(|s| s.get("name"))
                .and_then(Json::as_str),
            Some("eldr")
        );
    }

    #[test]
    fn tools_list_is_valid_and_lists_the_read_tools() {
        let r = parse(&handle("tools/list", &Json::Null, "2"));
        let tools = match r.get("result").and_then(|x| x.get("tools")) {
            Some(Json::Arr(a)) => a.clone(),
            _ => panic!("tools must be an array"),
        };
        assert_eq!(tools.len(), 4);
        let names: Vec<&str> = tools
            .iter()
            .filter_map(|t| t.get("name").and_then(Json::as_str))
            .collect();
        assert!(names.contains(&"get_status") && names.contains(&"get_disk_health"));
    }

    #[test]
    fn unknown_method_is_a_jsonrpc_error() {
        let r = parse(&handle("does/not/exist", &Json::Null, "3"));
        assert_eq!(
            r.get("error")
                .and_then(|e| e.get("code"))
                .and_then(Json::as_i64),
            Some(-32601)
        );
    }
}
