//! JSON 脚本执行器（agent 端面的最薄一层）：
//! 入参 {"ops":[{"op":"...","args":{...}}, ...]}，出参 {"ok":true,"reports":[...]}
//! 或 {"ok":false,"failed_step":n,"error"/"findings":...}。验证拦截同样走失败面。
//!
//! 对外口径：这是发布给用户的可执行文件，**任何输入错误都必须是可读诊断 + 规范退出码**，
//! 不允许 panic（Rust 回溯对用户毫无意义）。退出码：0 成功 / 1 执行失败 / 2 用法或输入错误。
use archcore_engine::{Engine, EngineError};
use serde_json::Value;
use std::fs;
use std::process::ExitCode;

const USAGE: &str = "\
Archbloom 脚本执行器

用法:
  archbloom-run <script.json>

脚本格式（二选一）:
  {\"ops\":[{\"op\":\"massing.build\",\"args\":{...}}, ...]}   按序执行 op
  {\"compose\":{...}}                                    直出场景（可带 variants）

选项:
  -h, --help      显示本帮助
  -V, --version   显示版本

退出码:
  0  成功
  1  执行失败（op 报错 / 验证拦截 / compose 失败）
  2  用法错误或输入文件不可用
";

/// 用法/输入错误：打印到 stderr 并返回退出码 2
fn usage_error(msg: &str) -> ExitCode {
    eprintln!("错误: {msg}");
    eprintln!();
    eprintln!("运行 `archbloom-run --help` 查看用法。");
    ExitCode::from(2)
}

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let Some(first) = args.next() else {
        // 无参数：给人话，而不是 panic
        eprintln!("错误: 缺少脚本路径。");
        eprintln!();
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };

    match first.as_str() {
        "-h" | "--help" => {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        "-V" | "--version" => {
            println!("archbloom-run {}", env!("CARGO_PKG_VERSION"));
            return ExitCode::SUCCESS;
        }
        _ => {}
    }

    // 多余参数属于用法错误（避免用户以为它支持批量）
    if args.next().is_some() {
        return usage_error("一次只能执行一个脚本");
    }

    let path = &first;
    let text = match fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) => return usage_error(&format!("读不到脚本文件 `{path}`：{e}")),
    };
    let script: Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(e) => return usage_error(&format!("脚本不是合法 JSON（{path}）：{e}")),
    };

    // 直出场景分支：{"compose": {...简报...}}（含 variants 时走多方案对比）
    if script.get("compose").is_some() {
        let spec = &script["compose"];
        let result = if spec.get("variants").is_some() {
            Engine::compose_variants(spec)
        } else {
            Engine::new().compose(spec)
        };
        let payload = match result {
            Ok(summary) => serde_json::json!({ "ok": true, "compose": summary }),
            Err(EngineError::Op(op_err)) => serde_json::json!({ "ok": false, "error": op_err }),
            Err(EngineError::Verify(findings)) => {
                serde_json::json!({ "ok": false, "findings": findings })
            }
            Err(EngineError::Conflict { expected, actual }) => serde_json::json!({
                "ok": false,
                "conflict": { "expected": expected, "actual": actual }
            }),
        };
        println!(
            "{}",
            serde_json::to_string_pretty(&payload).unwrap_or_else(|e| format!("{{\"ok\":false,\"error\":\"输出序列化失败: {e}\"}}"))
        );
        return if payload["ok"].as_bool().unwrap_or(false) {
            ExitCode::SUCCESS
        } else {
            ExitCode::from(1)
        };
    }

    let Some(ops) = script.get("ops").and_then(|v| v.as_array()) else {
        return usage_error("脚本既没有 `compose` 也没有 `ops` 数组——它可能不是 Archbloom 脚本");
    };

    // 逐项取 op 名；缺失或类型不对都属用法错误，指名第几步
    let mut steps: Vec<(String, Value)> = Vec::with_capacity(ops.len());
    for (i, o) in ops.iter().enumerate() {
        let Some(name) = o.get("op").and_then(|v| v.as_str()) else {
            return usage_error(&format!("第 {i} 步缺少字符串字段 `op`"));
        };
        steps.push((name.to_string(), o.get("args").cloned().unwrap_or(Value::Null)));
    }

    let mut engine = Engine::new();
    let payload = match engine.execute_script(&steps) {
        Ok(reports) => serde_json::json!({ "ok": true, "reports": reports }),
        Err((idx, e)) => match e {
            EngineError::Op(op_err) => {
                serde_json::json!({ "ok": false, "failed_step": idx, "error": op_err })
            }
            EngineError::Verify(findings) => {
                serde_json::json!({ "ok": false, "failed_step": idx, "findings": findings })
            }
            EngineError::Conflict { expected, actual } => serde_json::json!({
                "ok": false,
                "failed_step": idx,
                "conflict": { "expected": expected, "actual": actual }
            }),
        },
    };
    println!(
        "{}",
        serde_json::to_string_pretty(&payload).unwrap_or_else(|e| format!("{{\"ok\":false,\"error\":\"输出序列化失败: {e}\"}}"))
    );
    if payload["ok"].as_bool().unwrap_or(false) {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}
