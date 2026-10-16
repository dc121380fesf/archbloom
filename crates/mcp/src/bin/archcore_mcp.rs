//! archbloom-mcp：stdio JSON-RPC 循环（stdin 行进 → stdout 行出）
use archcore_mcp::McpServer;
use std::io::{self, BufRead, Write};

fn main() {
    let mut server = McpServer::new();
    let stdin = io::stdin();
    let mut stdout = io::stdout();
    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };
        if line.trim().is_empty() {
            continue;
        }
        if let Some(response) = server.handle_line(&line) {
            writeln!(stdout, "{response}").expect("写应答失败");
            let _ = stdout.flush();
        }
    }
}
