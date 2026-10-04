//! # rhai 脚本引擎集成探针（G6-M9）
//!
//! 跑**真实二进制**（`env!("CARGO_BIN_EXE_bim-server")`）的 `script` 子命令——
//! 照 cli_probe / http_probe 同款口径：断言退出码与 stdout/stderr，
//! 临时文件（脚本 / .bimdoc）用进程 id 隔离，跑完清理。
//!
//! ## 探针清单（3 个）
//!
//! 1. `script_models_and_saves_end_to_end`——脚本建模全流（造构件 → 建几何 →
//!    建视图/图纸 → 布置 → 保存）：exit 0 + 摘要输出 + `.bimdoc` 真落盘非空。
//! 2. `script_errors_refuse_unknown_op`——未知算子：非零退出 + stderr 含
//!    失败前缀与算子 id（错误响亮、带行号，绝不静默）。
//! 3. `script_opens_an_existing_document`——先 `new --save` 落盘，脚本
//!    `open_session` 打开：exit 0 + 摘要含恢复的元素。
//!
//! ## Windows 路径转义（实测发现）
//!
//! 脚本里的字符串路径**不能裸嵌 Windows 反斜杠**（`\U` 是 rhai 的无效转义，
//! 脚本编译失败）——[`script_path`] 把反斜杠统一转正斜杠（Windows API 两可）。

#![allow(clippy::unwrap_used, clippy::panic, clippy::expect_fun_call)]

use std::path::{Path, PathBuf};
use std::process::Command;

/// 临时路径（进程 id 隔离——并行测试不撞名）。
fn temp_path(tag: &str, ext: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "bim-server-script-probe-{}-{tag}.{ext}",
        std::process::id()
    ))
}

/// Windows 路径 → rhai 字符串安全形式（反斜杠转正斜杠——无效转义问题）。
fn script_path(p: &Path) -> String {
    p.display().to_string().replace('\\', "/")
}

/// 跑真实二进制，返回（退出码，stdout，stderr）。
fn run_server(args: &[&str]) -> (i32, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_bim-server"))
        .args(args)
        .output()
        .unwrap_or_else(|e| panic!("启动 bim-server 失败：{e}"));
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

/// 探针 1：脚本建模全流 → exit 0 + 摘要输出 + .bimdoc 真落盘非空。
#[test]
fn script_models_and_saves_end_to_end() {
    let script = temp_path("build", "rhai");
    let out_doc = temp_path("out", "bimdoc");
    std::fs::write(
        &script,
        format!(
            "let s = new_session();\n\
             let e = s.create_element(\"BEAM\", \"脚本梁\");\n\
             s.build_geometry(e, \"example.beam.rect\");\n\
             let v = s.create_view(\"俯视\");\n\
             let sh = s.create_sheet(\"一层平面\", \"建施-01\", \"A2\");\n\
             s.place_view_on_sheet(sh, v, 297.0, 210.0, 100);\n\
             print(\"summary: \" + s.summary());\n\
             s.save(\"{}\");\n",
            script_path(&out_doc)
        ),
    )
    .unwrap_or_else(|e| panic!("写脚本失败：{e}"));

    let (code, stdout, stderr) = run_server(&["script", "--path", &script.display().to_string()]);
    assert_eq!(code, 0, "应成功。stdout: {stdout} stderr: {stderr}");
    assert!(stdout.contains("summary: "), "应输出摘要：{stdout}");
    assert!(stdout.contains("元素 1"), "摘要应含元素数 1：{stdout}");
    assert!(stdout.contains("图纸 1"), "摘要应含图纸数 1：{stdout}");
    let saved = std::fs::metadata(&out_doc).unwrap_or_else(|e| panic!("读保存文件失败：{e}"));
    assert!(saved.len() > 0, "保存的 .bimdoc 应非空");

    let _ = std::fs::remove_file(&script);
    let _ = std::fs::remove_file(&out_doc);
}

/// 探针 2：坏脚本（未知算子）→ 非零退出 + 错误文本含算子 id（响亮拒绝）。
#[test]
fn script_errors_refuse_unknown_op() {
    let script = temp_path("bad", "rhai");
    std::fs::write(
        &script,
        "let s = new_session();\n\
         let e = s.create_element(\"BEAM\", \"\");\n\
         s.build_geometry(e, \"nonexistent.op\");\n",
    )
    .unwrap_or_else(|e| panic!("写脚本失败：{e}"));

    let (code, _stdout, stderr) = run_server(&["script", "--path", &script.display().to_string()]);
    assert_ne!(code, 0, "坏脚本应非零退出");
    assert!(
        stderr.contains("脚本执行失败"),
        "stderr 应含失败前缀：{stderr}"
    );
    assert!(
        stderr.contains("nonexistent.op"),
        "错误应含算子 id：{stderr}"
    );

    let _ = std::fs::remove_file(&script);
}

/// 探针 3：open_session 打开既有文档——先 new --save 落盘，脚本打开打印摘要。
#[test]
fn script_opens_an_existing_document() {
    let doc = temp_path("doc", "bimdoc");
    let script = temp_path("open", "rhai");

    let (code, stdout, stderr) = run_server(&[
        "new",
        "--name",
        "脚本梁",
        "--save",
        &doc.display().to_string(),
    ]);
    assert_eq!(code, 0, "new 应成功。stdout: {stdout} stderr: {stderr}");

    std::fs::write(
        &script,
        format!(
            "let s = open_session(\"{}\");\n\
             print(\"summary: \" + s.summary());\n",
            script_path(&doc)
        ),
    )
    .unwrap_or_else(|e| panic!("写脚本失败：{e}"));

    let (code, stdout, stderr) = run_server(&["script", "--path", &script.display().to_string()]);
    assert_eq!(code, 0, "脚本应成功。stdout: {stdout} stderr: {stderr}");
    assert!(
        stdout.contains("元素 1"),
        "打开的文档应含恢复的 1 个元素：{stdout}"
    );

    let _ = std::fs::remove_file(&script);
    let _ = std::fs::remove_file(&doc);
}
