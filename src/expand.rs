use std::collections::HashMap;
use std::future::Future;

use json_trait::{ForeignMutableJson, BuildableJson};
use cc_traits::{Collection, Get};

use crate::{Context, JsonLdOptions, LoadDocumentOptions, RemoteDocument};
use crate::error::{Result, JsonLdErrorCode::InvalidBaseIRI};
use crate::util::{is_jsonld_keyword, looks_like_a_jsonld_keyword, is_iri, resolve_with_str};
use crate::context::create_term_definition;

pub fn expand_iri<'a, 'b: 'a, T, F, R>(
		active_context: &'a mut Context<'b, T>, value: &'a str, options: &'a JsonLdOptions<T, F, R>, document_relative: bool,
		vocab: bool, local_context: Option<&'a T::Object>, mut defined: Option<&'a mut HashMap<String, bool>>) -> Result<Option<String>> where
	T: ForeignMutableJson + BuildableJson + Clone,
	T::Object: Collection<Item = T> + for<'c> Get<&'c str> + Clone,
	F: Fn(&str, &Option<LoadDocumentOptions>) -> R,
	R: Future<Output = Result<RemoteDocument<T>>>,
{
	if is_jsonld_keyword(value) || value == "" { return Ok(Some(value.to_string())) }
	if looks_like_a_jsonld_keyword(value) { return Ok(None) }
	if let Some(local_context) = local_context {
		if let Some(ref mut defined) = defined {
			if local_context.contains(value) && defined.get(value).map_or(false, |v| !v) {
				create_term_definition(active_context, local_context, value, defined, options,
					None, false, false, HashSet::new())?;
			}
		}
	}
	if let Some(definition) = active_context.term_definitions.get(value) {
		if definition.iri.as_ref().map_or(false, |iri| is_jsonld_keyword(&iri)) {
			return Ok(definition.iri.to_owned());
		}
	}
	if vocab {
		if let Some(definition) = active_context.term_definitions.get(value) {
			return Ok(definition.iri.to_owned());
		}
	}
	if value[1..].contains(":") {
		let mut iter = value.splitn(2, |s| s == ':');
		let prefix = iter.next().unwrap();
		let suffix = iter.next().unwrap();
		if prefix == "_" || suffix.starts_with("//") {
			return Ok(Some(value.to_string()));
		}
		if let Some(local_context) = local_context {
			if let Some(defined) = defined {
				if local_context.contains(prefix) && defined.get(prefix).map_or(false, |v| !v) {
					create_term_definition(active_context, local_context, prefix, defined, options,
						None, false, false, HashSet::new())?;
				}
			}
		}
		if let Some(definition) = active_context.term_definitions.get(prefix) {
			if let Some(ref iri) = definition.iri {
				if definition.prefix {
					return Ok(Some(iri.to_owned() + suffix));
				}
			}
		}
		if is_iri(value) {
			return Ok(Some(value.to_string()));
		}
	}
	if vocab {
		if let Some(ref vocabulary_mapping) = active_context.vocabulary_mapping {
			return Ok(Some(vocabulary_mapping.to_owned() + value));
		}
	}
	if document_relative {
		return Ok(Some(
			resolve_with_str(value, active_context.base_iri.as_ref().map(|s| s.as_str()))
				.map_err(|e| InvalidBaseIRI.to_error(Some(Box::new(e))))?.to_string()
		));
	}
	Ok(Some(value.to_string()))
}