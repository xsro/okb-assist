//! 期刊别名表（`config.json` 的 `journal_aliases` 段）。
//!
//! 用途：把常见缩写（`TRO` / `TAC` / `RAL` …）映射到**一组期刊全名**，使
//! `SO=TRO`、`journal: "TRO"`、`?journal=TRO` 能一次命中这些期刊在库里的所有写法
//! （大小写不一致；旧刊名/另一套拼写各占一行）。
//!
//! 匹配语义（刻意保守，避免误命中）：
//! - 别名本身大小写不敏感，比较前做「去首尾空白 + 折叠连续空白 + 转小写」；
//! - 命中别名后按**期刊全名精确匹配**（`journal COLLATE NOCASE IN (…)`），**不做子串**；
//!   未命中别名时仍走原来的子串 `LIKE`，行为不变；
//! - 同时匹配 `journal` 与 `i18n.en.journal` 两列（与检索式 `SO=` 的文本列一致）；
//! - 别名表只影响**过滤**：不改库里的 `journal` 值，也不合并 `get_facets` 的分桶。
//!
//! 与 `services::doc_type` 同样的定位：**唯一实现**，新增判断/枚举一律走这里，
//! 不要在 `query_parser` / `mcp_server` / 前端各写一套。

use serde_json::Value;

/// 参与别名匹配的列（与 `query_parser::C_JOURNAL` 保持一致）。
pub const JOURNAL_COLUMNS: &[&str] = &["journal", "json_extract(i18n, '$.en.journal')"];

/// 别名最大长度（字符）
pub const MAX_ALIAS_LEN: usize = 64;
/// 单条期刊名最大长度（字符）
pub const MAX_JOURNAL_LEN: usize = 200;
/// 别名条数上限
pub const MAX_ALIASES: usize = 500;

/// 一条别名：`alias` → 多个期刊全名。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalAlias {
    pub alias: String,
    pub journals: Vec<String>,
}

/// 归一化比较键：去首尾空白 → 折叠连续空白 → 转小写。
///
/// 只用于**查找**（`resolve` / 去重），存库时保留用户输入的原始写法。
pub fn normalize_key(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()
}

/// 从配置值宽松解析（坏条目跳过，不报错）——读取路径用。
///
/// 兼容两种写法：`{"items": [...]}` 与直接给数组 `[...]`。
pub fn from_value(raw: &Value) -> Vec<JournalAlias> {
    let items = match raw {
        Value::Array(arr) => arr.as_slice(),
        Value::Object(_) => raw.get("items").and_then(|v| v.as_array()).map(|a| a.as_slice()).unwrap_or(&[]),
        _ => &[],
    };
    let mut out: Vec<JournalAlias> = Vec::new();
    for item in items {
        let alias = item.get("alias").and_then(|v| v.as_str()).unwrap_or("").trim();
        if alias.is_empty() || alias.chars().count() > MAX_ALIAS_LEN {
            continue;
        }
        let journals: Vec<String> = item
            .get("journals")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str())
                    .map(|s| s.split_whitespace().collect::<Vec<_>>().join(" "))
                    .filter(|s| !s.is_empty() && s.chars().count() <= MAX_JOURNAL_LEN)
                    .collect()
            })
            .unwrap_or_default();
        if journals.is_empty() {
            continue;
        }
        let key = normalize_key(alias);
        // 同名别名只保留第一条（后面的忽略），与 `validate` 的「重复」判定一致
        if out.iter().any(|a| normalize_key(&a.alias) == key) {
            continue;
        }
        out.push(JournalAlias { alias: alias.to_string(), journals });
    }
    out
}

/// 严格校验（`PUT /assist/api/config` 用）：返回给前端的错误说明。
pub fn validate(raw: &Value) -> Result<(), String> {
    let items = match raw {
        Value::Array(arr) => arr,
        Value::Object(_) => match raw.get("items") {
            Some(Value::Array(arr)) => arr,
            Some(_) => return Err("journal_aliases.items 必须是数组".to_string()),
            None => return Err("journal_aliases 缺少 items 数组".to_string()),
        },
        _ => return Err("journal_aliases 必须是对象（{items: [...]}）或数组".to_string()),
    };
    if items.len() > MAX_ALIASES {
        return Err(format!("期刊别名最多 {} 条，当前 {} 条", MAX_ALIASES, items.len()));
    }
    let mut seen: Vec<String> = Vec::new();
    for (i, item) in items.iter().enumerate() {
        let n = i + 1;
        let alias = item
            .get("alias")
            .and_then(|v| v.as_str())
            .ok_or_else(|| format!("第 {} 条缺少别名字段 alias", n))?;
        let alias = alias.trim();
        if alias.is_empty() {
            return Err(format!("第 {} 条的 alias 不能为空", n));
        }
        if alias.chars().count() > MAX_ALIAS_LEN {
            return Err(format!("第 {} 条的 alias 超过 {} 个字符", n, MAX_ALIAS_LEN));
        }
        let key = normalize_key(alias);
        if seen.contains(&key) {
            return Err(format!("别名 `{}` 重复（第 {} 条），别名不区分大小写", alias, n));
        }
        seen.push(key);
        let journals = item
            .get("journals")
            .and_then(|v| v.as_array())
            .ok_or_else(|| format!("第 {} 条（`{}`）缺少 journals 数组", n, alias))?;
        if journals.is_empty() {
            return Err(format!("第 {} 条（`{}`）至少要写一个期刊名", n, alias));
        }
        // 空行/纯空白按「手写配置的排版噪音」处理：跳过（normalized_value 会真的丢掉它们），
        // 但至少要留下一个有效期刊名 —— 那才是真正的配置错误。
        let mut valid_names = 0usize;
        for j in journals {
            let name = j
                .as_str()
                .ok_or_else(|| format!("第 {} 条（`{}`）的 journals 里有非字符串项", n, alias))?
                .trim();
            if name.is_empty() {
                continue;
            }
            if name.chars().count() > MAX_JOURNAL_LEN {
                return Err(format!(
                    "第 {} 条（`{}`）的期刊名超过 {} 个字符",
                    n, alias, MAX_JOURNAL_LEN
                ));
            }
            valid_names += 1;
        }
        if valid_names == 0 {
            return Err(format!("第 {} 条（`{}`）至少要写一个期刊名（不能只有空白）", n, alias));
        }
    }
    Ok(())
}

/// 校验通过后的规范化值（去空白、去重、丢掉空项），用于落盘。
pub fn normalized_value(raw: &Value) -> Value {
    let aliases = from_value(raw);
    let items: Vec<Value> = aliases
        .iter()
        .map(|a| {
            let mut seen: Vec<String> = Vec::new();
            let journals: Vec<Value> = a
                .journals
                .iter()
                .filter(|name| {
                    let k = normalize_key(name);
                    if seen.contains(&k) {
                        false
                    } else {
                        seen.push(k);
                        true
                    }
                })
                .map(|name| Value::String(name.clone()))
                .collect();
            serde_json::json!({ "alias": a.alias, "journals": journals })
        })
        .collect();
    serde_json::json!({ "version": 1, "items": items })
}

/// 查找别名（大小写/多余空白不敏感），命中返回该别名对应的期刊全名列表。
pub fn resolve<'a>(aliases: &'a [JournalAlias], input: &str) -> Option<&'a [String]> {
    let key = normalize_key(input);
    if key.is_empty() {
        return None;
    }
    aliases
        .iter()
        .find(|a| normalize_key(&a.alias) == key)
        .map(|a| a.journals.as_slice())
}

/// 生成「任一列精确命中」的 SQL 片段与绑定值（`?` 占位符由调用方补，按出现顺序绑定）。
///
/// 结果形如 `(journal COLLATE NOCASE IN (?,?) OR json_extract(i18n, '$.en.journal') COLLATE NOCASE IN (?,?))`，
/// 命中大小写不同的写法（`IEEE Transactions on Robotics` / `IEEE TRANSACTIONS ON ROBOTICS`）。
pub fn in_clause(columns: &[&str], names: &[String]) -> (String, Vec<String>) {
    let placeholders = vec!["?"; names.len()].join(",");
    let mut sql = String::from("(");
    for (i, col) in columns.iter().enumerate() {
        if i > 0 {
            sql.push_str(" OR ");
        }
        sql.push_str(&format!("{} COLLATE NOCASE IN ({})", col, placeholders));
    }
    sql.push(')');
    let binds = columns.iter().flat_map(|_| names.iter().cloned()).collect();
    (sql, binds)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample() -> Vec<JournalAlias> {
        from_value(&json!({
            "version": 1,
            "items": [
                { "alias": "TRO", "journals": ["IEEE Transactions on Robotics", "IEEE Transactions on Robotics and Automation"] },
                { "alias": "  tac ", "journals": ["IEEE Transactions on Automatic Control"] },
                { "alias": "broken", "journals": [] },
                { "alias": "TRO", "journals": ["重复的别名应被忽略"] }
            ]
        }))
    }

    #[test]
    fn parse_skips_invalid_and_duplicates() {
        let a = sample();
        assert_eq!(a.len(), 2, "空 journals 与重复别名应被跳过: {:?}", a);
        assert_eq!(a[1].alias, "tac", "别名保留用户写法，只 trim 首尾");
    }

    #[test]
    fn resolve_is_case_and_space_insensitive() {
        let a = sample();
        assert_eq!(resolve(&a, "tro").map(|v| v.len()), Some(2));
        assert_eq!(resolve(&a, "  TRO  ").map(|v| v.len()), Some(2));
        assert_eq!(resolve(&a, "TaC").map(|v| v.len()), Some(1));
        assert!(resolve(&a, "TRO2").is_none());
        assert!(resolve(&a, "").is_none());
    }

    #[test]
    fn resolve_multiple_journals_in_order() {
        let a = sample();
        let names = resolve(&a, "TRO").unwrap();
        assert_eq!(names[0], "IEEE Transactions on Robotics");
        assert_eq!(names[1], "IEEE Transactions on Robotics and Automation");
    }

    #[test]
    fn in_clause_binds_every_column() {
        let names = vec!["A".to_string(), "B".to_string()];
        let (sql, binds) = in_clause(JOURNAL_COLUMNS, &names);
        assert!(sql.contains("journal COLLATE NOCASE IN (?,?)"), "{sql}");
        assert!(sql.contains("json_extract(i18n, '$.en.journal') COLLATE NOCASE IN (?,?)"), "{sql}");
        // 绑定顺序与 `?` 出现顺序一致：第一列 A,B；第二列 A,B
        assert_eq!(binds, vec!["A", "B", "A", "B"]);
    }

    #[test]
    fn validate_rejects_bad_input() {
        assert!(validate(&json!({ "items": [] })).is_ok());
        assert!(validate(&json!({ "items": [{ "alias": "TRO", "journals": [] }] })).is_err());
        assert!(validate(&json!({ "items": [{ "alias": "", "journals": ["x"] }] })).is_err());
        let dup = json!({ "items": [
            { "alias": "TRO", "journals": ["x"] },
            { "alias": "tro", "journals": ["y"] }
        ]});
        assert!(validate(&dup).unwrap_err().contains("重复"));
        assert!(validate(&json!([])).is_ok(), "裸数组也接受");
    }

    #[test]
    fn validate_ignores_blank_journal_lines() {
        // 手写 JSON 里常见的空串/缩进不算错误
        assert!(validate(&json!({ "items": [
            { "alias": "TRO", "journals": [" IEEE Transactions on Robotics ", "", "  "] }
        ]})).is_ok());
        // 但全空白 / 只有一个空串仍然报错
        assert!(validate(&json!({ "items": [{ "alias": "TRO", "journals": ["", " "] }] })).is_err());
    }

    #[test]
    fn normalized_value_dedupes_journals() {
        let v = normalized_value(&json!({
            "items": [{ "alias": " TRO ", "journals": ["A", " a ", "", "B"] }]
        }));
        let names: Vec<&str> = v["items"][0]["journals"]
            .as_array()
            .unwrap()
            .iter()
            .map(|x| x.as_str().unwrap())
            .collect();
        assert_eq!(names, vec!["A", "B"]);
        assert_eq!(v["items"][0]["alias"], "TRO");
        assert_eq!(v["version"], 1);
    }
}
