use json_trait::{ForeignMutableJson, TypedJson, BuildableJson};

use crate::Context;
use crate::error::Result;

pub fn compact_internal<'a, T>(active_context: Context<'a, T>, active_property: Option<&'a str>, element: TypedJson<'a, T>,
		compact_arrays: bool, ordered: bool) -> Result<T> where
	T: ForeignMutableJson + BuildableJson,
	T::Object: Clone
{
	todo!()
}