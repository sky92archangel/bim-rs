//! # 无头服务端 `bim-server`
//!
//! 定位（设计文档 7.8）：批处理、转换，以及 **DWG 转换服务的宿主**。
//!
//! ## 为什么 DWG 必须在这里，而不在桌面端
//!
//! DWG 用 `LibreDWG`（GPLv3+）。GPLv3+ **一旦链接进主程序，整个程序都要按 GPL 开源**。
//! 所以它必须被隔离成一个**独立进程**，经 IPC/HTTP 调用——这就是本进程存在的理由。
//! 桌面端永远不会链接它。
//!
//! ## CLI 批处理驱动（G6-M6，外部调用形态第一期）
//!
//! [`ExportOutput`] 的文档注释「便于命令行与测试断言」在本节落地——
//! 进程内 CoreSession API（§2.1 契约）之上的一层薄壳，每个子命令直接转发门面：
//!
//! ```text
//! bim-server demo                    # 端到端装配自检（默认）
//! bim-server new --name 一层梁 --category BEAM --op example.beam.rect --save out.bimdoc
//! bim-server info doc.bimdoc         # 打开并打印摘要
//! bim-server export doc.bimdoc --format dxf --out out.dxf --sheet 建施-01
//! bim-server export doc.bimdoc --format svg --out out.svg --view 俯视
//! ```
//!
//! 模块装载：CLI 默认装载 [`ExampleBeam`]（本 crate 既有依赖）——
//! 品类与算子的默认值直接引用模块导出常量 [`CATEGORY`] / [`GEOMETRY_OP_ID`]，
//! 与桌面端 `apps/desktop` 同款，保持单一事实源。
//!
//! ## HTTP 桥（G6-M7，外部调用形态第二期）
//!
//! `bim-server serve --host 127.0.0.1 --port 8300` 启动 axum REST 服务，
//! 暴露 CoreSession **会话态门面**（多步工作流：建会话 → 造构件 → 建几何 →
//! 建视图/图纸 → 布置 → 保存/导出 → 撤销/重做）——端点清单见 [`crate::http`] 模块文档。
//!
//! ## rhai 脚本引擎（G6-M9 第三期 + G6-M11 门面补全）
//!
//! `bim-server script --path build.rhai` 执行 rhai 脚本——脚本直接驱动
//! CoreSession（`new_session()` / `open_session(path)` 返回会话对象，
//! 方法调用建模式：`s.create_element(...)` / `s.build_geometry(...)` /
//! `s.save(...)`）。G6-M11 门面补全到 **31 函数**：删改流
//! （delete_element / update_element_properties / rebuild_geometry）、出图
//! （remove_placement / remove_sheet / regenerate_view）、清单查询
//! （list_elements / list_views / list_sheets / list_schemas，rhai Map 数组）、
//! 历史状态（can_undo / undo_depth 等）。函数清单与错误语义见
//! [`crate::script`] 模块文档——外部调用形态三期全落地：CLI（一次性批处理）
//! + HTTP（常驻 REST）+ rhai（脚本编排）。

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand, ValueEnum};
use core_app::CoreSession;
use core_doc::DraftingStandard;
use core_interop::Format;
use core_model::SheetId;
use example_beam::{CATEGORY, ExampleBeam, GEOMETRY_OP_ID};

/// HTTP 桥（G6-M7）：axum REST 暴露 CoreSession 会话态门面。
mod http;

/// rhai 脚本引擎（G6-M9）：脚本直接驱动 CoreSession。
mod script;

/// 既有骨架的模块装配（G6-M6 起服务侧命令共用）。
mod setup {
    use core_app::ModuleHost;
    use core_doc::DraftingStandard;
    use example_beam::ExampleBeam;

    /// 装载 ExampleBeam 并返回宿主（CLI 各子命令共用一段装配）。
    ///
    /// # Errors
    /// 模块装载失败时返回错误文本。
    pub fn install_example_beam() -> Result<ModuleHost, String> {
        let mut host = ModuleHost::new();
        host.install(Box::new(ExampleBeam))
            .map_err(|e| e.to_string())?;
        Ok(host)
    }

    /// 模块声明的专业图层装入制图标准。
    ///
    /// # Errors
    /// 图层装入失败时返回错误文本。
    pub fn apply_layers(host: &ModuleHost, standard: &mut DraftingStandard) -> Result<(), String> {
        host.apply_layers_to(standard)
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
}

/// 轻量化 BIM 内核平台 —— 无头服务端（批处理 / 转换 / DWG 隔离宿主）。
#[derive(Parser)]
#[command(name = "bim-server", version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

/// CLI 子命令（G6-M6）：每个命令直接驱动进程内 CoreSession 门面。
#[derive(Subcommand)]
enum Command {
    /// 端到端装配自检（五层贯通 + 存盘重开断言）
    Demo,
    /// 新建文档：造构件 + 建几何 + 保存
    New {
        /// 构件名（空 = 品类默认名）
        #[arg(long, default_value = "")]
        name: String,
        /// 品类 id（模块注册表键，默认取模块常量 BEAM）
        #[arg(long, default_value = CATEGORY)]
        category: String,
        /// 建几何用的算子 id（模块注册表键，默认取模块常量 example.beam.rect）
        #[arg(long, default_value = GEOMETRY_OP_ID)]
        op: String,
        /// 建几何后加一个视图并初生成（名称自定，如"俯视"——
        /// `export --view` 的目标；缺省不加）
        #[arg(long)]
        view: Option<String>,
        /// 保存路径（.bimdoc；缺省只打印不保存）
        #[arg(long)]
        save: Option<PathBuf>,
    },
    /// 打开文档并打印摘要
    Info {
        /// 文档路径（.bimdoc）
        #[arg(long)]
        doc: PathBuf,
    },
    /// 打开文档并导出图纸或视图
    Export {
        /// 文档路径（.bimdoc）
        #[arg(long)]
        doc: PathBuf,
        /// 输出格式（dxf / svg）
        #[arg(long, value_enum)]
        format: FormatArg,
        /// 输出文件路径
        #[arg(long)]
        out: PathBuf,
        /// 按图号导出一张图纸（与 --view 二选一）
        #[arg(long)]
        sheet: Option<String>,
        /// 按名称导出一个视图（与 --sheet 二选一）
        #[arg(long)]
        view: Option<String>,
    },
    /// 启动 HTTP 服务（REST 桥，G6-M7）
    Serve {
        /// 监听地址（默认本机回环）
        #[arg(long, default_value = "127.0.0.1")]
        host: String,
        /// 端口（0 = OS 分配，探针从 stdout 解析实际地址）
        #[arg(long, default_value_t = 8300)]
        port: u16,
    },
    /// 执行 rhai 脚本（脚本引擎，G6-M9）
    Script {
        /// 脚本路径（.rhai——脚本自管会话：new_session() / open_session(path)）
        #[arg(long)]
        path: PathBuf,
    },
}

/// `--format` 的 CLI 层枚举：字符串 → [`Format`] 的映射薄壳。
///
/// [`Format`] 全集里 DXF / SVG 是当前可用的出图出口（见模块文档的许可矩阵），
/// 其余格式（Step/Gltf/Ifc4x3/Dwg/Gim）或需真内核、或只导入，CLI 不暴露。
#[derive(Clone, Copy, ValueEnum)]
enum FormatArg {
    /// DXF —— 兼容读写
    Dxf,
    /// SVG —— 出图出口
    Svg,
}

impl From<FormatArg> for Format {
    fn from(arg: FormatArg) -> Self {
        match arg {
            FormatArg::Dxf => Format::Dxf,
            FormatArg::Svg => Format::Svg,
        }
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command.unwrap_or(Command::Demo) {
        Command::Demo => demo(),
        Command::New {
            name,
            category,
            op,
            view,
            save,
        } => new_document(&name, &category, &op, view.as_deref(), save.as_deref()),
        Command::Info { doc } => info(&doc),
        Command::Export {
            doc,
            format,
            out,
            sheet,
            view,
        } => export(&doc, format.into(), &out, sheet.as_deref(), view.as_deref()),
        Command::Serve { host, port } => run_serve(&host, port),
        Command::Script { path } => script::run_script(&path),
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(msg) => {
            eprintln!("✗ {msg}");
            ExitCode::FAILURE
        }
    }
}

/// `serve` 子命令的执行体：薄 runtime 阻塞 HTTP 服务（main 同步形态的 async 桥）。
///
/// # Errors
/// 运行时创建或服务运行失败时返回错误文本。
fn run_serve(host: &str, port: u16) -> Result<(), String> {
    // 其余子命令都是同步的，main 保持同步形态——这里包一个薄 runtime 阻塞其上
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("运行时创建失败：{e}"))?;
    rt.block_on(http::serve(host, port))
}

/// 端到端装配自检（骨架期既有行为，保留为 `demo` 子命令）。
fn demo() -> Result<(), String> {
    println!("轻量化 BIM 内核平台 —— 无头服务端");
    println!("职责：批处理 / 格式转换 / DWG 隔离转换服务宿主\n");

    let host = setup::install_example_beam()?;

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
    setup::apply_layers(&host, &mut standard)?;

    println!("\n许可隔离边界（合规红线）：");
    println!("  LGPL-2.1（WASM 形态）  → occt-wasm，可同进程");
    println!("  MPL-2.0                → IFC 解析，文件级 copyleft，可静态链接");
    println!("  GPLv3+                 → LibreDWG（DWG），【禁止】链接进主程序，");
    println!("                           只能在本进程内隔离运行，经 IPC/HTTP 调用");

    println!("\n✓ 无头装配贯通。");
    Ok(())
}

/// 装配一个 CLI 会话（ExampleBeam 模块 + 占位内核；occt feature 走真内核）。
fn new_session() -> Result<CoreSession, String> {
    #[cfg(feature = "occt")]
    let picked = CoreSession::with_real_kernel();
    #[cfg(not(feature = "occt"))]
    let picked = CoreSession::new();
    let mut session = picked.map_err(|e| e.to_string())?;
    let module = Box::new(ExampleBeam);
    session.install_module(module).map_err(|e| e.to_string())?;
    Ok(session)
}

/// `new` 子命令：新建文档 → 造构件 → 建几何 →（可选）建视图初生成 → 保存。
fn new_document(
    name: &str,
    category: &str,
    op: &str,
    view: Option<&str>,
    save: Option<&std::path::Path>,
) -> Result<(), String> {
    use core_doc::ViewOrientation;

    let mut session = new_session()?;

    let element = session
        .create_element(category, name)
        .map_err(|e| e.to_string())?;
    println!("✓ 构件已创建：{category}（id {element}）");

    let shape = session
        .build_geometry(element, op)
        .map_err(|e| e.to_string())?;
    println!("✓ 几何已建：算子 {op} → shape {shape}");

    if let Some(view_name) = view {
        let view_id = session
            .create_view(view_name, ViewOrientation::top())
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
        let (entities, warnings) = session
            .project_shape(shape, &view, &standard)
            .map_err(|e| e.to_string())?;
        println!(
            "✓ 视图已建：{view_name}（俯视）初生成 {} 条图元 / 警告 {} 条",
            entities.len(),
            warnings.len()
        );
    }

    if let Some(path) = save {
        session.save(path).map_err(|e| e.to_string())?;
        println!("✓ 已保存：{}", path.display());
    } else {
        println!("（未指定 --save，文档不落盘）");
    }

    let s = session.summary();
    println!(
        "摘要：模块 {} / 元素 {} / 几何 {}",
        s.modules, s.elements, s.geometry
    );
    Ok(())
}

/// `info` 子命令：打开文档打印摘要。
fn info(doc: &std::path::Path) -> Result<(), String> {
    let session = CoreSession::open(doc).map_err(|e| e.to_string())?;
    let drawing = session.document().drawing();
    println!("文档：{}", doc.display());
    let s = session.summary();
    println!(
        "模块 {} / 元素 {} / 几何 {} / 专业图层 {}",
        s.modules, s.elements, s.geometry, s.layers
    );
    println!(
        "视图 {} / 图纸 {}",
        drawing.view_count(),
        drawing.sheets.len()
    );

    for (id, element) in session.document().doc().elements() {
        let geom = element
            .geometry_ref
            .map(|shape| format!("，shape {shape}"))
            .unwrap_or_default();
        println!(
            "  构件 {}（{}）：{}{geom}",
            element.name, element.category, id
        );
    }
    for sheet in drawing.sheets.values() {
        println!(
            "  图纸 {}「{}」：{} 个布置",
            sheet.number,
            sheet.name,
            sheet.placements.len()
        );
    }
    Ok(())
}

/// 按**图号**找图纸（CLI 语义：用户看到的是图号，不是内部 id）。
fn find_sheet_by_number(drawing: &core_doc::Drawing, number: &str) -> Result<SheetId, String> {
    drawing
        .sheets
        .values()
        .find(|s| s.number == number)
        .map(|s| s.id)
        .ok_or_else(|| {
            let known: Vec<&str> = drawing.sheets.values().map(|s| s.number.as_str()).collect();
            format!("未找到图号 {number}（现有：{known:?}）")
        })
}

/// 按**名称**找视图。
fn find_view_by_name(drawing: &core_doc::Drawing, name: &str) -> Result<core_doc::View, String> {
    drawing
        .views
        .values()
        .find(|v| v.name == name)
        .cloned()
        .ok_or_else(|| {
            let known: Vec<&str> = drawing.views.values().map(|v| v.name.as_str()).collect();
            format!("未找到视图 {name}（现有：{known:?}）")
        })
}

/// `export` 子命令：打开文档导出图纸或视图（--sheet 与 --view 二选一）。
fn export(
    doc: &std::path::Path,
    format: Format,
    out: &std::path::Path,
    sheet: Option<&str>,
    view: Option<&str>,
) -> Result<(), String> {
    match (sheet, view) {
        (Some(_), Some(_)) => {
            return Err("--sheet 与 --view 只能二选一".to_string());
        }
        (None, None) => {
            return Err("请指定 --sheet <图号> 或 --view <视图名>".to_string());
        }
        _ => {}
    }

    let session = CoreSession::open(doc).map_err(|e| e.to_string())?;
    let drawing = session.document().drawing().clone();
    let standard = drawing.standard.clone();

    let output = if let Some(number) = sheet {
        let sheet_id = find_sheet_by_number(&drawing, number)?;
        session
            .interop()
            .export_sheet(format, &drawing, sheet_id)
            .map_err(|e| e.to_string())?
    } else {
        let name = view.unwrap_or_default();
        let target = find_view_by_name(&drawing, name)?;
        session
            .interop()
            .export_view(format, &target, &standard)
            .map_err(|e| e.to_string())?
    };

    std::fs::write(out, &output.bytes).map_err(|e| e.to_string())?;
    println!(
        "✓ 已导出 {}（{} 字节）→ {}",
        out.display(),
        output.bytes.len(),
        format_name(format)
    );
    Ok(())
}

/// 格式的人类可读名（不引 `Format` 的完整展示 API，只覆盖 CLI 两个出口）。
fn format_name(format: Format) -> &'static str {
    match format {
        Format::Dxf => "DXF",
        Format::Svg => "SVG",
        _ => "未知格式",
    }
}
