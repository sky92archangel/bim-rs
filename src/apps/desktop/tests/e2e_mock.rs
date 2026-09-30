//! M6a 端到端（mock 腿）自动化断言：加载 → 建模 → 出图 → 存盘 → 再打开。
//!
//! 门禁自动跑的真正载体（`cargo test -p bim-desktop`），`assert!` 形态而非打印。
//! 与 `src/main.rs` 走同一条六层装配链路（`CoreSession` 门面 + `example-beam`
//! 模块），用断言钉住五环的每一环——出图环（§8.5 环 3）是 M6a 补上的缺口。
//!
//! 批内不宣称：mock 投影忽略视向（理想化正面视，mock 内核语义）——视向真实性
//! 与几何精度断言留 occt 腿（M6b）。

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::panic))]

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use core_app::CoreSession;
use core_doc::ViewOrientation;
use core_model::ShapeId;
use example_beam::{CATEGORY, ExampleBeam, GEOMETRY_OP_ID};

// ---------------------------------------------------------------------------
// 临时文件（自管，不引入 tempfile；多测试并行用 pid + 序号区分路径）
// ---------------------------------------------------------------------------

static SEQ: AtomicU64 = AtomicU64::new(0);

fn temp_bimdoc(label: &str) -> PathBuf {
    let n = SEQ.fetch_add(1, Ordering::SeqCst);
    let mut path = std::env::temp_dir();
    path.push(format!(
        "bim-e2e-mock-{label}-{}-{n}.bimdoc",
        std::process::id()
    ));
    path
}

// ---------------------------------------------------------------------------
// helper：装载模块 + 建模完成的会话
// ---------------------------------------------------------------------------

fn modeled_session() -> (CoreSession, ShapeId) {
    let mut session = CoreSession::new().unwrap();
    session.install_module(Box::new(ExampleBeam)).unwrap();
    let beam = session.create_element(CATEGORY, "").unwrap();
    let shape = session.build_geometry(beam, GEOMETRY_OP_ID).unwrap();
    (session, shape)
}

// ---------------------------------------------------------------------------
// 环 1：加载——install_module 成功 + 算子已登记
// ---------------------------------------------------------------------------

#[test]
fn e2e_load_module_registers_geometry_ops() {
    let mut session = CoreSession::new().unwrap();
    // install_module Ok 即 api_version 校验通过（core-sdk 在装载时拒绝不认识的版本）。
    session.install_module(Box::new(ExampleBeam)).unwrap();
    assert!(
        session.modules().summary().geometry_ops >= 1,
        "装载 ExampleBeam 后应至少登记 1 个几何算子"
    );
}

// ---------------------------------------------------------------------------
// 环 2：建模——create_element + build_geometry 返回有效 ShapeId
// ---------------------------------------------------------------------------

#[test]
fn e2e_model_creates_shape() {
    let (session, shape) = modeled_session();
    assert!(
        session.document().doc().geometry_count() == 1,
        "build_geometry 后应有 1 份几何记录"
    );
    assert!(
        session.document().doc().geometry().contains_key(&shape),
        "build_geometry 返回的 ShapeId 应对应一份几何记录"
    );
}

// ---------------------------------------------------------------------------
// 环 3：出图——create_view 产生非空图元 + warnings 可观测（M6a 补的缺口环）
// ---------------------------------------------------------------------------

#[test]
fn e2e_drafting_view_yields_entities() {
    let (mut session, shape) = modeled_session();
    let view_id = session.create_view("俯视", ViewOrientation::top()).unwrap();
    let (view, standard) = {
        let drawing = session.document().drawing();
        (
            drawing.view(view_id).cloned().unwrap(),
            drawing.standard.clone(),
        )
    };

    // 初生成：图元非空 + up_to_date 已置位（reproject_into 内部承诺）
    assert!(!view.entities.is_empty(), "create_view 应产生非空图元");
    assert!(view.up_to_date, "初生成后视图应处于 up_to_date");

    // warnings 可观测（修 P1-2 的延续）：单形状诊断把 warnings 原样上抛，
    // 调用方可读取——mock 理想化投影对 Box 无警告，空 Vec 亦如实返回。
    let (diag_entities, diag_warnings) = session.project_shape(shape, &view, &standard).unwrap();
    assert!(!diag_entities.is_empty(), "单形状诊断应产出图元");
    assert!(
        diag_warnings.is_empty(),
        "mock 理想化投影不应产生警告：{diag_warnings:?}"
    );
}

// ---------------------------------------------------------------------------
// 环 4：存盘——容器含 drawing.json + views/ 段
// ---------------------------------------------------------------------------

#[test]
fn e2e_save_embeds_drawing_container() {
    let (mut session, _shape) = modeled_session();
    session.create_view("俯视", ViewOrientation::top()).unwrap();

    let path = temp_bimdoc("save");
    session.save(&path).unwrap();

    let file = fs::File::open(&path).unwrap();
    let mut zip = zip::ZipArchive::new(file).unwrap();
    let mut names = Vec::new();
    for i in 0..zip.len() {
        names.push(zip.by_index(i).unwrap().name().to_string());
    }
    assert!(
        names.iter().any(|n| n == "drawing.json"),
        "容器应含 drawing.json：{names:?}"
    );
    assert!(
        names
            .iter()
            .any(|n| n.starts_with("views/") && n.ends_with(".json")),
        "容器应含 views/ 段（每视图一个文件）：{names:?}"
    );
    assert!(
        names.iter().any(|n| n == "doc.json"),
        "容器应含权威副本 doc.json：{names:?}"
    );

    fs::remove_file(&path).unwrap();
}

// ---------------------------------------------------------------------------
// 环 5：再打开——元素 / 品类 / 几何 / Drawing 四项断言
// ---------------------------------------------------------------------------

#[test]
fn e2e_reopen_preserves_everything() {
    let (mut session, _shape) = modeled_session();
    session.create_view("俯视", ViewOrientation::top()).unwrap();
    let drawing_before = session.document().drawing().clone();
    let elements_before = session.document().element_count();
    let geometry_before = session.document().doc().geometry_count();

    let path = temp_bimdoc("reopen");
    session.save(&path).unwrap();
    let reopened = CoreSession::open(&path).unwrap();
    fs::remove_file(&path).unwrap();

    // 元素数往返一致
    assert_eq!(
        reopened.document().element_count(),
        elements_before,
        "元素数应往返一致"
    );
    // 品类一致
    assert!(
        reopened
            .document()
            .doc()
            .elements()
            .values()
            .all(|e| e.category.as_str() == CATEGORY),
        "重开后全部元素品类应为 {CATEGORY}"
    );
    // 几何记录数往返一致
    assert_eq!(
        reopened.document().doc().geometry_count(),
        geometry_before,
        "几何记录数应往返一致"
    );
    // Drawing 深相等（standard / sheets / views / dependencies 全比）
    let drawing_after = reopened.document().drawing();
    assert_eq!(drawing_after.view_count(), 1, "重开后应有 1 个视图");
    assert_eq!(*drawing_after, drawing_before, "Drawing 往返应保持深相等");
}
