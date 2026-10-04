//! # rhai 脚本引擎（G6-M9，外部调用形态第三期）
//!
//! [`rhai::Engine`] 嵌入 bim-server——脚本直接驱动 [`CoreSession`]，
//! 完成「建模 + 出图 + 导出」**多步工作流**。与 CLI 第一期（G6-M6，一次性子命令）、
//! HTTP 桥第二期（G6-M7，常驻 REST）同构：进程内 CoreSession API 之上的一层薄壳。
//!
//! ## 形态
//!
//! ```text
//! bim-server script --path build.rhai
//! ```
//!
//! 脚本自管会话——`new_session()` / `open_session(path)` 返回会话对象，
//! 之后全部操作走方法调用：
//!
//! ```rhai
//! let s = new_session();                       // 新建会话（装 ExampleBeam）
//! let e = s.create_element("BEAM", "一层梁");   // 造构件 → 元素 id（字符串）
//! s.build_geometry(e, "example.beam.rect");     // 建几何 → shape id
//! let v = s.create_view("俯视");                // 建视图（俯视）
//! let sh = s.create_sheet("一层平面", "建施-01", "A2");
//! s.place_view_on_sheet(sh, v, 297.0, 210.0, 100);
//! print(s.summary());                           // rhai 内置 print → stdout
//! s.save("out.bimdoc");
//! ```
//!
//! ## 会话模型
//!
//! [`NativeSession`] = `Arc<Mutex<CoreSession>>`——rhai 类型要求
//! `Variant + Clone`（`impl<T: Any + Clone + SendSync> Variant for T`），
//! [`CoreSession`] 含服务句柄不可 Clone，Arc 包一层 Clone 归零；
//! Send 全链已被 G6-M7 打通（`KernelBackend: Send`）。
//! 锁是**内部可变性**——注册函数第一参数 `&NativeSession`（rhai 方法调用
//! 自动匹配），每次调用 lock → 操作 → 释放 guard，guard 不跨调用。
//!
//! ## 脚本 API（第一版 15 函数，照 http.rs handler 语义）
//!
//! ```text
//! new_session()                     # 新建会话（装 ExampleBeam，占位/真内核按 feature）
//! open_session(path)                # 打开既有 .bimdoc
//! s.create_element(category, name)  # 造构件 → 元素 id（name 空串 = 品类默认名）
//! s.build_geometry(element_id, op)  # 建几何 → shape id
//! s.create_view(name)               # 建视图（朝向固定俯视）→ 视图 id
//! s.create_sheet(name, number, fmt) # 建图纸（fmt: "A2" 等 serde 长名）→ 图纸 id
//! s.place_view_on_sheet(sheet, view, x, y, scale)  # 布置（x/y mm，scale 分母）
//! s.save(path)                      # 保存 .bimdoc
//! s.export_sheet(fmt, sheet_id, out)  # 整图导出（fmt: "dxf"/"svg"）→ 字节数
//! s.export_view(fmt, view_id, out)    # 视图导出 → 字节数
//! s.summary()                       # 摘要文本
//! s.element_count() / s.geometry_count() / s.view_count() / s.sheet_count()
//! s.undo() / s.redo()               # 撤销 / 重做一步 → 是否成功
//! ```
//!
//! id 一律用字符串（UUID 的 `Display`）——脚本里可读、可直接传递。
//!
//! ## 错误语义（与本仓铁律一致：不 panic、不吞错）
//!
//! 全部注册函数返回 `Result<T, Box<EvalAltResult>>`——**rhai fallible 注册
//! 的精确边界**（`def_register!` 宏的 `RhaiResultOf<RET>`，E 必须是
//! `Box<EvalAltResult>`；返回 `Result<T, String>` 会被当普通函数、错误值
//! 直接漏进脚本）。[`rt_err`] 是字符串错误 → 运行时错误的统一出口；
//! [`run_script`] 捕获后打印错误原文并返回非零退出。无效 id / 未知算子 /
//! 未知图幅 / 越界布置等业务失败全部响亮拒绝，绝不静默。
//!
//! `print()` 是 rhai 内置（默认 on_print = stdout println）——脚本调试输出直达控制台。

use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};

use core_app::CoreSession;
use core_doc::{SheetFormat, ViewOrientation};
use core_interop::Format;
use core_model::{ElementId, SheetId, ViewId};
use rhai::{Engine, EvalAltResult, Position};
use uuid::Uuid;

/// 会话对象（rhai 类型）：`Arc<Mutex<CoreSession>>`——Clone 归零（Arc），
/// 内部可变性（Mutex）支撑 rhai 的 `&self` 方法调用。
#[derive(Clone)]
struct NativeSession(Arc<Mutex<CoreSession>>);

/// 字符串错误 → rhai 运行时错误（fallible 注册的统一出口——
/// `ErrorRuntime` 携带原文，脚本终止时打印）。
fn rt_err(msg: String) -> Box<EvalAltResult> {
    Box::new(EvalAltResult::ErrorRuntime(msg.into(), Position::NONE))
}

/// 锁助手：锁中毒（另一线程 panic 才会发生——脚本单线程不会）也响亮报错，
/// 不 `expect`（workspace lints deny unwrap/panic）。
fn lock(s: &mut NativeSession) -> Result<MutexGuard<'_, CoreSession>, String> {
    s.0.lock().map_err(|_| "会话锁中毒".to_string())
}

/// uuid 字符串 → 底层 [`Uuid`]（解析失败响亮拒绝）。
fn parse_uuid(s: &str, what: &str) -> Result<Uuid, String> {
    Uuid::parse_str(s).map_err(|_| format!("无效 {what}：{s}"))
}

/// `"dxf"` / `"svg"` → [`Format`]（照 http.rs `ExportFormat` 同款小写长名；
/// 未知格式响亮拒绝——CLI/HTTP 只暴露两个出口，脚本同口径）。
fn parse_format(s: &str) -> Result<Format, String> {
    match s {
        "dxf" => Ok(Format::Dxf),
        "svg" => Ok(Format::Svg),
        _ => Err(format!("未知导出格式 {s}（可用：dxf / svg）")),
    }
}

/// 图幅长名 → [`SheetFormat`]（serde 长名同款：`"A2"` / `"A4Extended"`）。
fn parse_sheet_format(s: &str) -> Result<SheetFormat, String> {
    match s {
        "A0" => Ok(SheetFormat::A0),
        "A1" => Ok(SheetFormat::A1),
        "A2" => Ok(SheetFormat::A2),
        "A3" => Ok(SheetFormat::A3),
        "A4" => Ok(SheetFormat::A4),
        "A4Extended" => Ok(SheetFormat::A4Extended),
        _ => Err(format!("未知图幅 {s}（可用：A0/A1/A2/A3/A4/A4Extended）")),
    }
}

/// 构建脚本引擎：注册 [`NativeSession`] 类型 + 全部 15 个函数
/// （全部 fallible——返回 `Result<T, Box<EvalAltResult>>`，见模块文档）。
fn build_engine() -> Engine {
    let mut engine = Engine::new();
    engine
        .register_type_with_name::<NativeSession>("Session")
        // —— 会话装配 ——
        .register_fn(
            "new_session",
            || -> Result<NativeSession, Box<EvalAltResult>> {
                crate::new_session()
                    .map(|s| NativeSession(Arc::new(Mutex::new(s))))
                    .map_err(rt_err)
            },
        )
        .register_fn(
            "open_session",
            |path: &str| -> Result<NativeSession, Box<EvalAltResult>> {
                CoreSession::open(Path::new(path))
                    .map(|s| NativeSession(Arc::new(Mutex::new(s))))
                    .map_err(|e| rt_err(e.to_string()))
            },
        )
        // —— 建模 ——
        .register_fn(
            "create_element",
            |s: &mut NativeSession,
             category: &str,
             name: &str|
             -> Result<String, Box<EvalAltResult>> {
                let mut session = lock(s).map_err(rt_err)?;
                let element = session
                    .create_element(category, name)
                    .map_err(|e| rt_err(e.to_string()))?;
                Ok(element.to_string())
            },
        )
        .register_fn(
            "build_geometry",
            |s: &mut NativeSession,
             element_id: &str,
             op: &str|
             -> Result<String, Box<EvalAltResult>> {
                let mut session = lock(s).map_err(rt_err)?;
                let element =
                    ElementId::from_uuid(parse_uuid(element_id, "element_id").map_err(rt_err)?);
                let shape = session
                    .build_geometry(element, op)
                    .map_err(|e| rt_err(e.to_string()))?;
                Ok(shape.to_string())
            },
        )
        // —— 出图 ——
        .register_fn(
            "create_view",
            |s: &mut NativeSession, name: &str| -> Result<String, Box<EvalAltResult>> {
                let mut session = lock(s).map_err(rt_err)?;
                let view = session
                    .create_view(name, ViewOrientation::top())
                    .map_err(|e| rt_err(e.to_string()))?;
                Ok(view.to_string())
            },
        )
        .register_fn(
            "create_sheet",
            |s: &mut NativeSession,
             name: &str,
             number: &str,
             format: &str|
             -> Result<String, Box<EvalAltResult>> {
                let mut session = lock(s).map_err(rt_err)?;
                let format = parse_sheet_format(format).map_err(rt_err)?;
                let sheet = session
                    .create_sheet(name, number, format)
                    .map_err(|e| rt_err(e.to_string()))?;
                Ok(sheet.to_string())
            },
        )
        .register_fn(
            "place_view_on_sheet",
            |s: &mut NativeSession,
             sheet_id: &str,
             view_id: &str,
             x: f64,
             y: f64,
             scale: i64|
             -> Result<(), Box<EvalAltResult>> {
                let mut session = lock(s).map_err(rt_err)?;
                let sheet = SheetId::from_uuid(parse_uuid(sheet_id, "sheet_id").map_err(rt_err)?);
                let view = ViewId::from_uuid(parse_uuid(view_id, "view_id").map_err(rt_err)?);
                let scale = u32::try_from(scale)
                    .map_err(|_| format!("比例分母越界：{scale}（须为正且在 u32 内）"))
                    .map_err(rt_err)?;
                session
                    .place_view_on_sheet(sheet, view, glam::DVec2::new(x, y), scale)
                    .map_err(|e| rt_err(e.to_string()))
            },
        )
        // —— 持久化 / 导出 ——
        .register_fn(
            "save",
            |s: &mut NativeSession, path: &str| -> Result<(), Box<EvalAltResult>> {
                lock(s)
                    .map_err(rt_err)?
                    .save(Path::new(path))
                    .map_err(|e| rt_err(e.to_string()))
            },
        )
        .register_fn(
            "export_sheet",
            |s: &mut NativeSession,
             format: &str,
             sheet_id: &str,
             out: &str|
             -> Result<i64, Box<EvalAltResult>> {
                let session = lock(s).map_err(rt_err)?;
                let format = parse_format(format).map_err(rt_err)?;
                let sheet = SheetId::from_uuid(parse_uuid(sheet_id, "sheet_id").map_err(rt_err)?);
                let drawing = session.document().drawing().clone();
                let output = session
                    .interop()
                    .export_sheet(format, &drawing, sheet)
                    .map_err(|e| rt_err(e.to_string()))?;
                std::fs::write(Path::new(out), &output.bytes).map_err(|e| rt_err(e.to_string()))?;
                Ok(output.bytes.len() as i64)
            },
        )
        .register_fn(
            "export_view",
            |s: &mut NativeSession,
             format: &str,
             view_id: &str,
             out: &str|
             -> Result<i64, Box<EvalAltResult>> {
                let session = lock(s).map_err(rt_err)?;
                let format = parse_format(format).map_err(rt_err)?;
                let view = ViewId::from_uuid(parse_uuid(view_id, "view_id").map_err(rt_err)?);
                let drawing = session.document().drawing().clone();
                let standard = drawing.standard.clone();
                let target = drawing
                    .view(view)
                    .cloned()
                    .ok_or_else(|| format!("视图 {view} 未登记进图纸"))
                    .map_err(rt_err)?;
                let output = session
                    .interop()
                    .export_view(format, &target, &standard)
                    .map_err(|e| rt_err(e.to_string()))?;
                std::fs::write(Path::new(out), &output.bytes).map_err(|e| rt_err(e.to_string()))?;
                Ok(output.bytes.len() as i64)
            },
        )
        // —— 查询 / 历史 ——
        .register_fn(
            "summary",
            |s: &mut NativeSession| -> Result<String, Box<EvalAltResult>> {
                let session = lock(s).map_err(rt_err)?;
                let sum = session.summary();
                let drawing = session.document().drawing();
                Ok(format!(
                    "模块 {} / 元素 {} / 几何 {} / 图层 {} / 视图 {} / 图纸 {}",
                    sum.modules,
                    sum.elements,
                    sum.geometry,
                    sum.layers,
                    drawing.view_count(),
                    drawing.sheets.len()
                ))
            },
        )
        .register_fn(
            "element_count",
            |s: &mut NativeSession| -> Result<i64, Box<EvalAltResult>> {
                Ok(lock(s).map_err(rt_err)?.summary().elements as i64)
            },
        )
        .register_fn(
            "geometry_count",
            |s: &mut NativeSession| -> Result<i64, Box<EvalAltResult>> {
                Ok(lock(s).map_err(rt_err)?.summary().geometry as i64)
            },
        )
        .register_fn(
            "view_count",
            |s: &mut NativeSession| -> Result<i64, Box<EvalAltResult>> {
                Ok(lock(s).map_err(rt_err)?.document().drawing().view_count() as i64)
            },
        )
        .register_fn(
            "sheet_count",
            |s: &mut NativeSession| -> Result<i64, Box<EvalAltResult>> {
                Ok(lock(s).map_err(rt_err)?.document().drawing().sheets.len() as i64)
            },
        )
        .register_fn(
            "undo",
            |s: &mut NativeSession| -> Result<bool, Box<EvalAltResult>> {
                let mut session = lock(s).map_err(rt_err)?;
                Ok(session.undo())
            },
        )
        .register_fn(
            "redo",
            |s: &mut NativeSession| -> Result<bool, Box<EvalAltResult>> {
                let mut session = lock(s).map_err(rt_err)?;
                Ok(session.redo())
            },
        );
    engine
}

/// `script` 子命令执行体：读脚本 → 建引擎 → 执行。
///
/// # Errors
/// 脚本读取失败或执行失败（含脚本内业务错误）时返回错误文本——main 打印
/// `✗ …` 并以非零退出。
pub fn run_script(path: &Path) -> Result<(), String> {
    let source = std::fs::read_to_string(path)
        .map_err(|e| format!("读脚本 {} 失败：{e}", path.display()))?;
    let engine = build_engine();
    println!("✓ 脚本引擎就绪（rhai），执行 {}", path.display());
    engine
        .run(&source)
        .map_err(|e| format!("脚本执行失败：{e}"))
}

#[cfg(test)]
mod tests {
    // workspace lints（unwrap_used / panic deny）在测试模块放开——仓库同款。
    #![cfg_attr(test, allow(clippy::unwrap_used, clippy::panic))]

    use super::*;

    /// 测试脚本里的断言助手：rhai 无 assert_eq（default features 不含
    /// debugging），`if != { throw(...) }` 是零依赖的等价形式。
    const ASSERT_EQ: &str = "
        fn assert_eq(a, b, what) {
            if a != b { throw(what + `：got ` + a + ` want ` + b); }
        }
    ";

    /// 临时文件路径（进程 id 隔离——并行测试不撞名）。
    fn temp_path(tag: &str, ext: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "bim-server-script-{}-{tag}.{ext}",
            std::process::id()
        ))
    }

    /// 钉 1：脚本建模流——造构件 → 建几何 → 计数对账。
    #[test]
    fn script_models_an_element_and_counts_reconcile() {
        let engine = build_engine();
        engine
            .run(&format!(
                "{ASSERT_EQ}
                let s = new_session();
                let e = s.create_element(\"BEAM\", \"测试梁\");
                s.build_geometry(e, \"example.beam.rect\");
                assert_eq(s.element_count(), 1, \"元素数\");
                assert_eq(s.geometry_count(), 1, \"几何数\");"
            ))
            .unwrap_or_else(|e| panic!("脚本执行失败：{e}"));
    }

    /// 钉 2：错误响应——未知算子响亮拒绝（Err 文本含算子 id），绝不静默。
    #[test]
    fn script_errors_refuse_unknown_op() {
        let engine = build_engine();
        let err = engine
            .run(
                "let s = new_session();
                 let e = s.create_element(\"BEAM\", \"\");
                 s.build_geometry(e, \"nonexistent.op\");",
            )
            .expect_err("未知算子应失败");
        assert!(
            err.to_string().contains("nonexistent.op"),
            "错误应含算子 id：{err}"
        );
    }

    /// 钉 3：图纸布置流——建视图/图纸 → 布置 → 图纸数对账。
    #[test]
    fn script_places_a_view_on_a_sheet() {
        let engine = build_engine();
        engine
            .run(&format!(
                "{ASSERT_EQ}
                let s = new_session();
                let v = s.create_view(\"俯视\");
                let sh = s.create_sheet(\"一层平面\", \"建施-01\", \"A2\");
                s.place_view_on_sheet(sh, v, 297.0, 210.0, 100);
                assert_eq(s.sheet_count(), 1, \"图纸数\");"
            ))
            .unwrap_or_else(|e| panic!("脚本执行失败：{e}"));
    }

    /// 钉 4：无效 id 响亮拒绝——坏 uuid 字符串报错（不 panic、不静默）。
    #[test]
    fn script_errors_refuse_bad_uuid() {
        let engine = build_engine();
        let err = engine
            .run(
                "let s = new_session();
                 s.build_geometry(\"not-a-uuid\", \"example.beam.rect\");",
            )
            .expect_err("坏 id 应失败");
        assert!(
            err.to_string().contains("not-a-uuid"),
            "错误应含原字符串：{err}"
        );
    }

    /// 钉 5：撤销 / 重做——脚本 undo 后元素数对账，redo 还原。
    #[test]
    fn script_undo_redo_reconcile_counts() {
        let engine = build_engine();
        engine
            .run(&format!(
                "{ASSERT_EQ}
                let s = new_session();
                s.create_element(\"BEAM\", \"梁一\");
                assert_eq(s.element_count(), 1, \"撤销前\");
                let ok = s.undo();
                assert_eq(ok, true, \"撤销应成功\");
                assert_eq(s.element_count(), 0, \"撤销后\");
                let ok = s.redo();
                assert_eq(ok, true, \"重做应成功\");
                assert_eq(s.element_count(), 1, \"重做后\");"
            ))
            .unwrap_or_else(|e| panic!("脚本执行失败：{e}"));
    }

    /// 钉 6：导出流——建视图 → export_view svg → 字节数为正（文件真落盘）。
    #[test]
    fn script_exports_a_view_to_svg() {
        let out = temp_path("export", "svg");
        let engine = build_engine();
        engine
            .run(&format!(
                "{ASSERT_EQ}
                let s = new_session();
                let v = s.create_view(\"俯视\");
                let bytes = s.export_view(\"svg\", v, {:?});
                if bytes <= 0 {{ throw(\"导出字节数应为正：\" + bytes); }}",
                out.display().to_string()
            ))
            .unwrap_or_else(|e| panic!("脚本执行失败：{e}"));
        let written = std::fs::metadata(&out).unwrap_or_else(|e| panic!("读导出文件失败：{e}"));
        assert!(written.len() > 0, "导出文件应为非空");
        let _ = std::fs::remove_file(&out);
    }

    /// 钉 7：脚本 print 进 on_print 回调（默认 stdout——测试换收集 sink 断言；
    /// 回调要求 'static，用 Arc<Mutex<String>> 归零借用）。
    #[test]
    fn script_print_goes_to_the_capture_hook() {
        let mut engine = build_engine();
        let printed = Arc::new(Mutex::new(String::new()));
        let sink = printed.clone();
        engine.on_print(move |s| {
            // 测试模块 allow unwrap——锁中毒在单线程测试里不会发生。
            sink.lock().unwrap().push_str(s);
        });
        engine
            .run("let s = new_session(); print(s.summary());")
            .unwrap_or_else(|e| panic!("脚本执行失败：{e}"));
        let text = printed.lock().unwrap();
        assert!(text.contains("模块"), "print 应输出摘要文本：{text}");
    }
}
