//! 格式嗅探（设计文档 7.2）。
//!
//! **为什么不只看扩展名**：用户从邮件里拖出来的文件经常叫 `1.dxf` 其实是 DWG，
//! 或者被 Outlook 改名成 `dat`。所以先看**文件头魔数**，扩展名只作兜底。
//!
//! ## 一个真实的坑：STEP 与 IFC 共用同一个头
//!
//! 两者都以 `ISO-10303-21;` 开头（IFC 就是 STEP 物理文件格式的一个 schema）。
//! 区分靠 `FILE_SCHEMA` 里有没有 `IFC`：
//!
//! ```text
//! ISO-10303-21;            ← STEP 与 IFC 都有
//! HEADER;
//! FILE_DESCRIPTION(...);
//! FILE_NAME(...);
//! FILE_SCHEMA(('IFC4X3'));  ← 有 IFC 才是 IFC
//! ```

use crate::capability::Format;

/// 嗅探时最多看多少字节（文件头足够，不整文件读进来）。
pub const PROBE_BYTES: usize = 4096;

/// 头部字节是否像二进制（前 128 字节里有 NUL）。
#[must_use]
pub fn looks_binary(head: &[u8]) -> bool {
    head.iter().take(128).any(|b| *b == 0)
}

/// 嗅探格式。
///
/// 顺序：**魔数 → 文本特征 → 扩展名**。任何一步都不 panic，认不出就返回 `None`
/// （由调用方决定是报错还是按用户手选格式兜底）。
#[must_use]
pub fn detect(filename: &str, head: &[u8]) -> Option<Format> {
    let probe = &head[..head.len().min(PROBE_BYTES)];

    // ① 二进制魔数先判（避免把二进制当文本）
    //    glTF 二进制容器 (.glb)
    if probe.starts_with(b"glTF") {
        return Some(Format::Gltf);
    }
    //    DWG：以 "AC1" 开头**且**是二进制。DXF 里也含 "AC1015"，但它是文本。
    if probe.starts_with(b"AC1") && looks_binary(probe) {
        return Some(Format::Dwg);
    }

    let upper = String::from_utf8_lossy(probe).to_ascii_uppercase();

    // ② ISO-10303-21 —— STEP / IFC 共用头，靠 FILE_SCHEMA 区分
    if upper.starts_with("ISO-10303-21") {
        if upper.contains("FILE_SCHEMA") && upper.contains("IFC") {
            return Some(Format::Ifc4x3);
        }
        return Some(Format::Step);
    }

    // ③ DXF 文本：有 $ACADVER，或形如 "0 / SECTION / HEADER"
    if upper.contains("$ACADVER")
        || (upper.trim_start().starts_with('0')
            && upper.contains("SECTION")
            && upper.contains("HEADER"))
    {
        return Some(Format::Dxf);
    }

    // ④ SVG（XML 文本）
    if upper.contains("<SVG") {
        return Some(Format::Svg);
    }

    // ⑤ glTF 文本（.gltf 是 JSON，带 asset.version 与 generator）
    if upper.contains("\"ASSET\"") && (upper.contains("KHRONOS") || upper.contains("\"VERSION\"")) {
        return Some(Format::Gltf);
    }

    // ⑥ 兜底：扩展名
    Format::from_filename(filename)
}

#[cfg(test)]
mod tests {
    use super::*;

    const STEP_HEAD: &[u8] = b"ISO-10303-21;\nHEADER;\nFILE_DESCRIPTION((''),'2;1');\nFILE_NAME('a.step','',(''),(''),'','','');\nFILE_SCHEMA(('AUTOMOTIVE_DESIGN { 1 0 10303 214 1 1 1 1 }'));\nENDSEC;\nDATA;\n";
    const IFC_HEAD: &[u8] = b"ISO-10303-21;\nHEADER;\nFILE_DESCRIPTION(('ViewDefinition [CoordinationView]'),'2;1');\nFILE_NAME('a.ifc','2024-01-01T00:00:00',(''),(''),'','','');\nFILE_SCHEMA(('IFC4X3'));\nENDSEC;\nDATA;\n";
    const DXF_HEAD: &[u8] =
        b"0\nSECTION\n2\nHEADER\n9\n$ACADVER\n1\nAC1015\n0\nENDSEC\n0\nSECTION\n2\nENTITIES\n";
    const SVG_HEAD: &[u8] =
        b"<?xml version=\"1.0\"?>\n<svg xmlns=\"http://www.w3.org/2000/svg\"></svg>";
    const GLTF_JSON: &[u8] =
        b"{\"asset\":{\"version\":\"2.0\",\"generator\":\"khronos\"},\"meshes\":[]}";

    #[test]
    fn step_header_is_recognised() {
        assert_eq!(detect("a.step", STEP_HEAD), Some(Format::Step));
    }

    #[test]
    fn ifc_is_distinguished_from_step_by_file_schema() {
        assert_eq!(detect("a.ifc", IFC_HEAD), Some(Format::Ifc4x3));
        // 关键：同一个头，只差 FILE_SCHEMA
        assert_eq!(detect("a.step", IFC_HEAD), Some(Format::Ifc4x3));
        assert_eq!(detect("a.ifc", STEP_HEAD), Some(Format::Step));
    }

    #[test]
    fn dxf_text_header_is_recognised() {
        assert_eq!(detect("a.dxf", DXF_HEAD), Some(Format::Dxf));
    }

    #[test]
    fn svg_is_recognised() {
        assert_eq!(detect("a.svg", SVG_HEAD), Some(Format::Svg));
    }

    #[test]
    fn gltf_json_is_recognised() {
        assert_eq!(detect("a.gltf", GLTF_JSON), Some(Format::Gltf));
    }

    #[test]
    fn glb_binary_magic_is_recognised() {
        let mut glb = b"glTF".to_vec();
        glb.extend_from_slice(&[2, 0, 0, 0, 0, 0, 0, 0]);
        glb.push(0);
        assert_eq!(detect("a.glb", &glb), Some(Format::Gltf));
    }

    #[test]
    fn dwg_binary_magic_is_recognised_and_dxf_is_not_confused_with_it() {
        let mut dwg = b"AC1015".to_vec();
        dwg.extend_from_slice(&[0, 0, 0, 0, 1, 2, 3]);
        assert_eq!(detect("a.dwg", &dwg), Some(Format::Dwg));

        // DXF 文本里也含 "AC1015"，但它是文本，不应被判成 DWG
        assert_eq!(detect("a.dxf", DXF_HEAD), Some(Format::Dxf));
    }

    #[test]
    fn unknown_content_falls_back_to_extension() {
        assert_eq!(detect("weird.dwg", b"\x01\x02\x03"), Some(Format::Dwg));
        assert_eq!(detect("weird.ifc", b"\x01\x02\x03"), Some(Format::Ifc4x3));
    }

    #[test]
    fn unknown_content_and_unknown_extension_returns_none() {
        assert_eq!(detect("mystery.bin", b"\x01\x02\x03"), None);
        assert_eq!(detect("", b""), None);
    }

    #[test]
    fn empty_file_with_known_extension_still_resolves() {
        assert_eq!(detect("x.step", b""), Some(Format::Step));
    }

    #[test]
    fn binary_detection_works() {
        assert!(looks_binary(&[0u8, 1, 2]));
        assert!(!looks_binary(b"hello world"));
    }

    #[test]
    fn detection_is_case_insensitive() {
        let head = b"iso-10303-21;\nheader;\nfile_schema(('IFC4'));";
        assert_eq!(detect("a.ifc", head), Some(Format::Ifc4x3));
    }
}
