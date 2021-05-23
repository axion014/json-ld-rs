use std::borrow::{Borrow, Cow};

use json_trait::{ForeignJson, ForeignMutableJson, BuildableJson, Object, Array, typed_json::*};

use cc_traits::{Get, GetMut, MapInsert, PushBack, Remove};

use url::{Url, ParseError};

use crate::{JsonLdContext, JsonOrReference};
use crate::error::{JsonLdError, JsonLdErrorCode::InvalidContextEntry};

pub fn is_jsonld_keyword(value: &str) -> bool {
	value.starts_with('@') && value.len() > 1 && match &value[1..] {
		"base" | "container" | "context" | "default" | "direction" | "embed" | "explicit" | "graph" | "id" |
			"included" | "index" | "json" | "language" | "list" | "nest" | "none" | "omitDefault" | "prefix" |
			"preserve" | "protected" | "requireAll" | "reverse" | "set" | "type" | "value" | "version" | "vocab" => true,
		_ => false
	}
}

pub fn looks_like_a_jsonld_keyword(value: &str) -> bool {
	value.starts_with('@') && value.len() > 1 && !value[1..].contains(|ch: char| ch.is_ascii_alphabetic())
}

pub fn resolve(r: &str, base: Option<&Url>) -> Result<Url, ParseError> {
	Url::options().base_url(base).parse(r)
}

pub fn resolve_with_str<T: Borrow<str>>(r: &str, base: Option<T>) -> Result<Url, ParseError> {
	resolve(r, base.map(|base| Url::parse(base.borrow())).transpose()?.as_ref())
}

pub fn is_iri(value: &str) -> bool {
	Url::parse(value).is_ok()
}

pub fn as_compact_iri(value: &str) -> Option<(&str, &str)> {
	value[1..].find(":").map(|index| (&value[..index], &value[(index + 1)..]))
}

pub fn make_lang_dir<D: AsRef<str>>(language: Option<String>, direction: Option<D>) -> String {
	let language = language.map(|language| language.to_ascii_lowercase()).unwrap_or_else(|| "".to_string());
	let direction = direction.as_ref().map(|direction| direction.as_ref()).unwrap_or("");
	if direction == "@none" && language != "" {
		language
	} else if (language == "@null"|| language == "@none") && direction != "" {
		direction.to_string()
	} else {
		language + &direction
	}
}

pub fn is_graph_object<T: ForeignJson>(value: &T::Object) -> bool {
	value.contains("@graph") && value.iter().filter(|(key, _)| key != &"@id" && key != &"@index").count() == 1
}

pub fn add_value<T: ForeignMutableJson + BuildableJson>(object: &mut T::Object, key: &str, value: T, as_array: bool) {
	if as_array && object.get(key).map_or(true, |v| v.as_array().is_none()) {
		object.insert(key.to_string(), T::empty_array().into());
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
				let mut array = T::empty_array();
				array.push_back(object.remove(key).unwrap());
				array.push_back(value);
				object.insert(key.to_string(), array.into());
			}
		} else {
			object.insert(key.to_string(), value);
		}
	}
}

pub fn map_context<'a, T: ForeignMutableJson + BuildableJson>(ctx: Cow<'a, T>) -> Result<JsonLdContext<'a, T>, JsonLdError> {
	match ctx {
		Cow::Owned(ctx) => match ctx.into_enum() {
			Owned::Array(ctx) => ctx.into_iter().map(|value| Ok(match value.into_enum() {
				// Only one level of recursion, I think
				Owned::Object(obj) => Some(JsonOrReference::JsonObject(Cow::Owned(obj))),
				Owned::String(reference) => Some(JsonOrReference::Reference(Cow::Owned(reference))),
				Owned::Null => None,
				_ => return Err(err!(InvalidContextEntry))
			})).collect::<Result<JsonLdContext<'a, T>, JsonLdError>>(),
			Owned::Object(ctx) => Ok(vec![Some(JsonOrReference::JsonObject(Cow::Owned(ctx)))]),
			Owned::String(reference) => Ok(vec![Some(JsonOrReference::Reference(Cow::Owned(reference)))]),
			Owned::Null => Ok(vec![None]),
			_ => Err(err!(InvalidContextEntry))
		},
		Cow::Borrowed(ctx) => match ctx.as_enum() {
			Borrowed::Array(ctx) => ctx.iter().map(|value| Ok(match value.as_enum() {
				// Only one level of recursion, I think
				Borrowed::Object(obj) => Some(JsonOrReference::JsonObject(Cow::Borrowed(obj))),
				Borrowed::String(reference) => Some(JsonOrReference::Reference(Cow::Borrowed(reference))),
				Borrowed::Null => None,
				_ => return Err(err!(InvalidContextEntry))
			})).collect::<Result<JsonLdContext<'a, T>, JsonLdError>>(),
			Borrowed::Object(ctx) => Ok(vec![Some(JsonOrReference::JsonObject(Cow::Borrowed(ctx)))]),
			Borrowed::String(reference) => Ok(vec![Some(JsonOrReference::Reference(Cow::Borrowed(reference)))]),
			Borrowed::Null => Ok(vec![None]),
			_ => Err(err!(InvalidContextEntry))
		}
	}
}

pub enum OneError<T: Iterator<Item = I>, I, E> {
	Ok(T),
	Err(Option<E>)
}

impl <T: Iterator<Item = I>, I, E> OneError<T, I, E> {
	pub fn new<O>(value: Result<O, E>) -> Self where O: IntoIterator<IntoIter = T> {
		match value {
			Ok(o) => OneError::Ok(o.into_iter()),
			Err(e) => OneError::Err(Some(e))
		}
	}
}

impl <T: Iterator<Item = I>, I, E> Iterator for OneError<T, I, E> {
	type Item = Result<I, E>;

	fn next(&mut self) -> Option<Self::Item> {
		match self {
			OneError::Ok(iter) => Ok(iter.next()).transpose(),
			OneError::Err(e) => e.take().map(|e| Err(e))
		}
	}
}