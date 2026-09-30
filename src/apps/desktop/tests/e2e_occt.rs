//! M6b 端到端（occt 真腿）自动化断言：防假腿 → 装载 → 建模 → 出图 → 存盘 →
//! 重开 → rebuild → regenerate（8 环断言链，M6b-4 第 8 道门禁的载体）。
//!
//! `#![cfg(feature = "occt")]` 门控：feature 关掉时本文件不参与编译，
//! 默认腿（CI mock）零影响（QA 矩阵 #1 假腿攻击：文件不参与编译）。
//! 真腿测试很重（wasmtime 编译），固定 `--release -- --test-threads=1` 跑
//! （防并行 OOM，见环境铁律 #4）。
//!
//! 与 `e2e_mock.rs` 走同一条六层装配链路（`CoreSession` 门面 + `example-beam`
//! 模块），但内核换成真实后端——§8.5 五环（加载/建模/出图/存盘/再打开）与
//! 关联更新闭环（rebuild → regenerate）在真内核上成立，§10 M6 行
//! "加载、建模、出图、存盘" 在 occt 腿成立。

#![cfg(feature = "occt")]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::panic))]

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use core_app::CoreSession;
use core_doc::ViewOrientation;
use core_model::{PropertyMap, Value};
use example_beam::{CATEGORY, ExampleBeam, GEOMETRY_OP_ID};

// ---------------------------------------------------------------------------
// 临时文件（自管，不引入 tempfile；多测试并行用 pid + 序号区分路径）
// ---------------------------------------------------------------------------

static SEQ: AtomicU64 = AtomicU64::new(0);

fn temp_bimdoc(label: &str) -> PathBuf {
    let n = SEQ.fetch_add(1, Ordering::SeqCst);
    let mut path = std::env::temp_dir();
    path.push(format!(
        "bim-e2e-occt-{label}-{}-{n}.bimdoc",
        std::process::id()
    ));
    path
}

// ---------------------------------------------------------------------------
// 8 环断言链（环间有真实状态传递：装核 → 装载 → 建模 → 出图 → 存盘 → 重开 →
// rebuild → regenerate），收在**同一个** #[test] 内。
// 如实说明：各环**不可独立失败**——前序环断言失败后，后续环不会执行；
// 但每条断言的消息都标注环号，失败时**可归因**到具体环。
// 拆成独立 #[test] 需每环重跑全链（约 8 倍真内核耗时）或引入共享态，
// 结构取舍见 M6 规格 §7 M6b-4；如需拆环属 M7。
// ---------------------------------------------------------------------------

#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "真实内核用例在 debug 下 wasmtime 编译过慢，请用 --release 运行"
)]
fn e2e_occt_full_loop_with_associative_rebuild() {
    // ── 环 1：防假腿守卫——backend_name() == "occt-wasm" 且 is_real() ──
    // （真内核 backend_name 为 "occt-wasm"，见 GeometryService 门禁用例口径）
    let mut session =
        CoreSession::with_real_kernel().expect("环1: occt 腿下 with_real_kernel 必须成功");
    assert_eq!(
        session.geometry().backend_name(),
        "occt-wasm",
        "环1: 防假腿——必须跑真实后端 occt-wasm，实得 {}",
        session.geometry().backend_name()
    );
    assert!(
        session.geometry().backend_kind().is_real(),
        "环1: 防假腿——backend_kind 必须是 Real"
    );

    // ── 环 2：装载 ExampleBeam（api_version 校验在 install 内）──
    session
        .install_module(Box::new(ExampleBeam))
        .expect("环2: ExampleBeam 装载必须通过");
    assert!(
        session.modules().summary().geometry_ops >= 1,
        "环2: 装载后应至少登记 1 个几何算子"
    );

    // ── 环 3：建模——create_element + build_geometry → geometry_count == 1 ──
    let beam = session
        .create_element(CATEGORY, "KL-1")
        .expect("环3: create_element 失败");
    let shape = session
        .build_geometry(beam, GEOMETRY_OP_ID)
        .expect("环3: build_geometry 失败");
    assert_eq!(
        session.document().doc().geometry_count(),
        1,
        "环3: build_geometry 后应有 1 份几何记录"
    );

    // ── 环 4：出图（俯视，真实视向）→ entities 非空 ──
    let view_id = session
        .create_view("俯视", ViewOrientation::top())
        .expect("环4: create_view 失败");
    let entity_count = session
        .document()
        .drawing()
        .view(view_id)
        .unwrap()
        .entities
        .len();
    assert!(entity_count > 0, "环4: occt 投影必须产出图元（真实视向）");

    // ── 环 5：存盘 → 容器五段齐全（manifest/doc/drawing/views/geom）──
    let drawing_before = session.document().drawing().clone();
    let path = temp_bimdoc("loop");
    session.save(&path).expect("环5: save 失败");
    {
        let file = fs::File::open(&path).expect("环5: 容器打不开");
        let mut zip = zip::ZipArchive::new(file).expect("环5: zip 解不开");
        let mut names = Vec::new();
        for i in 0..zip.len() {
            names.push(zip.by_index(i).unwrap().name().to_string());
        }
        for expected in ["manifest.json", "doc.json", "drawing.json"] {
            assert!(
                names.iter().any(|n| n == expected),
                "环5: 容器应含 {expected}：{names:?}"
            );
        }
        assert!(
            names
                .iter()
                .any(|n| n.starts_with("views/") && n.ends_with(".json")),
            "环5: 容器应含 views/ 段（每视图一个文件）：{names:?}"
        );
        assert!(
            names
                .iter()
                .any(|n| n.starts_with("geom/") && n.ends_with(".brep")),
            "环5: 容器应含 geom/ 段（BREP 副本）：{names:?}"
        );
    }

    // ── 环 6：重开（真腿）→ 元素 / 品类 / 几何 / Drawing 四项断言 ──
    let reopened = CoreSession::open_real(&path).expect("环6: open_real 失败");
    fs::remove_file(&path).unwrap();
    assert_eq!(
        reopened.document().element_count(),
        1,
        "环6: 元素数应往返一致"
    );
    assert!(
        reopened
            .document()
            .doc()
            .elements()
            .values()
            .all(|e| e.category.as_str() == CATEGORY),
        "环6: 重开后全部元素品类应为 {CATEGORY}"
    );
    assert_eq!(
        reopened.document().doc().geometry_count(),
        1,
        "环6: 几何记录数应往返一致"
    );
    assert_eq!(
        *reopened.document().drawing(),
        drawing_before,
        "环6: Drawing 应深相等（视图与图元随容器恢复）"
    );

    // ── 环 7：rebuild 改参数（截面宽 300 → 500）→ UpdateReport 合法 + stale 标记 ──
    let mut rebuilt = reopened;
    // 模块不随容器保存：重开后需重新装载才能取算子（模块是启动期动作）
    rebuilt
        .install_module(Box::new(ExampleBeam))
        .expect("环7: 重装 ExampleBeam 失败");
    let params = PropertyMap::new(CATEGORY)
        .with("width", Value::Float(500.0))
        .with("height", Value::Float(600.0))
        .with("length", Value::Float(6000.0))
        .with("material", Value::Text("C30".to_string()));
    let report = rebuilt
        .rebuild_geometry(beam, GEOMETRY_OP_ID, params)
        .expect("环7: rebuild_geometry 失败");
    assert_eq!(
        report.changed_shapes,
        vec![shape],
        "环7: changed_shapes 应回显该形状（复用原 ShapeId，构件身份不变）"
    );
    assert_eq!(report.stale_views, 1, "环7: 绑定该形状的视图必须被标脏");
    assert!(report.reprojected_entities > 0, "环7: 定向重投必须有产出");
    assert!(
        !rebuilt
            .document()
            .drawing()
            .view(view_id)
            .unwrap()
            .up_to_date,
        "环7: rebuild 后视图应 up_to_date=false，等待 regenerate"
    );

    // ── 环 8：regenerate → up_to_date 翻回 true ──
    let standard = rebuilt.document().drawing().standard.clone();
    let (written, _warnings) = rebuilt
        .regenerate_view(view_id, &standard)
        .expect("环8: regenerate_view 失败");
    assert!(written > 0, "环8: 重投应写回图元，实得 {written}");
    assert!(
        rebuilt
            .document()
            .drawing()
            .view(view_id)
            .unwrap()
            .up_to_date,
        "环8: regenerate 后视图应翻回 up_to_date=true"
    );
}
