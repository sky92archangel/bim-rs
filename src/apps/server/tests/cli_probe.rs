//! CLI 批处理驱动的端到端探针（G6-M6）：跑**真实二进制**（`env!("CARGO_BIN_EXE_…")`
//! 由 cargo 提供路径），断言退出码与输出——命令行闭环（new → info → export）的
//! 真实形态验证，不经 GUI 也不 mock。

use std::path::PathBuf;
use std::process::Command;

/// 跑一条 CLI 命令，返回 (退出码, stdout, stderr)。
fn run(args: &[&str]) -> (i32, String, String) {
    let exe = env!("CARGO_BIN_EXE_bim-server");
    let out = Command::new(exe)
        .args(args)
        .output()
        .expect("启动 bim-server 失败");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

/// 临时目录下的一次性文档路径（进程 id + 测试名隔离，测试结束不清理——临时目录）。
fn temp_doc(tag: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!("bim-cli-{}-{tag}.bimdoc", std::process::id()));
    path
}

#[test]
fn demo_self_check_passes() {
    let (code, stdout, stderr) = run(&["demo"]);
    assert_eq!(code, 0, "demo 应退出 0，stderr：{stderr}");
    assert!(
        stdout.contains("无头装配贯通"),
        "应打印装配贯通，实得：{stdout}"
    );
}

#[test]
fn new_then_info_roundtrip() {
    let doc = temp_doc("info");
    let doc_str = doc.to_string_lossy().to_string();

    // ① new：造构件 + 建几何 + 建视图 + 保存
    let (code, stdout, stderr) = run(&[
        "new",
        "--name",
        "一层梁",
        "--view",
        "俯视",
        "--save",
        &doc_str,
    ]);
    assert_eq!(code, 0, "new 应退出 0，stderr：{stderr}");
    assert!(
        stdout.contains("构件已创建"),
        "应打印构件创建，实得：{stdout}"
    );
    assert!(stdout.contains("几何已建"), "应打印几何，实得：{stdout}");
    assert!(stdout.contains("视图已建"), "应打印视图，实得：{stdout}");
    assert!(doc.exists(), "保存路径应存在");

    // ② info：打开打印摘要——元素/几何/视图数一致
    let (code, stdout, stderr) = run(&["info", "--doc", &doc_str]);
    assert_eq!(code, 0, "info 应退出 0，stderr：{stderr}");
    assert!(stdout.contains("一层梁"), "应列出构件名，实得：{stdout}");
    assert!(stdout.contains("视图 1"), "应报告 1 个视图，实得：{stdout}");

    let _ = std::fs::remove_file(&doc);
}

#[test]
fn new_with_view_then_export_svg() {
    let doc = temp_doc("svg");
    let doc_str = doc.to_string_lossy().to_string();
    let mut out_svg = std::env::temp_dir();
    out_svg.push(format!("bim-cli-{}-out.svg", std::process::id()));
    let out_str = out_svg.to_string_lossy().to_string();

    let (code, _, stderr) = run(&["new", "--view", "俯视", "--save", &doc_str]);
    assert_eq!(code, 0, "new 应退出 0，stderr：{stderr}");

    let (code, stdout, stderr) = run(&[
        "export", "--doc", &doc_str, "--format", "svg", "--out", &out_str, "--view", "俯视",
    ]);
    assert_eq!(code, 0, "export 应退出 0，stderr：{stderr}");
    assert!(stdout.contains("已导出"), "应打印导出，实得：{stdout}");
    assert!(out_svg.exists(), "SVG 应写出");
    let bytes = std::fs::read(&out_svg).unwrap_or_default();
    assert!(!bytes.is_empty(), "SVG 应非空");
    let head = String::from_utf8_lossy(&bytes[..bytes.len().min(200)]).to_string();
    assert!(head.contains("<svg"), "SVG 应有 svg 头，实得：{head}");

    let _ = std::fs::remove_file(&doc);
    let _ = std::fs::remove_file(&out_svg);
}

#[test]
fn export_requires_target() {
    let doc = temp_doc("notarget");
    let doc_str = doc.to_string_lossy().to_string();
    let (code, _, _) = run(&["new", "--save", &doc_str]);
    assert_eq!(code, 0, "new 应退出 0");

    let mut out = std::env::temp_dir();
    out.push(format!("bim-cli-{}-notarget.dxf", std::process::id()));
    let out_str = out.to_string_lossy().to_string();
    let (code, _, stderr) = run(&[
        "export", "--doc", &doc_str, "--format", "dxf", "--out", &out_str,
    ]);
    assert_ne!(code, 0, "无目标应失败");
    assert!(
        stderr.contains("--sheet") || stderr.contains("--view"),
        "应提示二选一，实得：{stderr}"
    );

    let _ = std::fs::remove_file(&doc);
}

#[test]
fn export_sheet_not_found_reports_known_numbers() {
    let doc = temp_doc("nosheet");
    let doc_str = doc.to_string_lossy().to_string();
    let (code, _, _) = run(&["new", "--save", &doc_str]);
    assert_eq!(code, 0);

    let mut out = std::env::temp_dir();
    out.push(format!("bim-cli-{}-nosheet.dxf", std::process::id()));
    let out_str = out.to_string_lossy().to_string();
    let (code, _, stderr) = run(&[
        "export",
        "--doc",
        &doc_str,
        "--format",
        "dxf",
        "--out",
        &out_str,
        "--sheet",
        "不存在-01",
    ]);
    assert_ne!(code, 0, "未知图号应失败");
    assert!(
        stderr.contains("未找到图号"),
        "应报未知图号，实得：{stderr}"
    );

    let _ = std::fs::remove_file(&doc);
}
