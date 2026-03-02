//! JSON value parsing utilities.

use std::collections::HashMap;

/// Convert a `serde_json::Value` to a string, skipping null values.
///
/// Returns `Some(s)` for non-null values, `None` for null.
/// Handles multiple JSON types: String, Number, Bool, and others.
pub fn json_value_to_string(val: &serde_json::Value) -> Option<String> {
    match val {
        serde_json::Value::Null => None,
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Number(n) => Some(n.to_string()),
        serde_json::Value::Bool(b) => Some(b.to_string()),
        other => Some(other.to_string()),
    }
}

/// Parse compact API rows (array-of-arrays format).
///
/// Expected format: `[[field1, field2, ...], [field1, field2, ...], ...]`
/// where the field names are provided separately.
pub fn parse_compact_rows(
    fields: &[String],
    data: &serde_json::Value,
) -> Vec<HashMap<String, String>> {
    let Some(arr) = data.as_array() else {
        return vec![];
    };
    arr.iter()
        .filter_map(|row| {
            let vals = row.as_array()?;
            let mut map = HashMap::new();
            for (i, field) in fields.iter().enumerate() {
                if let Some(val) = vals.get(i) {
                    if let Some(s) = json_value_to_string(val) {
                        map.insert(field.clone(), s);
                    }
                }
            }
            Some(map)
        })
        .collect()
}

/// Parse standard API rows (array-of-objects format).
///
/// Expected format:
/// - `[{"field1": value1, "field2": value2, ...}, ...]`
/// - OR `[{"attributes": {"field1": value1, ...}}, ...]`
pub fn parse_standard_rows(data: &serde_json::Value) -> Vec<HashMap<String, String>> {
    let Some(arr) = data.as_array() else {
        return vec![];
    };
    arr.iter()
        .filter_map(|row| {
            let obj = row
                .as_object()
                .and_then(|o| o.get("attributes"))
                .and_then(|a| a.as_object())
                .or_else(|| row.as_object())?;
            let mut map = HashMap::new();
            for (k, v) in obj {
                if let Some(s) = json_value_to_string(v) {
                    map.insert(k.clone(), s);
                }
            }
            Some(map)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_json_value_to_string_string() {
        let val = serde_json::json!("hello");
        assert_eq!(json_value_to_string(&val), Some("hello".to_string()));
    }

    #[test]
    fn test_json_value_to_string_null() {
        let val = serde_json::json!(null);
        assert_eq!(json_value_to_string(&val), None);
    }

    #[test]
    fn test_parse_compact_rows_basic() {
        let fields = vec!["strike".to_string(), "bid".to_string()];
        let data = serde_json::json!([["100.0", "1.50"], ["105.0", "2.00"]]);
        let rows = parse_compact_rows(&fields, &data);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].get("strike"), Some(&"100.0".to_string()));
        assert_eq!(rows[0].get("bid"), Some(&"1.50".to_string()));
    }

    #[test]
    fn test_parse_standard_rows_basic() {
        let data = serde_json::json!([
            {"attributes": {"strike": "100.0", "bid": "1.50"}},
            {"attributes": {"strike": "105.0", "bid": "2.00"}}
        ]);
        let rows = parse_standard_rows(&data);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].get("strike"), Some(&"100.0".to_string()));
    }
}
