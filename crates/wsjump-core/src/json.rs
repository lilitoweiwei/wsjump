//! Minimal JSON encoding for machine-readable command output.
//!
//! The CLI writes JSON by hand instead of depending on a serializer: the two
//! documents it emits are fixed shapes, and the only nontrivial part is string
//! escaping, which lives here so both shapes share one implementation.

/// Encode one string as a JSON string literal, quotes included.
pub fn quote(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            ch if (ch as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", ch as u32)),
            ch => out.push(ch),
        }
    }
    out.push('"');
    out
}

/// Encode a string list as a JSON array, preserving order.
pub fn array(items: &[String]) -> String {
    let parts: Vec<String> = items.iter().map(|item| quote(item)).collect();
    format!("[{}]", parts.join(","))
}

/// Encode an optional count as a JSON number, or `null` when absent.
pub fn optional_number(value: Option<i64>) -> String {
    match value {
        Some(number) => number.to_string(),
        None => "null".to_string(),
    }
}

/// Encode a workspace list as a JSON array of row objects.
///
/// Each row carries the root-relative `key`, the complete directory `name`, the
/// absolute `path`, the effective `tags`, the description, and the recorded
/// access time in Unix seconds (`null` when the index is unavailable).
pub fn workspaces(items: &[crate::workspace::Workspace]) -> String {
    let rows: Vec<String> = items
        .iter()
        .map(|ws| {
            format!(
                "{{\"key\":{},\"name\":{},\"path\":{},\"tags\":{},\"desc\":{},\"last_access\":{}}}",
                quote(&ws.key),
                quote(&ws.name),
                quote(&ws.path.display().to_string()),
                array(&ws.tags),
                quote(&ws.desc),
                optional_number(ws.last_access)
            )
        })
        .collect();
    format!("[{}]", rows.join(","))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_passes_through() {
        assert_eq!(quote("工作区 ai"), "\"工作区 ai\"");
    }

    #[test]
    fn quotes_backslashes_and_controls_are_escaped() {
        assert_eq!(quote("a\"b"), "\"a\\\"b\"");
        assert_eq!(quote("a\\b"), "\"a\\\\b\"");
        assert_eq!(quote("a\nb\tc"), "\"a\\nb\\tc\"");
        assert_eq!(quote("\u{1}"), "\"\\u0001\"");
    }

    #[test]
    fn arrays_keep_order_and_handle_empty() {
        assert_eq!(array(&[]), "[]");
        assert_eq!(array(&["b".to_string(), "a".to_string()]), "[\"b\",\"a\"]");
    }

    #[test]
    fn optional_numbers_render_null_when_absent() {
        assert_eq!(optional_number(Some(7)), "7");
        assert_eq!(optional_number(None), "null");
    }

    #[test]
    fn workspace_rows_carry_every_field() {
        let workspace = crate::workspace::Workspace {
            key: "c/_ai/demo".to_string(),
            name: "demo".to_string(),
            path: std::path::PathBuf::from("/ws/c/_ai/demo"),
            tags: vec!["ai".to_string(), "工具".to_string()],
            desc: "说明 \"引号\"".to_string(),
            last_access: Some(1_700_000_000),
        };
        assert_eq!(
            workspaces(&[workspace]),
            "[{\"key\":\"c/_ai/demo\",\"name\":\"demo\",\"path\":\"/ws/c/_ai/demo\",\
             \"tags\":[\"ai\",\"工具\"],\"desc\":\"说明 \\\"引号\\\"\",\
             \"last_access\":1700000000}]"
        );
        assert_eq!(workspaces(&[]), "[]");
    }
}
