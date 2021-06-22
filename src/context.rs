use std::collections::HashMap;
use std::borrow::Cow;
use std::iter::once;

use elsa::FrozenSet;

use futures::{future::{self, BoxFuture}, stream, StreamExt, TryStreamExt};

use json_trait::{ForeignJson, ForeignMutableJson, BuildableJson, typed_json::*, Object, Array};
use cc_traits::{Len, Get, Remove, MapInsert};

use url::Url;
use async_recursion::async_recursion;
use hashmap_entry_ownable::std_hash::EntryAPI;

use if_chain::if_chain;

use crate::{
	Context, JsonOrReference, LoadedContext, InverseContext,
	JsonLdOptions, JsonLdOptionsImpl, JsonLdProcessingMode, RemoteDocument, TermDefinition, Direction
};
use crate::util::{
	is_jsonld_keyword, looks_like_a_jsonld_keyword, resolve, is_iri, as_compact_iri, make_lang_dir, map_context
};
use crate::error::{Result, JsonLdErrorCode::*, JsonLdError};
use crate::remote::{load_remote, LoadDocumentOptions};
use crate::expand::expand_iri;
use crate::container::{Container, parse_container};

const MAX_CONTEXTS: usize = 25; // The number's placeholder

fn process_language<T: ForeignJson>(value: &T) -> Result<Option<String>> {
	Ok(match value.as_enum() {
		Borrowed::String(lang) => Some(lang.to_owned()),
		Borrowed::Null => None,
		_ => return Err(err!(InvalidDefaultLanguage))
	})
}

fn process_direction<T: ForeignJson>(value: &T, nullify: bool) -> Result<Option<Direction>> {
	Ok(match value.as_enum() {
		Borrowed::String(direction) => {
			Some(match direction.as_str() {
				"ltr" => Direction::LTR,
				"rtl" => Direction::RTL,
				_ => return Err(err!(InvalidBaseDirection))
			})
		},
		Borrowed::Null => if nullify { None } else { Some(Direction::None) },
		_ => return Err(err!(InvalidBaseDirection))
	})
}

#[async_recursion(?Send)]
pub(crate) async fn process_context<'a, 'b, T, F>(
		active_context: &'b Context<'a, T>, local_context: &[Option<JsonOrReference<'a, T>>], base_url: Option<&'b Url>,
		options: &JsonLdOptionsImpl<'a, T, F>, remote_contexts: &FrozenSet<Box<Url>>, override_protected: bool,
		mut propagate: bool, mut validate_scoped_context: bool) -> Result<Context<'a, T>> where
	T: ForeignMutableJson + BuildableJson,
	F: for<'c> Fn(&'c str, &'c Option<LoadDocumentOptions>) -> BoxFuture<'c, Result<RemoteDocument<T>>>
{
	let mut result = active_context.clone();
	result.inverse_context.take();
	if_chain! {
		if local_context.len() == 1;
		if let Some(JsonOrReference::JsonObject(ref ctx)) = local_context[0];
		if let Some(v) = ctx.get("@propagate");
		then { propagate = v.as_bool().ok_or(err!(InvalidPropagateValue))?; }
	}
	if propagate == false && result.previous_context.is_none() {
		result.previous_context = Some(Box::new(active_context.clone()));
	}
	local_context.iter().map(|context| async move {
		Ok(Some(match context {
			Some(JsonOrReference::Reference(iri)) => {
				let mut iri = Cow::Borrowed(&**iri);
				let loaded_context = loop {
					let context = resolve(&iri, base_url).map_err(|e| err!(LoadingDocumentFailed, , e))?;
					if validate_scoped_context == false && remote_contexts.contains(&context) { return Ok(None); }
					if remote_contexts.len() > MAX_CONTEXTS { return Err(err!(ContextOverflow)); }
					remote_contexts.insert(Box::new(context.clone()));
					if let Some(loaded_context) = options.loaded_contexts.get(&context) {
						break loaded_context;
					} else {
						let context_document = load_remote(context.as_str(), options.inner,
							Some("http://www.w3.org/ns/json-ld#context".to_string()),
							vec!["http://www.w3.org/ns/json-ld#context".to_string()]).await
							.map_err(|e| err!(LoadingRemoteContextFailed, , e))?;
						let base_url = Url::parse(&context_document.document_url).map_err(|e| err!(LoadingRemoteContextFailed, , e))?;
						let loaded_context = context_document.document.to_parsed()
							.map_err(|e| err!(LoadingRemoteContextFailed, , e))?
							.as_object_mut()
							.and_then(|obj| obj.remove("@context"));
						match loaded_context.map(|ctx| ctx.into_enum()) {
							Some(Owned::Object(ctx)) => {
								break options.loaded_contexts.insert(context, Box::new(LoadedContext { context: ctx, base_url }));
							},
							Some(Owned::String(redirection_iri)) => iri = Cow::Owned(redirection_iri),
							_ => return Err(err!(InvalidRemoteContext))
						};
						validate_scoped_context = false;
					};
				};
				Some((Cow::Borrowed(&loaded_context.context), Some(&loaded_context.base_url)))
			},
			Some(JsonOrReference::JsonObject(json)) => Some((Cow::Borrowed(&**json), None)),
			None => None
		}))
	}).collect::<stream::FuturesOrdered<_>>().filter_map(|context| future::ready(context.transpose()))
			.try_fold(result, |mut result, context| async move {
		if let Some((mut json, base)) = context {
			let base_url = base.or(base_url);
			if let Some(version) = json.get("@version") {
				if version.as_number() != Some(Some(1.1)) { return Err(err!(InvalidVersionValue)) }
				if let JsonLdProcessingMode::JsonLd1_0 = options.inner.processing_mode { return Err(err!(ProcessingModeConflict)); }
			}
			if let Some(import_url) = json.get("@import") {
				if let JsonLdProcessingMode::JsonLd1_0 = options.inner.processing_mode { return Err(err!(InvalidContextEntry)); }
				if let Some(import_url) = import_url.as_string() {
					let import = resolve(import_url, base_url).map_err(|e| err!(LoadingDocumentFailed, , e))?;
					let import = load_remote(import.as_str(), options.inner,
						Some("http://www.w3.org/ns/json-ld#context".to_string()),
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
					for (key, value) in import_context.iter() {
						if json.get(key).is_none() {
							json.to_mut().insert(key.to_string(), value.clone());
						}
					}
				} else {
					Err(err!(InvalidImportValue))?
				}
			}
			if let Some(value) = json.get("@base") {
				if remote_contexts.is_empty() {
					match value.as_enum() {
						Borrowed::String(iri) => {
							result.base_iri = Some(resolve(&iri, result.base_iri.as_ref())
								.map_err(|e| err!(InvalidBaseIRI, , e))?);
						},
						Borrowed::Null => result.base_iri = None,
						_ => return Err(err!(InvalidBaseIRI, "not string or null"))
					}
				}
			}
			if let Some(value) = json.get("@vocab") {
				result.vocabulary_mapping = match value.as_enum() {
					Borrowed::String(iri) => expand_iri!(&result, &iri, true)
						.map_err(|e| err!(InvalidVocabMapping, , e))?,
					Borrowed::Null => None,
					_ => return Err(err!(InvalidVocabMapping, "not string or null"))
				}
			}
			if let Some(value) = json.get("@language") {
				result.default_language = process_language(value)?;
			}
			if let Some(value) = json.get("@direction") {
				if let JsonLdProcessingMode::JsonLd1_0 = options.inner.processing_mode { return Err(err!(ProcessingModeConflict)); }
				result.default_base_direction = process_direction(value, true)?;
			}
			if json.contains("@propagate") {
				if let JsonLdProcessingMode::JsonLd1_0 = options.inner.processing_mode { return Err(err!(InvalidContextEntry)); }
			}

			let mut defined = HashMap::<String, bool>::new();
			let protected = json.get("@protected").map_or(Ok(false), |v| v.as_bool().ok_or(err!(InvalidProtectedValue)))?;
			for (key, value) in json.iter() {
				match key {
					"@base" | "@direction" | "@import" | "@language" |
						"@propagate" | "@protected" | "@version" | "@vocab" => {},
					_ => {
						create_term_definition(&mut result, &json, key, value.as_enum(), &mut defined, options.inner,
							base_url, protected, override_protected)?;

						// Scoped context validation; In the specification, this is done inside Create Term Definition,
						// but doing it here is preferred, because minimizing async part of the code help keep things simple
						if value.get_attr("@context").is_some() {
							process_context(&result, &result.term_definitions.get(key).unwrap().context,
								base_url, options, remote_contexts, true, true, false).await
								.map_err(|e| err!(InvalidScopedContext, , e))?;
						}
					}
				}
			}
			Ok(result)
		} else {
			if !override_protected && active_context.term_definitions.iter().any(|(_, def)| def.protected) {
				return Err(err!(InvalidContextNullification));
			}
			Ok(Context {
				base_iri: active_context.original_base_url.clone(),
				original_base_url: active_context.original_base_url.clone(),
				previous_context: if !propagate {
					Some(Box::new(result))
				} else {
					None
				},
				..Context::default()
			})
		}
	}).await
}

pub fn create_term_definition<'a, T, F>(
		active_context: &mut Context<'a, T>, local_context: &T::Object, term: &str, value: Borrowed<T>, defined: &mut HashMap<String, bool>,
		options: &JsonLdOptions<'a, T, F>, base_url: Option<&Url>, protected: bool, override_protected: bool) -> Result<()> where
	T: ForeignMutableJson + BuildableJson,
	F: for<'b> Fn(&'b str, &'b Option<LoadDocumentOptions>) -> BoxFuture<'b, Result<RemoteDocument<T>>>
{
	if let Some(defined) = defined.get(term) {
		if *defined { return Ok(()); }
		else { return Err(err!(CyclicIRIMapping)); }
	}
	if term == "" { return Err(err!(InvalidTermDefinition)); }
	defined.insert(term.to_string(), false);
	if term == "@type" {
		if let JsonLdProcessingMode::JsonLd1_0 = options.processing_mode { return Err(err!(KeywordRedefinition)); }
		if let Borrowed::Object(value) = value {
			if value.is_empty() { return Err(err!(KeywordRedefinition)); }
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
		context: vec![],
		container_mapping: container!(None),
		direction_mapping: None,
		index_mapping: None,
		language_mapping: None,
		nest_value: None,
		type_mapping: None
	};

	let mut process_id = |id, simple_term| {
		if let Some(id) = id {
			if id != term {
				if !is_jsonld_keyword(id) && looks_like_a_jsonld_keyword(id) { return Ok(()) }
				definition.iri = process_context_iri!(active_context, id, local_context, defined, options)?;
				if term.starts_with(":") || term.ends_with(":") || term.contains("/") {
					*defined.get_mut(term).unwrap() = true;
					if definition.iri != process_context_iri!(active_context, id, local_context, defined, options)? {
						return Err(err!(InvalidIRIMapping));
					}
				}
				if let Some(ref iri) = definition.iri {
					if !(term.contains(":") || term.contains("/")) && simple_term &&
							iri.starts_with("_") || iri.ends_with(&[':', '/', '?', '#', '[', ']', '@'] as &[_]) {
						definition.prefix = true;
					}
				}
				return Ok(());
			}
		}
		if let Some((prefix, suffix)) = as_compact_iri(term) {
			if let Some(prefix_definition) = local_context.get(prefix) {
				create_term_definition(active_context, local_context, prefix, prefix_definition.as_enum(), defined, options,
					None, false, false)?;
			}
			if let Some(prefix_definition) = active_context.term_definitions.get(prefix) {
				// FIXME: not sure what to do when prefix_definition.iri is None
				definition.iri = Some(prefix_definition.iri.clone().unwrap() + suffix);
			} else {
				definition.iri = Some(term.to_string());
			}
		} else if term.contains("/") {
			definition.iri = expand_iri!(active_context, term)?;
			if !definition.iri.as_ref().map_or(false, |s| is_iri(s)) { return Err(err!(InvalidIRIMapping)); }
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

	match value {
		Borrowed::String(id) => process_id(Some(&id), true)?,
		Borrowed::Null => {},
		Borrowed::Object(value) => {
			if value.get("@reverse").is_none() {
				if let Some(id) = value.get("@id") {
					match id.as_enum() {
						Borrowed::String(id) => process_id(Some(&id), false)?,
						Borrowed::Null => {},
						_ => return Err(err!(InvalidIRIMapping))
					}
				} else {
					process_id(None, false)?;
				}
			}
			if let Some(protected) = value.get("@protected") {
				if let JsonLdProcessingMode::JsonLd1_0 = options.processing_mode { return Err(err!(InvalidTermDefinition)); }
				definition.protected = protected.as_bool().unwrap();
			}
			if let Some(ty) = value.get("@type") {
				let ty = ty.as_string().ok_or(err!(InvalidTypeMapping))?;
				let ty = process_context_iri!(active_context, ty, local_context, defined, options)?;
				if let Some(ref ty) = ty {
					match ty.as_str() {
						"@json" | "@none" => {
							if let JsonLdProcessingMode::JsonLd1_0 = options.processing_mode { return Err(err!(InvalidTypeMapping)); }
						},
						"@id" | "@vocab" => {},
						_ => if !is_iri(ty) { return Err(err!(InvalidTypeMapping)); }
					}
				}
				definition.type_mapping = ty;
			}
			if let Some(reverse) = value.get("@reverse") {
				if value.contains("@id") || value.contains("@nest") { return Err(err!(InvalidReverseProperty)); }
				let reverse = reverse.as_string().ok_or(err!(InvalidIRIMapping))?;
				if looks_like_a_jsonld_keyword(reverse) { return Ok(()) }
				definition.iri = process_context_iri!(active_context, reverse, local_context, defined, options)?;
				if !is_iri(definition.iri.as_ref().unwrap()) { return Err(err!(InvalidIRIMapping)); }
				if let Some(container) = value.get("@container") {
					match container.as_enum() {
						Borrowed::String(container) => match container.as_str() {
							"@set" | "@index" => definition.container_mapping = parse_container(once(container.as_str()))?,
							_ => return Err(err!(InvalidReverseProperty))
						},
						Borrowed::Null => (),
						_ => return Err(err!(InvalidReverseProperty))
					};
				}
				definition.reverse_property = true;
				active_context.term_definitions.insert(term.to_string().into(), definition);
				*defined.get_mut(term).unwrap() = true;
				return Ok(());
			}
			if let Some(container) = value.get("@container") {
				definition.container_mapping = match container.as_enum() {
					Borrowed::Array(container) if options.processing_mode != JsonLdProcessingMode::JsonLd1_0 => {
						parse_container(container.iter()
							.map(|v| v.as_string().ok_or(err!(InvalidContainerMapping)))
							.collect::<Result<Vec<&str>>>()?)?
					},
					Borrowed::String(container) => {
						if options.processing_mode == JsonLdProcessingMode::JsonLd1_0 {
							if let "@graph" | "@id" | "@type" = container.as_str() {
								return Err(err!(InvalidContainerMapping));
							}
						}
						parse_container(once(container.as_str()))?
					},
					_ => return Err(err!(InvalidContainerMapping))
				};
				if let TypeContainer!() = definition.container_mapping {
					match definition.type_mapping.as_deref() {
						None => definition.type_mapping = Some("@id".to_string()),
						Some("@id") | Some("vocab") => {},
						_ => return Err(err!(InvalidTypeMapping))
					}
				}
			}
			if let Some(index) = value.get("@index") {
				if let JsonLdProcessingMode::JsonLd1_0 = options.processing_mode { return Err(err!(InvalidTermDefinition)); }
				if !definition.container_mapping.is_index() {
					return Err(err!(InvalidTermDefinition));
				}
				let index = index.as_string().ok_or(err!(InvalidTermDefinition))?;
				definition.index_mapping = Some(index.to_string());
			}
			if let Some(context) = value.get("@context") {
				if let JsonLdProcessingMode::JsonLd1_0 = options.processing_mode { return Err(err!(InvalidTermDefinition)); }
				let context = map_context(Cow::Owned(context.clone())).map_err(|e| err!(InvalidScopedContext, , e))?;
				definition.context = context;
				definition.base_url = base_url.cloned();
			}
			if !value.contains("@type") {
				if let Some(language) = value.get("@language") {
					definition.language_mapping = Some(process_language(language)?);
				}
				if let Some(direction) = value.get("@direction") {
					definition.direction_mapping = process_direction(direction, false)?;
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

	active_context.term_definitions.insert(term.to_string().into(), definition);
	*defined.get_mut(term).unwrap() = true;

	Ok(())
}

pub fn create_inverse_context<T: ForeignMutableJson + BuildableJson>(active_context: &Context<T>) -> InverseContext {
	let mut result = InverseContext::new();
	for (key, value) in active_context.term_definitions.iter() {
		let container_map = if let Some(ref iri) = value.iri {
			result.entry_ownable(iri).or_default()
		} else {
			continue;
		};
		let key = key.0.as_str();
		// concat all containers
		let type_language_map = container_map.entry_ownable(&value.container_mapping).or_insert_with(|| {
			let mut type_language_map = HashMap::new();
			type_language_map.insert("@language".to_string(), HashMap::new());
			type_language_map.insert("@type".to_string(), HashMap::new());
			let mut any = HashMap::new();
			any.insert("@none".to_string(), key.to_string());
			type_language_map.insert("@any".to_string(), any);
			type_language_map
		});
		let mut insert = |container, entry: &str| {
			type_language_map.get_mut(container).unwrap().entry_ownable(entry).or_insert_with(|| key.to_string());
		};
		if value.reverse_property { insert("@type", "@reverse"); }
		match value.type_mapping.as_ref().map(|s| s.as_str()) {
			Some("@none") => {
				insert("@language", "@any");
				insert("@type", "@any");
			},
			Some(type_mapping) => insert("@type", type_mapping),
			None => {
				let mut lang_dir = make_lang_dir(value.language_mapping.clone()
					.map(|lang| lang.unwrap_or_else(|| "@null".to_string())), value.direction_mapping.as_ref());
				if lang_dir == "" {
					lang_dir = make_lang_dir(active_context.default_language.clone(), active_context.default_base_direction.as_ref());
					insert("@language", "@none");
					insert("@type", "@none");
				}
				insert("@language", &lang_dir);
			}
		}
	}
	result
}

pub fn select_term<'a, T>(active_context: &'a Context<T>,
		var: &str, containers: Vec<Container>, type_language: &str, preferred_values: Vec<&str>) -> Option<&'a str> where
	T: ForeignMutableJson + BuildableJson
{
	let inverse_context = active_context.inverse_context.get_or_init(|| create_inverse_context(&active_context));
	let container_map = inverse_context.get(var).unwrap();
	// dbg!(container_map, type_language, &preferred_values);
	containers.iter().filter_map(|container| container_map.get(container))
		.map(|type_language_map| type_language_map.get(type_language).unwrap())
		.find_map(|value_map| preferred_values.iter()
			.find_map(|preferred_value| value_map.get(*preferred_value).map(|s| s.as_str())))
}