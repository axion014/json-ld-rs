use std::future::Future;
use std::collections::{HashMap, BTreeMap, BTreeSet};
use std::borrow::{Borrow, Cow};

use elsa::FrozenMap;
use once_cell::unsync::OnceCell;

use json_trait::{ForeignMutableJson, BuildableJson};

use maybe_owned::MaybeOwnedMut;

use url::Url;

#[macro_use]
mod macros;

mod error;
mod remote;
mod context;
mod compact;
mod expand;
mod util;

use crate::remote::LoadDocumentOptions;
use crate::error::Result;

#[derive(Clone, Eq, PartialEq)]
pub enum JsonOrReference<'a, T: ForeignMutableJson + BuildableJson> {
	JsonObject(Cow<'a, T::Object>),
	Reference(Cow<'a, str>),
}

pub enum Document<T: ForeignMutableJson + BuildableJson> {
	ParsedJson(T),
	RawJson(String)
}

impl <T: ForeignMutableJson + BuildableJson> Document<T> {
	fn to_parsed(self) -> std::result::Result<T, T::Err> {
		match self {
			Document::ParsedJson(v) => Ok(v),
			Document::RawJson(v) => T::from_str(&v)
		}
	}
}

pub struct RemoteDocument<T: ForeignMutableJson + BuildableJson> {
	pub content_type: String,
	pub context_url: Option<String>,
	pub document: Document<T>,
	pub document_url: String,
	pub profile: Option<String>
}

pub enum JsonLdInput<T: ForeignMutableJson + BuildableJson> {
	JsonObject(T::Object),
	Reference(String),
	RemoteDocument(RemoteDocument<T>),
}

type JsonLdContext<'a, T> = Vec<Option<JsonOrReference<'a, T>>>;

#[derive(Clone, Eq, PartialEq)]
pub enum Direction {
	LTR,
	RTL,
	None
}

impl AsRef<str> for Direction {
	fn as_ref(&self) -> &str {
		match self {
			Direction::LTR => "_ltr",
			Direction::RTL => "_rtl",
			Direction::None => "@none"
		}
	}
}

#[derive(Clone)]
struct TermDefinition<'a, T> where
	T: ForeignMutableJson + BuildableJson,
	T::Object: Clone
{
	iri: Option<String>,
	prefix: bool,
	protected: bool,
	reverse_property: bool,
	base_url: Option<Url>,
	context: Option<JsonOrReference<'a, T>>,
	container_mapping: Option<BTreeSet<String>>,
	direction_mapping: Option<Direction>,
	index_mapping: Option<String>,
	language_mapping: Option<String>,
	nest_value: Option<String>,
	type_mapping: Option<String>
}

#[derive(Clone, PartialEq, Eq, Ord)]
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

#[derive(Clone)]
pub struct Context<'a, T> where
	T: ForeignMutableJson + BuildableJson,
	T::Object: Clone
{
	term_definitions: BTreeMap<TermDefinitionKey, TermDefinition<'a, T>>,
	base_iri: Option<Url>,
	original_base_url: Option<Url>,
	inverse_context: OnceCell<HashMap<String, T>>,
	vocabulary_mapping: Option<String>,
	default_language: Option<String>,
	default_base_direction: Option<Direction>,
	previous_context: Option<Box<Context<'a, T>>>
}

impl <T> Default for Context<'_, T> where
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

#[derive(Clone, PartialEq, Eq)]
pub enum JsonLdProcessingMode {
	JsonLd1_1,
	JsonLd1_0
}

#[derive(Clone)]
pub struct JsonLdOptions<'a, T, F, R> where
	T: ForeignMutableJson + BuildableJson,
	F: Fn(&str, &Option<LoadDocumentOptions>) -> R + 'a,
	R: Future<Output = Result<RemoteDocument<T>>> + 'a
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

impl <'a, T, F, R> Default for JsonLdOptions<'a, T, F, R> where
	T: ForeignMutableJson + BuildableJson,
	F: Fn(&str, &Option<LoadDocumentOptions>) -> R + 'a,
	R: Future<Output = Result<RemoteDocument<T>>> + 'a
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

struct LoadedContext<T: ForeignMutableJson + BuildableJson> {
	context: T::Object,
	base_url: Url
}

pub struct JsonLdOptionsImpl<'a, T, F, R> where
	T: ForeignMutableJson + BuildableJson,
	F: Fn(&str, &Option<LoadDocumentOptions>) -> R + 'a,
	R: Future<Output = Result<RemoteDocument<T>>> + 'a
{
	inner: &'a JsonLdOptions<'a, T, F, R>,
	loaded_contexts: MaybeOwnedMut<'a, FrozenMap<Url, Box<LoadedContext<T>>>>
}

impl <'a, T, F, R> From<&'a JsonLdOptions<'a, T, F, R>> for JsonLdOptionsImpl<'a, T, F, R> where
	T: ForeignMutableJson + BuildableJson,
	F: Fn(&str, &Option<LoadDocumentOptions>) -> R + 'a,
	R: Future<Output = Result<RemoteDocument<T>>> + 'a
{
	fn from(value: &'a JsonLdOptions<'a, T, F, R>) -> Self {
		JsonLdOptionsImpl {
			inner: value,
			loaded_contexts: MaybeOwnedMut::Owned(FrozenMap::new())
		}
	}
}

pub mod JsonLdProcessor {
	use std::future::Future;
	use std::collections::HashSet;

	use maybe_owned::MaybeOwnedMut;

	use crate::{JsonLdContext, JsonLdInput, JsonLdOptions, JsonLdOptionsImpl, JsonOrReference, Context};
	use crate::error::{Result, JsonLdErrorCode::{InvalidContextEntry, InvalidBaseIRI}};
	use crate::remote::{self, LoadDocumentOptions};
	use crate::context::process_context;
	use crate::compact::compact_internal;
	use crate::util::{map_cow, MapCow, MapCowCallback};

	use json_trait::{ForeignJson, ForeignMutableJson, BuildableJson, typed_json::*, Array};
	use cc_traits::Get;
	use url::Url;

	pub async fn compact<'a, T, F, R>(input: &mut JsonLdInput<T>, ctx: Option<JsonLdContext<'a, T>>,
			options: impl Into<JsonLdOptionsImpl<'a, T, F, R>>) -> Result<T> where
		T: ForeignMutableJson + BuildableJson,
		F: Fn(&str, &Option<LoadDocumentOptions>) -> R + Clone + 'a,
		R: Future<Output = Result<crate::RemoteDocument<T>>> + Clone + 'a
	{
		let mut options = options.into();
		if let JsonLdInput::Reference(iri) = input {
			*input = JsonLdInput::RemoteDocument(remote::load_remote(&iri, &options, None, Vec::new()).await?);
		}

		// 4)
		let expand_options = JsonLdOptionsImpl {
			inner: &JsonLdOptions {
				ordered: false,
				..(*options.inner).clone()
			},
			loaded_contexts: MaybeOwnedMut::Borrowed(options.loaded_contexts.as_mut())
		};
		let expanded_input = expand(input, expand_options).await?;

		let context_base = if let JsonLdInput::RemoteDocument(ref doc) = input {
			Some(Url::parse(&doc.document_url).map_err(|e| err!(InvalidBaseIRI, , e))?)
		} else {
			options.inner.base.as_ref().map(|base| Url::parse(base).map_err(|e| err!(InvalidBaseIRI, , e))).transpose()?
		};

		// If context is a map having an @context entry, set context to that entry's value
		let context = ctx.map_or(Ok(vec![None]), |contexts| {
			struct MapContext;
			impl <'a, T> MapCow<'static, 'a, T::Object, Option<Result<JsonLdContext<'a, T>>>> for MapContext where
				T: ForeignMutableJson + BuildableJson
			{
				fn map<'b>(&self, json: &'b T::Object, cow: impl MapCowCallback<'a, 'b>) -> Option<Result<JsonLdContext<'a, T>>> {
					json.get("@context").map(|ctx| match ctx.as_enum() {
						Borrowed::Array(ctx) => ctx.iter().map(|value| Ok(match value.as_enum() {
							// Only one level of recursion, I think
							Borrowed::Object(obj) => Some(JsonOrReference::JsonObject(cow.wrap(obj))),
							Borrowed::String(reference) => Some(JsonOrReference::Reference(cow.wrap(reference))),
							Borrowed::Null => None,
							_ => return Err(err!(InvalidContextEntry))
						})).collect::<Result<JsonLdContext<'a, T>>>(),
						Borrowed::Object(ctx) => Ok(vec![Some(JsonOrReference::JsonObject(cow.wrap(ctx)))]),
						Borrowed::String(reference) => Ok(vec![Some(JsonOrReference::Reference(cow.wrap(reference)))]),
						Borrowed::Null => Ok(vec![None]),
						_ => Err(err!(InvalidContextEntry))
					})
				}
			}
			(if contexts.len() == 1 { contexts[0].as_ref() } else { None })
				.and_then(|ctx| match ctx {
					JsonOrReference::JsonObject(json) => Some(json),
					_ => None
				}).and_then(map_cow(MapContext)).unwrap_or(Ok(contexts))
		})?;

		let mut active_context = process_context(&Context::default(), context, context_base.as_ref(),
			&options, &mut HashSet::new(), false, true, true).await?;
		active_context.base_iri = if options.inner.compact_to_relative {
			context_base
		} else {
			options.inner.base.as_ref().map(|base| Url::parse(base).map_err(|e| err!(InvalidBaseIRI, , e))).transpose()?
		};
		compact_internal(&mut active_context, None, expanded_input.into(), &options, options.inner.compact_arrays, options.inner.ordered).await
	}

	pub async fn expand<'a, T, F, R>(input: &JsonLdInput<T>, options: impl Into<JsonLdOptionsImpl<'a, T, F, R>>) ->
			Result<<T as ForeignJson>::Array> where
		T: ForeignMutableJson + BuildableJson,
		F: Fn(&str, &Option<LoadDocumentOptions>) -> R + 'a,
		R: Future<Output = Result<crate::RemoteDocument<T>>> + 'a
	{
		todo!()
	}

	pub async fn flatten<'a, T, F, R>(input: &JsonLdInput<T>, context: Option<JsonLdContext<'_, T>>,
			options: impl Into<JsonLdOptionsImpl<'a, T, F, R>>) -> Result<T> where
		T: ForeignMutableJson + BuildableJson,
		F: Fn(&str, &Option<LoadDocumentOptions>) -> R + 'a,
		R: Future<Output = Result<crate::RemoteDocument<T>>> + 'a
	{
		todo!()
	}

	/*
	pub async fn fromRdf(input: RdfDataset, options: JsonLdOptions) -> dyn ExactSizeIterator<Item = dyn JsonType> {

	}

	pub async fn toRdf(input: JsonLdInput, options: JsonLdOptions) -> RdfDataset {

	}
	*/
}
