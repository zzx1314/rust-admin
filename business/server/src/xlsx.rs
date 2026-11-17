//! 极简 xlsx 读写。只服务「责任人库 Excel 导入」这一件事，不提供通用表格能力。
//!
//! 为什么不引第三方 Excel 依赖：场景固定——
//!
//! - 模板由平台自己发（`staff_import::template`），只用到「表头 + 说明页」；
//! - 用户填完（Excel / WPS / LibreOffice 重存）后列结构不变，读的时候按表头认列；
//! - Cargo 里已经有 `zip`，xlsx 就是「zip + 几个 XML」。
//!
//! 写：单元格一律写成 `inlineStr`（不生成 `sharedStrings.xml`），表头加粗并冻结首行。
//! 读：只取第一个工作表，兼容 `sharedStrings` / `inlineStr` / `str` / 数字四种单元格类型
//! （Excel 重存后文本会进 sharedStrings，数字单元格可能是 `1.38E10` 这种科学计数法）。

use std::io::{Cursor, Read, Seek, Write};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

/// 一张工作表：首行表头（加粗 + 冻结 + 筛选）+ 数据行。
pub struct Sheet {
    pub name: String,
    /// 表头；为空表示这张表没有表头
    pub headers: Vec<String>,
    pub rows: Vec<Vec<String>>,
    /// 列宽，与 `headers` 对位；缺省 18
    pub widths: Vec<f64>,
}

/// 读出来的一行：`number` 是 Excel 里的行号（1 开始），报错 / 回显都按它定位。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SheetRow {
    pub number: usize,
    pub cells: Vec<String>,
}

pub fn write_workbook(sheets: &[Sheet]) -> Result<Vec<u8>, String> {
    if sheets.is_empty() {
        return Err("至少需要一张工作表".to_string());
    }

    let mut buffer = Cursor::new(Vec::new());
    {
        let mut zip = ZipWriter::new(&mut buffer);
        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);

        write_entry(
            &mut zip,
            options,
            "[Content_Types].xml",
            &content_types(sheets),
        )?;
        write_entry(&mut zip, options, "_rels/.rels", ROOT_RELS)?;
        write_entry(&mut zip, options, "xl/workbook.xml", &workbook(sheets))?;
        write_entry(
            &mut zip,
            options,
            "xl/_rels/workbook.xml.rels",
            &workbook_rels(sheets),
        )?;
        write_entry(&mut zip, options, "xl/styles.xml", STYLES)?;
        for (index, sheet) in sheets.iter().enumerate() {
            write_entry(
                &mut zip,
                options,
                &format!("xl/worksheets/sheet{}.xml", index + 1),
                &sheet_xml(sheet),
            )?;
        }
        zip.finish().map_err(|e| format!("打包 xlsx 失败：{e}"))?;
    }
    Ok(buffer.into_inner())
}

/// 读第一个工作表的所有行（含空行，由调用方决定怎么处理）。
pub fn read_first_sheet(bytes: &[u8]) -> Result<Vec<SheetRow>, String> {
    let mut archive = ZipArchive::new(Cursor::new(bytes))
        .map_err(|e| format!("不是有效的 .xlsx 文件（读不出 zip 结构）：{e}"))?;
    let shared = read_shared_strings(&mut archive);
    let path = first_sheet_path(&mut archive);
    let xml = read_entry(&mut archive, &path)
        .ok_or_else(|| format!("xlsx 里找不到工作表 {path}，请用平台提供的模板填写"))?;
    Ok(parse_sheet(&xml, &shared))
}

// ------------------------------------------------------------ 写（模板）

const ROOT_RELS: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
<Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument\" Target=\"xl/workbook.xml\"/>\
</Relationships>";

const STYLES: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
<styleSheet xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\">\
<fonts count=\"2\">\
<font><sz val=\"11\"/><name val=\"\u{5b8b}\u{4f53}\"/><charset val=\"134\"/></font>\
<font><b/><sz val=\"11\"/><name val=\"\u{5b8b}\u{4f53}\"/><charset val=\"134\"/></font>\
</fonts>\
<fills count=\"2\"><fill><patternFill patternType=\"none\"/></fill><fill><patternFill patternType=\"gray125\"/></fill></fills>\
<borders count=\"1\"><border><left/><right/><top/><bottom/><diagonal/></border></borders>\
<cellStyleXfs count=\"1\"><xf numFmtId=\"0\" fontId=\"0\" fillId=\"0\" borderId=\"0\"/></cellStyleXfs>\
<cellXfs count=\"2\">\
<xf numFmtId=\"0\" fontId=\"0\" fillId=\"0\" borderId=\"0\" xfId=\"0\"/>\
<xf numFmtId=\"0\" fontId=\"1\" fillId=\"0\" borderId=\"0\" xfId=\"0\" applyFont=\"1\"/>\
</cellXfs>\
<cellStyles count=\"1\"><cellStyle name=\"Normal\" xfId=\"0\" builtinId=\"0\"/></cellStyles>\
</styleSheet>";

fn write_entry(
    zip: &mut ZipWriter<&mut Cursor<Vec<u8>>>,
    options: SimpleFileOptions,
    name: &str,
    content: &str,
) -> Result<(), String> {
    zip.start_file(name, options)
        .map_err(|e| format!("写入 {name} 失败：{e}"))?;
    zip.write_all(content.as_bytes())
        .map_err(|e| format!("写入 {name} 失败：{e}"))
}

fn content_types(sheets: &[Sheet]) -> String {
    let mut xml = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">\
<Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/>\
<Default Extension=\"xml\" ContentType=\"application/xml\"/>\
<Override PartName=\"/xl/workbook.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml\"/>\
<Override PartName=\"/xl/styles.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.styles+xml\"/>",
    );
    for index in 0..sheets.len() {
        xml.push_str(&format!(
            "<Override PartName=\"/xl/worksheets/sheet{}.xml\" \
ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml\"/>",
            index + 1
        ));
    }
    xml.push_str("</Types>");
    xml
}

fn workbook(sheets: &[Sheet]) -> String {
    let mut xml = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
<workbook xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\" \
xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\"><sheets>",
    );
    for (index, sheet) in sheets.iter().enumerate() {
        xml.push_str(&format!(
            "<sheet name=\"{}\" sheetId=\"{}\" r:id=\"rId{}\"/>",
            escape_xml(&sheet.name),
            index + 1,
            index + 1
        ));
    }
    xml.push_str("</sheets></workbook>");
    xml
}

fn workbook_rels(sheets: &[Sheet]) -> String {
    let mut xml = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">",
    );
    for index in 0..sheets.len() {
        xml.push_str(&format!(
            "<Relationship Id=\"rId{}\" \
Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet\" \
Target=\"worksheets/sheet{}.xml\"/>",
            index + 1,
            index + 1
        ));
    }
    xml.push_str(&format!(
        "<Relationship Id=\"rId{}\" \
Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles\" \
Target=\"styles.xml\"/>",
        sheets.len() + 1
    ));
    xml.push_str("</Relationships>");
    xml
}

fn sheet_xml(sheet: &Sheet) -> String {
    let has_header = !sheet.headers.is_empty();
    let total_rows = sheet.rows.len() + usize::from(has_header);
    let last_col = column_name(
        sheet
            .rows
            .iter()
            .map(Vec::len)
            .chain(std::iter::once(sheet.headers.len()))
            .max()
            .unwrap_or(1)
            .max(1)
            - 1,
    );

    let mut xml = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
<worksheet xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\">",
    );
    xml.push_str(&format!(
        "<dimension ref=\"A1:{last_col}{}\"/>",
        total_rows.max(1)
    ));
    if has_header {
        xml.push_str(
            "<sheetViews><sheetView workbookViewId=\"0\">\
<pane ySplit=\"1\" topLeftCell=\"A2\" activePane=\"bottomLeft\" state=\"frozen\"/>\
</sheetView></sheetViews>",
        );
    } else {
        xml.push_str("<sheetViews><sheetView workbookViewId=\"0\"/></sheetViews>");
    }
    xml.push_str("<sheetFormatPr defaultRowHeight=\"15\"/>");

    let width_count = sheet.headers.len().max(sheet.widths.len());
    if width_count > 0 {
        xml.push_str("<cols>");
        for index in 0..width_count {
            let width = sheet.widths.get(index).copied().unwrap_or(18.0);
            xml.push_str(&format!(
                "<col min=\"{}\" max=\"{}\" width=\"{width}\" customWidth=\"1\"/>",
                index + 1,
                index + 1
            ));
        }
        xml.push_str("</cols>");
    }

    xml.push_str("<sheetData>");
    let mut row_number = 0;
    if has_header {
        row_number = 1;
        xml.push_str("<row r=\"1\">");
        for (index, header) in sheet.headers.iter().enumerate() {
            push_cell(
                &mut xml,
                &format!("{}{}", column_name(index), row_number),
                header,
                Some(1),
            );
        }
        xml.push_str("</row>");
    }
    for row in &sheet.rows {
        row_number += 1;
        xml.push_str(&format!("<row r=\"{row_number}\">"));
        for (index, value) in row.iter().enumerate() {
            push_cell(
                &mut xml,
                &format!("{}{}", column_name(index), row_number),
                value,
                None,
            );
        }
        xml.push_str("</row>");
    }
    xml.push_str("</sheetData>");

    // autoFilter 必须排在 sheetData 之后
    if has_header && total_rows > 0 {
        xml.push_str(&format!("<autoFilter ref=\"A1:{last_col}{total_rows}\"/>"));
    }
    xml.push_str("</worksheet>");
    xml
}

fn push_cell(xml: &mut String, reference: &str, value: &str, style: Option<u8>) {
    let style = style.map(|s| format!(" s=\"{s}\"")).unwrap_or_default();
    xml.push_str(&format!(
        "<c r=\"{reference}\"{style} t=\"inlineStr\"><is><t xml:space=\"preserve\">{}</t></is></c>",
        escape_xml(value)
    ));
}

/// 0 开始的列号 → A / B / ... / AA
fn column_name(index: usize) -> String {
    let mut remaining = index + 1;
    let mut letters = Vec::new();
    while remaining > 0 {
        let remainder = (remaining - 1) % 26;
        letters.push(b'A' + remainder as u8);
        remaining = (remaining - 1) / 26;
    }
    letters.reverse();
    String::from_utf8(letters).unwrap_or_else(|_| "A".to_string())
}

// ------------------------------------------------------------ 读（上传）

fn read_entry<R: Read + Seek>(archive: &mut ZipArchive<R>, name: &str) -> Option<String> {
    let mut file = archive.by_name(name).ok()?;
    let mut buffer = Vec::new();
    file.read_to_end(&mut buffer).ok()?;
    Some(String::from_utf8_lossy(&buffer).into_owned())
}

/// 第一个工作表在 zip 里的路径：优先按 workbook.xml + rels 解析，
/// 解析不出来再退回 `xl/worksheets/sheet1.xml`（我们自己的模板就是这个路径）。
fn first_sheet_path<R: Read + Seek>(archive: &mut ZipArchive<R>) -> String {
    const FALLBACK: &str = "xl/worksheets/sheet1.xml";
    let Some(workbook) = read_entry(archive, "xl/workbook.xml") else {
        return FALLBACK.to_string();
    };
    let Some((start, end)) = next_tag(&workbook, "sheet", 0) else {
        return FALLBACK.to_string();
    };
    let Some(relation) = attr(&workbook[start..=end], "r:id") else {
        return FALLBACK.to_string();
    };
    let Some(rels) = read_entry(archive, "xl/_rels/workbook.xml.rels") else {
        return FALLBACK.to_string();
    };
    let mut cursor = 0;
    while let Some((rel_start, rel_end)) = next_tag(&rels, "Relationship", cursor) {
        let tag = &rels[rel_start..=rel_end];
        cursor = rel_end + 1;
        if attr(tag, "Id") != Some(relation) {
            continue;
        }
        let Some(target) = attr(tag, "Target") else {
            continue;
        };
        let target = target.trim_start_matches('/');
        return if target.starts_with("xl/") {
            target.to_string()
        } else {
            format!("xl/{target}")
        };
    }
    FALLBACK.to_string()
}

fn read_shared_strings<R: Read + Seek>(archive: &mut ZipArchive<R>) -> Vec<String> {
    let Some(xml) = read_entry(archive, "xl/sharedStrings.xml") else {
        return Vec::new();
    };
    let mut strings = Vec::new();
    let mut cursor = 0;
    while let Some((start, end)) = next_tag(&xml, "si", cursor) {
        let inner_start = end + 1;
        if xml[start..=end].ends_with("/>") {
            strings.push(String::new());
            cursor = inner_start;
            continue;
        }
        let close = xml[inner_start..]
            .find("</si>")
            .map(|offset| inner_start + offset)
            .unwrap_or(xml.len());
        strings.push(text_of(&xml[inner_start..close], "t"));
        cursor = (close + 5).min(xml.len());
    }
    strings
}

fn parse_sheet(xml: &str, shared: &[String]) -> Vec<SheetRow> {
    let mut rows = Vec::new();
    let mut cursor = 0;
    let mut position = 0usize;
    while let Some((start, end)) = next_tag(xml, "row", cursor) {
        position += 1;
        let tag = &xml[start..=end];
        let number = attr(tag, "r")
            .and_then(|value| value.trim().parse::<usize>().ok())
            .unwrap_or(position);
        let inner_start = end + 1;
        if tag.ends_with("/>") {
            rows.push(SheetRow {
                number,
                cells: Vec::new(),
            });
            cursor = inner_start;
            continue;
        }
        let close = xml[inner_start..]
            .find("</row>")
            .map(|offset| inner_start + offset)
            .unwrap_or(xml.len());
        rows.push(SheetRow {
            number,
            cells: parse_cells(&xml[inner_start..close], shared),
        });
        cursor = (close + 6).min(xml.len());
    }
    rows
}

fn parse_cells(xml: &str, shared: &[String]) -> Vec<String> {
    let mut cells: Vec<(usize, String)> = Vec::new();
    let mut cursor = 0;
    let mut position = 0usize;
    while let Some((start, end)) = next_tag(xml, "c", cursor) {
        let tag = &xml[start..=end];
        let column = attr(tag, "r").and_then(column_index).unwrap_or(position);
        position = column + 1;
        let self_closing = tag.ends_with("/>");
        let (value, next) = if self_closing {
            (String::new(), end + 1)
        } else {
            let close = xml[end + 1..]
                .find("</c>")
                .map(|offset| end + 1 + offset)
                .unwrap_or(xml.len());
            let value = cell_text(&xml[end + 1..close], attr(tag, "t"), shared);
            (value, (close + 4).min(xml.len()))
        };
        cells.push((column, value));
        cursor = next;
    }

    let width = cells
        .iter()
        .map(|(column, _)| column + 1)
        .max()
        .unwrap_or(0);
    let mut row = vec![String::new(); width];
    for (column, value) in cells {
        if let Some(slot) = row.get_mut(column) {
            *slot = value;
        }
    }
    row
}

/// `A12` → 0，`AB3` → 27
fn column_index(reference: &str) -> Option<usize> {
    let mut index = 0usize;
    let mut letters = 0usize;
    for character in reference.chars() {
        let upper = character.to_ascii_uppercase();
        if !upper.is_ascii_uppercase() {
            break;
        }
        index = index * 26 + (upper as usize - 'A' as usize + 1);
        letters += 1;
    }
    if letters == 0 { None } else { Some(index - 1) }
}

fn cell_text(inner: &str, kind: Option<&str>, shared: &[String]) -> String {
    match kind {
        Some("s") => text_of(inner, "v")
            .trim()
            .parse::<usize>()
            .ok()
            .and_then(|index| shared.get(index).cloned())
            .unwrap_or_default(),
        Some("inlineStr") => text_of(inner, "t"),
        // str / b / e / d：取 <v> 原文，不猜语义
        Some("str") | Some("b") | Some("e") | Some("d") => text_of(inner, "v"),
        // 数字：Excel 会把 13800000000 存成 1.38E10，这里还原成整数文本
        _ => normalize_number(&text_of(inner, "v")),
    }
}

fn normalize_number(raw: &str) -> String {
    let text = raw.trim();
    let numeric = !text.is_empty()
        && text
            .chars()
            .all(|c| c.is_ascii_digit() || matches!(c, '-' | '+' | '.' | 'e' | 'E'));
    if numeric
        && let Ok(value) = text.parse::<f64>()
        && value.is_finite()
        && value.fract() == 0.0
        && value.abs() < 1e15
    {
        return format!("{}", value as i64);
    }
    text.to_string()
}

/// 找到 `<tag` 开标签（含属性），返回 `(<` 的位置, `>` 的位置)。
/// 标签名后面必须是空白 / `>` / `/`，避免 `<c>` 匹配到 `<cols>`。
fn next_tag(xml: &str, tag: &str, from: usize) -> Option<(usize, usize)> {
    let needle = format!("<{tag}");
    let mut cursor = from.min(xml.len());
    while let Some(offset) = xml[cursor..].find(&needle) {
        let start = cursor + offset;
        let boundary = xml.as_bytes().get(start + needle.len()).copied();
        let matches = matches!(
            boundary,
            Some(b' ') | Some(b'\t') | Some(b'\r') | Some(b'\n') | Some(b'>') | Some(b'/')
        );
        if matches && let Some(end) = xml[start..].find('>') {
            return Some((start, start + end));
        }
        cursor = start + needle.len();
    }
    None
}

/// 取 XML 属性值（双引号或单引号都认）
fn attr<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let mut cursor = 0;
    while let Some(offset) = tag[cursor..].find(name) {
        let start = cursor + offset;
        cursor = start + name.len();
        let before = start
            .checked_sub(1)
            .and_then(|index| tag.as_bytes().get(index));
        let before_ok = matches!(before, Some(b' ') | Some(b'\t') | Some(b'\r') | Some(b'\n'));
        let rest = tag[cursor..].trim_start();
        let Some(rest) = rest.strip_prefix('=') else {
            continue;
        };
        let rest = rest.trim_start();
        let quote = rest.chars().next()?;
        if !before_ok || (quote != '"' && quote != '\'') {
            continue;
        }
        let body = &rest[1..];
        let end = body.find(quote)?;
        return Some(&body[..end]);
    }
    None
}

/// 拼接 `xml` 里所有 `<tag>` 的内部文本（富文本的 `<r><t>` 也会连起来）
fn text_of(xml: &str, tag: &str) -> String {
    let mut text = String::new();
    let mut cursor = 0;
    while let Some((start, end)) = next_tag(xml, tag, cursor) {
        if xml[start..=end].ends_with("/>") {
            cursor = end + 1;
            continue;
        }
        let close = xml[end + 1..]
            .find(&format!("</{tag}>"))
            .map(|offset| end + 1 + offset)
            .unwrap_or(xml.len());
        text.push_str(&unescape_xml(&xml[end + 1..close]));
        cursor = (close + tag.len() + 3).min(xml.len());
    }
    text
}

pub fn escape_xml(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&apos;"),
            _ => escaped.push(character),
        }
    }
    escaped
}

fn unescape_xml(text: &str) -> String {
    if !text.contains('&') {
        return text.to_string();
    }
    let mut decoded = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(position) = rest.find('&') {
        decoded.push_str(&rest[..position]);
        rest = &rest[position..];
        let Some(end) = rest.find(';') else {
            break;
        };
        let entity = &rest[1..end];
        let replacement = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "nbsp" => Some(' '),
            other => other
                .strip_prefix("#x")
                .or_else(|| other.strip_prefix("#X"))
                .and_then(|code| u32::from_str_radix(code, 16).ok())
                .or_else(|| {
                    other
                        .strip_prefix('#')
                        .and_then(|code| code.parse::<u32>().ok())
                })
                .and_then(char::from_u32),
        };
        match replacement {
            Some(character) => {
                decoded.push(character);
                rest = &rest[end + 1..];
            }
            // 不认识的实体原样保留（比如用户自己写进单元格的 &xxx;）
            None => {
                decoded.push('&');
                rest = &rest[1..];
            }
        }
    }
    decoded.push_str(rest);
    decoded
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Vec<Sheet> {
        vec![
            Sheet {
                name: "责任人".to_string(),
                headers: vec!["姓名".into(), "工号".into(), "技能".into()],
                rows: vec![
                    vec!["张三".into(), "EMP-01".into(), "红外测温 / 应急处置".into()],
                    vec!["李四 <a&b>".into(), "\"引号\"".into(), String::new()],
                ],
                widths: vec![16.0, 14.0, 28.0],
            },
            Sheet {
                name: "填写说明".to_string(),
                headers: vec!["列名".into(), "说明".into()],
                rows: vec![vec!["姓名".into(), "必填".into()]],
                widths: vec![12.0, 40.0],
            },
        ]
    }

    #[test]
    fn 模板写入后能原样读回() {
        let bytes = write_workbook(&sample()).expect("写 xlsx 失败");
        let rows = read_first_sheet(&bytes).expect("读 xlsx 失败");
        assert_eq!(rows.len(), 3, "表头 + 两行数据");
        assert_eq!(rows[0].number, 1);
        assert_eq!(rows[0].cells, vec!["姓名", "工号", "技能"]);
        assert_eq!(rows[1].cells, vec!["张三", "EMP-01", "红外测温 / 应急处置"]);
        // 空单元格不落盘，读回来会补空串；特殊字符走转义 / 反转义
        assert_eq!(rows[2].cells, vec!["李四 <a&b>", "\"引号\"", ""]);
    }

    #[test]
    fn 列号与单元格引用互转() {
        assert_eq!(column_name(0), "A");
        assert_eq!(column_name(25), "Z");
        assert_eq!(column_name(26), "AA");
        assert_eq!(column_index("AA12"), Some(26));
        assert_eq!(column_index("b3"), Some(1));
        assert_eq!(column_index("12"), None);
    }

    #[test]
    fn 科学计数法的数字单元格还原成整数() {
        assert_eq!(normalize_number("1.38E10"), "13800000000");
        assert_eq!(normalize_number("13800000000"), "13800000000");
        assert_eq!(normalize_number("0.5"), "0.5");
        assert_eq!(normalize_number("EMP-01"), "EMP-01");
    }

    #[test]
    fn 读取excel风格的shared_strings与稀疏单元格() {
        // 模拟 Excel 重存后的形状：文本进 sharedStrings、空列不写入、
        // 行/单元格都带 r 属性，数字单元格是科学计数法。
        let sheet = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
<worksheet xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\">\
<sheetData>\
<row r=\"1\"><c r=\"A1\" t=\"s\"><v>0</v></c><c r=\"B1\" t=\"s\"><v>1</v></c></row>\
<row r=\"3\"><c r=\"A3\" t=\"s\"><v>2</v></c><c r=\"C3\"><v>1.38E10</v></c></row>\
</sheetData></worksheet>";
        let shared = vec!["姓名".to_string(), "电话".to_string(), "王五".to_string()];
        let rows = parse_sheet(sheet, &shared);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].cells, vec!["姓名", "电话"]);
        assert_eq!(rows[1].number, 3);
        assert_eq!(rows[1].cells, vec!["王五", "", "13800000000"]);
    }
}
