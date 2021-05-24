use std::collections::HashMap;
use std::future::Future;

use json_trait::{ForeignMutableJson, BuildableJson};
use cc_traits::Get;

use url::Url;

use if_chain::if_chain;

use crate::{Context, JsonLdOptions, JsonLdOptionsImpl, LoadDocumentOptions, RemoteDocument};
use crate::error::{Result, JsonLdErrorCode::InvalidBaseIRI};
use crate::util::{is_jsonld_keyword, looks_like_a_jsonld_keyword, is_iri, resolve_with_str, as_compact_iri};
use crate::context::create_term_definition;

pub async fn expand_internal<'a, T, F, R>(active_context: &Context<'a, T>, active_property: Option<&str>, element: T,
		base_url: Option<&Url>, options: &'a JsonLdOptionsImpl<'a, T, F, R>, from_map: bool) -> Result<T>  where
	T: ForeignMutableJson + BuildableJson,
	F: Fn(&str, &Option<LoadDocumentOptions>) -> R,
	R: Future<Output = Result<RemoteDocument<T>>>
{
	todo!()
}

pub enum IRIExpansionArguments<'a, 'b: 'a, T, F, R> where
	T: ForeignMutableJson + BuildableJson,
	F: Fn(&str, &Option<LoadDocumentOptions>) -> R,
	R: Future<Output = Result<RemoteDocument<T>>>
{
	DefineTerms {
		active_context: &'a mut Context<'b, T>,
		local_context: &'a T::Object,
		defined: &'a mut HashMap<String, bool>,
		options: &'a JsonLdOptions<'a, T, F, R>
	},
	Normal(&'a Context<'b, T>)
}

impl <'a, 'b: 'a, T, F, R> IRIExpansionArguments<'a, 'b, T, F, R> where
	T: ForeignMutableJson + BuildableJson,
	F: Fn(&str, &Option<LoadDocumentOptions>) -> R,
	R: Future<Output = Result<RemoteDocument<T>>>
{
	fn active_context(&'a self) -> &'a Context<'b, T> {
		match self {
			Self::DefineTerms { active_context, .. } => active_context,
			Self::Normal(active_context) => active_context
		}
	}
}

pub fn expand_iri<'a, 'b: 'a, T, F, R>(mut args: IRIExpansionArguments<'a, 'b, T, F, R>, value: &'a str,
		document_relative: bool, vocab: bool) -> Result<Option<String>> where
	T: ForeignMutableJson + BuildableJson,
	F: Fn(&str, &Option<LoadDocumentOptions>) -> R,
	R: Future<Output = Result<RemoteDocument<T>>>
{
	if is_jsonld_keyword(value) || value == "" { return Ok(Some(value.to_string())) }
	if looks_like_a_jsonld_keyword(value) { return Ok(None) }
	if let IRIExpansionArguments::DefineTerms { ref mut active_context, local_context, ref mut defined, options } = args {
		if local_context.contains(value) && defined.get(value).map_or(false, |v| !v) {
			create_term_definition(active_context, local_context, value, defined, options, None, false, false)?;
		}
	}
	if let Some(definition) = args.active_context().term_definitions.get(value) {
		if vocab || definition.iri.as_ref().map_or(false, |iri| is_jsonld_keyword(&iri)) {
			return Ok(definition.iri.to_owned());
		}
	}
	if let Some((prefix, suffix)) = as_compact_iri(value) {
		if prefix == "_" || suffix.starts_with("//") {
			return Ok(Some(value.to_string()));
		}
		if let IRIExpansionArguments::DefineTerms { ref mut active_context, local_context, ref mut defined, options } = args {
			if local_context.contains(prefix) && defined.get(prefix).map_or(false, |v| !v) {
				create_term_definition(active_context, local_context, prefix, defined, options, None, false, false)?;
			}
		}
		if_chain! {
			if let Some(definition) = args.active_context().term_definitions.get(prefix);
			if let Some(ref iri) = definition.iri;
			if definition.prefix;
			then { return Ok(Some(iri.to_owned() + suffix)); }
		}
		if is_iri(value) {
			return Ok(Some(value.to_string()));
		}
	}
	if vocab {
		if let Some(ref vocabulary_mapping) = args.active_context().vocabulary_mapping {
			return Ok(Some(vocabulary_mapping.to_owned() + value));
		}
	}
	if document_relative {
		return Ok(Some(
			resolve_with_str(value, args.active_context().base_iri.as_ref().map(|s| s.as_str()))
				.map_err(|e| err!(InvalidBaseIRI, , e))?.to_string()
		));
	}
	Ok(Some(value.to_string()))
}