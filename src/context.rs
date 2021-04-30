use std::collections::{HashMap, HashSet};
use std::future::Future;

use json_trait::{ForeignJson, ForeignMutableJson, BuildableJson, TypedJson, Object, Array};
use cc_traits::Get;

use url::Url;
use async_recursion::async_recursion;

use crate::{
	Context, JsonLdContext, JsonOrReference, OwnedJsonOrReference,
	JsonLdOptions, JsonLdProcessingMode, RemoteDocument, TermDefinition, Direction
};
use crate::util::{is_jsonld_keyword, looks_like_a_jsonld_keyword, resolve, resolve_with_str};
use crate::error::{Result, JsonLdErrorCode::*, JsonLdError};
use crate::remote::{load_remote, LoadDocumentOptions};
use crate::expand::expand_iri;

const MAX_CONTEXTS: usize = 1000; // The number's placeholder

fn process_language<'a: 'b, 'b, T: ForeignJson<'a>>(value: &'b T) -> Result<Option<String>> {
	Ok(match value.as_enum() {
		TypedJson::String(lang) => {
			Some(lang.to_owned())
		},
		TypedJson::Null => None,
		_ => return Err(InvalidDefaultLanguage.to_error(None))
	})
}

fn process_direction<'a: 'b, 'b, T: ForeignJson<'a>>(value: &'b T) -> Result<Option<Direction>> {
	Ok(match value.as_enum() {
		TypedJson::String(direction) => {
			Some(match direction.as_str() {
				"ltr" => Direction::LTR,
				"rtl" => Direction::RTL,
				_ => return Err(InvalidBaseDirection.to_error(None))
			})
		},
		TypedJson::Null => None,
		_ => return Err(InvalidBaseDirection.to_error(None))
	})
}

fn process_container(container: Vec<String>) -> Result<Vec<String>> {
	let len = container.len();
	if container.iter().any(|s| s == "@list") && len > 1 {
		return Err(InvalidContainerMapping.to_error(None));
	} else if container.iter().any(|s| s == "@graph") && container.iter()
			.any(|s| s != "@graph" && s != "@id" && s != "@index" && s != "@set") {
		return Err(InvalidContainerMapping.to_error(None));
	} else if len > 1 && (container.iter().all(|s| s != "@set") || len != 2) {
		return Err(InvalidContainerMapping.to_error(None));
	}
	Ok(container)
}

#[async_recursion(?Send)]
pub async fn process_context<'a: 'b, 'b, T, F, R>(
		active_context: &'b mut Context<'a, T>, local_context: JsonLdContext<'a, 'b, T>,
		base_url: Option<&'b Url>, options: &'b JsonLdOptions<'a, T, F, R>, remote_contexts: &'b HashSet<Url>, override_protected: bool,
		mut propagate: bool, validate_scoped_context: bool) -> Result<Context<'a, T>> where
	T: ForeignMutableJson<'a> + BuildableJson<'a>,
	F: Fn(&str, &Option<LoadDocumentOptions>) -> R,
	R: Future<Output = Result<RemoteDocument<'a, T>>>
{
	let mut result = active_context.clone();
	active_context.inverse_context = None;
	if local_context.len() == 1 {
		if let Some(JsonOrReference::JsonObject(ctx)) = local_context[0] {
			if let Some(v) = ctx.get("@propagate") {
				propagate = v.as_bool().ok_or(InvalidPropagateValue.to_error(None))?;
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
					let context = resolve(&iri, base_url)
						.map_err(|e| LoadingDocumentFailed.to_error(Some(Box::new(e))))?;
					if validate_scoped_context == false && remote_contexts.contains(&context) { continue; }
					if remote_contexts.len() > MAX_CONTEXTS { return Err(ContextOverflow.to_error(None)); }
					// 4)
					let loaded_context = todo!();
					result = process_context(&mut result, loaded_context, base_url, options, remote_contexts,
						false, true, validate_scoped_context).await?;
				},
				JsonOrReference::JsonObject(mut json) => {
					if let Some(version) = json.get("@version") {
						if version.as_number() != Some(Some(1.1)) { Err(InvalidVersionValue.to_error(None))? }
						if let JsonLdProcessingMode::JsonLd1_0 = options.processing_mode { return Err(ProcessingModeConflict.to_error(None)); }
					}
					if let Some(import_url) = json.get("@import") {
						if let JsonLdProcessingMode::JsonLd1_0 = options.processing_mode { return Err(ProcessingModeConflict.to_error(None)); }
						if let Some(import_url) = import_url.as_string() {
							let import = resolve(import_url, base_url)
								.map_err(|e| LoadingDocumentFailed.to_error(Some(Box::new(e))))?;
							let import = load_remote(import.as_str(), options, Some("http://www.w3.org/ns/json-ld#context".to_string()),
								vec!["http://www.w3.org/ns/json-ld#context".to_string()]).await
									.map_err(|e| {
										if let LoadingDocumentFailed = e.code {
											JsonLdError { code: LoadingRemoteContextFailed, ..e }
										} else { e }
									})?;
							let import = import.document.to_parsed()
								.map_err(|e| LoadingRemoteContextFailed.to_error(Some(Box::new(e))))?;
							let import_context = import.get_attr("@context")
								.and_then(|ctx| ctx.as_object())
								.ok_or(InvalidRemoteContext.to_error(None))?;
							if import_context.contains("@import") { return Err(InvalidContextEntry.to_error(None)); }
							todo!();
						} else {
							Err(InvalidImportValue.to_error(None))?
						}
					}
					if let Some(value) = json.get("@base") {
						if remote_contexts.is_empty() {
							match value.as_enum() {
								TypedJson::String(iri) => {
									result.base_iri = Some(
										resolve_with_str(&iri, result.base_iri)
											.map_err(|e| InvalidBaseIRI.to_error(Some(Box::new(e))))?.to_string()
									);
								},
								TypedJson::Null => result.base_iri = None,
								_ => return Err(InvalidBaseIRI.with_description("not string or null", None))
							}
						}
					}
					if let Some(value) = json.get("@vocab") {
						result.vocabulary_mapping = match value.as_enum() {
							TypedJson::String(iri) => expand_iri(active_context, &iri, options, true, false, None, None)
								.map_err(|e| InvalidVocabMapping.to_error(Some(Box::new(e))))?,
							TypedJson::Null => None,
							_ => return Err(InvalidVocabMapping.with_description("not string or null", None))
						}
					}
					if let Some(value) = json.get("@language") {
						result.default_language = process_language(value)?;
					}
					if let Some(value) = json.get("@direction") {
						if let JsonLdProcessingMode::JsonLd1_0 = options.processing_mode { return Err(ProcessingModeConflict.to_error(None)); }
						result.default_base_direction = process_direction(value)?;
					}
					if json.contains("@propagate") {
						if let JsonLdProcessingMode::JsonLd1_0 = options.processing_mode { return Err(ProcessingModeConflict.to_error(None)); }
					}

					let mut defined = HashMap::<String, bool>::new();
					let protected = json.get("@protected").map(|v| v.as_bool().ok_or(InvalidProtectedValue.to_error(None)))
						.unwrap_or(Ok(false))?;
					for (key, _) in json.iter() {
						match key {
							"@base" | "@direction" | "@import" | "@language" |
								"@propagate" | "@protected" | "@version" | "@vocab" => {},
							_ => {
								create_term_definition(&mut result, &mut json, key, &mut defined, options,
									base_url, protected, override_protected, remote_contexts.clone())?;

								// Scoped context validation; In the specification, this is done inside Create Term Definition,
								// but doing it here is preferred, because minimizing async part of the code help keep things simple
								if let Some(context) = json.get(key).unwrap().get_attr("@context") {
									let context: JsonOrReference<'a, 'b, T> = match context.as_enum() {
										TypedJson::String(context) => JsonOrReference::Reference(context),
										TypedJson::Object(context) => JsonOrReference::JsonObject(context),
										_ => return Err(InvalidScopedContext.to_error(None))
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
				return Err(InvalidContextNullification.to_error(None));
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

pub fn create_term_definition<'a: 'b, 'b, T, F, R>(
		active_context: &'b mut Context<'a, T>, local_context: &'b T::Object, term: &'b str,
		defined: &'b mut HashMap<String, bool>, options: &'b JsonLdOptions<'a, T, F, R>, base_url: Option<&Url>, protected: bool,
		override_protected: bool, remote_contexts: HashSet<Url>) -> Result<()> where
	T: ForeignMutableJson<'a> + BuildableJson<'a>,
	F: Fn(&str, &Option<LoadDocumentOptions>) -> R,
	R: Future<Output = Result<RemoteDocument<'a, T>>>
{
	if let Some(defined) = defined.get(term) {
		if *defined { return Ok(()); }
		else { return Err(CyclicIRIMapping.to_error(None)); }
	}
	if term == "" { return Err(InvalidTermDefinition.to_error(None)); }
	let value_enum =  local_context.get(term).unwrap().as_enum();
	if term == "@type" {
		if let JsonLdProcessingMode::JsonLd1_0 = options.processing_mode { return Err(KeywordRedefinition.to_error(None)); }
		if let TypedJson::Object(value) = value_enum {
			for (key, value) in value.iter() {
				match key {
					"@container" if value.as_string() == Some("@set") => (),
					"@protected" => (),
					_ => return Err(KeywordRedefinition.to_error(None))
				}
			}
		} else {
			return Err(KeywordRedefinition.to_error(None));
		}
	} else {
		if is_jsonld_keyword(term) { return Err(KeywordRedefinition.to_error(None)); }
		if looks_like_a_jsonld_keyword(term) { return Ok(()) }
	}

	let previous_definition = active_context.term_definitions.remove(term);

	let mut definition: TermDefinition<'a, T> = TermDefinition {
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
						return Err(InvalidIRIMapping.to_error(None));
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
			return Err(InvalidIRIMapping.to_error(None));
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
						_ => process_id(None, false)?
					}
				}
			}
			if let Some(protected) = value.get("@protected") {
				if let JsonLdProcessingMode::JsonLd1_0 = options.processing_mode { return Err(InvalidTermDefinition.to_error(None)); }
				definition.protected = protected.as_bool().unwrap();
			}
			if let Some(t) = value.get("@type") {
				let t = t.as_string().ok_or(InvalidTypeMapping.to_error(None))?;
				let t = expand_iri(active_context, t, options, false, false, Some(local_context), Some(defined))?;
				if let Some(ref t) = t {
					match t.as_str() {
						"@json" | "@none" => {
							if let JsonLdProcessingMode::JsonLd1_0 = options.processing_mode { return Err(InvalidTermDefinition.to_error(None)); }
						},
						"@id" | "@vocab" => {},
						_ => return Err(InvalidTypeMapping.to_error(None))
					}
				}
				definition.type_mapping = t;
			}
			if let Some(reverse) = value.get("@reverse") {
				if value.contains("@id") || value.contains("@nest") { return Err(InvalidReverseProperty.to_error(None)); }
				let reverse = reverse.as_string().ok_or(InvalidIRIMapping.to_error(None))?;
				if looks_like_a_jsonld_keyword(reverse) { return Ok(()) }
				definition.iri = expand_iri(active_context, reverse, options, false, false, Some(local_context), Some(defined))?;
				if let Some(container) = value.get("@container") {
					definition.container_mapping = match container.as_enum() {
						TypedJson::String(container) => {
							match container.as_str() {
								"@set" | "@index" => Some(vec![container.to_owned()]),
								_ => return Err(InvalidReverseProperty.to_error(None))
							}
						},
						TypedJson::Null => None,
						_ => return Err(InvalidReverseProperty.to_error(None))
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
							container.iter().map(|v| v.as_string().map(|s| s.to_string()).ok_or(InvalidContainerMapping.to_error(None)))
								.collect::<Result<Vec<String>>>()?
						)?
					},
					TypedJson::String(container) => process_container(vec![container.to_owned()])?,
					_ => return Err(InvalidContainerMapping.to_error(None))
				});
				if let Some(ref container) = definition.container_mapping {
					if container.iter().any(|s| s == "@type") {
						match definition.type_mapping.as_ref().map(|s| s.as_str()) {
							None => definition.type_mapping = Some("@id".to_string()),
							Some("@id") | Some("vocab") => {},
							_ => return Err(InvalidTypeMapping.to_error(None))
						}
					}
				}
			}
			if let Some(index) = value.get("@index") {
				if let JsonLdProcessingMode::JsonLd1_0 = options.processing_mode { return Err(InvalidTermDefinition.to_error(None)); }
				if !definition.container_mapping.as_ref().map_or(false, |v| v.iter().any(|s| s == "@index")) {
					return Err(InvalidTermDefinition.to_error(None));
				}
				let index = index.as_string().ok_or(InvalidTermDefinition.to_error(None))?;
				definition.index_mapping = Some(index.to_string());
			}
			if let Some(context_raw) = value.get("@context") {
				if let JsonLdProcessingMode::JsonLd1_0 = options.processing_mode { return Err(InvalidTermDefinition.to_error(None)); }
				let context_owned = match context_raw.as_enum() {
					TypedJson::String(context) => OwnedJsonOrReference::Reference(context.to_owned()),
					TypedJson::Object(context) => OwnedJsonOrReference::JsonObject(context.to_owned()),
					_ => return Err(InvalidScopedContext.to_error(None))
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
				if let JsonLdProcessingMode::JsonLd1_0 = options.processing_mode { return Err(InvalidTermDefinition.to_error(None)); }
				let nest = nest.as_string().ok_or(InvalidNestValue.to_error(None))?;
				if is_jsonld_keyword(nest) && nest != "@nest" { return Err(InvalidNestValue.to_error(None)); }
				definition.nest_value = Some(nest.to_string());
			}
			if let Some(prefix) = value.get("@prefix") {
				if let JsonLdProcessingMode::JsonLd1_0 = options.processing_mode { return Err(InvalidTermDefinition.to_error(None)); }
				if term.contains(":") || term.contains("/") { return Err(InvalidTermDefinition.to_error(None)); }
				definition.prefix = prefix.as_bool().ok_or(InvalidPrefixValue.to_error(None))?;
				if let Some(ref iri) = definition.iri {
					if definition.prefix && is_jsonld_keyword(&iri) { return Err(InvalidTermDefinition.to_error(None)); }
				}
			}
			for (key, _) in value.iter() {
				match key {
					"@id" | "@reverse" | "@container" | "@context" | "@direction" | "@index" |
						"@language" | "@nest" | "@prefix" | "@protected" | "@type" => {},
					_ => return Err(InvalidTermDefinition.to_error(None))
				}
			}
		},
		_ => return Err(InvalidTermDefinition.to_error(None))
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
				return Err(ProtectedTermRedefinition.to_error(None));
			}
			definition = previous_definition;
		}
	}

	active_context.term_definitions.insert(term.to_string(), definition);
	defined.insert(term.to_string(), true);

	Ok(())
}