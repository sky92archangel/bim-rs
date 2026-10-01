//! M7 端到端（occt 真腿，建筑侧暖通 `hvac-duct`）：Loft 变径管真实 execute +
//! 棱台体积精确 oracle + 五环（照设计文档 §8.5）。
//!
//! `#![cfg(feature = "occt")]` 门控：feature 关掉时本文件不参与编译，
//! 默认腿（CI mock）零影响（QA 矩阵 #2 假腿攻击：文件不参与编译）。
//! 真腿测试很重（wasmtime JIT 编译），固定 `--release -- --test-threads=1` 跑
//! （防并行 OOM，见环境铁律 #4）。
//!
//! ## 为什么变径管是 SDK 的又一体检
//!
//! `Loft` 是变长放样算子（M5-B6.2 接线、M7a 截面 wire 齐备），此前**没有任何
//! 模块用过它**——电力侧两模块覆盖了 `MakeBox`/`MakeCylinder`/`Cut`/`Translate`/
//! `Pipe`/`MakeLineEdge`/`MakeWire`。第三个专业模块（建筑侧暖通）**零 SDK 改动**
//! 挂接并走通真实内核全链，即「接口若贴合单一专业，二阶段无法低成本扩展」
//! （立项 P0 风险）的持续证伪。
//!
//! ## 体积 oracle 为什么是解析精确值
//!
//! 起端 400×400、末端 800×800、延米 1000，两个**相似**矩形截面绕同一中心、
//! `ruled=true` 直纹放样 → 精确**棱台**（4 个梯形侧面 + 2 个方形端面）：
//!
//! ```text
//! V = h/3 × (S1 + S2 + √(S1·S2)) = 1000/3 × (160000 + 640000 + 320000)
//!   = 373 333 333.33 mm³
//! ```
//!
//! bbox 同样解析精确：max(宽) × max(高) × 延米 = 800 × 800 × 1000。

#![cfg(feature = "occt")]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::panic))]

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use core_app::{CoreSession, GeometryService};
use core_doc::ViewOrientation;
use core_kernel::SubShapeKind;
use core_model::{PropertyMap, Value};
use core_sdk::{GeometryOp, OpInput};
use hvac_duct::{
    CATEGORY, DEFAULT_HEIGHT_MM, DEFAULT_LENGTH_MM, DEFAULT_WIDTH_MM, DuctTransitionOp,
    GEOMETRY_OP_ID_RECT, HvacDuct,
};

// ---------------------------------------------------------------------------
// 临时文件（自管，不引入 tempfile；多测试并行用 pid + 序号区分路径）
// ---------------------------------------------------------------------------

static SEQ: AtomicU64 = AtomicU64::new(0);

fn temp_bimdoc(label: &str) -> PathBuf {
    let n = SEQ.fetch_add(1, Ordering::SeqCst);
    let mut path = std::env::temp_dir();
    path.push(format!(
        "bim-e2e-hvac-occt-{label}-{}-{n}.bimdoc",
        std::process::id()
    ));
    path
}

/// 变径管配方（起端 400×400、末端 800×800、延米 1000、anchor 原点）。
///
/// 两个相似正方形截面绕同一中心、直纹放样 → **精确棱台**。
fn square_frustum_recipe() -> core_sdk::OpOutput {
    let input = OpInput::new(
        PropertyMap::new(CATEGORY)
            .with("start_width", Value::Float(400.0))
            .with("start_height", Value::Float(400.0))
            .with("end_width", Value::Float(800.0))
            .with("end_height", Value::Float(800.0))
            .with("length", Value::Float(1000.0)),
    )
    .at([0.0, 0.0, 0.0]);
    DuctTransitionOp.apply(&input).expect("变径管配方应可生成")
}

// ---------------------------------------------------------------------------
// 钉 1：变径管 Loft 真实 execute → 精确棱台（体积 + bbox 双 oracle）
// ---------------------------------------------------------------------------

#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "真实内核用例在 debug 下 wasmtime 编译过慢，请用 --release 运行"
)]
fn duct_transition_loft_executes_with_exact_frustum_volume() {
    // ── 环 A：真实内核 execute 全链（8 线边 → 2 wire → Loft）──
    let mut g = GeometryService::new().expect("真实内核应可构造");
    assert_eq!(
        g.backend_name(),
        "occt-wasm",
        "防假腿——必须跑真实后端 occt-wasm"
    );
    let recipe = square_frustum_recipe();
    assert_eq!(recipe.step_count(), 11, "配方应为 11 步");
    let executed = g.execute(&recipe).expect("变径管 → Loft 全链应走通");
    assert_eq!(executed.step_count(), 11, "真实 execute 应执行全部 11 步");

    let final_handle = executed.final_handle().expect("应有最终结果句柄");

    // ── 环 B：Loft(is_solid=true) 产物应为**实体**（Solid==1）──
    let solid = g
        .kernel_mut()
        .sub_shape_count(final_handle, SubShapeKind::Solid)
        .expect("Solid 计数应可测");
    assert_eq!(solid, 1, "Loft(is_solid=true) 产物应为单实体，实得 {solid}");

    // ── 环 C：体积 oracle（±0.5%，棱台解析式）──
    // V = h/3 × (S1 + S2 + √(S1·S2))；a=400、b=800、h=1000。
    let v = g.kernel_mut().volume(final_handle).expect("volume 应可测");
    let (a, b, h) = (400.0_f64, 800.0_f64, 1000.0_f64);
    let expected = h / 3.0 * (a * a + b * b + a * b);
    assert!(
        (v - expected).abs() < 0.005 * expected,
        "体积 oracle：棱台 = {expected:.1} mm³，实得 {v:.1} mm³（±0.5%）"
    );

    // ── 环 D：bbox oracle（解析精确）：800 × 800 × 1000 ──
    let bb = g.kernel_mut().bounding_box(final_handle).expect("bbox");
    let span = [
        bb.max[0] - bb.min[0],
        bb.max[1] - bb.min[1],
        bb.max[2] - bb.min[2],
    ];
    assert!(
        (span[0] - b).abs() < 1e-6,
        "bbox X 跨度应为 {b}（末端截面宽），实得 {}",
        span[0]
    );
    assert!(
        (span[1] - b).abs() < 1e-6,
        "bbox Y 跨度应为 {b}（末端截面高），实得 {}",
        span[1]
    );
    assert!(
        (span[2] - h).abs() < 1e-6,
        "bbox Z 跨度应为 {h}（延米），实得 {}",
        span[2]
    );

    // ── 环 E：落盘文本真变了（最终 ≠ 任一截面 wire）且为 OCCT 标准文本 ──
    let handles = executed.handles();
    let start_wire_text = g
        .kernel_mut()
        .export_brep(handles[4])
        .expect("起端 wire 应可导出");
    let final_text = g
        .kernel_mut()
        .export_brep(final_handle)
        .expect("Loft 产物应可导出");
    assert!(
        final_text.contains("CASCADE"),
        "Loft 产物应为真实几何（OCCT 标准文本）"
    );
    assert_ne!(final_text, start_wire_text, "落盘文本必须区别于起端 wire");

    g.kernel_mut().release(final_handle).ok();
}

// ---------------------------------------------------------------------------
// 钉 2：五环（加载 → 建模 → 出图〔真实视向〕→ 存盘 → 再打开，照设计文档 §8.5）
// ── 建筑侧模块在真内核上的端到端载体（电力侧对应物在 e2e_power.rs 钉 5）。
// ---------------------------------------------------------------------------

#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "真实内核用例在 debug 下 wasmtime 编译过慢，请用 --release 运行"
)]
fn hvac_module_passes_five_loop_minimal_check_real() {
    // ── 环 1：加载（防假腿 + install_module）──
    let mut session =
        CoreSession::with_real_kernel().expect("环1: occt 腿下 with_real_kernel 必须成功");
    assert_eq!(
        session.geometry().backend_name(),
        "occt-wasm",
        "环1: 防假腿——必须跑真实后端"
    );
    session
        .install_module(Box::new(HvacDuct))
        .expect("环1: hvac-duct 装载必须通过");

    // ── 环 2：建模（create_element + build_geometry）──
    let duct = session
        .create_element(CATEGORY, "FG-2")
        .expect("环2: create_element 失败");
    session
        .build_geometry(duct, GEOMETRY_OP_ID_RECT)
        .expect("环2: build_geometry 失败");
    assert_eq!(
        session.document().doc().geometry_count(),
        1,
        "环2: build_geometry 后应有 1 份几何记录"
    );

    // ── 环 3：出图（俯视真实视向）→ entities 非空 ──
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
    assert!(entity_count > 0, "环3: occt 投影必须产出图元");

    // ── 环 4：存盘 → 容器五段齐全 ──
    let drawing_before = session.document().drawing().clone();
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

    // ── 环 5：再打开 → 元素 / 品类 / 属性 / 几何 / Drawing 断言 ──
    // （Drawing 整体不用位级 assert_eq!：投影图元坐标可能含浮点噪声
    // （先例：e2e_power 环 5 的 -1.25e-14 圆心经 serde_json 往返漂移 1 ULP）。
    // 规整字段深相等 + 图元级结构相等 + 坐标数值容差（1e-9 mm）。）
    let reopened = CoreSession::open_real(&path).expect("环5: open_real 失败");
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

    let reopened_drawing = reopened.document().drawing();
    assert_eq!(
        reopened_drawing.standard, drawing_before.standard,
        "环5: 制图标准应往返一致"
    );
    assert_eq!(
        reopened_drawing.sheets, drawing_before.sheets,
        "环5: 图纸应往返一致"
    );
    assert_eq!(
        reopened_drawing.dependencies, drawing_before.dependencies,
        "环5: 依赖图应往返一致"
    );
    assert_eq!(
        reopened_drawing.views.len(),
        drawing_before.views.len(),
        "环5: 视图数应往返一致"
    );
    for (vid, view) in &drawing_before.views {
        let reopened_view = reopened_drawing
            .views
            .get(vid)
            .unwrap_or_else(|| panic!("环5: 视图 {vid:?} 应随容器恢复"));
        assert_eq!(reopened_view.name, view.name, "环5: 视图名应一致");
        assert_eq!(reopened_view.kind, view.kind, "环5: 视图种类应一致");
        assert_eq!(
            reopened_view.scale_denominator, view.scale_denominator,
            "环5: 视图比例应一致"
        );
        assert_eq!(
            reopened_view.entities.len(),
            view.entities.len(),
            "环5: 图元数应往返一致"
        );
        for (before, after) in view.entities.iter().zip(&reopened_view.entities) {
            assert_eq!(before.id, after.id, "环5: 图元 id 应一致");
            assert_eq!(before.kind, after.kind, "环5: 图元种类应一致");
            assert_eq!(before.layer, after.layer, "环5: 图元图层应一致");
            assert_eq!(before.source, after.source, "环5: 图元几何来源应一致");
            assert_eq!(
                before.points.len(),
                after.points.len(),
                "环5: 图元点数应一致"
            );
            for (pb, pa) in before.points.iter().zip(&after.points) {
                assert!(
                    (pb.x - pa.x).abs() < 1e-9 && (pb.y - pa.y).abs() < 1e-9,
                    "环5: 图元坐标应数值一致（容差 1e-9 mm）：{pb:?} vs {pa:?}"
                );
            }
        }
    }
}
