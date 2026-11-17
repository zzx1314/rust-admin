//! 责任人库的 Excel 导入：模板生成 + 上传解析。
//!
//! 模板由平台发（`GET /api/staff/import-template`），列固定为
//! 姓名 / 工号 / 角色 / 班组 / 联系电话 / 在岗状态 / 技能，末尾附一张「填写说明」。
//! 导入时**按表头认列**（顺序可以调、多余的列忽略），只读第一张工作表。
//!
//! 这一层只做「文件 → 结构化行 + 行级错误」，落库（查重、编号分配）在
//! `handlers::schedules::import_staff`，因为要查数据库。

use crate::xlsx::{self, Sheet};

/// 下载时的文件名（中文名靠 `filename*=UTF-8''...` 传到浏览器）
pub const TEMPLATE_FILENAME: &str = "责任人导入模板.xlsx";

/// 一次最多导入的行数：再多请拆文件（也是防误传大表的护栏）
pub const MAX_ROWS: usize = 500;

/// 上传文件大小上限（模板 + 每人一行，2MB 足够）
pub const MAX_FILE_BYTES: usize = 4 * 1024 * 1024;

/// 在岗状态的可选值，与前端 `STAFF_STATUSES` 一致
pub const STAFF_STATUSES: [&str; 3] = ["在岗", "休息", "请假"];

/// 模板里的数据列（顺序即模板列顺序）
const COLUMNS: [&str; 7] = [
    "姓名",
    "工号",
    "角色",
    "班组",
    "联系电话",
    "在岗状态",
    "技能",
];

/// 一条通过校验、可以落库的责任人
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParsedStaff {
    pub name: String,
    pub staff_code: Option<String>,
    pub role: Option<String>,
    pub team: Option<String>,
    pub phone: Option<String>,
    pub status: Option<String>,
    pub skills: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedRow {
    /// Excel 行号（1 开始），回显 / 排错都按它
    pub row: usize,
    pub staff: ParsedStaff,
}

/// 文件整体可用、但这一行填得不对（或重复）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RejectedRow {
    pub row: usize,
    pub name: String,
    pub message: String,
}

#[derive(Debug, Default)]
pub struct ParseOutcome {
    pub rows: Vec<ParsedRow>,
    pub rejected: Vec<RejectedRow>,
}

impl ParseOutcome {
    pub fn total(&self) -> usize {
        self.rows.len() + self.rejected.len()
    }
}

/// 生成导入模板（xlsx 字节）。
pub fn template() -> Result<Vec<u8>, String> {
    let data = Sheet {
        name: "责任人".to_string(),
        headers: COLUMNS.iter().map(|value| value.to_string()).collect(),
        rows: Vec::new(),
        widths: vec![14.0, 14.0, 16.0, 14.0, 18.0, 12.0, 32.0],
    };
    let notes = Sheet {
        name: "填写说明".to_string(),
        headers: vec![
            "列名".to_string(),
            "是否必填".to_string(),
            "填写说明".to_string(),
        ],
        rows: vec![
            note(
                "姓名",
                "必填",
                "责任人姓名。与库里已有姓名重复、或同一份文件里重名的行会跳过。",
            ),
            note(
                "工号",
                "选填",
                "留空时自动生成 EMP-01 这类编号；与库里已有工号重复的行会跳过。",
            ),
            note("角色", "选填", "如 运维工程师 / 巡检主管。"),
            note("班组", "选填", "如 运维一组。"),
            note(
                "联系电话",
                "选填",
                "建议保持文本格式（前面加 ' 或先把列设为文本），否则 Excel 会吃掉前导 0。",
            ),
            note(
                "在岗状态",
                "选填",
                "只能填 在岗 / 休息 / 请假，留空按 在岗 处理。",
            ),
            note("技能", "选填", "多项用 / 分隔，如 红外测温 / 应急处置。"),
            note(
                "导入规则",
                "—",
                "只读第一张工作表；表头按名称识别，列顺序可以调整；空行自动忽略，一次最多 500 行。",
            ),
        ],
        widths: vec![14.0, 12.0, 70.0],
    };
    xlsx::write_workbook(&[data, notes])
}

fn note(column: &str, required: &str, text: &str) -> Vec<String> {
    vec![column.to_string(), required.to_string(), text.to_string()]
}

/// 表头能认出来的字段
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Field {
    Name,
    StaffCode,
    Role,
    Team,
    Phone,
    Status,
    Skills,
}

/// 表头别名：用户从旧表复制过来、或者改了列名大小写都能认。
fn field_of(header: &str) -> Option<Field> {
    let key: String = header
        .chars()
        .filter(|character| !character.is_whitespace() && !matches!(character, '*' | '＊'))
        .collect::<String>()
        .to_lowercase();
    let field = match key.as_str() {
        "姓名" | "责任人" | "责任人姓名" | "负责人" | "名字" | "name" | "人员姓名" => {
            Field::Name
        }
        "工号" | "编号" | "人员编号" | "员工编号" | "员工号" | "code" | "staffcode"
        | "staff_code" => Field::StaffCode,
        "角色" | "岗位" | "职务" | "role" => Field::Role,
        "班组" | "团队" | "小组" | "组别" | "team" => Field::Team,
        "联系电话" | "电话" | "手机" | "手机号" | "联系方式" | "phone" | "mobile" | "tel" => {
            Field::Phone
        }
        "状态" | "在岗状态" | "status" => Field::Status,
        "技能" | "技能标签" | "特长" | "skills" | "skill" => Field::Skills,
        _ => return None,
    };
    Some(field)
}

/// 解析上传的 xlsx。`Err` 表示整份文件不可用（不是 xlsx / 没有表头 / 超行数）。
pub fn parse_workbook(bytes: &[u8]) -> Result<ParseOutcome, String> {
    let rows = xlsx::read_first_sheet(bytes)?;

    // 表头行：前 10 行里第一行带「姓名」列的
    let mut header: Option<(usize, Vec<(usize, Field)>)> = None;
    for (index, row) in rows.iter().enumerate().take(10) {
        let cells: Vec<(usize, Field)> = row
            .cells
            .iter()
            .enumerate()
            .filter_map(|(column, value)| field_of(value).map(|field| (column, field)))
            .collect();
        if cells.iter().any(|(_, field)| *field == Field::Name) {
            header = Some((index, cells));
            break;
        }
    }
    let Some((header_index, columns)) = header else {
        return Err(
            "没找到表头（第一张工作表前 10 行里要有「姓名」列）。请下载最新模板后按列填写"
                .to_string(),
        );
    };

    let mut outcome = ParseOutcome::default();
    for row in rows.iter().skip(header_index + 1) {
        let value = |field: Field| -> Option<String> {
            columns
                .iter()
                .find(|(_, candidate)| *candidate == field)
                .and_then(|(column, _)| row.cells.get(*column))
                .map(|text| text.trim().to_string())
                .filter(|text| !text.is_empty())
        };
        // 整行都空：用户在模板末尾多按了回车，直接忽略
        if row.cells.iter().all(|cell| cell.trim().is_empty()) {
            continue;
        }
        let name = value(Field::Name).unwrap_or_default();
        let staff = ParsedStaff {
            name,
            staff_code: value(Field::StaffCode),
            role: value(Field::Role),
            team: value(Field::Team),
            phone: value(Field::Phone),
            status: value(Field::Status),
            skills: value(Field::Skills),
        };
        match validate(&staff) {
            Ok(()) => outcome.rows.push(ParsedRow {
                row: row.number,
                staff,
            }),
            Err(message) => outcome.rejected.push(RejectedRow {
                row: row.number,
                name: staff.name.clone(),
                message,
            }),
        }
    }

    if outcome.total() > MAX_ROWS {
        return Err(format!(
            "一次最多导入 {MAX_ROWS} 行，当前 {} 行，请拆成多个文件",
            outcome.total()
        ));
    }
    Ok(outcome)
}

fn validate(staff: &ParsedStaff) -> Result<(), String> {
    if staff.name.is_empty() {
        return Err("姓名为空".to_string());
    }
    if staff.name.chars().count() > 32 {
        return Err("姓名超过 32 个字".to_string());
    }
    if let Some(code) = &staff.staff_code
        && code.chars().count() > 32
    {
        return Err("工号超过 32 个字符".to_string());
    }
    if let Some(status) = &staff.status
        && !STAFF_STATUSES.contains(&status.as_str())
    {
        return Err(format!(
            "在岗状态「{status}」不认识，只能填 {}",
            STAFF_STATUSES.join(" / ")
        ));
    }
    let limits = [
        (staff.role.as_deref(), "角色"),
        (staff.team.as_deref(), "班组"),
        (staff.phone.as_deref(), "联系电话"),
        (staff.skills.as_deref(), "技能"),
    ];
    for (value, label) in limits {
        if let Some(value) = value
            && value.chars().count() > 64
        {
            return Err(format!("{label}超过 64 个字符"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::xlsx::{self, Sheet};

    fn workbook(rows: Vec<Vec<&str>>) -> Vec<u8> {
        let sheet = Sheet {
            name: "责任人".to_string(),
            headers: COLUMNS.iter().map(|value| value.to_string()).collect(),
            rows: rows
                .into_iter()
                .map(|row| row.into_iter().map(str::to_string).collect())
                .collect(),
            widths: vec![],
        };
        xlsx::write_workbook(&[sheet]).expect("写测试 xlsx 失败")
    }

    #[test]
    fn 模板能读回表头且没有数据行() {
        let bytes = template().expect("生成模板失败");
        let rows = xlsx::read_first_sheet(&bytes).expect("读模板失败");
        assert_eq!(rows[0].cells, COLUMNS.to_vec());
        assert_eq!(rows.len(), 1, "模板不应带示例数据行，避免被误导入");
    }

    #[test]
    fn 解析正常行并忽略空行() {
        let bytes = workbook(vec![
            vec![
                "张三",
                "EMP-01",
                "运维工程师",
                "运维一组",
                "13800000000",
                "在岗",
                "红外测温",
            ],
            vec!["", "", "", "", "", "", ""],
            vec!["李四", "", "巡检主管", "", "", "", ""],
        ]);
        let outcome = parse_workbook(&bytes).expect("解析失败");
        assert_eq!(outcome.total(), 2);
        assert_eq!(outcome.rows[0].row, 2);
        assert_eq!(outcome.rows[0].staff.name, "张三");
        assert_eq!(outcome.rows[0].staff.staff_code.as_deref(), Some("EMP-01"));
        assert_eq!(outcome.rows[1].row, 4, "空行被跳过，行号按 Excel 原样保留");
        assert_eq!(outcome.rows[1].staff.status, None);
    }

    #[test]
    fn 列顺序可以调整且多余列忽略() {
        let sheet = Sheet {
            name: "责任人".to_string(),
            headers: vec![
                "班组".to_string(),
                "姓名 *".to_string(),
                "备注".to_string(),
                "电话".to_string(),
            ],
            rows: vec![vec![
                "运维二组".to_string(),
                "王五".to_string(),
                "无关列".to_string(),
                "13900000000".to_string(),
            ]],
            widths: vec![],
        };
        let bytes = xlsx::write_workbook(&[sheet]).expect("写测试 xlsx 失败");
        let outcome = parse_workbook(&bytes).expect("解析失败");
        assert_eq!(outcome.rows[0].staff.name, "王五");
        assert_eq!(outcome.rows[0].staff.team.as_deref(), Some("运维二组"));
        assert_eq!(outcome.rows[0].staff.phone.as_deref(), Some("13900000000"));
    }

    #[test]
    fn 非法行只影响自己() {
        let bytes = workbook(vec![
            vec!["", "", "", "", "", "在岗", ""],
            vec!["张三", "", "", "", "", "值班中", ""],
            vec!["李四", "", "", "", "", "休息", ""],
        ]);
        let outcome = parse_workbook(&bytes).expect("解析失败");
        assert_eq!(outcome.rows.len(), 1);
        assert_eq!(outcome.rows[0].staff.name, "李四");
        assert_eq!(outcome.rejected.len(), 2);
        assert_eq!(outcome.rejected[0].message, "姓名为空");
        assert!(outcome.rejected[1].message.contains("在岗状态"));
    }

    /// 真·Excel 形状的文件（openpyxl 生成后提交在 tests/fixtures）：
    /// 文本走 inlineStr + 十进制实体转义、数字单元格 `t="n"`、空行是 `<row r="4"></row>`。
    #[test]
    fn 能解析excel生成的xlsx() {
        let bytes = include_bytes!("../tests/fixtures/staff-import-openpyxl.xlsx");
        let outcome = parse_workbook(bytes).expect("解析 Excel 生成的 xlsx 失败");
        assert_eq!(outcome.total(), 3, "空行应被忽略");
        assert!(outcome.rejected.is_empty(), "{:?}", outcome.rejected);
        assert_eq!(outcome.rows[0].staff.name, "张三");
        assert_eq!(outcome.rows[0].staff.staff_code.as_deref(), Some("EMP-101"));
        assert_eq!(outcome.rows[0].staff.phone.as_deref(), Some("13800000000"));
        assert_eq!(outcome.rows[1].staff.name, "李四");
        assert_eq!(outcome.rows[1].staff.phone.as_deref(), Some("13900000000"));
        assert_eq!(outcome.rows[1].staff.role.as_deref(), Some("巡检主管"));
        assert_eq!(outcome.rows[2].staff.name, "王五 & <测试>");
        assert_eq!(outcome.rows[2].staff.skills.as_deref(), Some("充电维护"));
    }

    #[test]
    fn 没有姓名列时整份文件报错() {
        let sheet = Sheet {
            name: "Sheet1".to_string(),
            headers: vec!["备注".to_string()],
            rows: vec![vec!["随便写".to_string()]],
            widths: vec![],
        };
        let bytes = xlsx::write_workbook(&[sheet]).expect("写测试 xlsx 失败");
        let error = parse_workbook(&bytes).expect_err("应报缺少表头");
        assert!(error.contains("没找到表头"), "{error}");
    }
}
