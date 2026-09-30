//! # xtask —— 仓库自动化任务
//!
//! 用法：
//!
//! ```text
//! cargo xtask check-deps   # 校验依赖方向（CI 硬门禁，见设计文档 2.2）
//! cargo xtask fmt          # 格式门禁（cargo fmt --all --check）
//! cargo xtask test         # 全量测试
//! cargo xtask clippy       # clippy（-D warnings）
//! cargo xtask gen-samples  # 生成黄金样本（M1 起启用）
//! cargo xtask golden       # 出图黄金样本比对（M5 起启用）
//! cargo xtask bench        # 性能回归（M3 起启用）
//! cargo xtask license-check # LGPL 声明随附（发布前启用）
//! cargo xtask help
//! ```
//!
//! ## 为什么 `check-deps` 值得存在
//!
//! 设计文档 2.2 那张"每个 crate 允许依赖谁"的表，光写在文档里没人会记得。
//! 铁律一旦被某次"顺手加一行 use"破坏，**编译器不会报错**——直到某天要换几何内核
//! 时才发现模块里塞满了 `occt_wasm::`。所以把它做成一个**失败即红的门禁**。
//!
//! 实现刻意**不引入第三方 TOML 解析库**：本仓库的清单结构足够简单，
//! 用几行文本解析即可，换来的是 xtask 自身零依赖（启动快、可审计）。

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::panic))]

use std::path::{Path, PathBuf};
use std::process::ExitCode;

fn main() -> ExitCode {
    let task = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "help".to_string());
    match run(&task) {
        Ok(()) => ExitCode::SUCCESS,
        Err(msg) => {
            eprintln!("\n✗ {msg}");
            ExitCode::FAILURE
        }
    }
}

fn run(task: &str) -> Result<(), String> {
    match task {
        "check-deps" => check_deps(),
        "fmt" => run_cargo(&["fmt", "--all", "--check"]),
        "test" => run_cargo(&["test", "--workspace"]),
        "clippy" => run_cargo(&[
            "clippy",
            "--workspace",
            "--all-targets",
            "--",
            "-D",
            "warnings",
        ]),
        "gen-samples" => {
            // 黄金样本（golden samples）依赖真实内核后端，属 M1 交付物。
            // 现在只把"该做什么"说清楚，避免静默成功让人误以为已经生成了样本。
            println!("gen-samples：尚未启用（依赖 M1 的真实内核后端）。");
            println!("  计划：对 testkit 里的固定夹具生成 BREP 文本黄金样本，");
            println!("        存于 testdata/golden/，供回归比对。");
            Ok(())
        }
        // 以下三项已被 .github/workflows/ci.yml 引用（且标注 continue-on-error）。
        // 在启用前它们必须"说清楚自己没做"，而不是让 CI 报 `未知任务`
        // ——后者会让红色失去信息量，久了就没人看 CI 了。
        "golden" => not_yet(
            "golden",
            "出图黄金样本比对",
            "M5 出图层落地后",
            vec![
                "对固定模型跑 平/立/剖 投影 + 标注，比对 testdata/golden/ 下的期望 SVG/DXF；",
                "线型、图层、可见/虚线归属逐条断言，避免'看起来对'。",
            ],
        ),
        "bench" => not_yet(
            "bench",
            "性能回归",
            "M3 几何构造落地后",
            vec![
                "基准：布尔运算、HLR 投影、网格化、拾取粗筛各自的耗时；",
                "与基线比对，超阈值即红（注意 occt-wasm 需 --release）。",
            ],
        ),
        "license-check" => not_yet(
            "license-check",
            "LGPL 声明随附",
            "发布前",
            vec![
                "发布产物须随附 occt-wasm 内嵌 WASM 的 LGPL-2.1 声明（设计文档 10.4 R4）；",
                "保证终端用户可替换该 WASM 组件（动态加载形态已满足，需在产物里留出替换路径）。",
            ],
        ),
        "help" | "--help" | "-h" => {
            print_help();
            Ok(())
        }
        other => Err(format!(
            "未知任务 `{other}`。可用任务见 `cargo xtask help`。"
        )),
    }
}

/// 尚未启用的任务：**如实说明**，并以 0 退出（CI 里标了 `continue-on-error`）。
///
/// 刻意不静默成功——静默成功会让人以为"这一项已经过了"。
fn not_yet(task: &str, what: &str, when: &str, plan: Vec<&str>) -> Result<(), String> {
    println!("{task}：尚未启用（{what}，计划在{when}启用）。");
    for line in plan {
        println!("  {line}");
    }
    Ok(())
}

fn print_help() {
    println!("xtask —— 仓库自动化任务");
    println!();
    println!("  cargo xtask check-deps    校验依赖方向（设计文档 2.2 铁律）");
    println!("  cargo xtask fmt           格式门禁（cargo fmt --all --check）");
    println!("  cargo xtask test          运行全量测试");
    println!("  cargo xtask clippy        运行 clippy（-D warnings）");
    println!("  cargo xtask gen-samples   生成黄金样本（M1 起启用）");
    println!("  cargo xtask golden        出图黄金样本比对（M5 起启用）");
    println!("  cargo xtask bench         性能回归（M3 起启用）");
    println!("  cargo xtask license-check LGPL 声明随附（发布前启用）");
    println!("  cargo xtask help          显示本帮助");
}

// =============================================================================
// 依赖方向门禁
// =============================================================================

/// 仓库内部 crate 的名单。
///
/// 凡出现在一个 crate 的依赖里、又命中此名单的，都是"内部依赖"，
/// 必须逐条对照设计文档 2.2 放行。
const INTERNAL_CRATES: &[&str] = &[
    "core-kernel",
    "core-model",
    "core-doc",
    "core-interop",
    "core-sdk",
    "core-app",
    "core-testkit",
    "example-beam",
];

/// **允许直接依赖 `occt-wasm` 的 crate**（设计文档 1.2 铁律 1 的边界）。
const KERNEL_BACKEND_ALLOWED: &[&str] = &["core-kernel", "core-interop"];

/// 按设计文档 2.2 表格，给出每个 core crate 允许依赖的内部 crate。
///
/// 返回 `None` 表示"这个 crate 不受该表约束"（app / module / 未知成员），
/// 由各自的专门规则处理。
fn internal_allow_list(crate_name: &str) -> Option<&'static [&'static str]> {
    match crate_name {
        "core-kernel" => Some(&[]),
        "core-model" => Some(&[]),
        "core-doc" => Some(&["core-model"]),
        "core-interop" => Some(&["core-kernel", "core-model"]),
        "core-sdk" => Some(&["core-model"]),
        "core-app" => Some(&[
            "core-kernel",
            "core-model",
            "core-doc",
            "core-interop",
            "core-sdk",
        ]),
        "core-testkit" => Some(&[
            "core-kernel",
            "core-model",
            "core-doc",
            "core-interop",
            "core-sdk",
            "core-app",
        ]),
        "xtask" => Some(&[]),
        _ => None,
    }
}

fn check_deps() -> Result<(), String> {
    let root = workspace_root();
    let root_manifest = read(&root.join("Cargo.toml"))?;
    let members = extract_members(&root_manifest);
    if members.is_empty() {
        return Err("未能从根 Cargo.toml 解析出 [workspace] members".to_string());
    }

    println!("依赖方向校验（设计文档 2.2）");
    println!("工作区根：{}", root.display());
    println!("成员数：{}", members.len());
    println!();

    let mut violations: Vec<String> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();

    for member in &members {
        let manifest_path = root.join(member).join("Cargo.toml");
        let manifest = read(&manifest_path)?;
        let name = field(&manifest, "name").unwrap_or_else(|| member.clone());
        let deps = deps_of(&manifest);
        let internal: Vec<&str> = deps
            .iter()
            .map(String::as_str)
            .filter(|d| INTERNAL_CRATES.contains(d))
            .collect();

        println!("  {name:<14} 内部依赖：[{}]", internal.join(", "));

        // ---- 规则 A：core-* 逐条对照 2.2 表 ----
        if let Some(allow) = internal_allow_list(&name) {
            for dep in &internal {
                if !allow.contains(dep) {
                    violations.push(format!(
                        "{name} 不允许依赖 {dep}（设计文档 2.2：{name} 只允许依赖 [{}]）",
                        if allow.is_empty() {
                            "无".to_string()
                        } else {
                            allow.join(", ")
                        }
                    ));
                }
            }
        } else if is_module(member) {
            // ---- 规则 B：modules/* 只允许依赖 core-sdk ----
            for dep in &internal {
                if *dep != "core-sdk" {
                    violations.push(format!(
                        "{name} 是专业模块，只允许依赖 core-sdk，却依赖了 {dep}（铁律 3）"
                    ));
                }
            }
        } else if is_app(member) {
            // ---- 规则 C：apps/* 是顶层，允许依赖任意 core-*，但应当有 core-app ----
            if !internal.contains(&"core-app") {
                warnings.push(format!(
                    "{name} 未依赖 core-app，应用外壳一般应通过它编排用例"
                ));
            }
        }

        // ---- 规则 D：occt-wasm 只允许出现在内核适配相关 crate ----
        if deps.iter().any(|d| d == "occt-wasm") && !KERNEL_BACKEND_ALLOWED.contains(&name.as_str())
        {
            violations.push(format!(
                "{name} 直接依赖 occt-wasm，违反铁律 1（只允许 {KERNEL_BACKEND_ALLOWED:?}）"
            ));
        }
    }

    println!();
    for w in &warnings {
        println!("⚠ {w}");
    }
    if violations.is_empty() {
        println!("✓ 依赖方向全部合规。");
        return Ok(());
    }
    Err(format!(
        "发现 {} 处依赖方向违规：\n  - {}",
        violations.len(),
        violations.join("\n  - ")
    ))
}

// =============================================================================
// 小工具
// =============================================================================

/// 工作区根目录：xtask 位于 `<root>/xtask`，故其清单目录的父目录即根。
fn workspace_root() -> PathBuf {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .parent()
        .map_or_else(|| manifest_dir.to_path_buf(), Path::to_path_buf)
}

fn is_module(member: &str) -> bool {
    member.replace('\\', "/").starts_with("modules/")
}

fn is_app(member: &str) -> bool {
    member.replace('\\', "/").starts_with("apps/")
}

fn read(path: &Path) -> Result<String, String> {
    std::fs::read_to_string(path).map_err(|e| format!("读取 {} 失败：{e}", path.display()))
}

fn run_cargo(args: &[&str]) -> Result<(), String> {
    println!("→ cargo {}", args.join(" "));
    let status = std::process::Command::new("cargo")
        .args(args)
        .current_dir(workspace_root())
        .status()
        .map_err(|e| format!("无法启动 cargo：{e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "`cargo {}` 失败（退出码 {:?}）",
            args.join(" "),
            status.code()
        ))
    }
}

/// 从清单里取一个 `[package]` 下的标量字段（如 `name`）。
fn field(manifest: &str, key: &str) -> Option<String> {
    let mut section = String::new();
    for line in manifest.lines() {
        let l = line.trim();
        if l.starts_with('[') && l.ends_with(']') {
            section = l.trim_matches(|c| c == '[' || c == ']').to_string();
            continue;
        }
        if section != "package" || l.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = l.split_once('=')
            && k.trim() == key
        {
            return Some(v.trim().trim_matches('"').to_string());
        }
    }
    None
}

/// 解析 `[workspace] members = [ "a", "b" ]`（支持跨行）。
fn extract_members(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut in_list = false;
    for line in text.lines() {
        let l = line.trim();
        if !in_list {
            if l.starts_with("members") && l.contains('[') {
                in_list = true;
                let rest = l.split_once('[').map_or("", |(_, r)| r);
                collect_quoted(rest, &mut out);
                if rest.contains(']') {
                    in_list = false;
                }
            }
        } else {
            collect_quoted(l, &mut out);
            if l.contains(']') {
                in_list = false;
            }
        }
    }
    out
}

/// 收集一行里所有双引号内的字符串（用于解析 TOML 数组字面量）。
fn collect_quoted(text: &str, out: &mut Vec<String>) {
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '"' {
            continue;
        }
        let mut buf = String::new();
        for c in chars.by_ref() {
            if c == '"' {
                break;
            }
            buf.push(c);
        }
        if !buf.is_empty() {
            out.push(buf);
        }
    }
}

/// 取出清单里所有依赖段的**依赖名**（去重）。
///
/// 覆盖 `[dependencies]` / `[dev-dependencies]` / `[build-dependencies]`
/// 以及 `[target.'cfg(...)'.dependencies]` 这类点号命名段。
fn deps_of(manifest: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut section = String::new();
    for line in manifest.lines() {
        let l = line.trim();
        if l.starts_with('[') && l.ends_with(']') {
            section = l.trim_matches(|c| c == '[' || c == ']').to_string();
            continue;
        }
        if l.is_empty() || l.starts_with('#') || !is_dep_section(&section) {
            continue;
        }
        if let Some(key) = dep_key(l)
            && !out.contains(&key)
        {
            out.push(key);
        }
    }
    out
}

fn is_dep_section(section: &str) -> bool {
    section == "dependencies"
        || section == "dev-dependencies"
        || section == "build-dependencies"
        || section.ends_with(".dependencies")
        || section.ends_with(".dev-dependencies")
        || section.ends_with(".build-dependencies")
}

/// 从 `foo = { ... }` / `foo.workspace = true` 里取依赖名 `foo`。
fn dep_key(line: &str) -> Option<String> {
    let (lhs, _) = line.split_once('=')?;
    let key = lhs.trim().trim_matches('"').trim();
    let key = key.split('.').next()?.trim();
    if key.is_empty() {
        None
    } else {
        Some(key.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn members_are_extracted_across_lines() {
        let text = "[workspace]\nmembers = [\n  \"a\",\n  \"crates/b\",\n]\n";
        assert_eq!(extract_members(text), vec!["a", "crates/b"]);
    }

    #[test]
    fn dependency_keys_are_read_from_all_dep_sections() {
        let text = "\
[package]
name = \"x\"
version.workspace = true

[dependencies]
core-model.workspace = true
serde = { workspace = true }

[dev-dependencies]
core-testkit.workspace = true

[target.'cfg(unix)'.dependencies]
libc = \"0.2\"
";
        let deps = deps_of(text);
        assert!(deps.contains(&"core-model".to_string()));
        assert!(deps.contains(&"serde".to_string()));
        assert!(deps.contains(&"core-testkit".to_string()));
        assert!(deps.contains(&"libc".to_string()));
        assert!(!deps.contains(&"name".to_string()), "package 段不是依赖");
    }

    #[test]
    fn dotted_dependency_keys_are_truncated() {
        assert_eq!(
            dep_key("core-model.workspace = true").as_deref(),
            Some("core-model")
        );
        assert_eq!(
            dep_key("serde = { workspace = true }").as_deref(),
            Some("serde")
        );
    }

    #[test]
    fn package_name_is_read() {
        let text = "[package]\nname = \"core-kernel\"\nedition.workspace = true\n";
        assert_eq!(field(text, "name").as_deref(), Some("core-kernel"));
    }

    #[test]
    fn the_allow_table_matches_the_design_doc() {
        assert_eq!(
            internal_allow_list("core-kernel").map(<[&str]>::len),
            Some(0)
        );
        assert_eq!(
            internal_allow_list("core-model").map(<[&str]>::len),
            Some(0)
        );
        assert_eq!(internal_allow_list("core-doc"), Some(&["core-model"][..]));
        assert!(
            internal_allow_list("bim-desktop").is_none(),
            "app 不受该表约束"
        );
    }

    #[test]
    fn module_and_app_paths_are_classified() {
        assert!(is_module("modules/example-beam"));
        assert!(is_app("apps/desktop"));
        assert!(!is_module("crates/core-app"));
    }
}
