//! # 无头服务端 `bim-server`（骨架期）
//!
//! 定位（设计文档 7.8）：批处理、转换，以及 **DWG 转换服务的宿主**。
//!
//! ## 为什么 DWG 必须在这里，而不在桌面端
//!
//! DWG 用 `LibreDWG`（GPLv3+）。GPLv3+ **一旦链接进主程序，整个程序都要按 GPL 开源**。
//! 所以它必须被隔离成一个**独立进程**，经 IPC/HTTP 调用——这就是本进程存在的理由。
//! 桌面端永远不会链接它。
//!
//! 当前 `main` 做 Core 装配自检，并打印隔离边界说明。

use std::process::ExitCode;

use core_app::ModuleHost;
use core_doc::DraftingStandard;
use example_beam::{CATEGORY, ExampleBeam};

fn main() -> ExitCode {
    println!("轻量化 BIM 内核平台 —— 无头服务端（骨架期）");
    println!("职责：批处理 / 格式转换 / DWG 隔离转换服务宿主\n");

    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(msg) => {
            eprintln!("✗ {msg}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let mut host = ModuleHost::new();
    host.install(Box::new(ExampleBeam))
        .map_err(|e| e.to_string())?;

    let s = host.summary();
    println!(
        "已装载模块 {}，注册表：品类 {} / 模式 {} / 工厂 {} / 专业图层 {}",
        host.module_count(),
        s.categories,
        s.schemas,
        s.factories,
        s.layers,
    );

    let mut standard = DraftingStandard::default();
    let applied = host
        .apply_layers_to(&mut standard)
        .map_err(|e| e.to_string())?;
    println!("制图标准装入专业图层：{applied} 个");

    // 走一次无头用例：造一个构件（不建几何），证明服务侧无需 GUI 即可驱动 Core。
    let beam = host
        .create_with_defaults(CATEGORY, "")
        .map_err(|e| e.to_string())?;
    println!(
        "无头试造构件：{}（品类 {}）",
        beam.name,
        beam.category.as_str()
    );

    println!("\n许可隔离边界（合规红线）：");
    println!("  LGPL-2.1（WASM 形态）  → occt-wasm，可同进程");
    println!("  MPL-2.0                → IFC 解析，文件级 copyleft，可静态链接");
    println!("  GPLv3+                 → LibreDWG（DWG），【禁止】链接进主程序，");
    println!("                           只能在本进程内隔离运行，经 IPC/HTTP 调用");

    println!("\n✓ 无头装配贯通。");
    Ok(())
}
