//! `.bimdoc` 容器读写（设计文档 4.2）——**zip 形态与目录形态**。
//!
//! ## 容器长什么样（v2）
//!
//! ```text
//! xxx.bimdoc（zip，或一个同名目录——设计文档原话"其实是一个 zip / 目录"）
//!  ├─ manifest.json          # 版本识别 + 元信息（本文件自己定）
//!  ├─ doc.json               # 文档语义部分（人可读、可 diff）——【权威副本】
//!  ├─ drawing.json           # 图纸头部：standard / sheets / dependencies + views 计数
//!  ├─ views/<viewid>.json    # 每个视图本体一份（设计文档 4.2 原话）
//!  └─ geom/<shapeid>.brep    # 每个 GeometryRecord 一份标准导出副本
//! ```
//!
//! ## 谁是权威：`doc.json`（务必知悉）
//!
//! `core-model` 的 [`Document`] **已经内嵌**了每份几何的 BREP 文本
//! （[`GeometryRecord::text`]），因此 [`Document`] 的 JSON 序列化里就带着几何正文。
//! 本容器据此定下口径：
//!
//! | 文件 | 角色 |
//! |---|---|
//! | `doc.json` | **权威副本**——`open` 时的唯一真相来源 |
//! | `geom/<shapeid>.brep` | **标准导出副本**——给外部工具看、给人 diff、给未来"几何后台重建"预留 |
//!
//! 二者**若都存在必须一致**：[`open_document`] / [`open_from_dir`] 会逐条比对，
//! 不一致**明确报错**，绝不静默取其一（设计文档 §1：宁可报错不可猜）。
//!
//! > 之所以选"doc.json 权威"而不是"geom 正文权威 + doc.json 只留壳"：
//! > `Document` 的 `PartialEq` 含 `geometry`，要让 `save → open` **语义往返相等**，
//! > 最稳的做法是让权威副本随 `Document` 一起走一遍 JSON；`geom/*.brep`
//! > 作为冗余导出件，反而成了"能独立核对的第二份证据"。
//!
//! ## 会话态基准元素：**不写盘**（设计文档 4.6.3）
//!
//! `Document.datums` 里可能含 `DatumScope::Session` 的基准元素（如"当前工作
//! 平面"）。这类**纯会话状态不进 `.bimdoc`**——`doc_json` 写盘前先过一遍
//! `Document::to_persistable`，把会话态元素剔除。少了这一步，下次打开会凭空
//! 多出一个本不存在的基准元素。
//!
//! ## 关于错误信息里的绝对路径（**有意为之，不修**）
//!
//! 容器错误里会带上 `path.display()`（如"创建 D:\\...\\x.bimdoc 失败"）。这是
//! **故意的**：`.bimdoc` 读写发生在**用户本机**，路径对用户是已知信息，写进错误
//! 里恰恰是为了让"到底是哪个文件坏了"一眼可见。设计文档 §1 的口径是"宁可报错
//! 不可猜"——含糊的错误信息比暴露一个用户本就知道的路径更糟。**故此项不视为缺陷。**
//!
//! ## 视图 / 图纸（`drawing.json` + `views/<viewid>.json`）
//!
//! v2 起（设计文档 4.2 的 `views/<viewid>.json` 段）**已实现**：入口同时吃
//! `&Document` 与 `&Drawing`（二者由 [`crate::DocumentService`] 配对管理），视图
//! 本体拆出 `views/<view-id>.json` 逐个落盘，余下头部（standard / sheets /
//! dependencies + views 计数）写 `drawing.json`。`open` 反向重组，并校验
//! "声明计数 ↔ 实际文件数"（宁可报错不可猜）。
//!
//! `Drawing` 的语义级一致性（`Drawing.views` 的 id 集合 ↔ `Document.views`）
//! **不在 store 做**——open 装配后由调用方走
//! [`crate::DocumentService::sync_drawing_registry`]。
//!
//! ## 为什么要同时支持"目录形态"
//!
//! zip 是交付形态，目录是**调试与测试形态**：可直接 `cat doc.json`、可 `git diff`、
//! 出错时能一眼看见是哪个文件坏了。两条路共用同一份"组装 / 校验"逻辑，
//! 只有最外层的读 / 写不同。

use std::fs;
use std::io::{Read, Seek, Write};
use std::path::Path;

use core_doc::Drawing;
use core_model::{Document, PropertySchema, ShapeId};
use serde::{Deserialize, Serialize};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

use crate::error::{AppError, AppResult};

/// 容器格式版本。
///
/// **任何不兼容的容器布局变更都要 +1**；`open` 时版本不符即报错，
/// 不做"尽力解析"（宁可报错不可猜）。
///
/// v2（M6a）：新增 `drawing.json` + `views/` 内容段——图纸随容器落盘。
/// 旧 v1 容器无此段，Drawing 无从恢复，故 v1 一律拒收（不做向后兼容）。
///
/// v2 + 可选段（G6-M2）：新增 `schemas.json`（属性模式持久化）。这是
/// **可选段**而非不兼容变更——旧程序读新容器直接忽略该段（按段显式读、
/// file_count 只供人核对不校验），新程序读旧 v2 容器容忍缺段（恢复为空
/// registry，行为退化为类型管理器为空、validate 不拦截）。故版本号保持 2。
pub const FORMAT_VERSION: u32 = 2;

/// 清单文件名。
pub const CONTAINER_MANIFEST: &str = "manifest.json";

/// 文档语义部分（权威副本）的文件名。
const DOC_ENTRY: &str = "doc.json";

/// 图纸头部（standard / sheets / dependencies + views 计数）的文件名。
pub const DRAWING_ENTRY: &str = "drawing.json";

/// 属性模式持久化段（G6-M2：自定义类型重开不丢）的文件名。
///
/// **可选段**：旧 v2 容器无此段，`open` 恢复为空 registry（向后兼容）。
pub const SCHEMAS_ENTRY: &str = "schemas.json";

/// 视图本体所在目录（每个 View 一份 `views/<view-id>.json`）。
pub const VIEWS_DIR: &str = "views";

/// 几何副本所在目录。
const GEOM_DIR: &str = "geom";

/// 几何副本扩展名（设计文档 4.2：`geom/<shapeid>.brep`）。
const GEOM_EXT: &str = "brep";

/// 写入清单的应用名（版本识别 / 溯源用）。
const APP_NAME: &str = env!("CARGO_PKG_NAME");

/// 容器清单（`manifest.json`）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Manifest {
    /// 容器布局版本（见 [`FORMAT_VERSION`]）
    format_version: u32,
    /// 写入本容器的应用名
    app: String,
    /// 创建时间（ISO 8601 / UTC；取自文档元信息，可能为空串）
    created_utc: String,
    /// 负载文件数（`doc.json` + `drawing.json` + 各 `views/*.json` + 各
    /// `geom/*.brep`），供人核对
    file_count: usize,
}

/// 几何副本在容器内的相对路径。
fn geom_entry(shape: ShapeId) -> String {
    format!("{GEOM_DIR}/{shape}.{GEOM_EXT}")
}

/// 组装清单文本。
fn manifest_json(doc: &Document, drawing: &Drawing) -> AppResult<String> {
    let manifest = Manifest {
        format_version: FORMAT_VERSION,
        app: APP_NAME.to_string(),
        created_utc: doc.meta().created_utc.clone(),
        // 1 份 doc.json + 1 份 drawing.json + 1 份 schemas.json（G6-M2 可选段）
        // + 每个视图一份 + 每份几何一份 .brep
        file_count: 1 + 1 + 1 + drawing.view_count() + doc.geometry_count(),
    };
    serde_json::to_string_pretty(&manifest)
        .map_err(|e| AppError::Container(format!("manifest.json 序列化失败：{e}")))
}

/// 解析并校验清单（**版本不认识就报错**）。
fn parse_manifest(text: &str) -> AppResult<Manifest> {
    let manifest: Manifest = serde_json::from_str(text)
        .map_err(|e| AppError::Container(format!("{CONTAINER_MANIFEST} 解析失败：{e}")))?;
    if manifest.format_version != FORMAT_VERSION {
        return Err(AppError::Container(format!(
            "不认识的 .bimdoc 格式版本 {}（本应用支持 {FORMAT_VERSION}）",
            manifest.format_version
        )));
    }
    Ok(manifest)
}

/// 组装 `doc.json`（权威副本）。
///
/// **写盘前先过 [`Document::to_persistable`]**：剔除会话态基准元素
/// （[`DatumScope::Session`]，设计文档 4.6.3）。少了这一步，会话态会随文件
/// 泄漏出去——下次打开时会凭空多出一个本不存在的基准元素。
fn doc_json(doc: &Document) -> AppResult<String> {
    serde_json::to_string_pretty(&doc.to_persistable())
        .map_err(|e| AppError::Container(format!("{DOC_ENTRY} 序列化失败：{e}")))
}

/// 解析 `doc.json`。
fn parse_doc(text: &str) -> AppResult<Document> {
    serde_json::from_str(text)
        .map_err(|e| AppError::Container(format!("{DOC_ENTRY} 解析失败：{e}")))
}

/// 逐条校验"导出的几何副本"与 `doc.json` 里的权威副本一致。
///
/// `read` 给出"按容器内相对路径取文本"的能力（zip 与目录两形态各给一个实现）。
/// 缺文件、内容不一致都返回明确错误。
fn verify_geometry<F>(doc: &Document, mut read: F) -> AppResult<()>
where
    F: FnMut(&str) -> AppResult<Option<String>>,
{
    for (shape, record) in doc.geometry() {
        let entry = geom_entry(*shape);
        match read(&entry)? {
            Some(text) => {
                if text != record.text {
                    return Err(AppError::Container(format!(
                        "几何副本不一致：{entry} 与 doc.json 中 {shape} 的正文不同\
                         （doc.json 是权威副本，导出副本必须与它逐字节相同）"
                    )));
                }
            }
            None => {
                return Err(AppError::Container(format!(
                    "缺少几何副本：{entry}（每个几何记录都必须有 {GEOM_DIR}/<shapeid>.{GEOM_EXT}）"
                )));
            }
        }
    }
    Ok(())
}

// ============================================================================
// 属性模式拆装（G6-M2：schemas.json 可选段——自定义类型重开不丢）
// ============================================================================

/// 把属性模式序列化为 `schemas.json`。
fn schemas_json(schemas: &[PropertySchema]) -> AppResult<String> {
    serde_json::to_string_pretty(schemas)
        .map_err(|e| AppError::Container(format!("{SCHEMAS_ENTRY} 序列化失败：{e}")))
}

/// 解析 `schemas.json`。
fn parse_schemas(text: &str) -> AppResult<Vec<PropertySchema>> {
    serde_json::from_str(text)
        .map_err(|e| AppError::Container(format!("{SCHEMAS_ENTRY} 解析失败：{e}")))
}

// ============================================================================
// Drawing 拆装（v2：drawing.json 头部 + views/<viewid>.json 本体）
// ============================================================================

/// 把 `Drawing` 序列化为"头部"（`drawing.json`）+ 每个视图一份 `views/` 条目。
///
/// 拆装方式：先整体序列化为 JSON 对象，把 `views` 字段**拆出**逐个写文件，
/// 原位换成 `views_count` 计数。open 反向：从 `views/` 文件重组 `views` 对象，
/// 再一次反序列化回 `Drawing`——两侧都走同一条 serde 路，语义往返相等。
fn drawing_header(drawing: &Drawing) -> AppResult<(String, Vec<(String, String)>)> {
    let mut value = serde_json::to_value(drawing)
        .map_err(|e| AppError::Container(format!("{DRAWING_ENTRY} 序列化失败：{e}")))?;
    let obj = value
        .as_object_mut()
        .ok_or_else(|| AppError::Container(format!("{DRAWING_ENTRY} 应为 JSON 对象")))?;
    let views = obj
        .remove("views")
        .ok_or_else(|| AppError::Container(format!("{DRAWING_ENTRY} 缺少 views 字段")))?;
    let views = views
        .as_object()
        .ok_or_else(|| AppError::Container("views 字段应为对象映射".to_string()))?;

    let mut entries = Vec::with_capacity(views.len());
    for (id, view) in views {
        let entry = format!("{VIEWS_DIR}/{id}.json");
        let text = serde_json::to_string_pretty(view)
            .map_err(|e| AppError::Container(format!("{entry} 序列化失败：{e}")))?;
        entries.push((entry, text));
    }

    obj.insert(
        "views_count".to_string(),
        serde_json::Value::from(entries.len()),
    );
    let text = serde_json::to_string_pretty(&value)
        .map_err(|e| AppError::Container(format!("{DRAWING_ENTRY} 序列化失败：{e}")))?;
    Ok((text, entries))
}

/// 从 `drawing.json` 头部 + `views/*.json` 内容重组 `Drawing`（**宁报错不猜**）：
///
/// - 声明的 `views_count` 与实际条目数不一致 → [`AppError::Container`]；
/// - 每个 `views/*.json` 必须可解析为 `View`（失败即报错，绝不跳过）；
/// - 文件名（`views/<id>.json`）与视图自身的 `id` 字段不一致 → 报错
///   （防"文件被改过名 / 手工搬动"的半坏容器）。
fn drawing_from_parts(header: &str, entries: Vec<(String, String)>) -> AppResult<Drawing> {
    let mut value: serde_json::Value = serde_json::from_str(header)
        .map_err(|e| AppError::Container(format!("{DRAWING_ENTRY} 解析失败：{e}")))?;
    let obj = value
        .as_object_mut()
        .ok_or_else(|| AppError::Container(format!("{DRAWING_ENTRY} 应为 JSON 对象")))?;
    let declared = obj
        .get("views_count")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| {
            AppError::Container(format!("{DRAWING_ENTRY} 缺少或含非法的 views_count 字段"))
        })? as usize;
    obj.remove("views_count");

    if entries.len() != declared {
        return Err(AppError::Container(format!(
            "views 段不一致：{DRAWING_ENTRY} 声明 {declared} 个视图，容器内实际 {} 个\
             （宁可报错不可猜）",
            entries.len()
        )));
    }

    let mut views = serde_json::Map::with_capacity(entries.len());
    for (entry, text) in entries {
        let view: serde_json::Value = serde_json::from_str(&text)
            .map_err(|e| AppError::Container(format!("{entry} 解析失败：{e}")))?;
        let core_doc::View { id, .. } = serde_json::from_value(view.clone())
            .map_err(|e| AppError::Container(format!("{entry} 不是合法的视图本体：{e}")))?;
        let stem = entry
            .strip_prefix(VIEWS_DIR)
            .and_then(|rest| rest.strip_prefix('/'))
            .and_then(|rest| rest.strip_suffix(".json"))
            .ok_or_else(|| AppError::Container(format!("views 段出现意外条目 {entry}")))?;
        if stem != id.to_string() {
            return Err(AppError::Container(format!(
                "视图文件名与本体 id 不一致：{entry} 的 stem 是 {stem}，本体 id 是 {id}"
            )));
        }
        views.insert(stem.to_string(), view);
    }

    obj.insert("views".to_string(), serde_json::Value::Object(views));
    serde_json::from_value(value)
        .map_err(|e| AppError::Container(format!("{DRAWING_ENTRY} 重组失败：{e}")))
}

/// 收集 zip 形态容器里的全部 `views/*.json` 条目。
fn read_views_zip<R: Read + Seek>(archive: &mut ZipArchive<R>) -> AppResult<Vec<(String, String)>> {
    let names: Vec<String> = archive
        .file_names()
        .filter(|n| n.starts_with(VIEWS_DIR) && n.ends_with(".json"))
        .map(|n| n.to_string())
        .collect();
    let mut out = Vec::with_capacity(names.len());
    for name in names {
        let text = read_zip_entry(archive, &name)?
            .ok_or_else(|| AppError::Container(format!("容器缺少 {name}")))?;
        out.push((name, text));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

/// 收集目录形态容器里的全部 `views/*.json`；目录不存在（空图纸）返回空。
fn read_views_dir(dir: &Path) -> AppResult<Vec<(String, String)>> {
    let views_dir = dir.join(VIEWS_DIR);
    let read = match fs::read_dir(&views_dir) {
        Ok(read) => read,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => {
            return Err(AppError::Container(format!(
                "读取 {} 失败：{e}",
                views_dir.display()
            )));
        }
    };
    let mut names = Vec::new();
    for entry in read {
        let entry = entry.map_err(|e| AppError::Container(format!("遍历 views 目录失败：{e}")))?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| AppError::Container("views 目录内出现非 UTF-8 文件名".to_string()))?;
        names.push(name.to_string());
    }
    names.sort();
    let mut out = Vec::with_capacity(names.len());
    for name in names {
        let path = views_dir.join(&name);
        let text = fs::read_to_string(&path)
            .map_err(|e| AppError::Container(format!("读取 {} 失败：{e}", path.display())))?;
        out.push((format!("{VIEWS_DIR}/{name}"), text));
    }
    Ok(out)
}

// ============================================================================
// zip 形态
// ============================================================================

/// 把文档写成一个 `.bimdoc`（zip）。
///
/// 路径约定后缀 `.bimdoc`，但本函数**不强制**——交给调用方决定（测试用临时名）。
pub fn save_document(
    path: &Path,
    doc: &Document,
    drawing: &Drawing,
    schemas: &[PropertySchema],
) -> AppResult<()> {
    let file = fs::File::create(path)
        .map_err(|e| AppError::Container(format!("创建 {} 失败：{e}", path.display())))?;
    let mut zip = ZipWriter::new(file);
    // 只开 deflate（见根 Cargo.toml 的说明：默认 features 会拖进一串无关依赖）
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);

    let (drawing_text, view_entries) = drawing_header(drawing)?;
    write_zip_entry(
        &mut zip,
        CONTAINER_MANIFEST,
        &manifest_json(doc, drawing)?,
        options,
    )?;
    write_zip_entry(&mut zip, DOC_ENTRY, &doc_json(doc)?, options)?;
    write_zip_entry(&mut zip, DRAWING_ENTRY, &drawing_text, options)?;
    write_zip_entry(&mut zip, SCHEMAS_ENTRY, &schemas_json(schemas)?, options)?;
    for (entry, text) in &view_entries {
        write_zip_entry(&mut zip, entry, text, options)?;
    }
    for (shape, record) in doc.geometry() {
        write_zip_entry(&mut zip, &geom_entry(*shape), &record.text, options)?;
    }

    zip.finish()
        .map_err(|e| AppError::Container(format!("zip 收尾失败：{e}")))?;
    Ok(())
}

/// 写入一个 zip 成员。
fn write_zip_entry<W: Write + Seek>(
    zip: &mut ZipWriter<W>,
    name: &str,
    body: &str,
    options: SimpleFileOptions,
) -> AppResult<()> {
    zip.start_file(name, options)
        .map_err(|e| AppError::Container(format!("zip 写入 {name} 失败：{e}")))?;
    zip.write_all(body.as_bytes())
        .map_err(|e| AppError::Container(format!("zip 写入 {name} 失败：{e}")))?;
    Ok(())
}

/// 从一个 `.bimdoc`（zip）读回文档、图纸与属性模式。
///
/// `schemas.json` 是**可选段**：旧 v2 容器无此段，恢复为空 `Vec`
/// （向后兼容——行为退化为类型管理器为空、validate 不拦截）。
pub fn open_document(path: &Path) -> AppResult<(Document, Drawing, Vec<PropertySchema>)> {
    let file = fs::File::open(path)
        .map_err(|e| AppError::Container(format!("打开 {} 失败：{e}", path.display())))?;
    let mut archive = ZipArchive::new(file).map_err(|e| {
        AppError::Container(format!("{} 不是有效的 zip / .bimdoc：{e}", path.display()))
    })?;

    let manifest_text = read_zip_entry(&mut archive, CONTAINER_MANIFEST)?
        .ok_or_else(|| AppError::Container(format!("容器缺少 {CONTAINER_MANIFEST}")))?;
    parse_manifest(&manifest_text)?;

    let doc_text = read_zip_entry(&mut archive, DOC_ENTRY)?
        .ok_or_else(|| AppError::Container(format!("容器缺少 {DOC_ENTRY}（不返回空文档）")))?;
    let doc = parse_doc(&doc_text)?;

    let drawing_text = read_zip_entry(&mut archive, DRAWING_ENTRY)?.ok_or_else(|| {
        AppError::Container(format!("容器缺少 {DRAWING_ENTRY}（v2 容器必须带图纸段）"))
    })?;
    let view_entries = read_views_zip(&mut archive)?;
    // 可选段：缺（旧 v2 容器）→ 空 Vec
    let schemas = match read_zip_entry(&mut archive, SCHEMAS_ENTRY)? {
        Some(text) => parse_schemas(&text)?,
        None => Vec::new(),
    };

    verify_geometry(&doc, |entry| read_zip_entry(&mut archive, entry))?;
    let drawing = drawing_from_parts(&drawing_text, view_entries)?;
    Ok((doc, drawing, schemas))
}

/// 读取一个 zip 成员为字符串；成员不存在返回 `Ok(None)`（其余错误照报）。
fn read_zip_entry<R: Read + Seek>(
    archive: &mut ZipArchive<R>,
    name: &str,
) -> AppResult<Option<String>> {
    match archive.by_name(name) {
        Ok(mut entry) => {
            let mut buf = String::new();
            entry
                .read_to_string(&mut buf)
                .map_err(|e| AppError::Container(format!("读取 {name} 失败：{e}")))?;
            Ok(Some(buf))
        }
        Err(zip::result::ZipError::FileNotFound) => Ok(None),
        Err(e) => Err(AppError::Container(format!("zip 读取 {name} 失败：{e}"))),
    }
}

// ============================================================================
// 目录形态（调试 / 测试用；与 zip 形态语义等价）
// ============================================================================

/// 把文档摊平写进一个目录（`manifest.json` / `doc.json` / `drawing.json` /
/// `schemas.json` / `views/*.json` / `geom/*.brep`）。
pub fn pack_to_dir(
    dir: &Path,
    doc: &Document,
    drawing: &Drawing,
    schemas: &[PropertySchema],
) -> AppResult<()> {
    fs::create_dir_all(dir)
        .map_err(|e| AppError::Container(format!("创建目录 {} 失败：{e}", dir.display())))?;
    let geom_dir = dir.join(GEOM_DIR);
    fs::create_dir_all(&geom_dir)
        .map_err(|e| AppError::Container(format!("创建目录 {} 失败：{e}", geom_dir.display())))?;
    let views_dir = dir.join(VIEWS_DIR);
    fs::create_dir_all(&views_dir)
        .map_err(|e| AppError::Container(format!("创建目录 {} 失败：{e}", views_dir.display())))?;

    let (drawing_text, view_entries) = drawing_header(drawing)?;
    fs::write(dir.join(CONTAINER_MANIFEST), manifest_json(doc, drawing)?)
        .map_err(|e| AppError::Container(format!("写 {CONTAINER_MANIFEST} 失败：{e}")))?;
    fs::write(dir.join(DOC_ENTRY), doc_json(doc)?)
        .map_err(|e| AppError::Container(format!("写 {DOC_ENTRY} 失败：{e}")))?;
    fs::write(dir.join(DRAWING_ENTRY), drawing_text)
        .map_err(|e| AppError::Container(format!("写 {DRAWING_ENTRY} 失败：{e}")))?;
    fs::write(dir.join(SCHEMAS_ENTRY), schemas_json(schemas)?)
        .map_err(|e| AppError::Container(format!("写 {SCHEMAS_ENTRY} 失败：{e}")))?;
    for (entry, text) in &view_entries {
        fs::write(dir.join(entry), text)
            .map_err(|e| AppError::Container(format!("写 {entry} 失败：{e}")))?;
    }
    for (shape, record) in doc.geometry() {
        let entry = geom_dir.join(format!("{shape}.{GEOM_EXT}"));
        fs::write(&entry, &record.text)
            .map_err(|e| AppError::Container(format!("写 {} 失败：{e}", entry.display())))?;
    }
    Ok(())
}

/// 从一个目录读回文档、图纸与属性模式。
///
/// `schemas.json` 是**可选段**：目录里无此文件（手工构造的旧 v2 容器）
/// 恢复为空 `Vec`（向后兼容）。
pub fn open_from_dir(dir: &Path) -> AppResult<(Document, Drawing, Vec<PropertySchema>)> {
    let manifest_path = dir.join(CONTAINER_MANIFEST);
    let manifest_text = fs::read_to_string(&manifest_path)
        .map_err(|e| AppError::Container(format!("读取 {} 失败：{e}", manifest_path.display())))?;
    parse_manifest(&manifest_text)?;

    let doc_path = dir.join(DOC_ENTRY);
    let doc_text = fs::read_to_string(&doc_path)
        .map_err(|e| AppError::Container(format!("读取 {} 失败：{e}", doc_path.display())))?;
    let doc = parse_doc(&doc_text)?;

    let drawing_path = dir.join(DRAWING_ENTRY);
    let drawing_text = fs::read_to_string(&drawing_path).map_err(|e| {
        AppError::Container(format!(
            "读取 {} 失败：{e}（v2 容器必须带图纸段）",
            drawing_path.display()
        ))
    })?;
    let view_entries = read_views_dir(dir)?;
    // 可选段：缺（手工构造的旧 v2 容器）→ 空 Vec
    let schemas = match fs::read_to_string(dir.join(SCHEMAS_ENTRY)) {
        Ok(text) => parse_schemas(&text)?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => {
            return Err(AppError::Container(format!(
                "读取 {} 失败：{e}",
                dir.join(SCHEMAS_ENTRY).display()
            )));
        }
    };

    let geom_dir = dir.join(GEOM_DIR);
    verify_geometry(&doc, |entry| {
        let p = geom_dir.join(entry.rsplit('/').next().unwrap_or(entry));
        match fs::read_to_string(&p) {
            Ok(text) => Ok(Some(text)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(AppError::Container(format!(
                "读取 {} 失败：{e}",
                p.display()
            ))),
        }
    })?;
    let drawing = drawing_from_parts(&drawing_text, view_entries)?;
    Ok((doc, drawing, schemas))
}

#[cfg(test)]
mod tests {
    use super::*;
    use core_doc::{Entity2D, View, ViewKind, ViewOrientation};
    use core_model::{Category, DatumElement, Element, GeometryRecord, LayerId};
    use glam::DVec2;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// 一个测试用临时目录（**不依赖 tempfile**）：名字里带进程 id 与计数器，
    /// 保证多进程 / 多用例下唯一；`Drop` 时递归删除。
    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new(tag: &str) -> Self {
            static COUNTER: AtomicU64 = AtomicU64::new(0);
            let n = COUNTER.fetch_add(1, Ordering::Relaxed);
            let mut path = std::env::temp_dir();
            path.push(format!("bimdoc-test-{}-{tag}-{n}", std::process::id()));
            fs::create_dir_all(&path).unwrap();
            Self { path }
        }

        fn join(&self, name: &str) -> PathBuf {
            self.path.join(name)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    /// 造一份"有元素、有基准、有几何"的测试文档。
    fn sample_document() -> Document {
        let mut doc = Document::new("往返用例");
        doc.insert_datum(DatumElement::level("F1", 0.0)).unwrap();

        let shape = ShapeId::new();
        doc.insert_geometry(GeometryRecord::brep(
            shape,
            "MOCK-BREP v1 box 100 200 300 6e6",
        ))
        .unwrap();
        let element = Element::new(Category::new("DEMO"), "柱-1").with_geometry(shape);
        doc.insert_element(element).unwrap();
        doc
    }

    /// 造一个带实体的纯绘图视图（平面视图要求基准元素，store 层不挂）。
    fn sample_view(name: &str) -> View {
        let mut v = View::new(name, ViewKind::Drafting);
        v.orientation = ViewOrientation::top();
        v.push_entity(Entity2D::line(
            LayerId::new(),
            DVec2::ZERO,
            DVec2::new(6000.0, 0.0),
        ));
        v
    }

    #[test]
    fn zip_round_trip_preserves_the_document() {
        let dir = TempDir::new("zip");
        let path = dir.join("model.bimdoc");
        let doc = sample_document();
        let drawing = Drawing::new();

        save_document(&path, &doc, &drawing, &[]).unwrap();
        let (back, back_drawing, _) = open_document(&path).unwrap();

        // 逐字段（Document: PartialEq）相等
        assert_eq!(back, doc);
        assert_eq!(back.element_count(), 1);
        assert_eq!(back.geometry_count(), 1);
        assert_eq!(back.datums().len(), 1);
        assert_eq!(back_drawing, drawing, "空图纸也要往返相等");
    }

    #[test]
    fn directory_round_trip_preserves_the_document() {
        let dir = TempDir::new("dir");
        let container = dir.join("model");
        let doc = sample_document();
        let drawing = Drawing::new();

        pack_to_dir(&container, &doc, &drawing, &[]).unwrap();
        let (back, back_drawing, _) = open_from_dir(&container).unwrap();

        assert_eq!(back, doc);
        assert_eq!(back_drawing, drawing);
    }

    #[test]
    fn unknown_format_version_is_rejected() {
        let dir = TempDir::new("badver");
        let container = dir.join("model");
        let doc = sample_document();
        pack_to_dir(&container, &doc, &Drawing::new(), &[]).unwrap();

        // 手写一个"未来版本"的清单进去
        let bad = r#"{"format_version": 999, "app": "future", "created_utc": "", "file_count": 1}"#;
        fs::write(container.join(CONTAINER_MANIFEST), bad).unwrap();

        let err = open_from_dir(&container).unwrap_err();
        assert!(
            matches!(&err, AppError::Container(msg) if msg.contains("格式版本")),
            "版本不认识必须报错，实得 {err:?}"
        );
    }

    #[test]
    fn missing_doc_json_is_an_error_not_an_empty_document() {
        let dir = TempDir::new("nodoc");
        let container = dir.join("model");
        let doc = sample_document();
        pack_to_dir(&container, &doc, &Drawing::new(), &[]).unwrap();

        fs::remove_file(container.join(DOC_ENTRY)).unwrap();
        let err = open_from_dir(&container).unwrap_err();
        assert!(
            matches!(&err, AppError::Container(msg) if msg.contains(DOC_ENTRY)),
            "缺 doc.json 必须报错而不是返回空文档，实得 {err:?}"
        );
    }

    #[test]
    fn missing_doc_json_in_a_zip_is_also_an_error() {
        // zip 形态走的是另一条读取路径，单独再钉一次，避免只测目录形态
        let dir = TempDir::new("zipnodoc");
        // 手工造一个只有 manifest 的 zip
        let path = dir.join("broken.bimdoc");
        let doc = sample_document();
        save_document(&path, &doc, &Drawing::new(), &[]).unwrap();
        // 重新打包：只写 manifest（模拟缺 doc.json 的坏容器）
        {
            let file = fs::File::create(&path).unwrap();
            let mut zip = ZipWriter::new(file);
            let options =
                SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
            zip.start_file(CONTAINER_MANIFEST, options).unwrap();
            zip.write_all(manifest_json(&doc, &Drawing::new()).unwrap().as_bytes())
                .unwrap();
            zip.finish().unwrap();
        }

        let err = open_document(&path).unwrap_err();
        assert!(
            matches!(&err, AppError::Container(msg) if msg.contains(DOC_ENTRY)),
            "zip 缺 doc.json 同样必须报错，实得 {err:?}"
        );
    }

    #[test]
    fn geometry_brep_files_are_materialized_for_every_record() {
        let dir = TempDir::new("geom");
        let container = dir.join("model");
        let mut doc = Document::new("多几何");
        let a = ShapeId::new();
        let b = ShapeId::new();
        doc.insert_geometry(GeometryRecord::brep(a, "MOCK-BREP v1 box 1 2 3 6"))
            .unwrap();
        doc.insert_geometry(GeometryRecord::brep(b, "MOCK-BREP v1 sphere 4 4 4 33.5"))
            .unwrap();

        pack_to_dir(&container, &doc, &Drawing::new(), &[]).unwrap();

        for (shape, record) in doc.geometry() {
            let p = container.join(GEOM_DIR).join(format!("{shape}.{GEOM_EXT}"));
            assert!(p.is_file(), "每个几何记录都应有副本文件：{}", p.display());
            let text = fs::read_to_string(&p).unwrap();
            assert_eq!(&text, &record.text);
        }
        // 目录里确实有 2 份
        let count = fs::read_dir(container.join(GEOM_DIR)).unwrap().count();
        assert_eq!(count, doc.geometry_count());
    }

    #[test]
    fn inconsistent_brep_copy_is_rejected() {
        let dir = TempDir::new("mismatch");
        let container = dir.join("model");
        let doc = sample_document();
        pack_to_dir(&container, &doc, &Drawing::new(), &[]).unwrap();

        // 把某一份导出副本改坏
        let shape = *doc.geometry().keys().next().unwrap();
        let p = container.join(GEOM_DIR).join(format!("{shape}.{GEOM_EXT}"));
        fs::write(&p, "MOCK-BREP v1 box 9 9 9 729").unwrap();

        let err = open_from_dir(&container).unwrap_err();
        assert!(
            matches!(&err, AppError::Container(msg) if msg.contains("不一致")),
            "doc.json 与 geom 副本不一致必须报错，实得 {err:?}"
        );
    }

    #[test]
    fn session_scoped_datums_are_not_written_to_the_container() {
        // 设计文档 4.6.3：会话态基准元素（"当前工作平面"）不进 .bimdoc。
        let dir = TempDir::new("session");
        let path = dir.join("model.bimdoc");
        let mut doc = sample_document();
        let mut work_plane = DatumElement::ref_plane("当前工作平面");
        work_plane.scope = core_model::DatumScope::Session;
        doc.insert_datum(work_plane).unwrap();
        assert_eq!(doc.persistent_datums().count(), 1, "文档级标高 1 个");
        assert_eq!(doc.session_datums().len(), 1, "会话态工作平面 1 个");

        save_document(&path, &doc, &Drawing::new(), &[]).unwrap();
        let (back, _, _) = open_document(&path).unwrap();

        // 打开后的文档 == 可持久化副本（会话态被剔除）
        assert_eq!(back, doc.to_persistable());
        assert_eq!(
            back.datums().len(),
            doc.persistent_datums().count(),
            "会话态基准元素被写进了 .bimdoc（设计文档 4.6.3 要求剔除）"
        );
        assert!(
            back.session_datums().is_empty(),
            "打开 .bimdoc 后不应出现会话态基准元素"
        );
    }

    #[test]
    fn a_non_zip_file_is_rejected() {
        let dir = TempDir::new("notzip");
        let path = dir.join("garbage.bimdoc");
        fs::write(&path, b"this is not a zip").unwrap();
        assert!(matches!(open_document(&path), Err(AppError::Container(_))));
    }

    #[test]
    fn a_missing_file_is_reported_not_an_empty_document() {
        let dir = TempDir::new("absent");
        let path = dir.join("nope.bimdoc");
        assert!(matches!(open_document(&path), Err(AppError::Container(_))));
    }

    // ---------- M6a-1：容器 v2 钉子（Drawing 进容器）----------

    #[test]
    fn bimdoc_v1_manifest_is_rejected() {
        // 版本纪律钉：v1 容器无 drawing.json / views/ 段，Drawing 无从恢复，
        // 手工构造的 format_version=1 清单必须被拒（不做向后兼容）。
        let dir = TempDir::new("v1manifest");
        let container = dir.join("model");
        let doc = sample_document();
        pack_to_dir(&container, &doc, &Drawing::new(), &[]).unwrap();

        let v1 = format!(
            r#"{{"format_version": 1, "app": "legacy", "created_utc": "", "file_count": {}}}"#,
            1 + doc.geometry_count()
        );
        fs::write(container.join(CONTAINER_MANIFEST), &v1).unwrap();

        let err = open_from_dir(&container).unwrap_err();
        assert!(
            matches!(&err, AppError::Container(msg) if msg.contains("格式版本 1")),
            "v1 清单必须被拒收，实得 {err:?}"
        );

        // zip 形态同样拒收（另一条读取路径）
        let zip_path = dir.join("legacy.bimdoc");
        save_document(&zip_path, &doc, &Drawing::new(), &[]).unwrap();
        let mut es: Vec<(String, Vec<u8>)> = {
            let f = fs::File::open(&zip_path).unwrap();
            let mut archive = ZipArchive::new(f).unwrap();
            (0..archive.len())
                .map(|i| {
                    let mut e = archive.by_index(i).unwrap();
                    let name = e.name().to_string();
                    let mut buf = Vec::new();
                    std::io::Read::read_to_end(&mut e, &mut buf).unwrap();
                    (name, buf)
                })
                .collect()
        };
        for (n, b) in es.iter_mut() {
            if n == CONTAINER_MANIFEST {
                *b = v1.as_bytes().to_vec();
            }
        }
        write_zip(&zip_path, &es);
        let err = open_document(&zip_path).unwrap_err();
        assert!(
            matches!(&err, AppError::Container(msg) if msg.contains("格式版本 1")),
            "zip 形态的 v1 清单同样必须被拒收，实得 {err:?}"
        );
    }

    #[test]
    fn bimdoc_drawing_roundtrip_preserves_views() {
        // 往返钉：带 2 视图（含实体）save → open → Drawing 整体相等。
        let dir = TempDir::new("drawingrt");
        let path = dir.join("drawing.bimdoc");
        let doc = sample_document();

        let mut drawing = Drawing::new();
        let v1 = sample_view("俯视");
        let v2 = sample_view("侧视");
        drawing.add_view(v1);
        drawing.add_view(v2);
        drawing.rebuild_index();
        assert_eq!(drawing.view_count(), 2);

        save_document(&path, &doc, &drawing, &[]).unwrap();

        // 容器结构先核对：drawing.json + 2 个 views/*.json
        let f = fs::File::open(&path).unwrap();
        let mut archive = ZipArchive::new(f).unwrap();
        let names: Vec<String> = (0..archive.len())
            .map(|i| archive.by_index(i).unwrap().name().to_string())
            .collect();
        assert!(
            names.contains(&DRAWING_ENTRY.to_string()),
            "容器必须含 {DRAWING_ENTRY}：{names:?}"
        );
        assert_eq!(
            names
                .iter()
                .filter(|n| n.starts_with("views/") && n.ends_with(".json"))
                .count(),
            2,
            "2 个视图应落 2 个 views/*.json：{names:?}"
        );
        drop(archive);

        let (back_doc, back_drawing, _) = open_document(&path).unwrap();
        assert_eq!(back_doc, doc, "文档往返相等");
        assert_eq!(
            back_drawing, drawing,
            "Drawing 整体（含实体、索引、标准）必须往返相等"
        );
        for (id, view) in &drawing.views {
            let back_view = back_drawing.view(*id).unwrap();
            assert_eq!(back_view, view, "视图 {id} 往返相等");
            assert_eq!(back_view.entities.len(), view.entities.len());
        }
    }

    #[test]
    fn bimdoc_views_entry_missing_is_rejected() {
        // 铁律⑥钉：drawing.json 声明 2 视图但 views/ 只剩 1 个文件 → open 必报错。
        let dir = TempDir::new("viewmiss");
        let container = dir.join("model");
        let doc = sample_document();

        let mut drawing = Drawing::new();
        drawing.add_view(sample_view("俯视"));
        drawing.add_view(sample_view("侧视"));
        pack_to_dir(&container, &doc, &drawing, &[]).unwrap();

        let mut views: Vec<PathBuf> = fs::read_dir(container.join(VIEWS_DIR))
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("json"))
            .collect();
        views.sort();
        assert_eq!(views.len(), 2, "探针前提：views/ 应有 2 个文件");
        fs::remove_file(&views[0]).unwrap();

        let err = open_from_dir(&container).unwrap_err();
        assert!(
            matches!(&err, AppError::Container(msg) if msg.contains("views 段不一致")),
            "声明 2 视图实际 1 个必须报错，实得 {err:?}"
        );

        // 解析失败同样报错：把另一个视图文件改成非法 JSON
        fs::write(&views[1], "{ 不是 json").unwrap();
        let err = open_from_dir(&container).unwrap_err();
        assert!(
            matches!(&err, AppError::Container(_)),
            "views/*.json 解析失败必须报错，实得 {err:?}"
        );
    }

    // ---- G6-M2：schema 持久化（schemas.json 可选段） ----

    /// G6-M2 钉 4：schemas 随容器往返——save 带 2 个模式 → open 恢复；
    /// 容器内确实有 schemas.json；zip 与目录两形态都钉。
    #[test]
    fn schemas_persist_through_both_container_shapes() {
        use core_model::{FieldType, PropertySchema};

        let schema = |name: &str| {
            PropertySchema::new(name).with_field(
                core_model::FieldDef::new("width", FieldType::Float, "宽")
                    .with_unit("mm")
                    .required(),
            )
        };
        let schemas = vec![schema("TRAY"), schema("DUCT")];

        // zip 形态
        let dir = TempDir::new("zipchemas");
        let path = dir.join("model.bimdoc");
        let doc = sample_document();
        save_document(&path, &doc, &Drawing::new(), &schemas).unwrap();
        let (_, _, back) = open_document(&path).unwrap();
        assert_eq!(back, schemas, "zip 形态 schemas 往返相等");

        // 目录形态
        let dir2 = TempDir::new("dirchemas");
        let container = dir2.join("model");
        pack_to_dir(&container, &doc, &Drawing::new(), &schemas).unwrap();
        let (_, _, back) = open_from_dir(&container).unwrap();
        assert_eq!(back, schemas, "目录形态 schemas 往返相等");

        // 容器结构先核对：zip 内确实有 schemas.json
        let f = fs::File::open(&path).unwrap();
        let mut archive = ZipArchive::new(f).unwrap();
        let names: Vec<String> = (0..archive.len())
            .map(|i| archive.by_index(i).unwrap().name().to_string())
            .collect();
        assert!(
            names.contains(&SCHEMAS_ENTRY.to_string()),
            "容器必须含 {SCHEMAS_ENTRY}：{names:?}"
        );
    }

    /// G6-M2 钉 5（向后兼容）：旧 v2 容器**没有** schemas.json → open 成功、
    /// 恢复空 Vec（不报错——可选段的明确语义；zip 与目录两形态都钉）。
    #[test]
    fn old_v2_container_without_schemas_still_opens_with_empty_registry() {
        let dir = TempDir::new("oldschemas");
        let doc = sample_document();

        // 目录形态：手工构造无 schemas.json 的旧 v2 容器
        let container = dir.join("model");
        pack_to_dir(&container, &doc, &Drawing::new(), &[]).unwrap();
        fs::remove_file(container.join(SCHEMAS_ENTRY)).unwrap();
        let (_, _, back) = open_from_dir(&container).unwrap();
        assert!(
            back.is_empty(),
            "缺 schemas.json 的旧容器恢复空 Vec（向后兼容）"
        );

        // zip 形态：手工重构一个无 schemas.json 的 zip
        let zip_path = dir.join("legacy.bimdoc");
        save_document(&zip_path, &doc, &Drawing::new(), &[]).unwrap();
        {
            let f = fs::File::open(&zip_path).unwrap();
            let mut archive = ZipArchive::new(f).unwrap();
            let kept: Vec<(String, Vec<u8>)> = (0..archive.len())
                .filter_map(|i| {
                    let mut e = archive.by_index(i).unwrap();
                    let name = e.name().to_string();
                    if name == SCHEMAS_ENTRY {
                        return None; // 模拟旧容器：没有这个段
                    }
                    let mut buf = Vec::new();
                    std::io::Read::read_to_end(&mut e, &mut buf).unwrap();
                    Some((name, buf))
                })
                .collect();
            write_zip(&zip_path, &kept);
        }
        let (_, _, back) = open_document(&zip_path).unwrap();
        assert!(
            back.is_empty(),
            "zip 缺 schemas.json 同样恢复空 Vec（向后兼容）"
        );
    }

    /// 独立的 zip 写出工具（v1 拒收钉复用；与 store 生产逻辑无关）。
    fn write_zip(path: &Path, entries: &[(String, Vec<u8>)]) {
        let f = fs::File::create(path).unwrap();
        let mut zip = ZipWriter::new(f);
        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
        for (name, body) in entries {
            zip.start_file(name.as_str(), options).unwrap();
            zip.write_all(body).unwrap();
        }
        zip.finish().unwrap();
    }
}
