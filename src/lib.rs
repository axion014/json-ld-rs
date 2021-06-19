#![feature(try_find)]
#![feature(unboxed_closures)]
#![feature(fn_traits)]

use std::future::Future;
use std::collections::{HashMap, BTreeMap, BTreeSet};
use std::borrow::{Borrow, Cow};

use elsa::{FrozenMap, FrozenSet};
use once_cell::unsync::OnceCell;

use json_trait::{ForeignJson, ForeignMutableJson, BuildableJson, typed_json::*, json};
use cc_traits::{Get, Len, PushBack, MapInsert, Remove};

use maybe_owned::MaybeOwned;

use url::Url;

#[macro_use]
mod macros;

pub mod error;
pub mod remote;
mod context;
mod compact;
mod expand;
mod util;

use crate::context::process_context;
use crate::compact::{compact_internal, compact_iri};
use crate::expand::expand_internal;
use crate::remote::LoadDocumentOptions;
use crate::error::{Result, JsonLdErrorCode::*};
use crate::util::map_context;

#[derive(Clone, Eq, PartialEq)]
pub enum JsonOrReference<'a, T: ForeignMutableJson + BuildableJson> {
	JsonObject(Cow<'a, T::Object>),
	Reference(Cow<'a, str>),
}

#[derive(Clone)]
pub enum Document<T: ForeignMutableJson + BuildableJson> {
	ParsedJson(T),
	RawJson(String)
}

impl <T: ForeignMutableJson + BuildableJson> Document<T> {
	pub fn to_parsed(self) -> std::result::Result<T, T::Err> {
		match self {
			Document::ParsedJson(v) => Ok(v),
			Document::RawJson(v) => T::from_str(&v)
		}
	}
}

#[derive(Clone)]
pub struct RemoteDocument<T: ForeignMutableJson + BuildableJson> {
	pub content_type: String,
	pub context_url: Option<String>,
	pub document: Document<T>,
	pub document_url: String,
	pub profile: Option<String>
}

#[derive(Clone)]
pub enum JsonLdInput<T: ForeignMutableJson + BuildableJson> {
	JsonObject(T::Object),
	Reference(String),
	RemoteDocument(RemoteDocument<T>),
}

type JsonLdContext<'a, T> = Vec<JsonOrReference<'a, T>>;
type OptionalContexts<'a, T> = Vec<Option<JsonOrReference<'a, T>>>;

#[derive(Clone, Eq, PartialEq)]
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
	context: OptionalContexts<'a, T>,
	container_mapping: Option<BTreeSet<String>>,
	direction_mapping: Option<Direction>,
	index_mapping: Option<String>,
	language_mapping: Option<Option<String>>,
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

type InverseContext = HashMap<String, HashMap<String, HashMap<String, HashMap<String, String>>>>;

#[derive(Clone)]
pub struct Context<'a, T> where
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
	F: for<'b> Fn(&'b str, &'b Option<LoadDocumentOptions>) -> R,
	R: Future<Output = Result<RemoteDocument<T>>>
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

type DefaultFuture<T> = std::future::Pending<Result<RemoteDocument<T>>>;
pub type JsonLdOptionsWithoutDocumentLoader<'a, T> =
	JsonLdOptions<'a, T, fn(&str, &Option<LoadDocumentOptions>) -> DefaultFuture<T>, DefaultFuture<T>>;
impl <'a, T> Default for JsonLdOptionsWithoutDocumentLoader<'a, T> where
	T: ForeignMutableJson + BuildableJson
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

impl <'a, T, F, R> JsonLdOptions<'a, T, F, R> where
	T: ForeignMutableJson + BuildableJson,
	F: for<'b> Fn(&'b str, &'b Option<LoadDocumentOptions>) -> R,
	R: Future<Output = Result<RemoteDocument<T>>>
{
	pub fn with_document_loader(document_loader: F) -> Self {
		JsonLdOptions {
			base: None,
			compact_arrays: true,
			compact_to_relative: true,
			document_loader: Some(document_loader),
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

struct JsonLdOptionsImpl<'a, T, F, R> where
	T: ForeignMutableJson + BuildableJson,
	F: for<'b> Fn(&'b str, &'b Option<LoadDocumentOptions>) -> R,
	R: Future<Output = Result<RemoteDocument<T>>>
{
	inner: &'a JsonLdOptions<'a, T, F, R>,
	loaded_contexts: MaybeOwned<'a, FrozenMap<Url, Box<LoadedContext<T>>>>
}

impl <'a, T, F, R> From<&'a JsonLdOptions<'a, T, F, R>> for JsonLdOptionsImpl<'a, T, F, R> where
	T: ForeignMutableJson + BuildableJson,
	F: for<'b> Fn(&'b str, &'b Option<LoadDocumentOptions>) -> R + Clone,
	R: Future<Output = Result<RemoteDocument<T>>>
{
	fn from(value: &'a JsonLdOptions<'a, T, F, R>) -> Self {
		JsonLdOptionsImpl {
			inner: value,
			loaded_contexts: MaybeOwned::Owned(FrozenMap::new())
		}
	}
}

pub async fn compact<'a, T, F, R>(input: &JsonLdInput<T>, ctx: Option<JsonLdContext<'a, T>>,
		options: &'a JsonLdOptions<'a, T, F, R>) -> Result<T::Object> where
	T: ForeignMutableJson + BuildableJson,
	F: for<'b> Fn(&'b str, &'b Option<LoadDocumentOptions>) -> R + Clone,
	R: Future<Output = Result<RemoteDocument<T>>> + Clone
{
	let options: JsonLdOptionsImpl<'a, T, F, R> = options.into();
	let input = if let JsonLdInput::Reference(iri) = input {
		Cow::Owned(JsonLdInput::RemoteDocument(remote::load_remote(&iri, &options.inner, None, Vec::new()).await?))
	} else {
		Cow::Borrowed(input)
	};

	// 4)
	let expand_options = JsonLdOptionsImpl {
		inner: &JsonLdOptions {
			ordered: false,
			..(*options.inner).clone()
		},
		loaded_contexts: MaybeOwned::Borrowed(options.loaded_contexts.as_ref())
	};
	let expanded_input = expand_with_loaded_contexts(&input, expand_options).await?;

	let context_base = if let JsonLdInput::RemoteDocument(ref doc) = *input {
		Some(Url::parse(&doc.document_url).map_err(|e| err!(InvalidBaseIRI, , e))?)
	} else {
		options.inner.base.as_ref().map(|base| Url::parse(base).map_err(|e| err!(InvalidBaseIRI, , e))).transpose()?
	};

	// If context is a map having an @context entry, set context to that entry's value
	let context = ctx.map_or(Ok(vec![None]), |mut contexts| {
		if contexts.len() == 1 {
			match contexts.remove(0) {
				JsonOrReference::JsonObject(mut json) => match json {
					Cow::Owned(ref mut json) => json.remove("@context").map(|json| map_context(Cow::Owned(json))),
					Cow::Borrowed(json) => json.get("@context").map(|json| map_context(Cow::Borrowed(json)))
				}.unwrap_or(Ok(vec![Some(JsonOrReference::JsonObject(json))])),
				JsonOrReference::Reference(iri) => Ok(vec![Some(JsonOrReference::Reference(iri))])
			}
		} else {
			Ok(contexts.into_iter().map(|ctx| Some(ctx)).collect())
		}
	})?;

	let mut active_context = process_context(&Context::default(), &context, context_base.as_ref(),
		&options, &FrozenSet::new(), false, true, true).await?;
	active_context.base_iri = if options.inner.compact_to_relative {
		context_base
	} else {
		options.inner.base.as_ref().map(|base| Url::parse(base).map_err(|e| err!(InvalidBaseIRI, , e))).transpose()?
	};
	let compacted_output = compact_internal(&mut active_context, None, expanded_input.into(), &options).await?;
	let mut compacted_output = match compacted_output.into_enum() {
		Owned::Object(object) => object,
		Owned::Array(array) => {
			if array.is_empty() {
				T::empty_object()
			} else {
				json!(T, {compact_iri(&active_context, "@graph", &options.inner, None, true, false)?: array})
			}
		},
		_ => panic!()
	};
	if context.iter().any(|ctx| ctx.as_ref().map_or(false, |ctx| match ctx {
		JsonOrReference::JsonObject(json) => !json.is_empty(),
		JsonOrReference::Reference(_) => true
	})) {
		let mut context = context.into_iter().map(|ctx| {
			if let Some(ctx) = ctx {
				match ctx {
					JsonOrReference::JsonObject(json) => json.into_owned().into(),
					JsonOrReference::Reference(iri) => iri.into_owned().into()
				}
			} else {
				T::null()
			}
		}).collect::<T::Array>();
		compacted_output.insert("@context".to_string(), if context.len() == 1 {
			context.remove(0).unwrap().into()
		} else {
			context.into()
		});
	}
	Ok(compacted_output)
}

pub async fn expand<'a, T, F, R>(input: &JsonLdInput<T>, options: &JsonLdOptions<'a, T, F, R>) ->
		Result<<T as ForeignJson>::Array> where
	T: ForeignMutableJson + BuildableJson,
	F: for<'b> Fn(&'b str, &'b Option<LoadDocumentOptions>) -> R + Clone,
	R: Future<Output = Result<RemoteDocument<T>>> + Clone
{
	expand_with_loaded_contexts(input, options.into()).await
}

async fn expand_with_loaded_contexts<'a, T, F, R>(input: &JsonLdInput<T>, options: JsonLdOptionsImpl<'a, T, F, R>) ->
		Result<<T as ForeignJson>::Array> where
	T: ForeignMutableJson + BuildableJson,
	F: for<'b> Fn(&'b str, &'b Option<LoadDocumentOptions>) -> R + Clone,
	R: Future<Output = Result<RemoteDocument<T>>> + Clone
{
	let input = if let JsonLdInput::Reference(iri) = input {
		Cow::Owned(JsonLdInput::RemoteDocument(remote::load_remote(&iri, &options.inner, None, Vec::new()).await?))
	} else {
		Cow::Borrowed(input)
	};
	let mut active_context = Context {
		base_iri: options.inner.base.as_ref().or_else(|| if let JsonLdInput::RemoteDocument(ref document) = *input {
			Some(&document.document_url)
		} else {
			None
		}).map(|iri| Url::parse(iri).map_err(|e| err!(InvalidBaseIRI, , e))).transpose()?,
		original_base_url: if let JsonLdInput::RemoteDocument(ref document) = *input {
			Some(&document.document_url)
		} else {
			options.inner.base.as_ref()
		}.map(|url| Url::parse(url).map_err(|e| err!(InvalidBaseIRI, , e))).transpose()?,
		..Context::default()
	};
	if let Some(ref expand_context) = options.inner.expand_context {
		let context = match expand_context {
			JsonOrReference::JsonObject(json) => json.get("@context")
				.map_or(Ok(vec![Some(JsonOrReference::JsonObject(json.clone()))]), |json| map_context(Cow::Borrowed(json))),
			JsonOrReference::Reference(iri) => Ok(vec![Some(JsonOrReference::Reference(iri.clone()))])
		}?;
		active_context = process_context(&active_context, &context, active_context.original_base_url.as_ref(),
			&options, &FrozenSet::new(), false, true, true).await?;
	}
	let (input, document_url) = match input.into_owned() {
		JsonLdInput::RemoteDocument(document) => {
			if let Some(ref context_url) = document.context_url {
				active_context = process_context(&active_context, &vec![Some(JsonOrReference::Reference(context_url.clone().into()))],
					Some(&Url::parse(context_url).map_err(|e| err!(InvalidBaseIRI, , e))?),
					&options, &FrozenSet::new(), false, true, true).await?;
			}
			(document.document.to_parsed().map_err(|e| err!(LoadingDocumentFailed, , e))?, Some(document.document_url.clone()))
		},
		JsonLdInput::JsonObject(json) => (json.into(), options.inner.base.clone()),
		JsonLdInput::Reference(_) => unreachable!()
	};
	let document_url = document_url.as_ref().map(|url| Url::parse(url).map_err(|e| err!(InvalidBaseIRI, , e))).transpose()?;
	let expanded_output = expand_internal(&active_context, None, input, document_url.as_ref(), &options, false).await?;
	Ok(match expanded_output {
		Owned::Object(mut object) if object.len() == 1 && object.contains("@graph") => {
			match object.remove("@graph").unwrap().into_enum() {
				// Only one level of recursion, for sure
				Owned::Array(array) => array,
				Owned::Null => T::empty_array(),
				expanded_output => json!(T, [expanded_output.into_untyped()])
			}
		},
		Owned::Array(array) => array,
		Owned::Null => T::empty_array(),
		expanded_output => json!(T, [expanded_output.into_untyped()])
	})
}

/*

pub async fn flatten<'a, T, F, R>(input: &JsonLdInput<T>, context: Option<JsonLdContext<'_, T>>,
		options: impl Into<JsonLdOptionsImpl<'a, T, F, R>>) -> Result<T> where
	T: ForeignMutableJson + BuildableJson,
	F: Fn(&str, &Option<LoadDocumentOptions>) -> R + 'a,
	R: Future<Output = Result<crate::RemoteDocument<T>>> + 'a
{
	todo!()
}

pub async fn frame() {
	todo!()
}

pub async fn from_rdf(input: RdfDataset, options: JsonLdOptions) -> dyn ExactSizeIterator<Item = dyn JsonType> {
	todo!()
}

pub async fn to_rdf(input: JsonLdInput, options: JsonLdOptions) -> RdfDataset {

}
*/
