use std::future::Future;
use std::collections::HashMap;

use json_trait::{ForeignJson, ForeignMutableJson, BuildableJson};

mod remote;
mod context;
mod compact;
mod expand;
mod util;
mod error;

use crate::remote::LoadDocumentOptions;
use crate::error::Result;

#[derive(Clone, Eq, PartialEq)]
pub enum JsonOrReference<'a: 'b, 'b, T: ForeignJson<'a>> {
	JsonObject(&'b T::Object),
	Reference(&'b str),
}

#[derive(Clone, Eq, PartialEq)]
pub enum OwnedJsonOrReference<'a, T: ForeignJson<'a>> {
	JsonObject(T::Object),
	Reference(String),
}

pub enum Document<'a, T: ForeignMutableJson<'a> + BuildableJson<'a>> {
	ParsedJson((T, std::marker::PhantomData<&'a T>)),
	RawJson(String)
}

impl <'a, T: ForeignMutableJson<'a> + BuildableJson<'a>> Document<'a, T> {
	fn to_parsed(self) -> std::result::Result<T, T::ParseError> {
		match self {
			Document::ParsedJson(v) => Ok(v.0),
			Document::RawJson(v) => T::from_str(&v)
		}
	}
}

pub struct RemoteDocument<'a, T: ForeignMutableJson<'a> + BuildableJson<'a>> {
	pub content_type: String,
	pub context_url: Option<String>,
	pub document: Document<'a, T>,
	pub document_url: String,
	pub profile: Option<String>
}

pub enum JsonLdInput<'a, T: ForeignMutableJson<'a> + BuildableJson<'a>> {
	JsonObject(T::Object),
	Reference(String),
	RemoteDocument(RemoteDocument<'a, T>),
}

type JsonLdContext<'a, 'b, T> = Vec<Option<JsonOrReference<'a, 'b, T>>>;

#[derive(Clone, Eq, PartialEq)]
pub enum Direction {
	LTR,
	RTL
}

#[derive(Clone)]
struct TermDefinition<'a, T: ForeignMutableJson<'a>> where T::Object: Clone {
	iri: Option<String>,
	prefix: bool,
	protected: bool,
	reverse_property: bool,
	base_url: Option<String>,
	context: Option<OwnedJsonOrReference<'a, T>>,
	container_mapping: Option<Vec<String>>,
	direction_mapping: Option<Direction>,
	index_mapping: Option<String>,
	language_mapping: Option<String>,
	nest_value: Option<String>,
	type_mapping: Option<String>
}

#[derive(Clone)]
pub struct Context<'a, T> where
	T: ForeignMutableJson<'a>,
	T::Object: Clone
{
	term_definitions: HashMap<String, TermDefinition<'a, T>>,
	base_iri: Option<String>,
	original_base_url: Option<String>,
	inverse_context: Option<HashMap<String, T>>,
	vocabulary_mapping: Option<String>,
	default_language: Option<String>,
	default_base_direction: Option<Direction>,
	previous_context: Option<Box<Context<'a, T>>>
}

impl <'a, T> Default for Context<'a, T> where
	T: ForeignMutableJson<'a>,
	T::Object: Clone
{
	fn default() -> Self {
		Context {
			term_definitions: HashMap::<String, TermDefinition<T>>::new(),
			base_iri: None,
			original_base_url: None,
			inverse_context: None,
			vocabulary_mapping: None,
			default_language: None,
			default_base_direction: None,
			previous_context: None
		}
	}
}

#[derive(Clone)]
pub enum JsonLdProcessingMode {
	JsonLd1_1,
	JsonLd1_0
}

#[derive(Clone)]
pub struct JsonLdOptions<'a, T, F, R> where
	T: 'a + ForeignMutableJson<'a> + BuildableJson<'a>,
	F: Fn(&str, &Option<LoadDocumentOptions>) -> R,
	R: Future<Output = Result<RemoteDocument<'a, T>>>
{
	pub base: Option<String>,

	pub compact_arrays: bool,
	pub compact_to_relative: bool,

	pub document_loader: Option<F>,
	pub expand_context: Option<JsonOrReference<'a, 'a, T>>,
	pub extract_all_scripts: bool,
	pub frame_expansion: bool,
	pub ordered: bool,
	pub processing_mode: JsonLdProcessingMode,
	pub produce_generalized_rdf: bool,
	pub rdf_direction: Option<Direction>,
	pub use_native_types: bool,
	pub use_rdf_type: bool,
}

pub mod JsonLdProcessor {
	use std::future::Future;
	use std::collections::HashSet;

	use crate::{JsonLdContext, JsonLdInput, JsonLdOptions, JsonOrReference, Context};
	use crate::error::{Result, JsonLdErrorCode::InvalidContextEntry};
	use crate::remote::{self, LoadDocumentOptions};
	use crate::context::process_context;
	use crate::compact::compact_internal;

	use json_trait::{ForeignJson, ForeignMutableJson, BuildableJson, TypedJson, Array};
	use cc_traits::Get;
	use url::Url;

	pub async fn compact<'a: 'b, 'b, T, F, R>(input: &'b mut JsonLdInput<'a, T>, ctx: Option<JsonLdContext<'a, 'b, T>>,
			options: &'b mut JsonLdOptions<'a, T, F, R>) -> Result<T> where
		T: ForeignMutableJson<'a> + BuildableJson<'a>,
		F: Fn(&str, &Option<LoadDocumentOptions>) -> R,
		R: Future<Output = Result<crate::RemoteDocument<'a, T>>>
	{
		if let JsonLdInput::Reference(iri) = input {
			*input = JsonLdInput::RemoteDocument(remote::load_remote(&iri, &options, None, Vec::new()).await?);
		}

		// 4)
		// Save and restore the options state
		let ordered_original = options.ordered;
		options.ordered = false;
		let expanded_input = expand(input, options).await?;
		options.ordered = ordered_original;

		let context_base = if let JsonLdInput::RemoteDocument(ref doc) = input {
			Some(&doc.document_url)
		} else {
			options.base.as_ref()
		};
		let context = if let Some(context) = ctx {
			if context.len() == 1 {
				if let Some(JsonOrReference::JsonObject(json)) = context[0] {
					if let Some(ctx) = json.get("@context") {
						match ctx.as_enum() {
							TypedJson::Object(ctx) => vec![Some(JsonOrReference::JsonObject(ctx))],
							TypedJson::Array(ctx) => {
								ctx.iter().map(|value| {
									Ok(match value.as_enum() {
										TypedJson::Object(obj) => Some(JsonOrReference::JsonObject(obj)),
										TypedJson::String(reference) => Some(JsonOrReference::Reference(reference)),
										TypedJson::Null => None,
										_ => return Err(InvalidContextEntry.to_error(None))
									})
								}).collect::<Result<JsonLdContext<'a, 'b, T>>>()?
							},
							_ => return Err(InvalidContextEntry.to_error(None))
						}
					} else {
						context
					}
				} else {
					context
				}
			} else {
				context
			}
		} else {
			vec![None]
		};
		let base_url = context_base.map(|v| Url::parse(v).unwrap());
		let mut active_context = process_context(&mut Context::default(), context, base_url.as_ref(),
			&options, &HashSet::new(), false, true, true).await?;
		active_context.base_iri = if options.compact_to_relative { context_base.cloned() } else { options.base.clone() };
		compact_internal(active_context, None, TypedJson::Array(&expanded_input), options.compact_arrays, options.ordered)
	}

	pub async fn expand<'a: 'b, 'b, T, F, R>(input: &'b JsonLdInput<'a, T>, options: &'b JsonLdOptions<'a, T, F, R>) ->
			Result<<T as ForeignJson<'a>>::Array> where
		T: ForeignMutableJson<'a> + BuildableJson<'a>,
		F: Fn(&str, &Option<LoadDocumentOptions>) -> R,
		R: Future<Output = Result<crate::RemoteDocument<'a, T>>>
	{
		todo!()
	}

	pub async fn flatten<'a: 'b, 'b, T, F, R>(input: &'b JsonLdInput<'a, T>, context: Option<JsonLdContext<'a, 'b, T>>,
			options: &'b JsonLdOptions<'a, T, F, R>) -> Result<T> where
		T: ForeignMutableJson<'a> + BuildableJson<'a>,
		F: Fn(&str, &Option<LoadDocumentOptions>) -> R,
		R: Future<Output = Result<crate::RemoteDocument<'a, T>>>
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
