//! QA 独立探针：攻击 `.bimdoc` 容器与会话门面中**工程师未覆盖**的路径。
//!
//! 由 QA 工程师（Edward）编写，**只**依赖 `core-app` / `core-model` / `core-sdk`
//! 的公开 API，不碰任何生产代码。运行：`cargo test -p core-app --test container_probe`
//!
//! 覆盖：空文档往返、三份几何往返、zip 形态几何篡改/缺失、清单版本篡改、
//! 非 UTF-8 垃圾、类型混淆（目录当 zip）、`doc.json` 合法 JSON 但结构错、
//! 中文名往返；以及会话门面的句柄释放、深相等、未登记算子/品类、重复构造、
//! 已释放句柄快照等边界。

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::panic))]

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use core_app::{
    AppError, CoreSession, GeometryService, open_document, open_from_dir, pack_to_dir,
    save_document,
};
use core_doc::Drawing;
use core_model::{
    Category, DatumElement, DatumScope, Document, Element, GeometryRecord, PropertyMap, ShapeId,
    Value,
};
use core_sdk::{
    BuildOp, BuildStep, CreateRequest, Element as SdkElement, ElementFactory, GeometryOp, Module,
    ModuleManifest, OpInput, OpOutput, Registry, SdkResult, build_element,
};

// ---------------------------------------------------------------------------
// 临时目录（自管，不引入 tempfile）
// ---------------------------------------------------------------------------

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(tag: &str) -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let mut path = std::env::temp_dir();
        path.push(format!("qa-probe-{}-{tag}-{n}", std::process::id()));
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

// ---------------------------------------------------------------------------
// 造文档 / zip 工具
// ---------------------------------------------------------------------------

/// 一份带 `n` 份几何的文档（可选带一个引用首份几何的元素）。
fn sample_doc(name: &str, n: usize) -> Document {
    let mut doc = Document::new(name);
    for i in 0..n {
        let s = ShapeId::new();
        doc.insert_geometry(GeometryRecord::brep(
            s,
            format!("MOCK-BREP v1 box {i} {i} {i} {}", i * i * i),
        ))
        .unwrap();
        if i == 0 {
            let e = Element::new(Category::new("QA"), "柱-1").with_geometry(s);
            doc.insert_element(e).unwrap();
        }
    }
    doc
}

fn zip_entries(path: &Path) -> Vec<(String, Vec<u8>)> {
    let f = fs::File::open(path).unwrap();
    let mut archive = zip::ZipArchive::new(f).unwrap();
    let mut out = Vec::new();
    for i in 0..archive.len() {
        let mut e = archive.by_index(i).unwrap();
        let name = e.name().to_string();
        let mut buf = Vec::new();
        e.read_to_end(&mut buf).unwrap();
        out.push((name, buf));
    }
    out
}

fn write_zip(path: &Path, entries: &[(String, Vec<u8>)]) {
    let f = fs::File::create(path).unwrap();
    let mut zip = zip::ZipWriter::new(f);
    let opts = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    for (name, body) in entries {
        zip.start_file(name, opts).unwrap();
        zip.write_all(body).unwrap();
    }
    zip.finish().unwrap();
}

fn mutate_entry(path: &Path, name: &str, new_body: &[u8]) {
    let mut es = zip_entries(path);
    let mut hit = false;
    for (n, b) in es.iter_mut() {
        if n == name {
            *b = new_body.to_vec();
            hit = true;
        }
    }
    assert!(hit, "探针前提：zip 里应存在成员 {name}");
    write_zip(path, &es);
}

fn drop_matching(path: &Path, pred: impl Fn(&str) -> bool) {
    let es = zip_entries(path);
    let kept: Vec<_> = es.into_iter().filter(|(n, _)| !pred(n)).collect();
    write_zip(path, &kept);
}

fn geom_names(entries: &[(String, Vec<u8>)]) -> Vec<String> {
    entries
        .iter()
        .map(|(n, _)| n.clone())
        .filter(|n| n.starts_with("geom/"))
        .collect()
}

// ===========================================================================
// C. 容器攻击
// ===========================================================================

#[test]
fn c1_empty_document_round_trips_both_forms() {
    let dir = TempDir::new("empty");
    let doc = Document::new("空");

    let zip = dir.join("empty.bimdoc");
    save_document(&zip, &doc, &Drawing::new(), &[]).unwrap();
    assert_eq!(open_document(&zip).unwrap().0, doc, "zip 形态空文档往返");

    let container = dir.join("emptydir");
    pack_to_dir(&container, &doc, &Drawing::new(), &[]).unwrap();
    assert_eq!(
        open_from_dir(&container).unwrap().0,
        doc,
        "目录形态空文档往返"
    );
}

#[test]
fn c2_three_geometry_round_trips_both_forms() {
    let dir = TempDir::new("three");
    let doc = sample_doc("三份几何", 3);

    let zip = dir.join("three.bimdoc");
    save_document(&zip, &doc, &Drawing::new(), &[]).unwrap();
    let back = open_document(&zip).unwrap().0;
    assert_eq!(back, doc);
    assert_eq!(back.geometry_count(), 3);

    let container = dir.join("threedir");
    pack_to_dir(&container, &doc, &Drawing::new(), &[]).unwrap();
    assert_eq!(open_from_dir(&container).unwrap().0.geometry_count(), 3);
}

#[test]
fn c3_zip_tampered_geom_is_rejected() {
    let dir = TempDir::new("tamperzip");
    let doc = sample_doc("篡改", 2);
    let zip = dir.join("t.bimdoc");
    save_document(&zip, &doc, &Drawing::new(), &[]).unwrap();

    let names = geom_names(&zip_entries(&zip));
    assert!(!names.is_empty());
    mutate_entry(&zip, &names[0], b"MOCK-BREP v1 box 9 9 9 729");

    let err = open_document(&zip).unwrap_err();
    assert!(
        matches!(&err, AppError::Container(msg) if msg.contains("不一致")),
        "zip 形态几何副本被篡改必须报错，实得 {err:?}"
    );
}

#[test]
fn c4_zip_missing_geom_is_rejected() {
    let dir = TempDir::new("missgeomzip");
    let doc = sample_doc("缺几何", 2);
    let zip = dir.join("m.bimdoc");
    save_document(&zip, &doc, &Drawing::new(), &[]).unwrap();

    drop_matching(&zip, |n| n.starts_with("geom/"));

    let err = open_document(&zip).unwrap_err();
    assert!(
        matches!(&err, AppError::Container(msg) if msg.contains("缺少几何副本")),
        "zip 形态缺 geom 副本必须报错，实得 {err:?}"
    );
}

#[test]
fn c5_zero_format_version_is_rejected_both_forms() {
    let dir = TempDir::new("ver0");
    let doc = sample_doc("版本", 1);
    let bad = br#"{"format_version":0,"app":"qa","created_utc":"","file_count":2}"#;

    let zip = dir.join("v.bimdoc");
    save_document(&zip, &doc, &Drawing::new(), &[]).unwrap();
    mutate_entry(&zip, "manifest.json", bad);
    let err = open_document(&zip).unwrap_err();
    assert!(
        matches!(&err, AppError::Container(msg) if msg.contains("格式版本")),
        "zip 形态 format_version=0 必须报错，实得 {err:?}"
    );

    let container = dir.join("vdir");
    pack_to_dir(&container, &doc, &Drawing::new(), &[]).unwrap();
    fs::write(container.join("manifest.json"), bad).unwrap();
    let err = open_from_dir(&container).unwrap_err();
    assert!(
        matches!(&err, AppError::Container(msg) if msg.contains("格式版本")),
        "目录形态 format_version=0 必须报错，实得 {err:?}"
    );
}

#[test]
fn c6a_non_utf8_garbage_file_is_err_not_panic() {
    let dir = TempDir::new("garbage");
    let path = dir.join("garbage.bimdoc");
    fs::write(&path, [0xFFu8, 0xFE, 0x00, 0x01, 0x80, 0x90]).unwrap();
    assert!(matches!(open_document(&path), Err(AppError::Container(_))));
}

#[test]
fn c6b_zip_with_non_utf8_docjson_is_err_not_panic() {
    let dir = TempDir::new("badutf8");
    let doc = sample_doc("坏编码", 1);
    let zip = dir.join("u.bimdoc");
    save_document(&zip, &doc, &Drawing::new(), &[]).unwrap();
    // 合法 zip，但 doc.json 是非法 UTF-8 字节
    mutate_entry(&zip, "doc.json", &[0xFFu8, 0xFE, 0xFD, 0x00, 0x80]);
    let err = open_document(&zip).unwrap_err();
    assert!(
        matches!(&err, AppError::Container(_)),
        "非法 UTF-8 的 doc.json 必须返回 Err，实得 {err:?}"
    );
}

#[test]
fn c7_directory_passed_as_zip_path_is_err() {
    let dir = TempDir::new("typeconfuse");
    let container = dir.join("adirectory");
    pack_to_dir(&container, &sample_doc("d", 1), &Drawing::new(), &[]).unwrap();
    // 类型混淆：把目录当 .bimdoc 传给 open_document
    assert!(
        matches!(open_document(&container), Err(AppError::Container(_))),
        "目录当 zip 传入必须返回 Err，不许 panic"
    );
}

#[test]
fn c8_valid_json_but_wrong_shape_docjson_is_err() {
    let dir = TempDir::new("wrongshape");
    let doc = sample_doc("结构错", 1);

    // 目录形态
    let container = dir.join("wsdir");
    pack_to_dir(&container, &doc, &Drawing::new(), &[]).unwrap();
    fs::write(container.join("doc.json"), b"{}").unwrap();
    assert!(
        matches!(open_from_dir(&container), Err(AppError::Container(_))),
        "doc.json 是 {{}} 必须报错而不是返回空文档"
    );

    // zip 形态（另一条读取路径）
    let zip = dir.join("ws.bimdoc");
    save_document(&zip, &doc, &Drawing::new(), &[]).unwrap();
    mutate_entry(&zip, "doc.json", b"{}");
    assert!(matches!(open_document(&zip), Err(AppError::Container(_))));
}

#[test]
fn c9_chinese_named_document_round_trips() {
    let dir = TempDir::new("chinese");
    let mut doc = Document::new("中文名文档-母线层");
    doc.insert_geometry(GeometryRecord::brep(
        ShapeId::new(),
        "MOCK-BREP v1 box 1 1 1 1",
    ))
    .unwrap();

    let zip = dir.join("中文.bimdoc");
    save_document(&zip, &doc, &Drawing::new(), &[]).unwrap();
    assert_eq!(open_document(&zip).unwrap().0, doc, "中文名文档 zip 往返");

    let container = dir.join("中文目录");
    pack_to_dir(&container, &doc, &Drawing::new(), &[]).unwrap();
    assert_eq!(
        open_from_dir(&container).unwrap().0,
        doc,
        "中文名文档目录往返"
    );
}

#[test]
fn c10_session_scoped_datum_must_not_be_persisted() {
    // 设计文档 4.6.3：`DatumScope::Session`（如"当前工作平面"）**不写进 .bimdoc**。
    // `Document` 提供了 `persistent_datums()` 正是为此；容器写入是否用了它是本条要查的。
    let dir = TempDir::new("sessiondatum");
    let mut doc = Document::new("会话态");
    doc.insert_datum(DatumElement::level("F1", 0.0)).unwrap();
    let mut wp = DatumElement::ref_plane("当前工作平面");
    wp.scope = DatumScope::Session;
    doc.insert_datum(wp).unwrap();

    assert_eq!(doc.persistent_datums().count(), 1, "文档级标高 1 个");
    assert_eq!(doc.session_datums().len(), 1, "会话态工作平面 1 个");

    let zip = dir.join("s.bimdoc");
    save_document(&zip, &doc, &Drawing::new(), &[]).unwrap();
    let back = open_document(&zip).unwrap().0;

    assert_eq!(
        back.datums().len(),
        doc.persistent_datums().count(),
        "会话态基准元素被写进了 .bimdoc：应为 {} 个，实得 {} 个（设计文档 4.6.3 要求剔除）",
        doc.persistent_datums().count(),
        back.datums().len()
    );
    assert_eq!(
        back.session_datums().len(),
        0,
        "打开 .bimdoc 后不应出现会话态基准元素"
    );
}

// ===========================================================================
// D. 会话门面攻击
// ===========================================================================

struct QaFactory;

impl ElementFactory for QaFactory {
    fn category(&self) -> &str {
        "QA_BOX"
    }
    fn display_name(&self) -> &str {
        "QA 盒"
    }
    fn create(&self, request: &CreateRequest) -> SdkResult<SdkElement> {
        let mut e = build_element(self.category(), request, "QA 盒");
        e.properties
            .set("dx", Value::Float(request.param_f64("dx").unwrap_or(10.0)));
        Ok(e)
    }
    fn default_params(&self) -> PropertyMap {
        PropertyMap::new("QA_BOX").with("dx", Value::Float(10.0))
    }
}

struct QaBoxOp;

impl GeometryOp for QaBoxOp {
    fn id(&self) -> &str {
        "qa.box"
    }
    fn display_name(&self) -> &str {
        "QA 方盒"
    }
    fn apply(&self, input: &OpInput) -> SdkResult<OpOutput> {
        let d = input.f64_or("dx", 10.0);
        Ok(OpOutput::of(vec![
            BuildStep::new(BuildOp::MakeBox)
                .with_f64("dx", d)
                .with_f64("dy", d)
                .with_f64("dz", d),
        ]))
    }
}

struct QaModule;

impl Module for QaModule {
    fn manifest(&self) -> ModuleManifest {
        ModuleManifest::new("qa.demo", "QA 示例", "0.1.0")
    }
    fn register(&self, registry: &mut Registry) -> SdkResult<()> {
        registry.register_module(&self.manifest())?;
        registry.register_category("QA_BOX", "QA 盒", "qa.demo")?;
        registry.register_factory(Box::new(QaFactory))?;
        registry.register_geometry_op(Box::new(QaBoxOp))?;
        Ok(())
    }
}

fn session_with_module() -> CoreSession {
    let mut s = CoreSession::new().unwrap();
    s.install_module(Box::new(QaModule)).unwrap();
    s
}

#[test]
fn d1_build_geometry_releases_all_handles() {
    let mut s = session_with_module();
    let id = s.create_element("QA_BOX", "盒").unwrap();
    s.build_geometry(id, "qa.box").unwrap();
    assert_eq!(
        s.geometry().kernel().live_count(),
        0,
        "build_geometry 后不应残留任何存活句柄"
    );
    assert!(s.geometry().is_empty(), "也不应留下缓存映射");
}

#[test]
fn d2_save_then_open_is_deeply_equal() {
    let mut s = session_with_module();
    let id = s.create_element("QA_BOX", "盒-A").unwrap();
    s.build_geometry(id, "qa.box").unwrap();

    let dir = TempDir::new("session");
    let path = dir.join("s.bimdoc");
    s.save(&path).unwrap();

    let reopened = CoreSession::open(&path).unwrap();
    assert_eq!(
        reopened.document().doc(),
        s.document().doc(),
        "save → CoreSession::open 后文档必须深度相等"
    );
}

#[test]
fn d3_unregistered_category_and_op_are_refused() {
    let mut s = session_with_module();
    assert!(
        s.create_element("NO_SUCH_CATEGORY", "x").is_err(),
        "未登记品类必须报错"
    );
    let id = s.create_element("QA_BOX", "盒").unwrap();
    assert!(
        s.build_geometry(id, "no.such.op").is_err(),
        "未登记算子必须报错"
    );
}

#[test]
fn d4_building_the_same_element_twice_is_refused() {
    let mut s = session_with_module();
    let id = s.create_element("QA_BOX", "盒").unwrap();
    s.build_geometry(id, "qa.box").unwrap();
    let second = s.build_geometry(id, "qa.box");
    assert!(
        matches!(second, Err(AppError::RecipeNotExecutable(_))),
        "同一元素重复构造几何必须被拒绝，实得 {second:?}"
    );
    // 且第二次失败不应污染内核（不留存活句柄）
    assert_eq!(s.geometry().kernel().live_count(), 0);
}

#[test]
fn d5_snapshot_of_a_released_handle_is_err() {
    let mut g = GeometryService::with_mock_kernel();
    let handle = g.kernel_mut().make_box(1.0, 1.0, 1.0).unwrap();
    g.kernel_mut().release(handle).unwrap();
    let err = g.snapshot(ShapeId::new(), handle).unwrap_err();
    assert!(
        matches!(err, AppError::Kernel(_)),
        "已释放句柄快照必须报错，实得 {err:?}"
    );
}
