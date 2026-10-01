//! M6b 端到端（occt 真腿）自动化断言：防假腿 → 装载 → 建模 → 出图 → 存盘 →
//! 重开 → rebuild → regenerate（**8 环各一条独立 `#[test]`**，M7 遗留拆环收口）。
//!
//! ## 拆环取舍（M7 收口裁定，替代原"单链收在一个 #[test]"的结构取舍）
//!
//! 原单链（M6b-4 第 8 道门禁载体）注释明确"拆成独立 `#[test]` 需每环重跑全链
//! （约 8 倍真内核耗时）或引入共享态"。M7c 已立先例（`e2e_power.rs` 每算子
//! 独立 test，"天然拆环、失败可归因"），本文件对齐该先例：
//! - **每环重跑自己需要的前缀**——装配链分层收进 helper（`real_session` /
//!   `modeled_session` / `viewed_session` / `reopened_with_view`），断言体保持
//!   与原单链逐字同语义；
//! - **代价**：门禁 occt 腿多 7 次 occt-wasm 加载（约 +1~1.5 分钟），
//!   换失败精确归因 + 单环重跑能力；
//! - **共享态方案不取**：`CoreSession` 非 `Sync`，环间顺序依赖（存盘→重开→
//!   rebuild→regenerate）与测试执行顺序耦合，脆弱。
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
use core_doc::{Drawing, ViewOrientation};
use core_model::{ElementId, PropertyMap, ShapeId, Value, ViewId};
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
// 装配 helper（分层复用：每环重跑自己需要的前缀，断言体不重复）
// ---------------------------------------------------------------------------

/// 装配真实内核会话（环 1/2 起点）。
fn real_session() -> CoreSession {
    CoreSession::with_real_kernel().expect("occt 腿下 with_real_kernel 必须成功")
}

/// 装配 + 装载 ExampleBeam + 建模（环 3/4/5 起点）。
/// 返回 (会话, beam 元素 id, shape)。
fn modeled_session() -> (CoreSession, ElementId, ShapeId) {
    let mut session = real_session();
    // 装载（api_version 校验在 install 内）
    session
        .install_module(Box::new(ExampleBeam))
        .expect("ExampleBeam 装载必须通过");
    // 建模
    let beam = session
        .create_element(CATEGORY, "KL-1")
        .expect("create_element 失败");
    let shape = session
        .build_geometry(beam, GEOMETRY_OP_ID)
        .expect("build_geometry 失败");
    (session, beam, shape)
}

/// 装配 + 建模 + 出图（俯视，真实视向）（环 5/6/7/8 前缀起点）。
/// 返回 (会话, view_id, beam 元素 id, shape)。
fn viewed_session() -> (CoreSession, ViewId, ElementId, ShapeId) {
    let (mut session, beam, shape) = modeled_session();
    let view_id = session
        .create_view("俯视", ViewOrientation::top())
        .expect("create_view 失败");
    (session, view_id, beam, shape)
}

/// 装配 + 建模 + 出图 + 存盘 + 真腿重开 + 重装模块（环 6/7/8 共同前缀）。
/// 返回 (重开会话, view_id, beam 元素 id, shape, 存盘前 Drawing 快照)。
/// 临时容器在重开后即删（每环自管）。
fn reopened_with_view(label: &str) -> (CoreSession, ViewId, ElementId, ShapeId, Drawing) {
    let (mut session, beam, shape) = modeled_session();
    let view_id = session
        .create_view("俯视", ViewOrientation::top())
        .expect("create_view 失败");
    let drawing_before = session.document().drawing().clone();
    let path = temp_bimdoc(label);
    session.save(&path).expect("save 失败");
    let mut reopened = CoreSession::open_real(&path).expect("open_real 失败");
    fs::remove_file(&path).unwrap();
    // 模块不随容器保存：重开后需重新装载才能取算子（模块是启动期动作）
    reopened
        .install_module(Box::new(ExampleBeam))
        .expect("重装 ExampleBeam 失败");
    (reopened, view_id, beam, shape, drawing_before)
}

/// rebuild 用的新参数（环 7/8 共用）：截面宽 300 → 500。
fn rebuild_params() -> PropertyMap {
    PropertyMap::new(CATEGORY)
        .with("width", Value::Float(500.0))
        .with("height", Value::Float(600.0))
        .with("length", Value::Float(6000.0))
        .with("material", Value::Text("C30".to_string()))
}

// ---------------------------------------------------------------------------
// 环 1：防假腿守卫——backend_name() == "occt-wasm" 且 is_real()
// ---------------------------------------------------------------------------

#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "真实内核用例在 debug 下 wasmtime 编译过慢，请用 --release 运行"
)]
fn e2e_ring1_real_backend_guard() {
    let session = real_session();
    // 真内核 backend_name 为 "occt-wasm"，见 GeometryService 门禁用例口径
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
}

// ---------------------------------------------------------------------------
// 环 2：装载 ExampleBeam（api_version 校验在 install 内）→ 算子已登记
// ---------------------------------------------------------------------------

#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "真实内核用例在 debug 下 wasmtime 编译过慢，请用 --release 运行"
)]
fn e2e_ring2_module_install_registers_ops() {
    let mut session = real_session();
    session
        .install_module(Box::new(ExampleBeam))
        .expect("环2: ExampleBeam 装载必须通过");
    assert!(
        session.modules().summary().geometry_ops >= 1,
        "环2: 装载后应至少登记 1 个几何算子"
    );
}

// ---------------------------------------------------------------------------
// 环 3：建模——create_element + build_geometry → geometry_count == 1
// ---------------------------------------------------------------------------

#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "真实内核用例在 debug 下 wasmtime 编译过慢，请用 --release 运行"
)]
fn e2e_ring3_modeling_creates_geometry() {
    let (session, _beam, _shape) = modeled_session();
    assert_eq!(
        session.document().doc().geometry_count(),
        1,
        "环3: build_geometry 后应有 1 份几何记录"
    );
}

// ---------------------------------------------------------------------------
// 环 4：出图（俯视，真实视向）→ entities 非空
// ---------------------------------------------------------------------------

#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "真实内核用例在 debug 下 wasmtime 编译过慢，请用 --release 运行"
)]
fn e2e_ring4_view_projects_entities() {
    let (session, view_id, _beam, _shape) = viewed_session();
    let entity_count = session
        .document()
        .drawing()
        .view(view_id)
        .unwrap()
        .entities
        .len();
    assert!(entity_count > 0, "环4: occt 投影必须产出图元（真实视向）");
}

// ---------------------------------------------------------------------------
// 环 5：存盘 → 容器五段齐全（manifest/doc/drawing/views/geom）
// ---------------------------------------------------------------------------

#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "真实内核用例在 debug 下 wasmtime 编译过慢，请用 --release 运行"
)]
fn e2e_ring5_save_container_sections() {
    let (session, _view_id, _beam, _shape) = viewed_session();
    let path = temp_bimdoc("save");
    session.save(&path).expect("环5: save 失败");

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

    fs::remove_file(&path).unwrap();
}

// ---------------------------------------------------------------------------
// 环 6：重开（真腿）→ 元素 / 品类 / 几何 / Drawing 四项断言
// ---------------------------------------------------------------------------

#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "真实内核用例在 debug 下 wasmtime 编译过慢，请用 --release 运行"
)]
fn e2e_ring6_reopen_roundtrip_preserves_everything() {
    let (reopened, _view_id, _beam, _shape, drawing_before) = reopened_with_view("reopen");
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
}

// ---------------------------------------------------------------------------
// 环 7：rebuild 改参数（截面宽 300 → 500）→ UpdateReport 合法 + stale 标记
// ---------------------------------------------------------------------------

#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "真实内核用例在 debug 下 wasmtime 编译过慢，请用 --release 运行"
)]
fn e2e_ring7_rebuild_reports_and_marks_stale() {
    let (mut rebuilt, view_id, beam, shape, _drawing_before) = reopened_with_view("rebuild");
    let report = rebuilt
        .rebuild_geometry(beam, GEOMETRY_OP_ID, rebuild_params())
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
}

// ---------------------------------------------------------------------------
// 环 8：regenerate → up_to_date 翻回 true
// ---------------------------------------------------------------------------

#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "真实内核用例在 debug 下 wasmtime 编译过慢，请用 --release 运行"
)]
fn e2e_ring8_regenerate_refreshes_view() {
    // rebuild 后视图标脏，regenerate 才有意义——前缀含 rebuild（环 7 的动作）。
    let (mut rebuilt, view_id, beam, _shape, _drawing_before) = reopened_with_view("regen");
    let _report = rebuilt
        .rebuild_geometry(beam, GEOMETRY_OP_ID, rebuild_params())
        .expect("环8: rebuild_geometry 失败");

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
