//! JSON 脚本执行器（agent 端面的最薄一层）：
//! 入参 {"ops":[{"op":"...","args":{...}}, ...]}，出参 {"ok":true,"reports":[...]}
//! 或 {"ok":false,"failed_step":n,"error"/"findings":...}。验证拦截同样走失败面。
use archcore_engine::{Engine, EngineError};
use serde_json::Value;
use std::fs;

fn main() {
    let path = std::env::args().nth(1).expect("用法: archcore-run <script.json>");
    let text = fs::read_to_string(&path).expect("读脚本失败");
    let script: Value = serde_json::from_str(&text).expect("脚本 JSON 解析失败");
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
            Err(EngineError::Op(op_err)) => {
                serde_json::json!({ "ok": false, "error": op_err })
            }
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
            serde_json::to_string_pretty(&payload).expect("输出序列化失败")
        );
        if !payload["ok"].as_bool().unwrap_or(false) {
            std::process::exit(1);
        }
        return;
    }
    let ops = script["ops"].as_array().expect("ops 必须是数组");
    let steps: Vec<(String, Value)> = ops
        .iter()
        .map(|o| {
            (
                o["op"].as_str().expect("op 字段缺失").to_string(),
                o.get("args").cloned().unwrap_or(Value::Null),
            )
        })
        .collect();

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
        serde_json::to_string_pretty(&payload).expect("输出序列化失败")
    );
    if !payload["ok"].as_bool().unwrap_or(false) {
        std::process::exit(1);
    }
}
