//! # 桌面端外壳 `bim-desktop`
//!
//! 两种运行形态（feature 分支，同一入口）：
//!
//! - **`--features gui`（M8 起点，已实现）**：真实图形窗口——视口（轨道相机 +
//!   面着色渲染）、拾取（复用 §6.6 KernelPicker 链路）、属性面板、构件列表、
//!   操作日志。实现见 [`gui`]。
//! - **默认（无 feature）**：控制台端到端演示——把五层逐层装配、跑通端到端
//!   建模+出图闭环并打印诊断：
//!
//! ```text
//! CoreSession → 装载 ExampleBeam → 造一根梁 → 造几何 → 出图（俯视投影）→ 存 .bimdoc → 重开 → 断言
//! ```
//!
//! 自动化断言载体：`apps/desktop/tests/e2e_mock.rs`（门禁自动跑，`assert!` 形态）。
//!
//! ## 为什么现在能直接用 `example-beam` 自己的算子
//!
//! `example-beam` 的几何算子是 `MakeBox → Translate`（按定位点放置）。内核门面
//! （`Kernel::translate`）现已暴露平移算子，[`core_app::GeometryService::execute`]
//! 能执行平移步骤并**如实记下定位点**——因此本演示**直接用 `example-beam`
//! 注册的算子**（`example.beam.rect`），不再需要骨架期那个临时的"只发一步
//! MakeBox"方盒模块。
//!
//! ⚠ **注意控制台演示跑在占位内核上**：[`CoreSession::new`] 显式选用
//! [`core_app::GeometryService::with_mock_kernel`]，其平移只是把偏移累加进占位
//! 几何的 `origin`（并写进 `MOCK-BREP v3` 文本），**不是** OCCT 的精确布尔/变换。
//! 出图环的 P0 投影同理：mock 内核的投影是**理想化正面视（忽略视向）**，
//! 只证明图元生成与数据流贯通。真几何与真投影由 `occt` feature + 真实后端保证；
//! 这里要证明的是**五层装配与端到端数据流贯通**（新建 → 建几何 → 出图 →
//! 存盘 → 重开 → 校验），而非几何精度。

use std::process::ExitCode;

#[cfg(feature = "gui")]
mod gui;

#[cfg(feature = "gui")]
mod theme;

/// 草图纯几何算法库（G4-M14）：纯算法零依赖 GUI 状态，不门控 feature（默认腿可单测）。
mod sketch_geom;

/// 草图工具纯逻辑（G4-M15）：状态机 + 捕捉 / 正交纯函数，不门控 feature（默认腿可单测）。
mod sketch_tool;

/// 草图编辑器浮窗（G4-M15）：egui 渲染与交互（依赖 gui feature）。
#[cfg(feature = "gui")]
mod sketch_editor;

#[cfg(feature = "gui")]
fn main() -> ExitCode {
    match gui::run_gui() {
        Ok(()) => ExitCode::SUCCESS,
        Err(msg) => {
            eprintln!("✗ {msg}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(not(feature = "gui"))]
use core_app::CoreSession;

#[cfg(not(feature = "gui"))]
use core_doc::{DraftingStandard, ViewOrientation};

#[cfg(not(feature = "gui"))]
use example_beam::{CATEGORY, ExampleBeam, GEOMETRY_OP_ID};

#[cfg(not(feature = "gui"))]
fn main() -> ExitCode {
    println!("轻量化 BIM 内核平台 —— 桌面端（骨架期）");
    println!("真实 GUI 属 M1+ 交付物；本次运行做的是端到端建模闭环演示。\n");

    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(msg) => {
            eprintln!("✗ {msg}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(not(feature = "gui"))]
fn run() -> Result<(), String> {
    // --- 1. 组装会话并装载模块 ---
    let mut session = CoreSession::new().map_err(|e| e.to_string())?;
    session
        .install_module(Box::new(ExampleBeam))
        .map_err(|e| e.to_string())?;

    println!("已装载模块（{}）：", session.modules().module_count());
    for m in session.modules().manifests() {
        println!("  - {} v{}  {}", m.id, m.version, m.name);
    }

    // --- 2. 注册表概览 ---
    let reg = session.modules().summary();
    println!(
        "\n注册表：品类 {} / 属性模式 {} / 工厂 {} / 算子 {} / 标注 {} / 专业图层 {}",
        reg.categories,
        reg.schemas,
        reg.factories,
        reg.geometry_ops,
        reg.annotation_rules,
        reg.layers
    );

    // --- 3. 把模块声明的专业图层装进制图标准 ---
    let mut standard = DraftingStandard::default();
    let applied = session
        .modules()
        .apply_layers_to(&mut standard)
        .map_err(|e| e.to_string())?;
    println!("专业图层装入制图标准：{applied} 个");

    // --- 4. 走一次"新建构件"用例（例：混凝土梁）---
    let beam = session
        .create_element(CATEGORY, "")
        .map_err(|e| e.to_string())?;
    let beam_name = session
        .document()
        .doc()
        .element(beam)
        .map(|e| e.name.clone())
        .unwrap_or_default();
    println!("新建构件：{beam_name}（品类 {CATEGORY}，id {beam}）");

    // --- 5. 造几何（执行 example-beam 的 MakeBox → Translate 配方）---
    let shape = session
        .build_geometry(beam, GEOMETRY_OP_ID)
        .map_err(|e| e.to_string())?;
    println!(
        "造几何：算子 {GEOMETRY_OP_ID} → shape {shape}（几何记录 {} 份）",
        session.document().doc().geometry_count()
    );

    // --- 6. 出图环（M6a 新增）：建俯视图并做 P0 投影初生成 ---
    let view_id = session
        .create_view("俯视", ViewOrientation::top())
        .map_err(|e| e.to_string())?;
    let (view, standard) = {
        let drawing = session.document().drawing();
        let view = drawing
            .view(view_id)
            .cloned()
            .ok_or_else(|| format!("视图 {view_id} 未登记进图纸"))?;
        let standard = drawing.standard.clone();
        (view, standard)
    };
    let (diag_entities, diag_warnings) = session
        .project_shape(shape, &view, &standard)
        .map_err(|e| e.to_string())?;
    println!(
        "出图：视图 {view_id}（俯视）初生成 {} 条图元；单形状诊断 {} 条 / 警告 {} 条",
        view.entities.len(),
        diag_entities.len(),
        diag_warnings.len()
    );

    // --- 7. 存盘到临时目录下的 .bimdoc ---
    let drawing_before = session.document().drawing().clone();
    let mut path = std::env::temp_dir();
    path.push(format!("bim-skeleton-{}.bimdoc", std::process::id()));
    session.save(&path).map_err(|e| e.to_string())?;
    println!("存盘：{}", path.display());

    // --- 8. 重新打开 ---
    let reopened = CoreSession::open(&path).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_file(&path);

    // --- 9. 断言：元素数量与品类一致 + Drawing 往返 ---
    let count = reopened.document().element_count();
    if count != 1 {
        return Err(format!("重开后元素数应为 1，实得 {count}"));
    }
    let all_beam = reopened
        .document()
        .doc()
        .elements()
        .values()
        .all(|e| e.category.as_str() == CATEGORY);
    if !all_beam {
        return Err("重开后存在非 BEAM 品类的元素".to_string());
    }

    // Drawing 往返（M6a 新增）：视图数、实体数、整图深相等
    let reopened_drawing = reopened.document().drawing();
    if reopened_drawing.view_count() != 1 {
        return Err(format!(
            "重开后视图数应为 1，实得 {}",
            reopened_drawing.view_count()
        ));
    }
    let reopened_view = reopened_drawing
        .views
        .values()
        .next()
        .ok_or_else(|| "重开后图纸中无视图".to_string())?;
    if reopened_view.entities.len() != view.entities.len() {
        return Err(format!(
            "重开后视图实体数应为 {}，实得 {}",
            view.entities.len(),
            reopened_view.entities.len()
        ));
    }
    if *reopened_drawing != drawing_before {
        return Err("重开后 Drawing 与存盘前不一致（往返丢失数据）".to_string());
    }

    let sum = reopened.summary();
    println!(
        "重开校验：元素 {} / 几何 {} / 视图 {} / 品类一致 ✓（模块 {}，专业图层 {}）",
        sum.elements,
        sum.geometry,
        reopened_drawing.view_count(),
        sum.modules,
        sum.layers
    );

    println!("\n✓ 五层装配贯通：kernel → model → doc/interop → sdk → app → desktop");
    println!("✓ 端到端闭环：新建 → 建几何（含平移）→ 出图 → 存盘 → 重开 → 校验");
    Ok(())
}
