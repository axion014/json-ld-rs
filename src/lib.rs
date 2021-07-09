#![feature(try_find)]

use std::borrow::{Borrow, Cow};
use std::collections::{BTreeMap, HashMap};

use futures::future::BoxFuture;

use elsa::{FrozenMap, FrozenSet};
use once_cell::unsync::OnceCell;

use cc_traits::{Get, Len, MapInsert, PushBack, Remove};
use json_trait::typed_json::*;
use json_trait::{json, BuildableJson, ForeignJson, ForeignMutableJson};

use maybe_owned::MaybeOwned;

use url::Url;

#[macro_use]
mod macros;

mod compact;
mod container;
mod context;
pub mod error;
mod expand;
pub mod remote;
mod util;

use crate::compact::{compact_internal, compact_iri};
use crate::container::Container;
use crate::context::process_context;
use crate::error::JsonLdErrorCode::*;
use crate::error::Result;
use crate::expand::{expand_internal, expand_object};
use crate::remote::LoadDocumentOptions;
use crate::util::ContextJson;

#[derive(Clone, Eq, PartialEq, Debug)]
pub enum JsonOrReference<'a, T: ForeignMutableJson + BuildableJson> {
	JsonObject(Cow<'a, T::Object>),
	Reference(Cow<'a, str>)
}

#[derive(Clone, Debug)]
pub enum Document<T: ForeignMutableJson + BuildableJson> {
	ParsedJson(T),
	RawJson(String)
}

impl<T: ForeignMutableJson + BuildableJson> Document<T> {
	pub fn into_parsed(self) -> std::result::Result<T, T::Err> {
		match self {
			Document::ParsedJson(v) => Ok(v),
			Document::RawJson(v) => T::from_str(&v)
		}
	}

	pub fn to_parsed(&self) -> std::result::Result<T, T::Err> {
		match self {
			Document::ParsedJson(v) => Ok(v.clone()),
			Document::RawJson(v) => T::from_str(v)
		}
	}
}

#[derive(Clone, Debug)]
pub struct RemoteDocument<T: ForeignMutableJson + BuildableJson> {
	pub content_type: String,
	pub context_url: Option<String>,
	pub document: Document<T>,
	pub document_url: String,
	pub profile: Option<String>
}

#[derive(Clone, Debug)]
pub enum JsonLdInput<'a, T: ForeignMutableJson + BuildableJson> {
	JsonObject(&'a T::Object),
	Reference(&'a str),
	RemoteDocument(&'a RemoteDocument<T>)
}

type JsonLdContext<'a, T> = Vec<JsonOrReference<'a, T>>;
type OptionalContexts<'a, T> = Vec<Option<JsonOrReference<'a, T>>>;

#[derive(Clone, Eq, PartialEq, Debug)]
pub enum Direction {
	LTR,
	RTL,
	None
}

impl AsRef<str> for Direction {
	fn as_ref(&self) -> &str {
		match self {
			Direction::LTR => "ltr",
			Direction::RTL => "rtl",
			Direction::None => "@none"
		}
	}
}

#[derive(Clone, Debug)]
struct TermDefinition<'a, T>
where
	T: ForeignMutableJson + BuildableJson,
	T::Object: Clone
{
	iri: Option<String>,
	prefix: bool,
	protected: bool,
	reverse_property: bool,
	base_url: Option<Url>,
	context: OptionalContexts<'a, T>,
	container_mapping: Container,
	direction_mapping: Option<Direction>,
	index_mapping: Option<String>,
	language_mapping: Option<Option<String>>,
	nest_value: Option<String>,
	type_mapping: Option<String>
}

#[derive(Clone, PartialEq, Eq, Ord, Debug)]
struct TermDefinitionKey(String);

impl PartialOrd for TermDefinitionKey {
	fn partial_cmp(&self, rhs: &Self) -> Option<std::cmp::Ordering> {
		let cmp_len = self.0.len().cmp(&rhs.0.len());
		Some(if cmp_len == std::cmp::Ordering::Equal { self.0.cmp(&rhs.0) } else { cmp_len })
	}
}

impl From<String> for TermDefinitionKey {
	fn from(value: String) -> Self {
		TermDefinitionKey(value)
	}
}

impl Borrow<str> for TermDefinitionKey {
	fn borrow(&self) -> &str {
		self.0.as_str()
	}
}

type InverseContext = HashMap<String, HashMap<Container, HashMap<String, HashMap<String, String>>>>;

#[derive(Clone, Debug)]
pub struct Context<'a, T>
where
	T: ForeignMutableJson + BuildableJson,
	T::Object: Clone
{
	term_definitions: BTreeMap<TermDefinitionKey, TermDefinition<'a, T>>,
	base_iri: Option<Url>,
	original_base_url: Option<Url>,
	inverse_context: OnceCell<InverseContext>,
	vocabulary_mapping: Option<String>,
	default_language: Option<String>,
	default_base_direction: Option<Direction>,
	previous_context: Option<Box<Context<'a, T>>>
}

impl<T> Default for Context<'_, T>
where
	T: ForeignMutableJson + BuildableJson,
	T::Object: Clone
{
	fn default() -> Self {
		Context {
			term_definitions: BTreeMap::<TermDefinitionKey, TermDefinition<T>>::new(),
			base_iri: None,
			original_base_url: None,
			inverse_context: OnceCell::new(),
			vocabulary_mapping: None,
			default_language: None,
			default_base_direction: None,
			previous_context: None
		}
	}
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum JsonLdProcessingMode {
	JsonLd1_1,
	JsonLd1_0
}

#[derive(Clone, Debug)]
pub struct JsonLdOptions<'a, T, F = for<'b> fn(&'b str, &'b Option<LoadDocumentOptions>) -> BoxFuture<'b, Result<RemoteDocument<T>>>>
where
	T: ForeignMutableJson + BuildableJson,
	F: for<'b> Fn(&'b str, &'b Option<LoadDocumentOptions>) -> BoxFuture<'b, Result<RemoteDocument<T>>>
{
	pub base: Option<String>,

	pub compact_arrays: bool,
	pub compact_to_relative: bool,

	pub document_loader: Option<F>,
	pub expand_context: Option<JsonOrReference<'a, T>>,
	pub extract_all_scripts: bool,
	pub frame_expansion: bool,
	pub ordered: bool,
	pub processing_mode: JsonLdProcessingMode,
	pub produce_generalized_rdf: bool,
	pub rdf_direction: Option<String>,
	pub use_native_types: bool,
	pub use_rdf_type: bool
}

impl<'a, T, F> Default for JsonLdOptions<'a, T, F>
where
	T: ForeignMutableJson + BuildableJson,
	F: for<'b> Fn(&'b str, &'b Option<LoadDocumentOptions>) -> BoxFuture<'b, Result<RemoteDocument<T>>>
{
	fn default() -> Self {
		JsonLdOptions {
			base: None,
			compact_arrays: true,
			compact_to_relative: true,
			document_loader: None,
			expand_context: None,
			extract_all_scripts: false,
			frame_expansion: false,
			ordered: false,
			processing_mode: JsonLdProcessingMode::JsonLd1_1,
			produce_generalized_rdf: true,
			rdf_direction: None,
			use_native_types: false,
			use_rdf_type: false
		}
	}
}

#[derive(Debug)]
struct LoadedContext<T: ForeignMutableJson + BuildableJson> {
	context: T::Object,
	base_url: Url
}

struct JsonLdOptionsImpl<'a, T, F>
where
	T: ForeignMutableJson + BuildableJson,
	F: for<'b> Fn(&'b str, &'b Option<LoadDocumentOptions>) -> BoxFuture<'b, Result<RemoteDocument<T>>>
{
	inner: &'a JsonLdOptions<'a, T, F>,
	loaded_contexts: MaybeOwned<'a, FrozenMap<Url, Box<LoadedContext<T>>>>
}

impl<'a, T, F> From<&'a JsonLdOptions<'a, T, F>> for JsonLdOptionsImpl<'a, T, F>
where
	T: ForeignMutableJson + BuildableJson,
	F: for<'b> Fn(&'b str, &'b Option<LoadDocumentOptions>) -> BoxFuture<'b, Result<RemoteDocument<T>>>
{
	fn from(value: &'a JsonLdOptions<'a, T, F>) -> Self {
		JsonLdOptionsImpl {
			inner: value,
			loaded_contexts: MaybeOwned::Owned(FrozenMap::new())
		}
	}
}

pub async fn compact<'a, T, F>(input: JsonLdInput<'_, T>, ctx: Option<Cow<'a, T>>, options: &'a JsonLdOptions<'a, T, F>) -> Result<T::Object>
where
	T: ForeignMutableJson + BuildableJson,
	F: for<'b> Fn(&'b str, &'b Option<LoadDocumentOptions>) -> BoxFuture<'b, Result<RemoteDocument<T>>> + Clone
{
	let options: JsonLdOptionsImpl<'a, T, F> = options.into();
	let doc;
	let input = if let JsonLdInput::Reference(iri) = input {
		doc = remote::load_remote(&iri, &options.inner, None, Vec::new()).await?;
		JsonLdInput::RemoteDocument(&doc)
	} else {
		input
	};

	// 4)
	let expand_options = JsonLdOptionsImpl {
		inner: &JsonLdOptions {
			ordered: false,
			..(*options.inner).clone()
		},
		loaded_contexts: MaybeOwned::Borrowed(options.loaded_contexts.as_ref())
	};
	let expanded_input = expand_with_loaded_contexts(input.clone(), expand_options).await?;

	let context_base = if let JsonLdInput::RemoteDocument(doc) = input {
		Some(Url::parse(&doc.document_url).map_err(|e| err!(InvalidBaseIRI, , e))?)
	} else {
		options
			.inner
			.base
			.as_ref()
			.map(|base| Url::parse(base).map_err(|e| err!(InvalidBaseIRI, , e)))
			.transpose()?
	};

	// If context is a map having an @context entry, set context to that entry's value
	let context = ctx.map_or(Ok(vec![None]), |contexts| {
		let mut contexts = JsonLdContext::from_json(contexts)?;
		if contexts.len() == 1 {
			match contexts.remove(0) {
				JsonOrReference::JsonObject(mut json) => match json {
					Cow::Owned(ref mut json) => json.remove("@context").map(|json| OptionalContexts::from_json(Cow::Owned(json))),
					Cow::Borrowed(json) => json.get("@context").map(|json| OptionalContexts::from_json(Cow::Borrowed(json)))
				}
				.unwrap_or(Ok(vec![Some(JsonOrReference::JsonObject(json))])),
				JsonOrReference::Reference(iri) => Ok(vec![Some(JsonOrReference::Reference(iri))])
			}
		} else {
			Ok(contexts.into_iter().map(|ctx| Some(ctx)).collect())
		}
	})?;

	let mut active_context = process_context(&Context::default(), &context, context_base.as_ref(), &options, &FrozenSet::new(), false, true, true).await?;
	if active_context.base_iri.is_none() {
		active_context.base_iri = if options.inner.compact_to_relative {
			context_base
		} else {
			options
				.inner
				.base
				.as_ref()
				.map(|base| Url::parse(base).map_err(|e| err!(InvalidBaseIRI, , e)))
				.transpose()?
		};
	}
	let compacted_output = compact_internal(&mut active_context, None, expanded_input.into(), &options).await?;
	let mut compacted_output = match compacted_output.into_enum() {
		Owned::Object(object) => object,
		Owned::Array(array) => {
			if array.is_empty() {
				T::empty_object()
			} else {
				json!(T, { compact_iri(&active_context, "@graph", &options.inner, None, true, false)?: array })
			}
		}
		_ => panic!()
	};
	if context.iter().any(|ctx| {
		ctx.as_ref().map_or(false, |ctx| match ctx {
			JsonOrReference::JsonObject(json) => !json.is_empty(),
			JsonOrReference::Reference(_) => true
		})
	}) {
		let mut context = context
			.into_iter()
			.map(|ctx| {
				if let Some(ctx) = ctx {
					match ctx {
						JsonOrReference::JsonObject(json) => json.into_owned().into(),
						JsonOrReference::Reference(iri) => iri.into_owned().into()
					}
				} else {
					T::null()
				}
			})
			.collect::<T::Array>();
		compacted_output.insert("@context".to_string(), if context.len() == 1 { context.remove(0).unwrap().into() } else { context.into() });
	}
	Ok(compacted_output)
}

pub async fn expand<T, F>(input: JsonLdInput<'_, T>, options: &JsonLdOptions<'_, T, F>) -> Result<<T as ForeignJson>::Array>
where
	T: ForeignMutableJson + BuildableJson,
	F: for<'a> Fn(&'a str, &'a Option<LoadDocumentOptions>) -> BoxFuture<'a, Result<RemoteDocument<T>>> + Clone
{
	expand_with_loaded_contexts(input, options.into()).await
}

async fn expand_with_loaded_contexts<T, F>(input: JsonLdInput<'_, T>, options: JsonLdOptionsImpl<'_, T, F>) -> Result<<T as ForeignJson>::Array>
where
	T: ForeignMutableJson + BuildableJson,
	F: for<'a> Fn(&'a str, &'a Option<LoadDocumentOptions>) -> BoxFuture<'a, Result<RemoteDocument<T>>> + Clone
{
	let doc;
	let input = if let JsonLdInput::Reference(iri) = input {
		doc = remote::load_remote(&iri, &options.inner, None, Vec::new()).await?;
		JsonLdInput::RemoteDocument(&doc)
	} else {
		input
	};
	let mut active_context = Context {
		base_iri: options
			.inner
			.base
			.as_ref()
			.or_else(|| {
				if let JsonLdInput::RemoteDocument(ref document) = input {
					Some(&document.document_url)
				} else {
					None
				}
			})
			.map(|iri| Url::parse(iri).map_err(|e| err!(InvalidBaseIRI, , e)))
			.transpose()?,
		original_base_url: if let JsonLdInput::RemoteDocument(document) = input {
			Some(&document.document_url)
		} else {
			options.inner.base.as_ref()
		}
		.map(|url| Url::parse(url).map_err(|e| err!(InvalidBaseIRI, , e)))
		.transpose()?,
		..Context::default()
	};
	if let Some(ref expand_context) = options.inner.expand_context {
		let context = match expand_context {
			JsonOrReference::JsonObject(json) => json.get("@context").map_or(Ok(vec![Some(JsonOrReference::JsonObject(Cow::Borrowed(&**json)))]), |json| {
				OptionalContexts::from_json(Cow::Borrowed(json))
			}),
			JsonOrReference::Reference(iri) => Ok(vec![Some(JsonOrReference::Reference(Cow::Borrowed(&**iri)))])
		}?;
		active_context = process_context(
			&active_context,
			&context,
			active_context.original_base_url.as_ref(),
			&options,
			&FrozenSet::new(),
			false,
			true,
			true
		)
		.await?;
	}
	let expanded_output = match input {
		JsonLdInput::RemoteDocument(document) => {
			if let Some(ref context_url) = document.context_url {
				active_context = process_context(
					&active_context,
					&vec![Some(JsonOrReference::Reference(context_url.clone().into()))],
					Some(&Url::parse(context_url).map_err(|e| err!(InvalidBaseIRI, , e))?),
					&options,
					&FrozenSet::new(),
					false,
					true,
					true
				)
				.await?;
			}
			let document_url = Url::parse(&document.document_url).map_err(|e| err!(InvalidBaseIRI, , e))?;
			let input = document.document.to_parsed().map_err(|e| err!(LoadingDocumentFailed, , e))?;
			expand_internal(&active_context, None, &input, Some(&document_url), &options, false).await?
		}
		JsonLdInput::JsonObject(json) => {
			let document_url = options.inner.base.as_ref().map(|url| Url::parse(url).map_err(|e| err!(InvalidBaseIRI, , e))).transpose()?;
			expand_object(&active_context, None, json, document_url.as_ref(), &options, false).await?
		},
		JsonLdInput::Reference(_) => unreachable!()
	};
	Ok(match expanded_output {
		Owned::Object(mut object) if object.len() == 1 && object.contains("@graph") => {
			match object.remove("@graph").unwrap().into_enum() {
				// Only one level of recursion, for sure
				Owned::Array(array) => array,
				Owned::Null => T::empty_array(),
				expanded_output => json!(T, [expanded_output.into_untyped()])
			}
		}
		Owned::Array(array) => array,
		Owned::Null => T::empty_array(),
		expanded_output => json!(T, [expanded_output.into_untyped()])
	})
}

// pub async fn flatten<'a, T, F, R>(input: &JsonLdInput<T>, context: Option<JsonLdContext<'_, T>>,
// options: impl Into<JsonLdOptionsImpl<'a, T, F, R>>) -> Result<T> where
// T: ForeignMutableJson + BuildableJson,
// F: Fn(&str, &Option<LoadDocumentOptions>) -> R + 'a,
// R: Future<Output = Result<crate::RemoteDocument<T>>> + 'a
// {
// todo!()
// }
//
// pub async fn frame() {
// todo!()
// }
//
// pub async fn from_rdf(input: RdfDataset, options: JsonLdOptions) -> dyn ExactSizeIterator<Item = dyn JsonType> {
// todo!()
// }
//
// pub async fn to_rdf(input: JsonLdInput, options: JsonLdOptions) -> RdfDataset {
//
// }
