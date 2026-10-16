//! MCP 协议壳 v0 —— stdio JSON-RPC 行协议，手写零依赖。
//! 壳薄、执行面厚：工具面十件（list_commands/execute/batch/inspect/undo/checkpoint/preview/commit/reject/compose），
//! op 的权威参数文档由 list_commands 自描述带出。验证拦截的 findings 原样进 isError 应答。
use archcore_engine::{Engine, EngineError};
use serde_json::{json, Value};

pub struct McpServer {
    pub engine: Engine,
}

const PROTOCOL_VERSION: &str = "2025-06-18";

fn ok(id: Value, result: Value) -> String {
    json!({"jsonrpc": "2.0", "id": id, "result": result}).to_string()
}

fn err(id: Value, code: i64, message: &str) -> String {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}}).to_string()
}

fn tool_text(payload: Value, is_error: bool) -> Value {
    json!({
        "content": [{"type": "text", "text": payload.to_string()}],
        "isError": is_error
    })
}

fn tools_catalog() -> Value {
    let t = |name: &str, desc: &str, props: Value, required: &[&str]| {
        json!({
            "name": name,
            "description": desc,
            "inputSchema": {
                "type": "object",
                "properties": props,
                "required": required
            }
        })
    };
    json!([
        t("list_commands", "命令清单：op id、参数表、行为注解（能力发现的权威来源）", json!({}), &[]),
        t("execute", "执行单个 op；失败显式返回，验证拦截带 findings",
            json!({"op": {"type": "string"}, "args": {"type": "object"}}), &["op"]),
        t("batch", "按序执行多个 op，全有回滚；失败指名步序号",
            json!({"steps": {"type": "array", "items": {"type": "object"}}}), &["steps"]),
        t("inspect", "读取场景几何事实快照与层级摘要（只读）", json!({}), &[]),
        t("compose", "直出场景：结构化简报→场地/方案/软装/交付物一条龙（逐步事务，失败即停）",
            json!({"spec": {"type": "object", "description": "场景简报{name,site,buildings,furniture,deliverables,renderer}"}}), &["spec"]),
        t("undo", "撤销上一步", json!({}), &[]),
        t("checkpoint", "存检查点（场景+历史），返回 checkpoint id",
            json!({"label": {"type": "string"}}), &[]),
        t("preview", "detached 预演一组步骤（不碰活文档），返回报告+事实变化+验证结果三件套",
            json!({"label": {"type": "string"}, "steps": {"type": "array", "items": {"type": "object"}}}), &["steps"]),
        t("commit", "提交预览（修订号安全：审阅期间文档被改则拒绝）",
            json!({"preview_id": {"type": "string"}}), &["preview_id"]),
        t("reject", "拒绝预览，弃单",
            json!({"preview_id": {"type": "string"}}), &["preview_id"])
    ])
}

impl McpServer {
    pub fn new() -> Self {
        Self { engine: Engine::new() }
    }

    /// 处理一行 JSON-RPC：请求返回应答行，通知（无 id）返回 None
    pub fn handle_line(&mut self, line: &str) -> Option<String> {
        let msg: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(e) => return Some(err(Value::Null, -32700, &format!("parse error: {e}"))),
        };
        let id = msg.get("id").cloned();
        let Some(id) = id else {
            return None; // 通知不回包
        };
        // 捕获 handler panic：坏输入/内部缺陷不得杀死 stdio 服务（审计 #1 双保险）
        let idv = id.clone();
        let method = msg["method"].as_str().unwrap_or("").to_string();
        let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.dispatch(id, &method, &msg)
        }));
        match caught {
            Ok(resp) => resp,
            Err(_) => Some(err(idv, -32603, "internal panic（已捕获，服务继续）")),
        }
    }

    fn dispatch(&mut self, id: Value, method: &str, msg: &Value) -> Option<String> {
        match method {
            "initialize" => Some(ok(
                id,
                json!({
                    "protocolVersion": PROTOCOL_VERSION,
                    "capabilities": {"tools": {}},
                    "serverInfo": {"name": "archbloom-mcp", "version": "0.2"}
                }),
            )),
            "tools/list" => Some(ok(id, json!({"tools": tools_catalog()}))),
            "tools/call" => Some(self.tool_call(id, &msg["params"])),
            other => Some(err(id, -32601, &format!("未知方法: {other}"))),
        }
    }

    fn tool_call(&mut self, id: Value, params: &Value) -> String {
        let name = params["name"].as_str().unwrap_or("");
        let args = params.get("arguments").cloned().unwrap_or(Value::Null);
        let (payload, is_error) = match name {
            "list_commands" => (json!({"commands": self.engine.capabilities()}), false),
            "execute" => {
                let op = args["op"].as_str().unwrap_or("");
                let op_args = args.get("args").cloned().unwrap_or(Value::Null);
                match self.engine.execute(op, &op_args) {
                    Ok(report) => (json!({"ok": true, "report": report}), false),
                    Err(EngineError::Op(e)) => (json!({"ok": false, "error": e}), true),
                    Err(EngineError::Verify(f)) => (json!({"ok": false, "findings": f}), true),
                    Err(EngineError::Conflict { expected, actual }) => (
                        json!({"ok": false, "conflict": {"expected": expected, "actual": actual}}),
                        true,
                    ),
                }
            }
            "batch" => {
                let steps: Vec<(String, Value)> = args["steps"]
                    .as_array()
                    .map(|arr| {
                        arr.iter()
                            .map(|s| {
                                (
                                    s["op"].as_str().unwrap_or("").to_string(),
                                    s.get("args").cloned().unwrap_or(Value::Null),
                                )
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                match self.engine.execute_script(&steps) {
                    Ok(reports) => (json!({"ok": true, "reports": reports}), false),
                    Err((idx, EngineError::Op(e))) => {
                        (json!({"ok": false, "failed_step": idx, "error": e}), true)
                    }
                    Err((idx, EngineError::Verify(f))) => {
                        (json!({"ok": false, "failed_step": idx, "findings": f}), true)
                    }
                    Err((idx, EngineError::Conflict { expected, actual })) => (
                        json!({"ok": false, "failed_step": idx, "conflict": {"expected": expected, "actual": actual}}),
                        true,
                    ),
                }
            }
            "inspect" => {
                let facts = archcore_ops::geo_facts(&self.engine.scene);
                let objects: Vec<Value> = self
                    .engine
                    .scene
                    .objects
                    .iter()
                    .map(|o| json!({"id": o.id, "name": o.name, "parent": o.parent}))
                    .collect();
                (
                    json!({"ok": true, "facts": facts, "hierarchy": objects}),
                    false,
                )
            }
            "compose" => {
                let spec = args.get("spec").cloned().unwrap_or(args.clone());
                let result = if spec.get("variants").is_some() {
                    Engine::compose_variants(&spec)
                } else {
                    self.engine.compose(&spec)
                };
                match result {
                    Ok(summary) => (json!({"ok": true, "compose": summary}), false),
                    Err(EngineError::Op(e)) => (json!({"ok": false, "error": e}), true),
                    Err(EngineError::Verify(f)) => (json!({"ok": false, "findings": f}), true),
                    Err(EngineError::Conflict { expected, actual }) => (
                        json!({"ok": false, "conflict": {"expected": expected, "actual": actual}}),
                        true,
                    ),
                }
            }
            "undo" => match self.engine.undo() {
                Ok(report) => (json!({"ok": true, "report": report}), false),
                Err(e) => (json!({"ok": false, "error": format!("{e}")}), true),
            },
            "checkpoint" => {
                let label = args["label"].as_str().unwrap_or("");
                let ckpt = self.engine.checkpoint(label);
                (json!({"ok": true, "checkpoint": ckpt}), false)
            }
            "preview" => {
                let label = args["label"].as_str().unwrap_or("");
                let steps: Vec<(String, Value)> = args["steps"]
                    .as_array()
                    .map(|arr| {
                        arr.iter()
                            .map(|s| {
                                (
                                    s["op"].as_str().unwrap_or("").to_string(),
                                    s.get("args").cloned().unwrap_or(Value::Null),
                                )
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                match self.engine.preview(label, &steps) {
                    Ok(summary) => (json!({"ok": true, "preview": summary}), false),
                    Err((idx, e)) => (
                        json!({"ok": false, "failed_step": idx, "error": format!("{e}")}),
                        true,
                    ),
                }
            }
            "commit" => {
                let pid = args["preview_id"].as_str().unwrap_or("");
                match self.engine.commit(pid) {
                    Ok(summary) => (json!({"ok": true, "committed": summary}), false),
                    Err(EngineError::Conflict { expected, actual }) => (
                        json!({"ok": false, "conflict": {"expected": expected, "actual": actual}}),
                        true,
                    ),
                    Err(e) => (json!({"ok": false, "error": format!("{e}")}), true),
                }
            }
            "reject" => {
                let pid = args["preview_id"].as_str().unwrap_or("");
                match self.engine.reject(pid) {
                    Ok(()) => (json!({"ok": true}), false),
                    Err(e) => (json!({"ok": false, "error": format!("{e}")}), true),
                }
            }
            other => (json!({"ok": false, "error": format!("未知工具: {other}")}), true),
        };
        ok(id, tool_text(payload, is_error))
    }
}

impl Default for McpServer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(server: &mut McpServer, text: &str) -> Value {
        let resp = server.handle_line(text).expect("应有应答");
        serde_json::from_str(&resp).unwrap()
    }

    #[test]
    fn handshake_initialize() {
        let mut s = McpServer::new();
        let r = call(&mut s, r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#);
        assert_eq!(r["result"]["protocolVersion"], PROTOCOL_VERSION);
        assert!(r["result"]["capabilities"]["tools"].is_object());
    }

    #[test]
    fn tools_list_includes_execute() {
        let mut s = McpServer::new();
        let r = call(&mut s, r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#);
        let names: Vec<&str> = r["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|t| t["name"].as_str())
            .collect();
        assert!(names.contains(&"execute") && names.contains(&"batch"));
    }

    #[test]
    fn execute_mesh_create_round_trip() {
        let mut s = McpServer::new();
        let r = call(
            &mut s,
            r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"execute","arguments":{"op":"mesh.create","args":{"name":"墙","positions":[[0,0,0],[1,0,0],[1,1,0]],"indices":[0,1,2]}}}}"#,
        );
        assert_eq!(r["result"]["isError"], false);
        let inner: Value = serde_json::from_str(r["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(inner["ok"], true);
    }

    /// batch 失败：isError=true 且带 failed_step + 结构化错误（宪法条款经协议面成立）
    #[test]
    fn batch_failure_is_structured() {
        let mut s = McpServer::new();
        let r = call(
            &mut s,
            r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"batch","arguments":{"steps":[{"op":"object.create_group","args":{"name":"A"}},{"op":"object.delete","args":{"id":"obj-404"}}]}}}"#,
        );
        assert_eq!(r["result"]["isError"], true);
        let inner: Value = serde_json::from_str(r["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(inner["failed_step"], 1);
    }

    #[test]
    fn unknown_method_is_jsonrpc_error() {
        let mut s = McpServer::new();
        let r = call(&mut s, r#"{"jsonrpc":"2.0","id":5,"method":"resources/list"}"#);
        assert_eq!(r["error"]["code"], -32601);
    }

    /// 通知（无 id）不回包
    #[test]
    fn notification_no_response() {
        let mut s = McpServer::new();
        assert!(s
            .handle_line(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#)
            .is_none());
    }

    /// 审批环：preview（detached）→ commit（修订号安全）经协议面走通
    #[test]
    fn preview_commit_over_protocol() {
        let mut s = McpServer::new();
        let r = call(
            &mut s,
            r#"{"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"preview","arguments":{"label":"建墙","steps":[{"op":"mesh.create","args":{"name":"墙","positions":[[0,0,0],[1,0,0],[1,1,0]],"indices":[0,1,2]}}]}}}"#,
        );
        assert_eq!(r["result"]["isError"], false);
        let inner: Value =
            serde_json::from_str(r["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        let pid = inner["preview"]["id"].as_str().unwrap().to_string();
        let r2 = call(
            &mut s,
            &format!(
                r#"{{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{{"name":"commit","arguments":{{"preview_id":"{pid}"}}}}}}"#
            ),
        );
        assert_eq!(r2["result"]["isError"], false);
    }
}
