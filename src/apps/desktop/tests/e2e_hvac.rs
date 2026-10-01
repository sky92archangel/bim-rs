//! M7 端到端（mock 腿，建筑侧暖通 `hvac-duct`）：加载 → 建模 → 出图 → 存盘 →
//! 再打开 五环断言。
//!
//! 与 `e2e_mock.rs`（example-beam）同一条六层装配链路，但模块换成建筑侧的
//! `hvac-duct`——M7 验收「电力侧与建筑侧各至少一个可用模块」的**默认腿载体**
//! （CI mock 可跑，不需要 occt feature）。矩形风管段只用 `MakeBox`/`Translate`，
//! 占位内核即支持，因此五环能在 mock 腿全通。
//!
//! `Loft`（变径管）为真实内核专属（占位后端响亮拒绝 `Unsupported`），
//! 其真实 execute 断言在 `e2e_hvac_occt.rs`（`--features occt --release`）。

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::panic))]

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use core_app::CoreSession;
use core_doc::ViewOrientation;
use hvac_duct::{
    CATEGORY, DEFAULT_HEIGHT_MM, DEFAULT_LENGTH_MM, DEFAULT_WIDTH_MM, GEOMETRY_OP_ID_RECT, HvacDuct,
};

// ---------------------------------------------------------------------------
// 临时文件（自管，不引入 tempfile；多测试并行用 pid + 序号区分路径）
// ---------------------------------------------------------------------------

static SEQ: AtomicU64 = AtomicU64::new(0);

fn temp_bimdoc(label: &str) -> PathBuf {
    let n = SEQ.fetch_add(1, Ordering::SeqCst);
    let mut path = std::env::temp_dir();
    path.push(format!(
        "bim-e2e-hvac-{label}-{}-{n}.bimdoc",
        std::process::id()
    ));
    path
}

// ---------------------------------------------------------------------------
// 五环断言（加载 → 建模 → 出图 → 存盘 → 再打开，照设计文档 §8.5）
// ---------------------------------------------------------------------------

#[test]
fn hvac_module_passes_five_loop_minimal_check() {
    // ── 环 1：加载（mock 腿：CoreSession::new 即占位内核）──
    let mut session = CoreSession::new().expect("环1: CoreSession::new 失败");
    session
        .install_module(Box::new(HvacDuct))
        .expect("环1: hvac-duct 装载必须通过");
    assert!(
        session
            .modules()
            .registry()
            .geometry_op_ids()
            .contains(&GEOMETRY_OP_ID_RECT),
        "环1: 装载后注册表里应有 {GEOMETRY_OP_ID_RECT}"
    );

    // ── 环 2：建模（create_element + build_geometry）──
    let duct = session
        .create_element(CATEGORY, "FG-1")
        .expect("环2: create_element 失败");
    let _shape = session
        .build_geometry(duct, GEOMETRY_OP_ID_RECT)
        .expect("环2: build_geometry 失败");
    assert_eq!(
        session.document().doc().geometry_count(),
        1,
        "环2: build_geometry 后应有 1 份几何记录"
    );

    // ── 环 3：出图（mock 理想化投影）→ entities 非空 ──
    let view_id = session
        .create_view("俯视", ViewOrientation::top())
        .expect("环3: create_view 失败");
    let entity_count = session
        .document()
        .drawing()
        .view(view_id)
        .unwrap()
        .entities
        .len();
    assert!(entity_count > 0, "环3: mock 投影必须产出图元");

    // ── 环 4：存盘 → 容器五段齐全 ──
    let path = temp_bimdoc("five");
    session.save(&path).expect("环4: save 失败");
    {
        let file = fs::File::open(&path).expect("环4: 容器打不开");
        let mut zip = zip::ZipArchive::new(file).expect("环4: zip 解不开");
        let mut names = Vec::new();
        for i in 0..zip.len() {
            names.push(zip.by_index(i).unwrap().name().to_string());
        }
        for expected in ["manifest.json", "doc.json", "drawing.json"] {
            assert!(
                names.iter().any(|n| n == expected),
                "环4: 容器应含 {expected}：{names:?}"
            );
        }
        assert!(
            names
                .iter()
                .any(|n| n.starts_with("views/") && n.ends_with(".json")),
            "环4: 容器应含 views/ 段：{names:?}"
        );
        assert!(
            names
                .iter()
                .any(|n| n.starts_with("geom/") && n.ends_with(".brep")),
            "环4: 容器应含 geom/ 段：{names:?}"
        );
    }

    // ── 环 5：再打开 → 元素 / 品类 / 几何 / Drawing 四项断言 ──
    let drawing_before = {
        // save 之后会话仍持有 Drawing；重新取一次用于比对。
        session.document().drawing().clone()
    };
    let reopened = CoreSession::open(&path).expect("环5: open 失败");
    fs::remove_file(&path).unwrap();
    assert_eq!(
        reopened.document().element_count(),
        1,
        "环5: 元素数应往返一致"
    );
    assert!(
        reopened
            .document()
            .doc()
            .elements()
            .values()
            .all(|e| e.category.as_str() == CATEGORY),
        "环5: 重开后全部元素品类应为 {CATEGORY}"
    );
    // 风管属性应随元素往返（专业字段 Core 不认识，但必须原样存取）。
    let duct_elem = reopened
        .document()
        .doc()
        .elements()
        .values()
        .next()
        .expect("环5: 重开后应有元素");
    assert_eq!(
        duct_elem.properties.get_float("width"),
        Some(DEFAULT_WIDTH_MM),
        "环5: 风管宽属性应往返一致"
    );
    assert_eq!(
        duct_elem.properties.get_float("height"),
        Some(DEFAULT_HEIGHT_MM),
        "环5: 风管高属性应往返一致"
    );
    assert_eq!(
        duct_elem.properties.get_float("length"),
        Some(DEFAULT_LENGTH_MM),
        "环5: 延米长度属性应往返一致"
    );
    assert_eq!(
        reopened.document().doc().geometry_count(),
        1,
        "环5: 几何记录数应往返一致"
    );
    assert_eq!(
        *reopened.document().drawing(),
        drawing_before,
        "环5: Drawing 应深相等（视图与图元随容器恢复）"
    );
}
