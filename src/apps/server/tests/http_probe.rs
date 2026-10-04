//! HTTP 桥的端到端探针（G6-M7）：spawn **真实二进制**（`serve --port 0`，OS 分配
//! 端口，从 stdout 解析实际地址），手写 TCP HTTP/1.1 探针（零新依赖——不经 mock
//! 的真实 HTTP 闭环），断言状态码与 JSON 体——建模导出多步工作流的真实形态验证。

#![allow(clippy::unwrap_used, clippy::panic, clippy::expect_fun_call)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};

/// 起一个真实 HTTP 服务（`--port 0` 让 OS 分配），从 stdout 启动行解析实际端口。
fn spawn_server() -> (Child, u16) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_bim-server"))
        .args(["serve", "--port", "0"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("启动 bim-server serve 失败");
    let stdout = child.stdout.take().expect("服务无 stdout");
    let mut reader = BufReader::new(stdout);
    let mut line = String::new();
    reader
        .read_line(&mut line)
        .expect("读服务启动行失败（服务可能未启动）");
    // 行格式：✓ HTTP 服务已启动：http://127.0.0.1:PORT
    let port: u16 = line
        .rsplit(':')
        .next()
        .and_then(|p| p.trim().parse().ok())
        .unwrap_or_else(|| panic!("无法从启动行解析端口：{line}"));
    (child, port)
}

/// 手写 HTTP/1.1 探针：连接 → 请求 → 读到 EOF（Connection: close）→ (状态码, 响应体)。
fn http(port: u16, method: &str, path: &str, body: Option<&str>) -> (u16, String) {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("连接服务失败");
    let payload = body.unwrap_or("");
    let req = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
        payload.len()
    );
    stream.write_all(req.as_bytes()).expect("写请求失败");
    let mut raw = String::new();
    stream.read_to_string(&mut raw).expect("读响应失败");
    let status: u16 = raw
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| panic!("无法解析状态行：{raw}"));
    let body = raw.split("\r\n\r\n").nth(1).unwrap_or("").to_string();
    (status, body)
}

/// 从 JSON 响应体提取字符串字段（serde_json 解析——不给字符串扫描留机会）。
fn extract_str(body: &str, key: &str) -> String {
    let v: serde_json::Value = serde_json::from_str(body).expect("响应体不是合法 JSON");
    v.get(key)
        .and_then(|s| s.as_str())
        .unwrap_or_else(|| panic!("体中无 {key} 字段：{body}"))
        .to_string()
}

/// 临时目录下的一次性路径（进程 id 隔离，测试结束不清理——临时目录）。
fn temp_path(tag: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!("bim-http-{}-{tag}", std::process::id()));
    path
}

#[test]
fn health_and_session_lifecycle() {
    let (mut child, port) = spawn_server();

    // ① health：服务名 + 版本
    let (code, body) = http(port, "GET", "/health", None);
    assert_eq!(code, 200, "health 应 200，实得：{body}");
    assert!(body.contains("bim-server"), "应报服务名，实得：{body}");

    // ② 创建会话（空 body = 新建并装 ExampleBeam）
    let (code, body) = http(port, "POST", "/sessions", Some("{}"));
    assert_eq!(code, 200, "创建会话应 200，实得：{body}");
    let sid = extract_str(&body, "session_id");
    assert!(!sid.is_empty(), "session_id 应非空");

    // ③ summary：模块 1（ExampleBeam）/ 元素 0
    let (code, body) = http(port, "GET", &format!("/sessions/{sid}/summary"), None);
    assert_eq!(code, 200, "summary 应 200，实得：{body}");
    assert!(body.contains("\"elements\":0"), "应报 0 元素，实得：{body}");

    // ④ 销毁
    let (code, _) = http(port, "DELETE", &format!("/sessions/{sid}"), None);
    assert_eq!(code, 200, "销毁应 200");

    // ⑤ 销毁后 summary → 404
    let (code, body) = http(port, "GET", &format!("/sessions/{sid}/summary"), None);
    assert_eq!(code, 404, "销毁后应 404，实得：{body}");

    let _ = child.kill();
}

#[test]
fn modeling_and_export_flow() {
    let (mut child, port) = spawn_server();

    // ① 创建会话
    let (code, body) = http(port, "POST", "/sessions", Some("{}"));
    assert_eq!(code, 200, "创建会话应 200，实得：{body}");
    let sid = extract_str(&body, "session_id");

    // ② 造构件（全默认：BEAM + 品类默认名）
    let (code, body) = http(
        port,
        "POST",
        &format!("/sessions/{sid}/elements"),
        Some("{}"),
    );
    assert_eq!(code, 200, "造构件应 200，实得：{body}");
    let element_id = extract_str(&body, "element_id");

    // ③ 建几何（全默认：example.beam.rect）
    let (code, body) = http(
        port,
        "POST",
        &format!("/sessions/{sid}/geometry"),
        Some(&format!(r#"{{"element_id":"{element_id}"}}"#)),
    );
    assert_eq!(code, 200, "建几何应 200，实得：{body}");
    let shape_id = extract_str(&body, "shape_id");
    assert!(!shape_id.is_empty(), "shape_id 应非空");

    // ④ 建视图（俯视）
    let (code, body) = http(
        port,
        "POST",
        &format!("/sessions/{sid}/views"),
        Some(r#"{"name":"俯视"}"#),
    );
    assert_eq!(code, 200, "建视图应 200，实得：{body}");
    let view_id = extract_str(&body, "view_id");

    // ⑤ 建图纸（A2）
    let (code, body) = http(
        port,
        "POST",
        &format!("/sessions/{sid}/sheets"),
        Some(r#"{"name":"一层平面","number":"建施-01","format":"A2"}"#),
    );
    assert_eq!(code, 200, "建图纸应 200，实得：{body}");
    let sheet_id = extract_str(&body, "sheet_id");

    // ⑥ 布置视图（A2 中心，1:100）
    let (code, body) = http(
        port,
        "POST",
        &format!("/sessions/{sid}/placements"),
        Some(&format!(
            r#"{{"sheet_id":"{sheet_id}","view_id":"{view_id}","x":297.0,"y":210.0,"scale":100}}"#
        )),
    );
    assert_eq!(code, 200, "布置视图应 200，实得：{body}");

    // ⑦ 保存（请求体用 serde_json::json! 构造——Windows 路径反斜杠自动转义，
    //    format! 裸拼会把 `\U`/`\A` 当非法 JSON 转义 → 400）
    let doc = temp_path("flow.bimdoc");
    let body = serde_json::json!({ "path": doc.display().to_string() }).to_string();
    let (code, _) = http(port, "POST", &format!("/sessions/{sid}/save"), Some(&body));
    assert_eq!(code, 200, "保存应 200");
    assert!(doc.exists(), "保存路径应存在");

    // ⑧ 导出 SVG（按视图名）
    let svg = temp_path("flow.svg");
    let body = serde_json::json!({
        "format": "svg",
        "out": svg.display().to_string(),
        "view": "俯视",
    })
    .to_string();
    let (code, body) = http(
        port,
        "POST",
        &format!("/sessions/{sid}/export"),
        Some(&body),
    );
    assert_eq!(code, 200, "导出应 200，实得：{body}");
    assert!(!body.contains("\"bytes\":0"), "SVG 应非空，实得：{body}");
    assert!(svg.exists(), "SVG 应落盘");

    // ⑨ 撤销（撤销布置 → ok）
    let (code, body) = http(port, "POST", &format!("/sessions/{sid}/undo"), Some("{}"));
    assert_eq!(code, 200, "撤销应 200，实得：{body}");
    assert!(body.contains("\"ok\":true"), "撤销应成功，实得：{body}");

    // ⑩ 重做
    let (code, body) = http(port, "POST", &format!("/sessions/{sid}/redo"), Some("{}"));
    assert_eq!(code, 200, "重做应 200，实得：{body}");
    assert!(body.contains("\"ok\":true"), "重做应成功，实得：{body}");

    // ⑪ 导出无目标 → 400
    let body = serde_json::json!({
        "format": "svg",
        "out": svg.display().to_string(),
    })
    .to_string();
    let (code, body) = http(
        port,
        "POST",
        &format!("/sessions/{sid}/export"),
        Some(&body),
    );
    assert_eq!(code, 400, "无目标应 400，实得：{body}");

    let _ = child.kill();
    let _ = std::fs::remove_file(&doc);
    let _ = std::fs::remove_file(&svg);
}
