use std::borrow::Cow;

use json_trait::typed_json::*;
use json_trait::{json, Array, BuildableJson, ForeignJson, ForeignMutableJson, Object};

use cc_traits::{Get, GetMut, MapInsert, PushBack, Remove};

use url::{ParseError, Url};

use crate::error::JsonLdError;
use crate::error::JsonLdErrorCode::InvalidLocalContext;
use crate::{JsonLdContext, JsonOrReference, OptionalContexts};

pub fn is_jsonld_keyword(value: &str) -> bool {
	value.starts_with('@')
		&& value.len() > 1
		&& match &value[1..] {
			"base" | "container" | "context" | "default" | "direction" | "embed" | "explicit" | "graph" | "id" | "included" | "index" | "json" | "language" | "list" | "nest"
			| "none" | "omitDefault" | "prefix" | "preserve" | "protected" | "requireAll" | "reverse" | "set" | "type" | "value" | "version" | "vocab" => true,
			_ => false
		}
}

pub fn looks_like_a_jsonld_keyword(value: &str) -> bool {
	value.starts_with('@') && value.len() > 1 && !value[1..].contains(|ch: char| !ch.is_ascii_alphabetic())
}

pub fn resolve(r: &str, base: Option<&Url>) -> Result<Url, ParseError> {
	Url::options().base_url(base).parse(r)
}

pub fn is_iri(value: &str) -> bool {
	Url::parse(value).is_ok()
}

pub fn as_compact_iri(value: &str) -> Option<(&str, &str)> {
	if value == "" {
		return None;
	}
	value[1..].find(":").map(|index| (&value[..(index + 1)], &value[(index + 2)..]))
}

#[test]
fn test_as_compact_iri() {
	assert_eq!(as_compact_iri("prefix:suffix"), Some(("prefix", "suffix")));
}

pub fn make_lang_dir<D: AsRef<str>>(language: Option<String>, direction: Option<D>) -> String {
	let language = language.map_or_else(|| "".to_string(), |language| language.to_ascii_lowercase());
	let direction = direction.as_ref().map_or("", |direction| direction.as_ref());
	match (language.as_str(), direction) {
		("", "") => language,
		(_, "") => language,
		("", "@none") => "@none".to_string(),
		(_, "@none") => language,
		("@null" | "@none", _) => "_".to_string() + direction,
		_ => language + "_" + &direction
	}
}

pub fn is_graph_object<T: ForeignJson>(value: &T::Object) -> bool {
	let mut non_optional_keys = value.iter().map(|(key, _)| key).filter(|key| key != &"@id" && key != &"@index");
	non_optional_keys.next().as_deref() == Some("@graph") && non_optional_keys.next() == None
}

pub fn add_value<T: ForeignMutableJson + BuildableJson>(object: &mut T::Object, key: &str, value: T, as_array: bool) {
	if as_array && object.get(key).map_or(true, |v| v.as_array().is_none()) {
		let mut array = T::empty_array();
		if let Some(original_value) = object.remove(key) {
			array.push_back(original_value);
		}
		object.insert(key.to_string(), array.into());
	}
	if value.as_array().is_some() {
		let value = value.into_array().unwrap();
		for v in value.into_iter() {
			add_value(object, key, v, false);
		}
	} else {
		if let Some(original_value) = object.get_mut(key) {
			if let Some(array) = original_value.as_array_mut() {
				array.push_back(value);
			} else {
				let original_value = object.remove(key).unwrap();
				object.insert(key.to_string(), json!(T, [original_value, value]));
			}
		} else {
			object.insert(key.to_string(), value);
		}
	}
}

pub trait ContextJson<'a, T: ForeignMutableJson + BuildableJson>: Sized {
	type Item;

	fn from_json(ctx: Cow<'a, T>) -> Result<Self, JsonLdError> {
		match ctx {
			Cow::Owned(ctx) => match ctx.into_enum() {
				Owned::Array(ctx) => Self::array(ctx.into_iter().map(|value| {
					Ok(match value.into_enum() {
						// Only one level of recursion, I think
						Owned::Object(ctx) => Self::item(JsonOrReference::JsonObject(Cow::Owned(ctx))),
						Owned::String(reference) => Self::item(JsonOrReference::Reference(Cow::Owned(reference))),
						Owned::Null => Self::null()?,
						_ => return Err(err!(InvalidLocalContext))
					})
				})),
				Owned::Object(ctx) => Ok(Self::unit(Self::item(JsonOrReference::JsonObject(Cow::Owned(ctx))))),
				Owned::String(reference) => Ok(Self::unit(Self::item(JsonOrReference::Reference(Cow::Owned(reference))))),
				Owned::Null => Ok(Self::unit(Self::null()?)),
				_ => Err(err!(InvalidLocalContext))
			},
			Cow::Borrowed(ctx) => match ctx.as_enum() {
				Borrowed::Array(ctx) => Self::array(ctx.iter().map(|value| {
					Ok(match value.as_enum() {
						// Only one level of recursion, I think
						Borrowed::Object(ctx) => Self::item(JsonOrReference::JsonObject(Cow::Borrowed(ctx))),
						Borrowed::String(reference) => Self::item(JsonOrReference::Reference(Cow::Borrowed(reference))),
						Borrowed::Null => Self::null()?,
						_ => return Err(err!(InvalidLocalContext))
					})
				})),
				Borrowed::Object(ctx) => Ok(Self::unit(Self::item(JsonOrReference::JsonObject(Cow::Borrowed(ctx))))),
				Borrowed::String(reference) => Ok(Self::unit(Self::item(JsonOrReference::Reference(Cow::Borrowed(reference))))),
				Borrowed::Null => Ok(Self::unit(Self::null()?)),
				_ => Err(err!(InvalidLocalContext))
			}
		}
	}

	fn array(items: impl IntoIterator<Item = Result<Self::Item, JsonLdError>>) -> Result<Self, JsonLdError>;
	fn unit(item: Self::Item) -> Self;
	fn item(item: JsonOrReference<'a, T>) -> Self::Item;
	fn null() -> Result<Self::Item, JsonLdError>;
}

impl<'a, T: ForeignMutableJson + BuildableJson> ContextJson<'a, T> for JsonLdContext<'a, T> {
	type Item = JsonOrReference<'a, T>;

	fn array(items: impl IntoIterator<Item = Result<Self::Item, JsonLdError>>) -> Result<Self, JsonLdError> {
		items.into_iter().collect()
	}

	fn unit(item: Self::Item) -> Self {
		vec![item]
	}

	fn item(item: JsonOrReference<'a, T>) -> Self::Item {
		item
	}

	fn null() -> Result<Self::Item, JsonLdError> {
		Err(err!(InvalidLocalContext))
	}
}

impl<'a, T: ForeignMutableJson + BuildableJson> ContextJson<'a, T> for OptionalContexts<'a, T> {
	type Item = Option<JsonOrReference<'a, T>>;

	fn array(items: impl IntoIterator<Item = Result<Self::Item, JsonLdError>>) -> Result<Self, JsonLdError> {
		items.into_iter().collect()
	}

	fn unit(item: Self::Item) -> Self {
		vec![item]
	}

	fn item(item: JsonOrReference<'a, T>) -> Self::Item {
		Some(item)
	}

	fn null() -> Result<Self::Item, JsonLdError> {
		Ok(None)
	}
}

pub enum OneError<T: Iterator<Item = I>, I, E> {
	Ok(T),
	Err(Option<E>)
}

impl<T: Iterator<Item = I>, I, E> OneError<T, I, E> {
	pub fn new<O>(value: Result<O, E>) -> Self
	where
		O: IntoIterator<IntoIter = T>
	{
		match value {
			Ok(o) => OneError::Ok(o.into_iter()),
			Err(e) => OneError::Err(Some(e))
		}
	}
}

impl<T: Iterator<Item = I>, I, E> Iterator for OneError<T, I, E> {
	type Item = Result<I, E>;

	fn next(&mut self) -> Option<Self::Item> {
		match self {
			OneError::Ok(iter) => Ok(iter.next()).transpose(),
			OneError::Err(e) => e.take().map(|e| Err(e))
		}
	}
}
