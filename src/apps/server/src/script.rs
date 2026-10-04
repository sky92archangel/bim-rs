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
//! ## 脚本 API（31 函数，照 http.rs handler / GUI 语义；G6-M11 门面补全）
//!
//! ```text
//! new_session()                     # 新建会话（装 ExampleBeam，占位/真内核按 feature）
//! open_session(path)                # 打开既有 .bimdoc
//! s.create_element(category, name)  # 造构件 → 元素 id（name 空串 = 品类默认名）
//! s.create_generic_element(name)    # 0 输入构件（不走品类）→ 元素 id
//! s.build_geometry(element_id, op)  # 建几何 → shape id
//! s.delete_element(element_id)      # 删构件（撤销步 + 孤儿自动清理）→ 删除报告文本
//! s.set_element_visibility(id, b)   # 元素显隐（撤销步，随 .bimdoc 持久化）
//! s.create_view(name)               # 建视图（朝向固定俯视）→ 视图 id
//! s.create_sheet(name, number, fmt) # 建图纸（fmt: "A2" 等 serde 长名）→ 图纸 id
//! s.place_view_on_sheet(sheet, view, x, y, scale)  # 布置（x/y mm，scale 分母）
//! s.remove_placement(sheet, index)  # 按下标移除布置 → 移除的视图 id
//! s.remove_sheet(sheet_id)          # 删图纸
//! s.regenerate_view(view_id)        # 视图重投 → 报告文本（图元数 / 警告数）
//! s.save(path)                      # 保存 .bimdoc
//! s.export_sheet(fmt, sheet_id, out)  # 整图导出（fmt: "dxf"/"svg"）→ 字节数
//! s.export_view(fmt, view_id, out)    # 视图导出 → 字节数
//! s.summary()                       # 摘要文本
//! s.element_count() / s.geometry_count() / s.view_count() / s.sheet_count()
//! s.list_elements()                 # 元素清单（{id, category, name, has_geom, visible}）
//! s.list_views()                    # 视图清单（{id, name}）
//! s.list_sheets()                   # 图纸清单（{id, number, name, fmt, placements}）
//! s.list_schemas()                  # 属性模式名清单
//! s.can_undo() / s.can_redo()       # 历史可否移动
//! s.undo_depth() / s.redo_depth()   # 历史栈深度
//! s.undo() / s.redo()               # 撤销 / 重做一步 → 是否成功
//! # —— 属性更新（G6-M11：PropertyMap 注册，G5 关联更新链路的脚本入口）——
//! let pm = new_property_map("example.beam");   # 构造空属性（指定模式名）
//! pm.set("width_mm", 300.0);                   # 置值（bool/int/float/string）
//! s.update_element_properties(element_id, pm)  # 回写元素属性（撤销步）
//! s.rebuild_geometry(element_id, op, pm)       # 关联更新重建 → 报告文本
//! ```
//!
//! id 一律用字符串（UUID 的 `Display`）——脚本里可读、可直接传递；
//! 清单查询返回 rhai Map 数组（`el.id` / `el.name` 直接访问）。
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
use core_model::{ElementId, PropertyMap, SheetId, ViewId};
use rhai::{Dynamic, Engine, EvalAltResult, Position};
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

/// rhai 动态值 → [`core_model::Value`]（G6-M11 属性更新的类型路由：
/// bool / int / float / string 四类；`Ref` 不暴露——脚本里引用语义复杂，
/// 响亮拒绝而非静默丢弃）。
fn dyn_to_value(v: Dynamic, key: &str) -> Result<core_model::Value, String> {
    if let Some(b) = v.clone().try_cast::<bool>() {
        return Ok(core_model::Value::Bool(b));
    }
    if let Some(i) = v.clone().try_cast::<i64>() {
        return Ok(core_model::Value::Int(i));
    }
    if let Some(f) = v.clone().try_cast::<f64>() {
        return Ok(core_model::Value::Float(f));
    }
    if let Some(s) = v.clone().try_cast::<String>() {
        return Ok(core_model::Value::Text(s));
    }
    Err(format!(
        "属性 {key} 的值类型不支持（可用：布尔 / 整数 / 浮点 / 字符串）"
    ))
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
        )
        // —— 历史状态（G6-M11：脚本决策用）——
        .register_fn(
            "can_undo",
            |s: &mut NativeSession| -> Result<bool, Box<EvalAltResult>> {
                Ok(lock(s).map_err(rt_err)?.can_undo())
            },
        )
        .register_fn(
            "can_redo",
            |s: &mut NativeSession| -> Result<bool, Box<EvalAltResult>> {
                Ok(lock(s).map_err(rt_err)?.can_redo())
            },
        )
        .register_fn(
            "undo_depth",
            |s: &mut NativeSession| -> Result<i64, Box<EvalAltResult>> {
                Ok(lock(s).map_err(rt_err)?.undo_depth() as i64)
            },
        )
        .register_fn(
            "redo_depth",
            |s: &mut NativeSession| -> Result<i64, Box<EvalAltResult>> {
                Ok(lock(s).map_err(rt_err)?.redo_depth() as i64)
            },
        )
        // —— 建模 / 编辑补全（G6-M11）——
        .register_fn(
            "create_generic_element",
            |s: &mut NativeSession, name: &str| -> Result<String, Box<EvalAltResult>> {
                let mut session = lock(s).map_err(rt_err)?;
                let element = session
                    .create_generic_element(name)
                    .map_err(|e| rt_err(e.to_string()))?;
                Ok(element.to_string())
            },
        )
        .register_fn(
            "delete_element",
            |s: &mut NativeSession, element_id: &str| -> Result<String, Box<EvalAltResult>> {
                let mut session = lock(s).map_err(rt_err)?;
                let element =
                    ElementId::from_uuid(parse_uuid(element_id, "element_id").map_err(rt_err)?);
                let report = session
                    .delete_element(element)
                    .map_err(|e| rt_err(e.to_string()))?;
                Ok(format!(
                    "已删元素（几何 {} 个 / 清理 {} 视图 / 孤儿图元 {} / 孤儿标注 {}）",
                    report.removed_shapes.len(),
                    report.cleaned_views,
                    report.removed_entities,
                    report.removed_annotations
                ))
            },
        )
        .register_fn(
            "set_element_visibility",
            |s: &mut NativeSession,
             element_id: &str,
             visible: bool|
             -> Result<(), Box<EvalAltResult>> {
                let mut session = lock(s).map_err(rt_err)?;
                let element =
                    ElementId::from_uuid(parse_uuid(element_id, "element_id").map_err(rt_err)?);
                session
                    .set_element_visibility(element, visible)
                    .map_err(|e| rt_err(e.to_string()))
            },
        )
        // —— 出图补全（G6-M11）——
        .register_fn(
            "remove_placement",
            |s: &mut NativeSession,
             sheet_id: &str,
             index: i64|
             -> Result<String, Box<EvalAltResult>> {
                let mut session = lock(s).map_err(rt_err)?;
                let sheet = SheetId::from_uuid(parse_uuid(sheet_id, "sheet_id").map_err(rt_err)?);
                let index = usize::try_from(index)
                    .map_err(|_| format!("布置下标越界：{index}（须为非负）"))
                    .map_err(rt_err)?;
                let view = session
                    .remove_placement(sheet, index)
                    .map_err(|e| rt_err(e.to_string()))?;
                Ok(view.to_string())
            },
        )
        .register_fn(
            "remove_sheet",
            |s: &mut NativeSession, sheet_id: &str| -> Result<(), Box<EvalAltResult>> {
                let mut session = lock(s).map_err(rt_err)?;
                let sheet = SheetId::from_uuid(parse_uuid(sheet_id, "sheet_id").map_err(rt_err)?);
                session
                    .remove_sheet(sheet)
                    .map_err(|e| rt_err(e.to_string()))
            },
        )
        .register_fn(
            "regenerate_view",
            |s: &mut NativeSession, view_id: &str| -> Result<String, Box<EvalAltResult>> {
                let mut session = lock(s).map_err(rt_err)?;
                let view = ViewId::from_uuid(parse_uuid(view_id, "view_id").map_err(rt_err)?);
                let standard = session.document().drawing().standard.clone();
                let (entities, warnings) = session
                    .regenerate_view(view, &standard)
                    .map_err(|e| rt_err(e.to_string()))?;
                Ok(format!(
                    "重投 {entities} 图元（警告 {} 条）",
                    warnings.len()
                ))
            },
        )
        // —— 清单查询（G6-M11：rhai Map 数组，脚本里 el.id / el.name 直接访问）——
        .register_fn(
            "list_elements",
            |s: &mut NativeSession| -> Result<rhai::Array, Box<EvalAltResult>> {
                let session = lock(s).map_err(rt_err)?;
                Ok(session
                    .document()
                    .doc()
                    .elements()
                    .values()
                    .map(|e| {
                        let mut m = rhai::Map::new();
                        m.insert("id".into(), Dynamic::from(e.id.to_string()));
                        m.insert(
                            "category".into(),
                            Dynamic::from(e.category.as_str().to_string()),
                        );
                        m.insert("name".into(), Dynamic::from(e.name.clone()));
                        m.insert("has_geom".into(), Dynamic::from(e.geometry_ref.is_some()));
                        m.insert("visible".into(), Dynamic::from(e.visible));
                        m
                    })
                    .map(Dynamic::from)
                    .collect())
            },
        )
        .register_fn(
            "list_views",
            |s: &mut NativeSession| -> Result<rhai::Array, Box<EvalAltResult>> {
                let session = lock(s).map_err(rt_err)?;
                Ok(session
                    .document()
                    .drawing()
                    .views
                    .iter()
                    .map(|(id, v)| {
                        let mut m = rhai::Map::new();
                        m.insert("id".into(), Dynamic::from(id.to_string()));
                        m.insert("name".into(), Dynamic::from(v.name.clone()));
                        m
                    })
                    .map(Dynamic::from)
                    .collect())
            },
        )
        .register_fn(
            "list_sheets",
            |s: &mut NativeSession| -> Result<rhai::Array, Box<EvalAltResult>> {
                let session = lock(s).map_err(rt_err)?;
                Ok(session
                    .document()
                    .drawing()
                    .sheets
                    .iter()
                    .map(|(id, sh)| {
                        let mut m = rhai::Map::new();
                        m.insert("id".into(), Dynamic::from(id.to_string()));
                        m.insert("number".into(), Dynamic::from(sh.number.clone()));
                        m.insert("name".into(), Dynamic::from(sh.name.clone()));
                        m.insert("fmt".into(), Dynamic::from(sh.format.code()));
                        m.insert(
                            "placements".into(),
                            Dynamic::from(sh.placement_count() as i64),
                        );
                        m
                    })
                    .map(Dynamic::from)
                    .collect())
            },
        )
        .register_fn(
            "list_schemas",
            |s: &mut NativeSession| -> Result<rhai::Array, Box<EvalAltResult>> {
                let session = lock(s).map_err(rt_err)?;
                Ok(session
                    .list_schemas()
                    .iter()
                    .map(|sc| Dynamic::from(sc.type_name.clone()))
                    .collect())
            },
        )
        // —— 属性更新（G6-M11：PropertyMap 注册，G5 关联更新链路的脚本入口）——
        .register_type_with_name::<PropertyMap>("PropertyMap")
        .register_fn(
            "new_property_map",
            |schema: &str| -> Result<PropertyMap, Box<EvalAltResult>> {
                Ok(PropertyMap::new(schema))
            },
        )
        .register_fn(
            "set",
            |pm: &mut PropertyMap, key: &str, v: Dynamic| -> Result<(), Box<EvalAltResult>> {
                let value = dyn_to_value(v, key).map_err(rt_err)?;
                pm.set(key, value);
                Ok(())
            },
        )
        .register_fn(
            "update_element_properties",
            |s: &mut NativeSession,
             element_id: &str,
             params: PropertyMap|
             -> Result<(), Box<EvalAltResult>> {
                let mut session = lock(s).map_err(rt_err)?;
                let element =
                    ElementId::from_uuid(parse_uuid(element_id, "element_id").map_err(rt_err)?);
                session
                    .update_element_properties(element, params)
                    .map_err(|e| rt_err(e.to_string()))
            },
        )
        .register_fn(
            "rebuild_geometry",
            |s: &mut NativeSession,
             element_id: &str,
             op_id: &str,
             params: PropertyMap|
             -> Result<String, Box<EvalAltResult>> {
                let mut session = lock(s).map_err(rt_err)?;
                let element =
                    ElementId::from_uuid(parse_uuid(element_id, "element_id").map_err(rt_err)?);
                let report = session
                    .rebuild_geometry(element, op_id, params)
                    .map_err(|e| rt_err(e.to_string()))?;
                Ok(format!(
                    "已重建（改 {} 形状 / 演化 {} / 指纹 {} / 重投 {} 图元 / 标注重算 {}）",
                    report.changed_shapes.len(),
                    report.evolution_resolved,
                    report.fingerprint_resolved,
                    report.reprojected_entities,
                    report.remeasured_annotations
                ))
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

    /// 钉 8（G6-M11）：删除流——造构件 + 建几何 → delete_element →
    /// 元素与几何一并消失 + 报告文本非空。
    #[test]
    fn script_deletes_an_element_with_removal_report() {
        let engine = build_engine();
        engine
            .run(&format!(
                "{ASSERT_EQ}
                let s = new_session();
                let e = s.create_element(\"BEAM\", \"测试梁\");
                s.build_geometry(e, \"example.beam.rect\");
                assert_eq(s.element_count(), 1, \"删除前\");
                let report = s.delete_element(e);
                if report.len() <= 0 {{ throw(\"删除报告应为非空：\" + report); }}
                assert_eq(s.element_count(), 0, \"删除后\");
                assert_eq(s.geometry_count(), 0, \"几何一并删\");"
            ))
            .unwrap_or_else(|e| panic!("脚本执行失败：{e}"));
    }

    /// 钉 9（G6-M11）：清单查询——list_elements / list_views / list_sheets
    /// 返回 Map 数组，`el.id` / `el.fmt` 直接访问且与造构件返回对账。
    #[test]
    fn script_lists_elements_views_sheets() {
        let engine = build_engine();
        engine
            .run(&format!(
                "{ASSERT_EQ}
                let s = new_session();
                let e = s.create_element(\"BEAM\", \"测试梁\");
                let els = s.list_elements();
                assert_eq(els.len(), 1, \"元素清单\");
                assert_eq(els[0].id, e, \"清单 id 与造构件返回一致\");
                assert_eq(els[0].category, \"BEAM\", \"品类\");
                assert_eq(els[0].has_geom, false, \"建几何前无几何\");
                s.build_geometry(e, \"example.beam.rect\");
                let v = s.create_view(\"俯视\");
                let sh = s.create_sheet(\"一层平面\", \"建施-01\", \"A2\");
                let views = s.list_views();
                assert_eq(views.len(), 1, \"视图清单\");
                assert_eq(views[0].id, v, \"视图 id\");
                let sheets = s.list_sheets();
                assert_eq(sheets.len(), 1, \"图纸清单\");
                assert_eq(sheets[0].id, sh, \"图纸 id\");
                assert_eq(sheets[0].fmt, \"A2\", \"图幅\");
                assert_eq(sheets[0].placements, 0, \"布置数\");"
            ))
            .unwrap_or_else(|e| panic!("脚本执行失败：{e}"));
    }

    /// 钉 10（G6-M11）：属性更新——new_property_map + pm.set（float/string
    /// 两种路由）→ update_element_properties 是撤销步（can_undo 对账）。
    #[test]
    fn script_updates_element_properties() {
        let engine = build_engine();
        engine
            .run(&format!(
                "{ASSERT_EQ}
                let s = new_session();
                let e = s.create_element(\"BEAM\", \"测试梁\");
                let pm = new_property_map(\"example.beam\");
                pm.set(\"width\", 300.0);
                pm.set(\"note\", \"加宽\");
                s.update_element_properties(e, pm);
                assert_eq(s.can_undo(), true, \"属性更新应是撤销步\");
                let ok = s.undo();
                assert_eq(ok, true, \"属性更新可撤销\");"
            ))
            .unwrap_or_else(|e| panic!("脚本执行失败：{e}"));
    }

    /// 钉 11（G6-M11）：关联更新重建——rebuild_geometry 用新参数重建几何 →
    /// 元素与几何计数不变（复用 ShapeId，替换非新增）+ 报告非空。
    #[test]
    fn script_rebuilds_geometry_with_new_params() {
        let engine = build_engine();
        engine
            .run(&format!(
                "{ASSERT_EQ}
                let s = new_session();
                let e = s.create_element(\"BEAM\", \"测试梁\");
                s.build_geometry(e, \"example.beam.rect\");
                let pm = new_property_map(\"example.beam\");
                pm.set(\"width\", 400.0);
                let report = s.rebuild_geometry(e, \"example.beam.rect\", pm);
                if report.len() <= 0 {{ throw(\"重建报告应为非空：\" + report); }}
                assert_eq(s.element_count(), 1, \"元素仍在\");
                assert_eq(s.geometry_count(), 1, \"几何替换非新增\");"
            ))
            .unwrap_or_else(|e| panic!("脚本执行失败：{e}"));
    }

    /// 钉 12（G6-M11）：历史状态——can_undo/undo_depth/can_redo/redo_depth
    /// 与栈严格对账。
    #[test]
    fn script_history_state_reconciles() {
        let engine = build_engine();
        engine
            .run(&format!(
                "{ASSERT_EQ}
                let s = new_session();
                assert_eq(s.can_undo(), false, \"空栈\");
                assert_eq(s.undo_depth(), 0, \"空栈深度\");
                s.create_element(\"BEAM\", \"梁一\");
                s.create_element(\"BEAM\", \"梁二\");
                assert_eq(s.undo_depth(), 2, \"两步撤销\");
                assert_eq(s.can_undo(), true, \"可撤销\");
                s.undo();
                s.undo();
                assert_eq(s.can_redo(), true, \"可重做\");
                assert_eq(s.redo_depth(), 2, \"重做栈 2 步\");"
            ))
            .unwrap_or_else(|e| panic!("脚本执行失败：{e}"));
    }

    /// 钉 13（G6-M11）：属性类型路由——数组等不支持类型响亮拒绝（不静默丢弃）。
    #[test]
    fn script_rejects_unsupported_property_type() {
        let engine = build_engine();
        let err = engine
            .run(
                "let pm = new_property_map(\"example.beam\");
                 pm.set(\"bad\", [1, 2, 3]);",
            )
            .expect_err("数组值应失败");
        assert!(
            err.to_string().contains("类型不支持"),
            "错误应含类型拒绝文案：{err}"
        );
    }
}
