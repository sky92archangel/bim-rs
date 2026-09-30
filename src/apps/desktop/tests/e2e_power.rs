//! M7c 端到端（电力侧标杆 `cable-tunnel`）：防假腿 → 装载 → 建模 → 真实
//! execute → 落盘断言。
//!
//! `#![cfg(feature = "occt")]` 门控：feature 关掉时本文件不参与编译，
//! 默认腿（CI mock）零影响（QA 矩阵 #2 假腿攻击：文件不参与编译）。
//! 真腿测试很重（wasmtime JIT 编译），固定 `--release -- --test-threads=1` 跑
//! （防并行 OOM，见环境铁律 #4）。
//!
//! **每算子独立 `#[test]`**——不沿用 `e2e_occt.rs` 的单环链结构（M7c 规格
//! 遗留取舍 #2：模块级 e2e 每算子独立 test，天然拆环、失败可归因）。
//! 每条钉子的判据照 M7c-3 钉子表：
//!
//! | 钉子 | 判据 |
//! |---|---|
//! | `tunnel_rect_recipe_is_well_ordered` | apply → 配方合法 + Cut 步元数 2 + warnings |
//! | `tunnel_ductbank_recipe_uses_only_recipe_expressible_operands` | 配方每步操作数均可被 Recipe 表达（「可用」尺子静态侧） |
//! | `tunnel_rect_executes_and_persists` | CoreSession + install_module → build_geometry → 落盘含 `geom/<id>.brep` 且含 "CASCADE" + 体积 oracle ±0.5% + warnings 非空可观测 |
//! | `tunnel_ductbank_executes_pipe_end_to_end` | ductbank → Pipe 全链 → 落盘文本与 profile/spine 均不同 + bbox oracle + 种类计数 |
//! | `tunnel_module_passes_five_loop_minimal_check` | 加载 → 建模 → 出图 → 存盘 → 再打开 五环等效断言 |
//!
//! ⚠ **ductbank 落盘断言不用体积**（规格 §6 原判据「π r² L 断面近似」与
//! M7a 既有实测冲突——`pipe` 产物为非实体（`sub_shape_count(Solid)==0`，
//! occt.rs:3219 既有实测），`volume()` 对非实体不可靠。改用 **bbox oracle**
//! （断面尺寸 × 脊线长）+ **种类计数**（先探针实测再写死，M7b P3-② 口径）
//! + **落盘文本变化**。裁定说明见 M7c 报告「偏差清单」。

#![cfg(feature = "occt")]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::panic))]

use std::fs;
use std::io::Read;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use cable_tunnel::{
    CATEGORY, CableTunnel, DEFAULT_CLEAR_HEIGHT_MM, DEFAULT_CLEAR_WIDTH_MM, DEFAULT_DUCT_LENGTH_MM,
    DEFAULT_DUCT_RADIUS_MM, DEFAULT_LENGTH_MM, DUCT_SECTION_PER_RADIUS, GEOMETRY_OP_ID_DUCTBANK,
    GEOMETRY_OP_ID_RECT, TunnelDuctbankOp, TunnelRectOp,
};
use core_app::{CoreSession, GeometryService};
use core_doc::ViewOrientation;
use core_kernel::SubShapeKind;
use core_model::{PropertyMap, Value};
use core_sdk::{GeometryOp, OpInput, Registry};

// ---------------------------------------------------------------------------
// 临时文件（自管，不引入 tempfile；多测试并行用 pid + 序号区分路径）
// ---------------------------------------------------------------------------

static SEQ: AtomicU64 = AtomicU64::new(0);

fn temp_bimdoc(label: &str) -> PathBuf {
    let n = SEQ.fetch_add(1, Ordering::SeqCst);
    let mut path = std::env::temp_dir();
    path.push(format!(
        "bim-e2e-power-{label}-{}-{n}.bimdoc",
        std::process::id()
    ));
    path
}

/// 装载 cable-tunnel 的注册表（e2e 语义：模块装载后注册表里的算子可用）。
fn installed_registry() -> Registry {
    let mut reg = Registry::new();
    reg.install(&CableTunnel).expect("cable-tunnel 应能装载");
    reg
}

// ---------------------------------------------------------------------------
// 钉 1：tunnel.rect 配方有序性（默认腿判据；文件 cfg 门控下跑 occt 腿，
// 纯 SDK apply 断言、不建内核——debug 下亦可跑，故不加 debug ignore）。
// 默认腿的等价覆盖在 cable-tunnel lib.rs 单测（默认腿可跑）。
// ---------------------------------------------------------------------------

#[test]
fn tunnel_rect_recipe_is_well_ordered() {
    let reg = installed_registry();
    let op = reg
        .geometry_op(GEOMETRY_OP_ID_RECT)
        .expect("装载后注册表里应有 power.tunnel.rect");

    let input = OpInput::new(PropertyMap::new(CATEGORY).with("length", Value::Float(9000.0)))
        .at([1000.0, 0.0, 3000.0]);
    let out = op.apply(&input).expect("rect 配方应可生成");

    assert_eq!(
        out.step_count(),
        4,
        "配方应为 4 步（毛坯→孔→Cut→Translate）"
    );
    assert_eq!(out.steps[2].op, core_sdk::BuildOp::Cut);
    // Cut 步元数 2：吃毛坯 + 孔刀具
    assert_eq!(out.steps[2].inputs, vec![0, 1], "Cut 步应吃毛坯 + 孔刀具");
    // warnings 断言：超宽落警告
    let wide = OpInput::new(PropertyMap::new(CATEGORY).with("clear_width", Value::Float(3600.0)));
    let wide_out = op.apply(&wide).expect("超宽配方应可生成");
    assert_eq!(wide_out.warnings.len(), 1, "净宽超过 3m 必须落 1 条警告");

    assert!(out.is_well_ordered(), "配方应有序");
    for step in &out.steps {
        assert!(step.validate_arity().is_ok(), "每步元数应合法");
    }
}

// ---------------------------------------------------------------------------
// 钉 2：tunnel.ductbank 配方每步操作数均可被 Recipe 表达（「可用」尺子
// 静态侧；同样纯 SDK 断言，不加 debug ignore）。
// ---------------------------------------------------------------------------

#[test]
fn tunnel_ductbank_recipe_uses_only_recipe_expressible_operands() {
    let reg = installed_registry();
    let op = reg
        .geometry_op(GEOMETRY_OP_ID_DUCTBANK)
        .expect("装载后注册表里应有 power.tunnel.ductbank");

    let input = OpInput::new(PropertyMap::new(CATEGORY).with("length", Value::Float(6000.0)))
        .at([0.0, 0.0, 0.0]);
    let out = op.apply(&input).expect("ductbank 配方应可生成");

    // 8 步：MakeLineEdge×4 → MakeWire(断面) → MakeLineEdge(脊线) → MakeWire → Pipe
    assert_eq!(out.step_count(), 8);
    assert_eq!(out.steps[4].op, core_sdk::BuildOp::MakeWire, "断面 wire");
    assert_eq!(out.steps[6].op, core_sdk::BuildOp::MakeWire, "脊线 wire");
    assert_eq!(out.steps[7].op, core_sdk::BuildOp::Pipe);
    // Pipe 的 inputs 顺序恒为 [profile, spine]（几何服务 Pipe 派发臂口径）
    assert_eq!(
        out.steps[7].inputs,
        vec![4, 6],
        "Pipe inputs = [profile, spine]"
    );

    // 「可用」尺子（静态侧）：每步的 inputs 只引用前序步骤、args 只用 §5.1
    // 权威 key——即配方每步操作数均可被 Recipe 表达。
    assert!(out.is_well_ordered());
    for (i, step) in out.steps.iter().enumerate() {
        assert!(step.validate_arity().is_ok(), "第 {i} 步元数应合法");
        for key in step.args.keys() {
            // MakeLineEdge 的 6 坐标 key + MakeWire/Pipe 无 args（§5.1 权威表）
            assert!(
                matches!(key.as_str(), "x1" | "y1" | "z1" | "x2" | "y2" | "z2"),
                "第 {i} 步参数 key `{key}` 不在设计文档 §5.1 权威集内"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// 钉 3：tunnel.rect 真实 execute + 落盘断言（occt 腿）
// ---------------------------------------------------------------------------

#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "真实内核用例在 debug 下 wasmtime 编译过慢，请用 --release 运行"
)]
fn tunnel_rect_executes_and_persists() {
    // ── 环 A：CoreSession（with_real_kernel）+ install_module → build_geometry
    //（build_geometry 内部走真实 execute 全链）──
    let mut session = CoreSession::with_real_kernel().expect("occt 腿下 with_real_kernel 必须成功");
    assert_eq!(
        session.geometry().backend_name(),
        "occt-wasm",
        "防假腿——必须跑真实后端 occt-wasm"
    );
    session
        .install_module(Box::new(CableTunnel))
        .expect("cable-tunnel 装载必须通过");
    let tunnel = session
        .create_element(CATEGORY, "TD-1")
        .expect("create_element 失败");
    let shape = session
        .build_geometry(tunnel, GEOMETRY_OP_ID_RECT)
        .expect("build_geometry（真实 execute 全链）失败");
    assert_eq!(
        session.document().doc().geometry_count(),
        1,
        "build_geometry 后应有 1 份几何记录"
    );

    // ── 环 B：save 后容器含 geom/<id>.brep 且文本含 "CASCADE" ──
    let path = temp_bimdoc("rect");
    session.save(&path).expect("save 失败");
    let brep_text = {
        let file = fs::File::open(&path).expect("容器打不开");
        let mut zip = zip::ZipArchive::new(file).expect("zip 解不开");
        let entry = format!("geom/{shape}.brep");
        let mut f = zip
            .by_name(&entry)
            .unwrap_or_else(|e| panic!("容器应含 {entry}（BREP 副本）：{e}"));
        let mut text = String::new();
        f.read_to_string(&mut text).expect("BREP 副本应可读");
        text
    };
    assert!(
        brep_text.contains("CASCADE"),
        "BREP 副本应为 OCCT 标准文本（含 CASCADE Topology）"
    );

    // ── 环 C：体积 oracle（±0.5%）——盒体积 − 孔数×π r² L。
    // 句柄已随 build_geometry 释放；用独立真实内核把落盘文本装回再测
    // （顺带验证「落盘文本可往返重建」）。
    let mut g = GeometryService::new().expect("真实内核应可构造");
    let handle = g
        .kernel_mut()
        .import_brep(&brep_text)
        .expect("落盘 BREP 应可装回真内核");
    let v = g.kernel_mut().volume(handle).expect("volume 应可测");
    let expected = DEFAULT_CLEAR_WIDTH_MM * DEFAULT_CLEAR_HEIGHT_MM * DEFAULT_LENGTH_MM
        - std::f64::consts::PI
            * DEFAULT_DUCT_RADIUS_MM
            * DEFAULT_DUCT_RADIUS_MM
            * DEFAULT_DUCT_LENGTH_MM;
    assert!(
        (v - expected).abs() < 0.005 * expected,
        "体积 oracle：盒−孔={expected:.1} mm³，实得 {v:.1} mm³（±0.5%）"
    );
    g.kernel_mut().release(handle).ok();

    // ── 环 D：warnings 非空可观测（遗留 #6 闭合）——超宽参数经真实 execute
    // 流转仍可观测，且随落盘（snapshot）可取 ──
    let recipe = TunnelRectOp
        .apply(&OpInput::new(
            PropertyMap::new(CATEGORY).with("clear_width", Value::Float(3600.0)),
        ))
        .expect("超宽配方应可生成");
    assert!(!recipe.warnings.is_empty(), "净宽超 3m 应落配方警告");
    let executed = g.execute(&recipe).expect("超宽配方应可真实执行");
    assert!(
        !executed.warnings.is_empty(),
        "warnings 应随真实 execute 流转可观测（遗留 #6）"
    );
    let final_handle = executed.final_handle().expect("超宽配方应有结果句柄");
    let warn_brep = g
        .kernel_mut()
        .export_brep(final_handle)
        .expect("结果应可导出 BREP");
    assert!(warn_brep.contains("CASCADE"), "结果应为真实几何");

    fs::remove_file(&path).ok();
}

// ---------------------------------------------------------------------------
// 钉 4：tunnel.ductbank → Pipe 全链（occt 腿）。
// ⚠ 落盘断言**不用体积**（pipe 产物非实体，见文件头裁定说明）——
// 用 bbox oracle + 种类计数（先探针实测再写死）+ 落盘文本变化。
// ---------------------------------------------------------------------------

#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "真实内核用例在 debug 下 wasmtime 编译过慢，请用 --release 运行"
)]
fn tunnel_ductbank_executes_pipe_end_to_end() {
    // ductbank → Pipe 全链走**真实 execute 路径**（§2「可用」尺子）。
    let mut g = GeometryService::new().expect("真实内核应可构造");
    let input = OpInput::new(PropertyMap::new(CATEGORY).with("length", Value::Float(6000.0)))
        .at([0.0, 0.0, 0.0]);
    let recipe = TunnelDuctbankOp
        .apply(&input)
        .expect("ductbank 配方应可生成");
    assert_eq!(recipe.step_count(), 8, "配方应为 8 步");
    let executed = g.execute(&recipe).expect("ductbank → Pipe 全链应走通");
    assert_eq!(executed.step_count(), 8, "真实 execute 应执行全部 8 步");

    // 落盘文本与 profile/spine 均不同（「落盘文本真变了」钉）。
    let handles = executed.handles();
    let profile_text = g
        .kernel_mut()
        .export_brep(handles[4])
        .expect("断面 wire 应可导出");
    let spine_text = g
        .kernel_mut()
        .export_brep(handles[6])
        .expect("脊线 wire 应可导出");
    let final_handle = executed.final_handle().expect("应有最终句柄");
    let final_text = g
        .kernel_mut()
        .export_brep(final_handle)
        .expect("Pipe 产物应可导出");
    assert!(
        final_text.contains("CASCADE"),
        "Pipe 产物应为真实几何（OCCT 标准文本）"
    );
    assert_ne!(final_text, profile_text, "落盘文本必须区别于断面 wire");
    assert_ne!(final_text, spine_text, "落盘文本必须区别于脊线 wire");

    // bbox oracle（外径 × 脊长）：沿 +X 脊线延伸 L、断面 s×s。
    let s = DUCT_SECTION_PER_RADIUS * DEFAULT_DUCT_RADIUS_MM;
    let l = DEFAULT_LENGTH_MM;
    let bb = g.kernel_mut().bounding_box(final_handle).expect("bbox");
    let span = [
        bb.max[0] - bb.min[0],
        bb.max[1] - bb.min[1],
        bb.max[2] - bb.min[2],
    ];
    assert!(
        (span[0] - l).abs() < 1e-6,
        "bbox 应沿脊线(+X)延展到脊长 {l}，实得 {}",
        span[0]
    );
    assert!(
        (span[1] - s).abs() < 1e-6,
        "bbox 断面宽应为 {s}，实得 {}",
        span[1]
    );
    assert!(
        (span[2] - s).abs() < 1e-6,
        "bbox 断面高应为 {s}，实得 {}",
        span[2]
    );

    // 种类计数（先探针实测再写死——M7b P3-② 口径）：pipe 产物为非实体。
    let solid = g
        .kernel_mut()
        .sub_shape_count(final_handle, SubShapeKind::Solid)
        .expect("Solid 计数应可测");
    let shell = g
        .kernel_mut()
        .sub_shape_count(final_handle, SubShapeKind::Shell)
        .expect("Shell 计数应可测");
    assert_eq!(
        solid, 0,
        "pipe 产物应非实体（sub_shape_count(Solid)==0，occt.rs:3219 既有实测口径）"
    );
    assert!(shell >= 1, "pipe 产物应含壳（Shell≥1），实得 {shell}");

    // 落盘（snapshot）→ 文本含 CASCADE。
    let record = g
        .snapshot(core_model::ShapeId::new(), final_handle)
        .expect("snapshot 应可落盘");
    assert!(
        record.text.contains("CASCADE"),
        "落盘文本应为 OCCT 标准文本"
    );
}

// ---------------------------------------------------------------------------
// 钉 5：五环等效断言（加载 → 建模 → 出图 → 存盘 → 再打开，照设计文档 §8.5）
// ---------------------------------------------------------------------------

#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "真实内核用例在 debug 下 wasmtime 编译过慢，请用 --release 运行"
)]
fn tunnel_module_passes_five_loop_minimal_check() {
    // ── 环 1：加载（防假腿 + install_module）──
    let mut session =
        CoreSession::with_real_kernel().expect("环1: occt 腿下 with_real_kernel 必须成功");
    assert_eq!(
        session.geometry().backend_name(),
        "occt-wasm",
        "环1: 防假腿——必须跑真实后端"
    );
    session
        .install_module(Box::new(CableTunnel))
        .expect("环1: cable-tunnel 装载必须通过");

    // ── 环 2：建模（create_element + build_geometry）──
    let tunnel = session
        .create_element(CATEGORY, "TD-2")
        .expect("环2: create_element 失败");
    session
        .build_geometry(tunnel, GEOMETRY_OP_ID_RECT)
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

    // ── 环 5：再打开 → 元素 / 几何 / Drawing 三项断言 ──
    // （Drawing 整体深相等不用位级 assert_eq!：本投影含浮点噪声圆心
    // （-1.25e-14，17 位有效数字），经 serde_json 默认解析器往返会漂移
    // 1 ULP（serde_json 非 float_roundtrip 模式对 17 位数字的已知限制；
    // 16 位的 -6.2e-15 无损）。先例 e2e_occt 环 6 深相等能过只因
    // example-beam 图元坐标恰好规整。故：规整字段深相等 + 图元级结构
    // 相等 + 坐标数值容差（1e-9 mm，覆盖 ULP 噪声、放过不了真错）。）
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
