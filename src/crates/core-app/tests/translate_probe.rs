//! QA 独立探针（Round 2）：攻击新增的平移链路（`Kernel::translate` / `BuildOp::Translate`）。
//!
//! 由 QA 工程师（Edward）编写，只依赖 `core-app` / `core-model` / `core-sdk` /
//! `core-kernel` 的公开 API。运行：`cargo test -p core-app --test translate_probe`
//!
//! 重点查：平移是否真的把"位置"落到文档里，还是"看起来成功、实则丢数据"。

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::panic))]

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use core_app::{AppError, CoreSession, GeometryService};
use core_kernel::Kernel;
use core_model::{PropertyMap, Value};
use core_sdk::{
    BuildOp, BuildStep, CreateRequest, Element as SdkElement, ElementFactory, GeometryOp, Module,
    ModuleManifest, OpInput, OpOutput, Registry, SdkResult, build_element,
};

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(tag: &str) -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let mut path = std::env::temp_dir();
        path.push(format!("qa-tr-{}-{tag}-{n}", std::process::id()));
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
// 1. 内核门面：平移的基本语义
// ---------------------------------------------------------------------------

#[test]
fn t1_translate_yields_new_live_handle_with_unchanged_volume() {
    let mut k = Kernel::mock();
    let h = k.make_box(100.0, 200.0, 300.0).unwrap();
    let v0 = k.volume(h).unwrap();

    let moved = k.translate(h, 1000.0, 0.0, 3000.0).unwrap();
    assert!(k.is_alive(moved), "平移结果句柄应存活");
    assert!(k.is_alive(h), "原句柄应仍然有效");
    assert_ne!(moved, h, "平移应产生新句柄");
    assert!(
        (k.volume(moved).unwrap() - v0).abs() < 1e-9,
        "平移是刚体运动，体积不变：{v0} vs {}",
        k.volume(moved).unwrap()
    );
}

#[test]
fn t2_zero_translate_semantics_are_a_new_equivalent_handle() {
    // 文档口径："平移产生新形状，原句柄仍有效"。因此 (0,0,0) 也应得到**新句柄**，
    // 而不是把原句柄原样返回（那会让调用方误以为没发生任何事）。
    let mut k = Kernel::mock();
    let h = k.make_box(10.0, 20.0, 30.0).unwrap();
    let same = k.translate(h, 0.0, 0.0, 0.0).unwrap();
    assert!(k.is_alive(h));
    assert_ne!(same, h, "零平移应得到新句柄（当前行为：等价副本）");
    assert!((k.volume(same).unwrap() - 6000.0).abs() < 1e-9);
}

#[test]
fn t3_translate_of_a_released_handle_is_err_not_panic() {
    let mut k = Kernel::mock();
    let h = k.make_box(1.0, 1.0, 1.0).unwrap();
    k.release(h).unwrap();
    assert!(k.translate(h, 1.0, 0.0, 0.0).is_err(), "已释放句柄必须 Err");
}

#[test]
fn t4_translate_rejects_nan_and_infinite_deltas() {
    let mut k = Kernel::mock();
    let h = k.make_box(1.0, 1.0, 1.0).unwrap();
    for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert!(
            k.translate(h, bad, 0.0, 0.0).is_err(),
            "非有限偏移必须 Err，实得 dx={bad}"
        );
    }
}

// ---------------------------------------------------------------------------
// 2. 配方层：两步 MakeBox + Translate
// ---------------------------------------------------------------------------

#[test]
fn t5_two_step_recipe_has_correct_handle_order_and_releases_all() {
    let mut g = GeometryService::with_mock_kernel();
    let recipe = OpOutput::of(vec![
        BuildStep::new(BuildOp::MakeBox)
            .with_f64("dx", 100.0)
            .with_f64("dy", 200.0)
            .with_f64("dz", 300.0),
        BuildStep::new(BuildOp::Translate)
            .from_steps(vec![0])
            .with_f64("x", 1000.0)
            .with_f64("y", 0.0)
            .with_f64("z", 3000.0),
    ]);

    let executed = g.execute(&recipe).unwrap();
    assert_eq!(executed.step_count(), 2, "两步配方应产出两个句柄");
    let last = executed.final_handle().unwrap();
    assert_eq!(last, executed.handles()[1], "末句柄应是最后一步的结果");
    assert_ne!(executed.handles()[0], last, "平移步应产出新句柄");
    assert_eq!(g.kernel().live_count(), 2, "执行后两个中间物都应存活");

    for h in executed.handles() {
        g.kernel_mut().release(*h).unwrap();
    }
    assert_eq!(g.kernel().live_count(), 0, "全部释放后应清零");
}

/// M3 起 `Rotate` 已接线，"未实现算子会被拒"不再成立；但"**非法参数必须响亮
/// 失败、且不留半成品几何**"这个契约仍然有效——只是驱动源换成**真正非法的参数**。
#[test]
fn t6_invalid_operator_arguments_fail_loudly_and_leave_no_geometry() {
    let mut g = GeometryService::with_mock_kernel();
    let recipe = OpOutput::of(vec![
        BuildStep::new(BuildOp::MakeBox)
            .with_f64("dx", 1.0)
            .with_f64("dy", 1.0)
            .with_f64("dz", 1.0),
        // 圆锥下底半径 r1 = -1 → 内核 `non_negative` 拒绝，走真实错误路径。
        BuildStep::new(BuildOp::MakeCone)
            .with_f64("r1", -1.0)
            .with_f64("r2", 0.0)
            .with_f64("height", 10.0),
    ]);
    let r = g.execute(&recipe);
    assert!(
        matches!(r, Err(AppError::Kernel(_))),
        "非法参数必须响亮失败（真实内核错误路径），实得 {r:?}"
    );
    assert_eq!(g.kernel().live_count(), 0, "失败不应留下半成品几何");
}

/// M3 正向对偶：`Rotate` 现在真的会旋转（体积不变、3D 姿态改变）。
#[test]
fn t7_rotate_now_actually_rotates() {
    let mut g = GeometryService::with_mock_kernel();
    let recipe = OpOutput::of(vec![
        BuildStep::new(BuildOp::MakeBox)
            .with_f64("dx", 10.0)
            .with_f64("dy", 20.0)
            .with_f64("dz", 30.0),
        BuildStep::new(BuildOp::Rotate)
            .from_steps(vec![0])
            .with_f64("px", 0.0)
            .with_f64("py", 0.0)
            .with_f64("pz", 0.0)
            .with_f64("dx", 0.0)
            .with_f64("dy", 0.0)
            .with_f64("dz", 1.0)
            .with_f64("angle_rad", std::f64::consts::FRAC_PI_2),
    ]);
    let executed = g.execute(&recipe).unwrap();
    let h = executed.final_handle().unwrap();
    assert!(g.kernel().is_alive(h));

    // 刚体旋转不改变体积。
    assert!((g.kernel_mut().volume(h).unwrap() - 6000.0).abs() < 1e-9);
    // 绕 Z 轴 90°：包围盒 x/y 尺寸互换 → 姿态确实变了。
    let [sx, sy, _] = g.kernel_mut().bounding_box(h).unwrap().size();
    assert!(
        (sx - 20.0).abs() < 1e-6 && (sy - 10.0).abs() < 1e-6,
        "绕 Z 转 90° 后 x/y 应互换，实得 [{sx}, {sy}]"
    );
}

// ---------------------------------------------------------------------------
// 3. 端到端：平移偏移量到底进没进 BREP 文本？
// ---------------------------------------------------------------------------

struct TrFactory;

impl ElementFactory for TrFactory {
    fn category(&self) -> &str {
        "TR_BOX"
    }
    fn display_name(&self) -> &str {
        "TR 盒"
    }
    fn create(&self, request: &CreateRequest) -> SdkResult<SdkElement> {
        let mut e = build_element(self.category(), request, "TR 盒");
        for (k, d) in [
            ("dx", 100.0),
            ("dy", 200.0),
            ("dz", 300.0),
            ("x", 1000.0),
            ("y", 0.0),
            ("z", 3000.0),
        ] {
            e.properties
                .set(k, Value::Float(request.param_f64(k).unwrap_or(d)));
        }
        Ok(e)
    }
    fn default_params(&self) -> PropertyMap {
        PropertyMap::new("TR_BOX")
            .with("dx", Value::Float(100.0))
            .with("dy", Value::Float(200.0))
            .with("dz", Value::Float(300.0))
            .with("x", Value::Float(1000.0))
            .with("y", Value::Float(0.0))
            .with("z", Value::Float(3000.0))
    }
}

struct TrOp;

impl GeometryOp for TrOp {
    fn id(&self) -> &str {
        "tr.box"
    }
    fn display_name(&self) -> &str {
        "TR 平移盒"
    }
    fn apply(&self, input: &OpInput) -> SdkResult<OpOutput> {
        let body = BuildStep::new(BuildOp::MakeBox)
            .with_f64("dx", input.f64_or("dx", 100.0))
            .with_f64("dy", input.f64_or("dy", 200.0))
            .with_f64("dz", input.f64_or("dz", 300.0));
        let moved = BuildStep::new(BuildOp::Translate)
            .from_steps(vec![0])
            .with_f64("x", input.f64_or("x", 1000.0))
            .with_f64("y", input.f64_or("y", 0.0))
            .with_f64("z", input.f64_or("z", 3000.0));
        Ok(OpOutput::of(vec![body, moved]))
    }
}

struct TrModule;

impl Module for TrModule {
    fn manifest(&self) -> ModuleManifest {
        ModuleManifest::new("tr.demo", "TR 示例", "0.1.0")
    }
    fn register(&self, registry: &mut Registry) -> SdkResult<()> {
        registry.register_module(&self.manifest())?;
        registry.register_category("TR_BOX", "TR 盒", "tr.demo")?;
        registry.register_factory(Box::new(TrFactory))?;
        registry.register_geometry_op(Box::new(TrOp))?;
        Ok(())
    }
}

#[test]
fn t8_offset_must_persist_into_the_saved_brep_text() {
    let mut s = CoreSession::new().unwrap();
    s.install_module(Box::new(TrModule)).unwrap();
    let id = s.create_element("TR_BOX", "梁").unwrap();
    let shape = s.build_geometry(id, "tr.box").unwrap();
    assert!(
        s.document().doc().geometry_record(shape).is_some(),
        "几何应落进文档"
    );

    let text = s
        .document()
        .doc()
        .geometry_record(shape)
        .unwrap()
        .text
        .clone();

    // 参照：一个**不平移**的 100×200×300 盒的 BREP 文本
    let mut rk = Kernel::mock();
    let bh = rk.make_box(100.0, 200.0, 300.0).unwrap();
    let untranslated = rk.export_brep(bh).unwrap();

    // 若平移被记进 BREP 文本，这两份文本必须不同。
    assert_ne!(
        text, untranslated,
        "定位点 (1000,0,3000) 未进入 BREP 文本 —— 平移在存盘后丢失（文本 = {text:?}）"
    );

    // 存盘 → 重开后也应保留位置
    let dir = TempDir::new("trans");
    let path = dir.join("t.bimdoc");
    s.save(&path).unwrap();
    let back = CoreSession::open(&path).unwrap();
    let t2 = back
        .document()
        .doc()
        .geometry_record(shape)
        .unwrap()
        .text
        .clone();
    assert_eq!(t2, text, "重开后 BREP 文本应与存盘前一致");
    assert_ne!(t2, untranslated, "重开后位置仍应保留");
}

// ---------------------------------------------------------------------------
// 4. M3 配方分发层：Extrude / Revolve / MakeTorus 必须走 `execute` **真执行**
//    （此前这三条 execute_step 新臂只被编译检查过，从没真跑过——补齐正/负向钉子）
// ---------------------------------------------------------------------------

/// `BuildOp::Extrude` 走 `execute`：结果必须（a）Ok、（b）落盘文本与输入形状不同
/// （v2 平移教训的直接复用：不许"声称改了却不落盘"）、（c）体积符合 mock 占位口径。
#[test]
fn t9_extrude_recipe_actually_changes_and_persists() {
    let mut g = GeometryService::with_mock_kernel();
    let recipe = OpOutput::of(vec![
        BuildStep::new(BuildOp::MakeBox)
            .with_f64("dx", 10.0)
            .with_f64("dy", 10.0)
            .with_f64("dz", 2.0),
        BuildStep::new(BuildOp::Extrude)
            .from_steps(vec![0])
            .with_f64("dx", 0.0)
            .with_f64("dy", 0.0)
            .with_f64("dz", 50.0),
    ]);
    let executed = g.execute(&recipe).expect("拉伸配方应可执行");
    let input = executed.handles()[0];
    let solid = executed.final_handle().unwrap();

    let before = g.kernel_mut().export_brep(input).unwrap();
    let after = g.kernel_mut().export_brep(solid).unwrap();
    assert_ne!(
        before, after,
        "拉伸必须改变存下的 BREP 文本（否则就是静默成功）"
    );

    // mock 占位口径：size = |原尺寸| + |增量| → 10×10×(2+50) = 5200
    assert!(
        (g.kernel_mut().volume(solid).unwrap() - 5200.0).abs() < 1e-9,
        "拉伸后体积应为 5200，实得 {}",
        g.kernel_mut().volume(solid).unwrap()
    );
}

/// `BuildOp::Revolve` 走 `execute`：角度必须真起作用——同一轮廓、不同 `angle_rad`
/// 必须得到**不同**的落盘文本（钉死"角度参数被忽略"）。
#[test]
fn t10_revolve_recipe_actually_changes_and_persists() {
    let mut g = GeometryService::with_mock_kernel();
    let build = |angle: f64| {
        OpOutput::of(vec![
            BuildStep::new(BuildOp::MakeBox)
                .with_f64("dx", 10.0)
                .with_f64("dy", 10.0)
                .with_f64("dz", 2.0),
            BuildStep::new(BuildOp::Revolve)
                .from_steps(vec![0])
                .with_f64("px", 0.0)
                .with_f64("py", 0.0)
                .with_f64("pz", 0.0)
                .with_f64("dx", 0.0)
                .with_f64("dy", 0.0)
                .with_f64("dz", 1.0)
                .with_f64("angle_rad", angle),
        ])
    };
    let half = g
        .execute(&build(std::f64::consts::PI))
        .expect("半圈旋转体配方应可执行");
    let full = g
        .execute(&build(std::f64::consts::TAU))
        .expect("整圈旋转体配方应可执行");

    let half_text = g
        .kernel_mut()
        .export_brep(half.final_handle().unwrap())
        .unwrap();
    let full_text = g
        .kernel_mut()
        .export_brep(full.final_handle().unwrap())
        .unwrap();
    assert_ne!(
        half_text, full_text,
        "不同 angle_rad 必须得到不同文本——角度参数不许被忽略"
    );
}

/// `BuildOp::MakeTorus` 走 `execute`：正向（Ok + 体积 > 0 + 文本非空）；
/// 负向（`minor_radius` 为负）必须经真实内核错误路径响亮失败，且不留半成品几何。
#[test]
fn t11_torus_recipe_builds_and_bad_args_fail_loudly() {
    // 正向：合法参数
    let mut g = GeometryService::with_mock_kernel();
    let ok = OpOutput::of(vec![
        BuildStep::new(BuildOp::MakeTorus)
            .with_f64("major_radius", 10.0)
            .with_f64("minor_radius", 2.0),
    ]);
    let executed = g.execute(&ok).expect("合法圆环配方应可执行");
    let h = executed.final_handle().unwrap();
    assert!(g.kernel_mut().volume(h).unwrap() > 0.0, "圆环体积应为正");
    assert!(
        !g.kernel_mut().export_brep(h).unwrap().is_empty(),
        "圆环落盘文本应非空"
    );

    // 负向：独立服务（从零计），minor_radius = -1 → positive() 拒绝 → InvalidArgument
    let mut g2 = GeometryService::with_mock_kernel();
    assert_eq!(g2.kernel().live_count(), 0);
    let bad = OpOutput::of(vec![
        BuildStep::new(BuildOp::MakeTorus)
            .with_f64("major_radius", 10.0)
            .with_f64("minor_radius", -1.0),
    ]);
    let r = g2.execute(&bad);
    assert!(
        matches!(r, Err(AppError::Kernel(_))),
        "非法圆环参数必须响亮失败（真实内核错误路径）：{r:?}"
    );
    assert_eq!(g2.kernel().live_count(), 0, "失败不得留下半成品几何");
}
