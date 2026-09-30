//! 几何构造探针（M3 几何构造）：专攻**复合体拆分 / 单实体仪式 / 空结果**，
//! 以及"新算子不得静默成功"这一条 v2 平移教训的直接复用。
//!
//! 由**实施工程师编写于 M3 批次 4**，属首轮自证（非独立对抗复核）；对 M3 的
//! **独立对抗复核见 QA 报告**。本文件**只依赖 `core-kernel` 的公开 API**
//! （[`Kernel`] 门面的 `make_*` / `fuse` / `translate` / `rotate` /
//! `extrude` / `revolve` / `split_solids` / `single_solid` / `sub_shapes` /
//! `bounding_box` / `export_brep` / `volume`）。运行：
//! `cargo test -p core-app --test geometry_construction_probe`
//!
//! 动机（设计文档 §5.3）：布尔结果恒为 `TopoDS_Compound`，**不是**实体。要把
//! 它当实体用，必须先 `split_solids` 拆分、或用 `single_solid` 显式要求"恰好
//! 一个"。若 `single_solid` 在多体时静默取 first，则"桥架少了一段"这类问题会
//! 在几百步之后才暴露——本探针就是钉死"多体必须响亮报错"。

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::panic))]

use core_kernel::types::SubShapeKind;
use core_kernel::{Kernel, KernelError};

// ---------------------------------------------------------------------------
// 钉子 A —— 一次布尔确实能产出多个互不相连体（且相连时仍是一个）
// ---------------------------------------------------------------------------

/// **关键反假绿钉子**：占位后端若不在 `fuse` 时真判 AABB 重叠，这里会永远得 1，
/// 断言 `== 2` 直接红，极易被人把断言改成 `== 1`，从而把 M3 唯一的复合体验证
/// 悄悄废掉。因此本钉子同时钉**不相连 → 2** 与 **重叠 → 1** 两面。
#[test]
fn a_single_boolean_can_yield_two_disjoint_solids() {
    let mut k = Kernel::mock();
    let a = k.make_box(10.0, 10.0, 10.0).unwrap();

    // 挪到远处 → 与 a 不相连
    let far_box = k.make_box(10.0, 10.0, 10.0).unwrap();
    let far = k.translate(far_box, 100.0, 0.0, 0.0).unwrap();
    let fused = k.fuse(a, far).unwrap();
    let solids = k.split_solids(fused).unwrap();
    assert_eq!(
        solids.len(),
        2,
        "一次布尔产出了两个互不相连体，必须能拆出来"
    );
    assert_eq!(k.sub_shape_count(fused, SubShapeKind::Solid).unwrap(), 2);

    // 反面：部分重叠 → 相连 → 仍是 1 个实体
    let near_box = k.make_box(10.0, 10.0, 10.0).unwrap();
    let near = k.translate(near_box, 5.0, 0.0, 0.0).unwrap();
    let fused2 = k.fuse(a, near).unwrap();
    assert_eq!(
        k.split_solids(fused2).unwrap().len(),
        1,
        "重叠的两盒 fuse 后应仍是 1 个实体"
    );
    k.release_many(&solids);
}

// ---------------------------------------------------------------------------
// 钉子 B —— `single_solid` 对多体必须报错（绝不得静默取 first）
// ---------------------------------------------------------------------------

#[test]
fn b_single_solid_refuses_multiple_solids() {
    let mut k = Kernel::mock();
    let a = k.make_box(10.0, 10.0, 10.0).unwrap();
    let far_box = k.make_box(10.0, 10.0, 10.0).unwrap();
    let far = k.translate(far_box, 100.0, 0.0, 0.0).unwrap();
    let fused = k.fuse(a, far).unwrap();

    match k.single_solid(fused) {
        Err(KernelError::NotSingleSolid { count }) => {
            assert_eq!(count, 2, "多体数应如实报出");
        }
        other => panic!("多体必须响亮拒绝（不得取 first），实得 {other:?}"),
    }

    // 对偶：恰好一个实体时必须正常放行
    let single = k.make_box(3.0, 4.0, 5.0).unwrap();
    let ok = k.single_solid(single).expect("单实体应正常返回");
    assert!(k.is_alive(ok), "单实体返回的应是可用的新句柄");
}

// ---------------------------------------------------------------------------
// 钉子 C —— 空结果报 `EmptyResult`（在**显式为空**的路径上钉，不赌 cut 语义）
// ---------------------------------------------------------------------------

/// 显式构造一个"0 实体的复合体"（v3 文本 `solid_count = 0`），经公开
/// `import_brep` 进入内核，再走 `split_solids` / `single_solid`。
///
/// **不**用 `cut(a, a)` 之类推断空——占位 `cut` 的 `volume=(sa-sb).max(0)` 会
/// 得 0 体积但仍返回一个 compound，据此断言空就是"赌语义"，不是钉子。
#[test]
fn c_empty_solid_set_reports_empty_result() {
    let mut k = Kernel::mock();
    let empty = k
        .import_brep("MOCK-BREP v3 compound 10 20 30 6000 0 0 0 0 0 0 0")
        .expect("合法 v3 文本（solid_count = 0）应可解析");

    assert!(
        k.split_solids(empty).unwrap().is_empty(),
        "0 实体的复合体应拆出空集"
    );
    assert_eq!(k.sub_shape_count(empty, SubShapeKind::Solid).unwrap(), 0);
    match k.single_solid(empty) {
        Err(KernelError::EmptyResult(_)) => {}
        other => panic!("空实体集必须报 EmptyResult，实得 {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// v2 平移教训的直接复用：凡"声称改了什么的算子"，其 BREP 文本必须变
// ---------------------------------------------------------------------------

#[test]
fn d_rotate_is_not_a_silent_success() {
    let mut k = Kernel::mock();
    let h = k.make_box(10.0, 20.0, 30.0).unwrap();
    let before = k.export_brep(h).unwrap();
    let rot = k
        .rotate(h, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, std::f64::consts::FRAC_PI_2)
        .unwrap();
    let after = k.export_brep(rot).unwrap();

    assert_ne!(before, after, "旋转必须改变存下的文本（否则就是静默成功）");
    assert!(
        (k.volume(rot).unwrap() - k.volume(h).unwrap()).abs() < 1e-9,
        "刚体旋转不改变体积"
    );
    // 绕 Z 轴 90°：包围盒的 x/y 尺寸互换
    let [sx, sy, _] = k.bounding_box(rot).unwrap().size();
    assert!(
        (sx - 20.0).abs() < 1e-6 && (sy - 10.0).abs() < 1e-6,
        "绕 Z 转 90° 后 x/y 应互换，实得 [{sx}, {sy}]"
    );
}

#[test]
fn e_new_primitives_have_distinct_tags_and_positive_volume() {
    let mut k = Kernel::mock();
    let cone = k.make_cone(3.0, 0.0, 10.0).unwrap();
    let torus = k.make_torus(10.0, 2.0).unwrap();

    assert!(k.volume(cone).unwrap() > 0.0, "圆锥体积应为正");
    assert!(k.volume(torus).unwrap() > 0.0, "圆环体积应为正");
    assert!(k.export_brep(cone).unwrap().contains(" cone "));
    assert!(k.export_brep(torus).unwrap().contains(" torus "));
    assert_ne!(k.export_brep(cone).unwrap(), k.export_brep(torus).unwrap(),);
}

#[test]
fn f_extrude_and_revolve_change_the_shape_and_angles_matter() {
    let mut k = Kernel::mock();
    let base = k.make_box(10.0, 10.0, 2.0).unwrap();
    let base_text = k.export_brep(base).unwrap();

    let ext = k.extrude(base, 0.0, 0.0, 50.0).unwrap();
    assert!(k.export_brep(ext).unwrap().contains(" extrude "));
    assert_ne!(k.export_brep(ext).unwrap(), base_text);
    assert!(
        k.volume(ext).unwrap() > k.volume(base).unwrap(),
        "拉伸应使体积增大"
    );

    // 角度必须真正参与：半圈 vs 整圈 → 体积不同
    let half = k
        .revolve(base, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, std::f64::consts::PI)
        .unwrap();
    let full = k
        .revolve(base, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, std::f64::consts::TAU)
        .unwrap();
    assert!(
        k.volume(full).unwrap() > k.volume(half).unwrap(),
        "整圈扫掠体积应大于半圈（角度必须进体积）"
    );
}

/// 反向探针：`single_solid` **不得**被实现成"取 first 静默成功"。
/// 若多体返回 Ok，则本用例红——这是对"静默取 first"的直接反证。
#[test]
fn g_single_solid_never_silently_returns_first_of_many() {
    let mut k = Kernel::mock();
    let a = k.make_box(1.0, 1.0, 1.0).unwrap();
    let b0 = k.make_box(1.0, 1.0, 1.0).unwrap();
    let b = k.translate(b0, 50.0, 0.0, 0.0).unwrap();
    let fused = k.fuse(a, b).unwrap();

    assert!(
        k.single_solid(fused).is_err(),
        "多体时 single_solid 必须 Err，绝不能静默返回第一个"
    );
}
