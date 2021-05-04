use json_trait::{ForeignMutableJson, TypedJson, BuildableJson};

use crate::Context;
use crate::error::Result;

pub fn compact_internal<'a: 'b, 'b, T>(active_context: Context<'a, T>, active_property: Option<&'b str>, element: TypedJson<'a, 'b, T>,
		compact_arrays: bool, ordered: bool) -> Result<T> where
	T: ForeignMutableJson<'a> + BuildableJson<'a>,
	T::Object: Clone
{
	todo!()
}