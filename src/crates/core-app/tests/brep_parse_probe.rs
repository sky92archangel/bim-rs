//! QA 独立探针（Round 3）：专攻 `MOCK-BREP` v1/v2 解析的**兼容性**与**严格性**，
//! 以及 `origin` 是否真的进了网格/投影（而非只改文本）。
//!
//! 由 QA 工程师（Edward）编写，只依赖 `core-kernel` 的公开 API
//! （[`Kernel`] 门面的 `import_brep` / `export_brep` / `tessellate` /
//! `project_edges` / `volume`）。运行：
//! `cargo test -p core-app --test brep_parse_probe`
//!
//! 动机：工程师自行把解析从 `filter_map`（静默吞坏 token）改成了严格解析，
//! 这是**它主动改的**，必须独立验证"严得对"（宁可报错不可猜），并专门探
//! v1(4 段) 与 v2(7 段) 之间的**段数歧义**。

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::panic))]

use core_kernel::types::{DVec2, DVec3};
use core_kernel::{CurveGeometry, Kernel, KernelError, ViewDef};

// ---------------------------------------------------------------------------
// A. v1 / v2 兼容：旧文档必须仍可读回，且落在原点
// ---------------------------------------------------------------------------

#[test]
fn p1_legacy_v1_box_still_loads_with_zero_origin() {
    let mut k = Kernel::mock();
    let h = k
        .import_brep("MOCK-BREP v1 box 100 200 300 6e6")
        .expect("v1 文本必须仍可解析");
    assert!(
        (k.volume(h).unwrap() - 6_000_000.0).abs() < 1e-6,
        "体积应读回"
    );
    // 再导出升级为 v3，origin 与 rotation 归零、solid_count=1 → 文本以
    // " 0 0 0 0 0 0 1" 收尾（origin(3) + rotation(3) + solid_count(1)）。
    let text = k.export_brep(h).unwrap();
    assert!(
        text.starts_with("MOCK-BREP v3"),
        "再导出应升级为 v3：{text}"
    );
    assert!(
        text.ends_with(" 0 0 0 0 0 0 1"),
        "v1 读回应落在原点（origin/rotation 归零，单实体）：{text}"
    );
}

#[test]
fn p2_legacy_v1_sphere_still_loads() {
    let mut k = Kernel::mock();
    let h = k
        .import_brep("MOCK-BREP v1 sphere 4 4 4 33.5")
        .expect("v1 球体文本必须仍可解析");
    assert!(k.export_brep(h).unwrap().ends_with(" 0 0 0 0 0 0 1"));
}

#[test]
fn p3_v2_roundtrips_origin_exactly() {
    let mut k = Kernel::mock();
    // v2 读回后 origin 必须精确保留；再导出升级为 v3（rotation 归零、solid_count=1）。
    let h = k
        .import_brep("MOCK-BREP v2 box 100 200 300 6000000 1000 0 3000")
        .expect("合法 v2 文本应可解析");
    assert_eq!(
        k.export_brep(h).unwrap(),
        "MOCK-BREP v3 box 100 200 300 6000000 1000 0 3000 0 0 0 1",
        "v2 origin 应逐字节保留并升级为 v3"
    );
}

// ---------------------------------------------------------------------------
// B. 严格性：畸形文本一律 Err，且绝不 panic（宁可报错不可猜）
// ---------------------------------------------------------------------------

#[test]
fn p4_malformed_texts_are_all_rejected_without_panic() {
    // (文本, 说明) —— 每一条都必须 `Err`；任何一条 panic 就是探针最想要的破绽。
    let cases: &[(&str, &str)] = &[
        ("", "空字符串"),
        ("   ", "纯空白"),
        ("MOCK-BREP", "只有魔数"),
        ("MOCK-BREP v1", "缺版本之后的一切"),
        ("MOCK-BREP v1 box", "缺所有数字"),
        ("MOCK-BREP v1 box 1 2", "v1 只有 2 个数字（应 4）"),
        ("MOCK-BREP v1 box 1 2 3", "v1 只有 3 个数字（应 4）"),
        (
            "MOCK-BREP v1 box 1 2 3 4 5",
            "v1 有 5 个数字（应 4，多一个）",
        ),
        ("MOCK-BREP v2 box 1 2 3 6", "v2 只有 4 个数字（应 7）"),
        ("MOCK-BREP v2 box 1 2 3 6 1 2", "v2 只有 6 个数字（应 7）"),
        (
            "MOCK-BREP v2 box 1 2 3 6 1 2 3 4",
            "v2 有 8 个数字（应 7，多一个）",
        ),
        ("MOCK-BREP v1 box abc 2 3 4", "非数字 token abc"),
        ("MOCK-BREP v2 box 1 2 3 6 x 2 3", "origin 里混入非数字"),
        ("MOCK-BREP v9 box 1 2 3 4", "未知版本 v9"),
        ("MOCK-BREP v1 pyramid 1 2 3 4", "未知种类"),
        ("MOCKBREP v1 box 1 2 3 4", "魔数拼错"),
        (
            "MOCK-BREP v2 box 1 2 3 6 1 2 3 extra",
            "末尾多一个非数字 token",
        ),
    ];
    for (text, why) in cases {
        let mut k = Kernel::mock();
        let r = k.import_brep(text);
        assert!(
            matches!(r, Err(KernelError::Geometry { .. })),
            "畸形文本应报 Geometry 错误（{why}）：{text:?} → {r:?}"
        );
        assert_eq!(k.live_count(), 0, "失败的导入不得留下句柄（{why}）");
    }
}

/// **关键歧义探针**：v2 = "在 v1 末尾追加三段"。那么一条**恰好 6 段数字、
/// 但版本标注为 v1** 的畸形文本，会不会因为段数落进 7 段区间而被误判成 v2？
/// 结论要求：必须靠**版本 token** 判定，段数只是二次校验 → 一律 Err。
#[test]
fn p5_version_token_disambiguates_v1_six_numbers_from_v2() {
    let mut k = Kernel::mock();
    // v1 版本 + 6 个数字：绝不能因为"数字够 6 个"就被当成 v2 接受。
    let r = k.import_brep("MOCK-BREP v1 box 1 2 3 6 100 200");
    assert!(
        matches!(r, Err(KernelError::Geometry { .. })),
        "v1 版本 + 6 段数字必须 Err，实得 {r:?}"
    );
    // 反向：v2 版本 + 4 个数字（旧式正文）也必须 Err，不得悄悄按 v1 收下。
    let r2 = k.import_brep("MOCK-BREP v2 box 1 2 3 6");
    assert!(
        matches!(r2, Err(KernelError::Geometry { .. })),
        "v2 版本 + 4 段数字必须 Err，实得 {r2:?}"
    );
    assert_eq!(k.live_count(), 0);
}

/// **非有限数漏洞（P2，已修复）**：`f64::from_str` 接受 `NaN` / `inf` /
/// `-inf`。这些是**合法 f64 token**，单靠 `parse::<f64>()` 拦不住，会让坏文档
/// 把非有限几何带进内核（随后 `tessellate` 产出 NaN 顶点），与"宁可报错不可猜"
/// 相悖。
///
/// 修复后 `from_text` 对每个 `size` / `volume` / `origin` 分量显式 `is_finite()`
/// 校验，任一非有限即 `Err`。本用例已去掉 `#[ignore]`，正式并入共享门禁。
#[test]
fn p6_non_finite_numeric_tokens_must_be_rejected() {
    let mut k = Kernel::mock();
    for bad in [
        "MOCK-BREP v2 box 1 2 3 6 NaN 0 0",
        "MOCK-BREP v2 box 1 2 3 6 inf 0 0",
        "MOCK-BREP v2 box 1 2 3 6 -inf 0 0",
        "MOCK-BREP v1 box NaN 2 3 6",
        "MOCK-BREP v1 box 1 2 3 inf",
        "MOCK-BREP v1 box 1 2 3 -inf",
        "MOCK-BREP v2 box 1 2 3 NaN 0 0 0",
        "MOCK-BREP v2 box 1 2 3 6 0 0 inf",
    ] {
        let r = k.import_brep(bad);
        assert!(
            matches!(r, Err(KernelError::Geometry { .. })),
            "非有限数 token 应被拒绝（否则几何携带 NaN/inf）：{bad:?} → {r:?}"
        );
    }
    assert_eq!(k.live_count(), 0);
}

// ---------------------------------------------------------------------------
// C. origin 真的生效到网格/投影了吗？volume 仍是不变量？
// ---------------------------------------------------------------------------

#[test]
fn p7_origin_moves_the_mesh_and_projection_but_not_volume() {
    let mut k = Kernel::mock();
    let h = k.make_box(10.0, 20.0, 30.0).unwrap();
    let v0 = k.volume(h).unwrap();
    let moved = k.translate(h, 5.0, 6.0, 7.0).unwrap();

    // 体积不变
    assert!((k.volume(moved).unwrap() - v0).abs() < 1e-9, "平移不改体积");

    // 网格顶点整体位移到 origin
    let m0 = k.tessellate(h, 0.1, 0.5).unwrap();
    let m1 = k.tessellate(moved, 0.1, 0.5).unwrap();
    assert_eq!(m0.positions[0], [0.0, 0.0, 0.0]);
    assert_eq!(
        m1.positions[0],
        [5.0, 6.0, 7.0],
        "网格顶点必须挪到定位点（不能只改文本）"
    );
    assert_eq!(m0.positions.len(), m1.positions.len(), "顶点数不因平移而变");

    // 投影轮廓整体位移 (origin.x, origin.y)
    let p0 = k.project_edges(h, ViewDef::front(DVec3::ZERO)).unwrap();
    let p1 = k.project_edges(moved, ViewDef::front(DVec3::ZERO)).unwrap();
    let first = |p: &core_kernel::Projection| match p.visible[0].data {
        CurveGeometry::Line { a, .. } => a,
        _ => panic!("box 投影应是线段"),
    };
    assert_eq!(first(&p0), DVec2::new(0.0, 0.0));
    assert_eq!(first(&p1), DVec2::new(5.0, 6.0), "投影必须带上定位点偏移");
}

#[test]
fn p8_multiple_translates_accumulate_and_reload_keeps_position() {
    let mut k = Kernel::mock();
    let h = k.make_box(10.0, 20.0, 30.0).unwrap();
    let m1 = k.translate(h, 1000.0, 0.0, 3000.0).unwrap();
    let m2 = k.translate(m1, 5.0, -2.0, 1.0).unwrap();
    let text = k.export_brep(m2).unwrap();
    assert_eq!(
        text, "MOCK-BREP v3 box 10 20 30 6000 1005 -2 3001 0 0 0 1",
        "偏移应逐次累加"
    );
    // 文本 → 重装载 → 再导出：位置原样保留（存盘不丢）
    let back = k.import_brep(&text).unwrap();
    assert_eq!(k.export_brep(back).unwrap(), text, "重装载后位置必须保留");
    assert!((k.volume(back).unwrap() - 6000.0).abs() < 1e-9);
}

// ---------------------------------------------------------------------------
// D. 反向攻击：拒非有限**不得矫枉过正**，误伤有限但非常规的合法输入
// ---------------------------------------------------------------------------

/// engineer 的 `is_finite()` 修复必须**恰好**拦下非有限值，而不能顺手把
/// 有限值也拒了（若 `-0.0` 之类被拒，就是矫枉过正 —— 属回归）。
#[test]
fn p9_finite_but_unusual_values_are_still_accepted() {
    let mut k = Kernel::mock();

    // `-0.0` 是**有限**的：size / origin 出现 `-0.0` 必须照收（不得误伤）。
    let ok = k.import_brep("MOCK-BREP v2 box -0.0 2 3 6 -0.0 0 0");
    assert!(
        ok.is_ok(),
        "-0.0 是有限值，必须接受（若被拒即矫枉过正）：{ok:?}"
    );

    // 常规合法 v1 / v2 仍必须接受（回归护栏）。
    assert!(k.import_brep("MOCK-BREP v1 box 1 2 3 6").is_ok());
    assert!(k.import_brep("MOCK-BREP v2 box 1 2 3 6 1 2 3").is_ok());
}

/// 小写 `nan` / `inf` / `infinity` 同样是 `parse::<f64>()` 能接受的词，
/// 且 **v1 的 size 分量**这一条路径此前未被覆盖 —— 必须一并拦下。
#[test]
fn p10_lowercase_and_v1_sizepath_non_finite_are_rejected() {
    let mut k = Kernel::mock();
    for bad in [
        "MOCK-BREP v1 box nan 2 3 6",            // v1 size + 小写 nan
        "MOCK-BREP v1 box 1 2 3 inf",            // v1 volume + 小写 inf
        "MOCK-BREP v2 box NaN 2 3 6 0 0 0",      // v2 size + NaN
        "MOCK-BREP v2 box 1 2 3 6 infinity 0 0", // v2 origin + infinity
    ] {
        let r = k.import_brep(bad);
        assert!(
            matches!(r, Err(KernelError::Geometry { .. })),
            "非有限数（含小写 nan/inf、v1 size 路径）必须 Err：{bad:?} → {r:?}"
        );
    }
    assert_eq!(k.live_count(), 0);
}
