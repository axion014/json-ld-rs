use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::borrow::Cow;

use json_trait::{ForeignJson, ForeignMutableJson, BuildableJson, TypedJson, Object, Array};
use cc_traits::{Get, MapInsert};

use url::Url;
use async_recursion::async_recursion;

use crate::{
	Context, JsonLdContext, JsonOrReference,
	JsonLdOptions, JsonLdProcessingMode, RemoteDocument, TermDefinition, Direction
};
use crate::util::{is_jsonld_keyword, looks_like_a_jsonld_keyword, resolve, resolve_with_str, is_iri, as_compact_iri};
use crate::error::{Result, JsonLdErrorCode::*, JsonLdError};
use crate::remote::{load_remote, LoadDocumentOptions};
use crate::expand::expand_iri;

const MAX_CONTEXTS: usize = 1000; // The number's placeholder

fn process_language<T: ForeignJson>(value: &T) -> Result<Option<String>> {
	Ok(match value.as_enum() {
		TypedJson::String(lang) => Some(lang.to_owned()),
		TypedJson::Null => None,
		_ => return Err(err!(InvalidDefaultLanguage))
	})
}

fn process_direction<T: ForeignJson>(value: &T) -> Result<Option<Direction>> {
	Ok(match value.as_enum() {
		TypedJson::String(direction) => {
			Some(match direction.as_str() {
				"ltr" => Direction::LTR,
				"rtl" => Direction::RTL,
				_ => return Err(err!(InvalidBaseDirection))
			})
		},
		TypedJson::Null => None,
		_ => return Err(err!(InvalidBaseDirection))
	})
}

fn process_container(container: Vec<String>) -> Result<Vec<String>> {
	let len = container.len();
	if container.iter().any(|s| s == "@list") && len > 1 {
		return Err(err!(InvalidContainerMapping));
	} else if container.iter().any(|s| s == "@graph") && container.iter()
			.any(|s| s != "@graph" && s != "@id" && s != "@index" && s != "@set") {
		return Err(err!(InvalidContainerMapping));
	} else if len > 1 && (container.iter().all(|s| s != "@set") || len != 2) {
		return Err(err!(InvalidContainerMapping));
	}
	Ok(container)
}

#[async_recursion(?Send)]
pub async fn process_context<'a: 'b, 'b, T, F, R>(
		active_context: &'b mut Context<'a, T>, local_context: JsonLdContext<'b, T>, base_url: Option<&'b Url>,
		options: &JsonLdOptions<T, F, R>, remote_contexts: &HashSet<Url>, override_protected: bool,
		mut propagate: bool, validate_scoped_context: bool) -> Result<Context<'a, T>> where
	T: ForeignMutableJson + BuildableJson,
	F: Fn(&str, &Option<LoadDocumentOptions>) -> R,
	R: Future<Output = Result<RemoteDocument<T>>>
{
	let mut result = active_context.clone();
	active_context.inverse_context = None;
	if local_context.len() == 1 {
		if let Some(JsonOrReference::JsonObject(ref ctx)) = local_context[0] {
			if let Some(v) = ctx.get("@propagate") {
				propagate = v.as_bool().ok_or(err!(InvalidPropagateValue))?;
			}
		}
	}
	if propagate == false && result.previous_context.is_none() {
		result.previous_context = Some(Box::new(active_context.clone()));
	}
	for context in local_context {
		if let Some(context) = context {
			match context {
				JsonOrReference::Reference(iri) => {
					let context = resolve(&iri, base_url).map_err(|e| err!(LoadingDocumentFailed, , e))?;
					if validate_scoped_context == false && remote_contexts.contains(&context) { continue; }
					if remote_contexts.len() > MAX_CONTEXTS { return Err(err!(ContextOverflow)); }
					// 4)
					let loaded_context = todo!();
					result = process_context(&mut result, loaded_context, base_url, options, remote_contexts,
						false, true, validate_scoped_context).await?;
				},
				JsonOrReference::JsonObject(mut json) => {
					if let Some(version) = json.get("@version") {
						if version.as_number() != Some(Some(1.1)) { return Err(err!(InvalidVersionValue)) }
						if let JsonLdProcessingMode::JsonLd1_0 = options.processing_mode { return Err(err!(ProcessingModeConflict)); }
					}
					if let Some(import_url) = json.get("@import") {
						if let JsonLdProcessingMode::JsonLd1_0 = options.processing_mode { return Err(err!(ProcessingModeConflict)); }
						if let Some(import_url) = import_url.as_string() {
							let import = resolve(import_url, base_url).map_err(|e| err!(LoadingDocumentFailed, , e))?;
							let import = load_remote(import.as_str(), options, Some("http://www.w3.org/ns/json-ld#context".to_string()),
								vec!["http://www.w3.org/ns/json-ld#context".to_string()]).await
									.map_err(|e| {
										if let LoadingDocumentFailed = e.code {
											JsonLdError { code: LoadingRemoteContextFailed, ..e }
										} else { e }
									})?;
							let import = import.document.to_parsed().map_err(|e| err!(LoadingRemoteContextFailed, , e))?;
							let import_context = import.get_attr("@context")
								.and_then(|ctx| ctx.as_object())
								.ok_or(err!(InvalidRemoteContext))?;
							if import_context.contains("@import") { return Err(err!(InvalidContextEntry)); }
							todo!();
						} else {
							Err(err!(InvalidImportValue))?
						}
					}
					if let Some(value) = json.get("@base") {
						if remote_contexts.is_empty() {
							match value.as_enum() {
								TypedJson::String(iri) => {
									result.base_iri = Some(
										resolve_with_str(&iri, result.base_iri)
											.map_err(|e| err!(InvalidBaseIRI, , e))?.to_string()
									);
								},
								TypedJson::Null => result.base_iri = None,
								_ => return Err(err!(InvalidBaseIRI, "not string or null"))
							}
						}
					}
					if let Some(value) = json.get("@vocab") {
						result.vocabulary_mapping = match value.as_enum() {
							TypedJson::String(iri) => expand_iri(active_context, &iri, options, true, false, None, None)
								.map_err(|e| err!(InvalidVocabMapping, , e))?,
							TypedJson::Null => None,
							_ => return Err(err!(InvalidVocabMapping, "not string or null"))
						}
					}
					if let Some(value) = json.get("@language") {
						result.default_language = process_language(value)?;
					}
					if let Some(value) = json.get("@direction") {
						if let JsonLdProcessingMode::JsonLd1_0 = options.processing_mode { return Err(err!(ProcessingModeConflict)); }
						result.default_base_direction = process_direction(value)?;
					}
					if json.contains("@propagate") {
						if let JsonLdProcessingMode::JsonLd1_0 = options.processing_mode { return Err(err!(ProcessingModeConflict)); }
					}

					let mut defined = HashMap::<String, bool>::new();
					let protected = json.get("@protected").map(|v| v.as_bool().ok_or(err!(InvalidProtectedValue)))
						.unwrap_or(Ok(false))?;
					for (key, _) in json.iter() {
						match key {
							"@base" | "@direction" | "@import" | "@language" |
								"@propagate" | "@protected" | "@version" | "@vocab" => {},
							_ => {
								create_term_definition(&mut result, &json, key, &mut defined, options,
									base_url, protected, override_protected, remote_contexts.clone())?;

								// Scoped context validation; In the specification, this is done inside Create Term Definition,
								// but doing it here is preferred, because minimizing async part of the code help keep things simple
								if let Some(context) = json.get(key).unwrap().get_attr("@context") {
									let context = match context.as_enum() {
										TypedJson::String(context) => JsonOrReference::Reference(Cow::Borrowed(context)),
										TypedJson::Object(context) => JsonOrReference::JsonObject(Cow::Borrowed(context)),
										_ => return Err(err!(InvalidScopedContext))
									};
									process_context(active_context, vec![Some(context)], base_url, options, remote_contexts,
										true, true, false).await?;
								}
							}
						}
					}
				}
			}
		} else {
			if !override_protected && active_context.term_definitions.iter().any(|(_, def)| def.protected) {
				return Err(err!(InvalidContextNullification));
			}
			result = Context {
				term_definitions: HashMap::<String, TermDefinition<T>>::new(),
				base_iri: active_context.original_base_url.clone(),
				original_base_url: active_context.original_base_url.clone(),
				inverse_context: None,
				vocabulary_mapping: None,
				default_language: None,
				default_base_direction: None,
				previous_context: if !propagate {
					Some(Box::new(result))
				} else {
					None
				}
			};
		}
	}
	Ok(result)
}

pub fn create_term_definition<T, F, R>(
		active_context: &mut Context<'_, T>, local_context: &T::Object, term: &str,
		defined: &mut HashMap<String, bool>, options: &JsonLdOptions<T, F, R>, base_url: Option<&Url>,
		protected: bool, override_protected: bool, remote_contexts: HashSet<Url>) -> Result<()> where
	T: ForeignMutableJson + BuildableJson,
	F: Fn(&str, &Option<LoadDocumentOptions>) -> R,
	R: Future<Output = Result<RemoteDocument<T>>>
{
	if let Some(defined) = defined.get(term) {
		if *defined { return Ok(()); }
		else { return Err(err!(CyclicIRIMapping)); }
	}
	if term == "" { return Err(err!(InvalidTermDefinition)); }
	let value_enum =  local_context.get(term).unwrap().as_enum();
	if term == "@type" {
		if let JsonLdProcessingMode::JsonLd1_0 = options.processing_mode { return Err(err!(KeywordRedefinition)); }
		if let TypedJson::Object(value) = value_enum {
			for (key, value) in value.iter() {
				match key {
					"@container" if value.as_string() == Some("@set") => (),
					"@protected" => (),
					_ => return Err(err!(KeywordRedefinition))
				}
			}
		} else {
			return Err(err!(KeywordRedefinition));
		}
	} else {
		if is_jsonld_keyword(term) { return Err(err!(KeywordRedefinition)); }
		if looks_like_a_jsonld_keyword(term) { return Ok(()) }
	}

	let previous_definition = active_context.term_definitions.remove(term);

	let mut definition = TermDefinition {
		iri: None,
		prefix: false,
		protected,
		reverse_property: false,
		base_url: None,
		context: None,
		container_mapping: None,
		direction_mapping: None,
		index_mapping: None,
		language_mapping: None,
		nest_value: None,
		type_mapping: None
	};

	let mut process_id = |id, simple_term| {
		if let Some(id) = id {
			if id != term {
				if looks_like_a_jsonld_keyword(id) { return Ok(()) }
				definition.iri = expand_iri(active_context, id, options, false, false, Some(local_context), Some(defined))?;
				if term.starts_with(":") || term.ends_with(":") || term.contains("/") {
					defined.insert(term.to_string(), true);
					if definition.iri != expand_iri(active_context, id, options, false, false, Some(local_context), Some(defined))? {
						return Err(err!(InvalidIRIMapping));
					}
				}
				if !(term.contains(":") || term.contains("/")) && simple_term {

				}
				return Ok(());
			}
		}
		if term[1..].contains(":") {

		} else if term.contains("/") {
			definition.iri = expand_iri(active_context, term, options, false, false, Some(local_context), Some(defined))?;
		} else if term == "@type" {
			definition.iri = Some("@type".to_string());
		} else if let Some(ref vocabulary_mapping) = active_context.vocabulary_mapping {
			let mut iri = vocabulary_mapping.clone();
			iri.push_str(term);
			definition.iri = Some(iri);
		} else {
			return Err(err!(InvalidIRIMapping));
		}
		Ok(())
	};

	match value_enum {
		TypedJson::String(id) => process_id(Some(&id), true)?,
		TypedJson::Null => {},
		TypedJson::Object(value) => {
			if let Some(id) = value.get("@id") {
				if value.get("@reverse").is_none() {
					match id.as_enum() {
						TypedJson::String(id) => process_id(Some(&id), false)?,
						TypedJson::Null => {},
						_ => return Err(err!(InvalidIRIMapping))
					}
				}
			}
			if let Some(protected) = value.get("@protected") {
				if let JsonLdProcessingMode::JsonLd1_0 = options.processing_mode { return Err(err!(InvalidTermDefinition)); }
				definition.protected = protected.as_bool().unwrap();
			}
			if let Some(t) = value.get("@type") {
				let t = t.as_string().ok_or(err!(InvalidTypeMapping))?;
				let t = expand_iri(active_context, t, options, false, false, Some(local_context), Some(defined))?;
				if let Some(ref t) = t {
					match t.as_str() {
						"@json" | "@none" => {
							if let JsonLdProcessingMode::JsonLd1_0 = options.processing_mode { return Err(err!(InvalidTermDefinition)); }
						},
						"@id" | "@vocab" => {},
						_ => return Err(err!(InvalidTypeMapping))
					}
				}
				definition.type_mapping = t;
			}
			if let Some(reverse) = value.get("@reverse") {
				if value.contains("@id") || value.contains("@nest") { return Err(err!(InvalidReverseProperty)); }
				let reverse = reverse.as_string().ok_or(err!(InvalidIRIMapping))?;
				if looks_like_a_jsonld_keyword(reverse) { return Ok(()) }
				definition.iri = expand_iri(active_context, reverse, options, false, false, Some(local_context), Some(defined))?;
				if let Some(container) = value.get("@container") {
					definition.container_mapping = match container.as_enum() {
						TypedJson::String(container) => {
							match container.as_str() {
								"@set" | "@index" => Some(vec![container.to_owned()]),
								_ => return Err(err!(InvalidReverseProperty))
							}
						},
						TypedJson::Null => None,
						_ => return Err(err!(InvalidReverseProperty))
					};
				}
				definition.reverse_property = true;
				active_context.term_definitions.insert(term.to_string(), definition);
				defined.insert(term.to_string(), true);
				return Ok(());
			}
			if let Some(container) = value.get("@container") {
				definition.container_mapping = Some(match container.as_enum() {
					TypedJson::Array(container) => {
						process_container(
							container.iter().map(|v| v.as_string().map(|s| s.to_string()).ok_or(err!(InvalidContainerMapping)))
								.collect::<Result<Vec<String>>>()?
						)?
					},
					TypedJson::String(container) => process_container(vec![container.to_owned()])?,
					_ => return Err(err!(InvalidContainerMapping))
				});
				if let Some(ref container) = definition.container_mapping {
					if container.iter().any(|s| s == "@type") {
						match definition.type_mapping.as_ref().map(|s| s.as_str()) {
							None => definition.type_mapping = Some("@id".to_string()),
							Some("@id") | Some("vocab") => {},
							_ => return Err(err!(InvalidTypeMapping))
						}
					}
				}
			}
			if let Some(index) = value.get("@index") {
				if let JsonLdProcessingMode::JsonLd1_0 = options.processing_mode { return Err(err!(InvalidTermDefinition)); }
				if !definition.container_mapping.as_ref().map_or(false, |v| v.iter().any(|s| s == "@index")) {
					return Err(err!(InvalidTermDefinition));
				}
				let index = index.as_string().ok_or(err!(InvalidTermDefinition))?;
				definition.index_mapping = Some(index.to_string());
			}
			if let Some(context_raw) = value.get("@context") {
				if let JsonLdProcessingMode::JsonLd1_0 = options.processing_mode { return Err(err!(InvalidTermDefinition)); }
				let context_owned = match context_raw.as_enum() {
					TypedJson::String(context) => JsonOrReference::Reference(Cow::Owned(context.to_owned())),
					TypedJson::Object(context) => JsonOrReference::JsonObject(Cow::Owned(context.to_owned())),
					_ => return Err(err!(InvalidScopedContext))
				};
				definition.context = Some(context_owned);
				definition.base_url = base_url.map(|v| v.to_string());
			}
			if !value.contains("@type") {
				if let Some(language) = value.get("@language") {
					definition.language_mapping = process_language(language)?;
				}
				if let Some(direction) = value.get("@direction") {
					definition.direction_mapping = process_direction(direction)?;
				}
			}
			if let Some(nest) = value.get("@nest") {
				if let JsonLdProcessingMode::JsonLd1_0 = options.processing_mode { return Err(err!(InvalidTermDefinition)); }
				let nest = nest.as_string().ok_or(err!(InvalidNestValue))?;
				if is_jsonld_keyword(nest) && nest != "@nest" { return Err(err!(InvalidNestValue)); }
				definition.nest_value = Some(nest.to_string());
			}
			if let Some(prefix) = value.get("@prefix") {
				if let JsonLdProcessingMode::JsonLd1_0 = options.processing_mode { return Err(err!(InvalidTermDefinition)); }
				if term.contains(":") || term.contains("/") { return Err(err!(InvalidTermDefinition)); }
				definition.prefix = prefix.as_bool().ok_or(err!(InvalidPrefixValue))?;
				if let Some(ref iri) = definition.iri {
					if definition.prefix && is_jsonld_keyword(&iri) { return Err(err!(InvalidTermDefinition)); }
				}
			}
			for (key, _) in value.iter() {
				match key {
					"@id" | "@reverse" | "@container" | "@context" | "@direction" | "@index" |
						"@language" | "@nest" | "@prefix" | "@protected" | "@type" => {},
					_ => return Err(err!(InvalidTermDefinition))
				}
			}
		},
		_ => return Err(err!(InvalidTermDefinition))
	};
	if let Some(previous_definition) = previous_definition {
		if !override_protected && previous_definition.protected {
			// Check everything except `protected`
			if definition.iri != previous_definition.iri ||
					definition.prefix != previous_definition.prefix ||
					definition.reverse_property != previous_definition.reverse_property ||
					definition.base_url != previous_definition.base_url ||
					definition.context != previous_definition.context ||
					definition.container_mapping != previous_definition.container_mapping ||
					definition.direction_mapping != previous_definition.direction_mapping ||
					definition.index_mapping != previous_definition.index_mapping ||
					definition.language_mapping != previous_definition.language_mapping ||
					definition.nest_value != previous_definition.nest_value ||
					definition.type_mapping != previous_definition.type_mapping {
				return Err(err!(ProtectedTermRedefinition));
			}
			definition = previous_definition;
		}
	}

	active_context.term_definitions.insert(term.to_string(), definition);
	defined.insert(term.to_string(), true);

	Ok(())
}