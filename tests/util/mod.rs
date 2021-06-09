pub mod type_state;
pub mod record;

use serde_json::Value;

pub fn json_ld_eq(a: &Value, b: &Value, ordered: bool) -> bool {
	match a {
		Value::Object(a) => b.as_object().map_or(false, |b| if ordered {
			a.len() == b.len() && a.iter().zip(b.iter()).all(|((k, a), (l, b))| k == l && json_ld_eq(a, b, true))
		} else {
			a.len() == b.len() && a.iter().all(|(key, a)| b.get(key).map_or(false, |b| {
				if key == "@list" {
					if let Value::Array(a) = a {
						b.as_array().map_or(false, |b| {
							a.len() == b.len() && a.iter().zip(b.iter()).all(|(a, b)| json_ld_eq(a, b, false))
						})
					} else {
						json_ld_eq(a, b, false)
					}
				} else {
					json_ld_eq(a, b, false)
				}
			}))
		}),
		Value::Array(a) => b.as_array().map_or(false, |b| if ordered {
			a.len() == b.len() && a.iter().zip(b.iter()).all(|(a, b)| json_ld_eq(a, b, true))
		} else {
			a.len() == b.len() && a.iter().all(|a| b.iter().any(|b| json_ld_eq(a, b, false)))
		}),
		Value::Number(a) => b.as_f64().zip(a.as_f64()).map_or(false, |(a, b)| a == b),
		Value::String(a) => b.as_str().map_or(false, |b| *a == b),
		Value::Null => b.is_null(),
		Value::Bool(a) => b.as_bool().map_or(false, |b| *a == b)
	}
}