use std::collections::{HashMap, BTreeMap, BTreeSet};
use std::future::Future;
use std::borrow::Cow;

use json_trait::{ForeignMutableJson, BuildableJson, typed_json::{self, *}, Object, Array, MutableObject, json};
use cc_traits::{Get, GetMut, MapInsert, PushBack, Len, Remove};

use elsa::FrozenSet;
use maybe_owned::MaybeOwned;

use async_recursion::async_recursion;

use url::Url;

use if_chain::if_chain;

use crate::{
	Context, JsonLdOptions, JsonLdOptionsImpl, LoadDocumentOptions, RemoteDocument,
	TermDefinition, JsonLdProcessingMode, Direction
};
use crate::error::{Result, JsonLdErrorCode::*};
use crate::util::{
	is_jsonld_keyword, looks_like_a_jsonld_keyword, is_iri, is_graph_object,
	resolve_with_str, as_compact_iri, add_value, map_context
};
use crate::context::{process_context, create_term_definition};

#[async_recursion(?Send)]
pub async fn expand_internal<'a: 'b, 'b, T, F, R>(active_context: &'b Context<'a, T>, active_property: Option<&'b str>, element: T,
		base_url: Option<&'b Url>, options: &'b JsonLdOptionsImpl<'a, T, F, R>, from_map: bool) -> Result<Owned<T>>  where
	T: ForeignMutableJson + BuildableJson,
	F: Fn(&str, &Option<LoadDocumentOptions>) -> R + Clone,
	R: Future<Output = Result<RemoteDocument<T>>> + Clone
{
	let frame_expansion = options.inner.frame_expansion && active_property != Some("@default");
	let definition = active_property.and_then(|active_property| active_context.term_definitions.get(active_property));
	let property_scoped_context = definition.and_then(|definition| definition.context.as_ref());
	match element.into_enum() {
		Owned::Null => Ok(Owned::Null),
		Owned::Array(array) => {
			let mut result = T::empty_array();
			for item in array {
				let expanded_item = expand_internal(active_context, active_property, item, base_url,
					&JsonLdOptionsImpl {
						inner: &JsonLdOptions { frame_expansion, ..(*options.inner).clone() },
						loaded_contexts: MaybeOwned::Borrowed(&options.loaded_contexts)
					}, from_map).await?;
				match expanded_item {
					Owned::Array(array) => {
						if definition.and_then(|definition| definition.container_mapping.as_ref())
								.map_or(false, |container| container.contains("@list")) {
							result.push_back(json!(T, {"@list": array}));
						} else {
							result.extend(array);
						}
					},
					Owned::Null => {},
					_ => {
						result.push_back(expanded_item.into_untyped());
					}
				}
			}
			Ok(Owned::Array(result))
		},
		Owned::Object(obj) => {
			let mut active_context = Cow::Borrowed(if_chain! {
				if let Some(previous_context) = active_context.previous_context.as_deref();
				if !from_map;
				if if obj.len() == 1 {
					match expand_iri!(active_context, obj.iter().next().unwrap().0)?.as_deref() {
						Some("@value") | Some("@id") => false,
						_ => true
					}
				} else {
					!obj.iter().try_find(|(key, _)| Ok(expand_iri!(active_context, key)?.as_deref() == Some("@value")))?.is_some()
				};
				then { previous_context } else { active_context }
			});
			if let Some(property_scoped_context) = property_scoped_context {
				active_context = Cow::Owned(process_context(&active_context, vec![Some(property_scoped_context.clone())],
					definition.unwrap().base_url.as_ref(), options, &FrozenSet::new(), true, true, true).await?);
			}
			let mut obj = obj.into_iter().collect::<BTreeMap<_, _>>();
			if let Some(context) = obj.remove("@context") {
				active_context = Cow::Owned(process_context(&active_context,
					map_context(Cow::Owned(context))?,
					base_url, options, &FrozenSet::new(), false, true, true).await?);
			}
			let type_scoped_context = active_context.clone();
			let mut input_type = None;
			for (key, value) in obj.iter() {
				if expand_iri!(&active_context, key)?.as_deref() == Some("@type") {
					if let Some(value) = value.as_array() {
						for (term, context) in value.iter()
								.filter_map(|term| term.as_string())
								.collect::<BTreeSet<_>>().iter()
								.map(|term| {
									input_type = expand_iri!(&active_context, term)?;
									Ok(term)
								}).filter_map(|term| {
									term.map(|term| type_scoped_context.term_definitions.get(*term)
										.and_then(|definition| definition.context.as_ref())
										.map(|context| (term, context))).transpose()
								}).collect::<Result<BTreeMap<_, _>>>()? {
							active_context = Cow::Owned(process_context(&active_context, vec![Some((*context).clone())],
								active_context.term_definitions.get(*term).unwrap().base_url.as_ref(),
								options, &FrozenSet::new(), false, false, true).await?);
						}
					} else if let Some(term) = value.as_string() {
						input_type = expand_iri!(&active_context, term)?;
						if let Some(context) = type_scoped_context.term_definitions.get(term).and_then(|definition| definition.context.as_ref()) {
							active_context = Cow::Owned(process_context(&active_context, vec![Some((*context).clone())],
								active_context.term_definitions.get(term).unwrap().base_url.as_ref(),
								options, &FrozenSet::new(), false, false, true).await?);
						}
					}
				}
			}
			let mut result = T::empty_object();
			expand_object(&mut result, &active_context, &type_scoped_context, active_property,
				obj, base_url, &input_type, options).await?;
			if let Some(value) = result.get("@value") {
				let mut count = 1;
				let mut literal = false;
				let mut invalid_typed_value = false;
				if let Some(ty) = result.get("@type") {
					count += 1;
					if result.contains("@language") || result.contains("@direction") {
						return Err(err!(InvalidValueObject));
					}
					if ty.as_string() == Some("@json") { literal = true; }
					else { invalid_typed_value = !ty.as_string().map_or(false, |ty| is_iri(ty)); }
				} else {
					if result.contains("@language") { count += 1; }
					if result.contains("@direction") { count += 1; }
				}
				if result.contains("@index") { count += 1; }
				if result.len() != count { return Err(err!(InvalidValueObject)); }
				if !literal {
					if value.is_null() || value.as_array().map_or(false, |array| array.is_empty()) {
						return Ok(Owned::Null);
					}
					if value.as_string().is_none() && result.contains("@language") {
						return Err(err!(InvalidLanguageTaggedValue));
					}
					if invalid_typed_value { return Err(err!(InvalidTypedValue)); }
				}
			} else if result.get("@type").map_or(false, |ty| ty.as_array().is_none()) {
				let ty = json!(T, [result.remove("@type").unwrap()]);
				result.insert("@type".to_string(), ty);
			} else if let Some(set) = result.remove("@set") {
				if result.len() != if result.contains("@index") { 1 } else { 0 } {
					return Err(err!(InvalidSetOrListObject));
				}
				match set.into_enum() {
					Owned::Object(set) => result = set,
					set => return Ok(set)
				}
			} else if result.contains("@list") && result.len() != if result.contains("@index") { 2 } else { 1 } {
				return Err(err!(InvalidSetOrListObject));
			}
			if (result.len() == 1 && result.contains("@language")) ||
					(active_property.is_none() || active_property == Some("@graph")) &&
					(result.is_empty() || (result.len() == 1 && (result.contains("@value") || result.contains("@list") ||
					(!options.inner.frame_expansion && result.contains("@id"))))) {
				return Ok(Owned::Null);
			}
			Ok(Owned::Object(result))
		},
		value => {
			if active_property.is_none() || active_property == Some("@graph") { return Ok(Owned::Null) }
			Ok(Owned::Object(if let Some(property_scoped_context) = property_scoped_context {
				expand_value(&process_context(active_context, vec![Some(property_scoped_context.clone())],
					definition.unwrap().base_url.as_ref(), options, &FrozenSet::new(), false, true, true).await?,
					definition, value)?
			} else {
				expand_value(active_context, definition, value)?
			}))
		}
	}
}

#[async_recursion(?Send)]
async fn expand_object<'a, T, F, R>(result: &mut T::Object,
		active_context: &'a Context<'a, T>, type_scoped_context: &Context<'a, T>, active_property: Option<&'a str>,
		element: impl MutableObject<T> + 'a, base_url: Option<&'a Url>, input_type: &Option<String>,
		options: &'a JsonLdOptionsImpl<'a, T, F, R>) -> Result<()> where
	T: ForeignMutableJson + BuildableJson,
	F: Fn(&str, &Option<LoadDocumentOptions>) -> R + Clone,
	R: Future<Output = Result<RemoteDocument<T>>> + Clone
{
	let mut nests = BTreeMap::new();

	// Expand all keys, drop any that could not be expanded
	for (key, expanded_property, value) in element.into_iter()
			.filter_map(|(key, value)| expand_iri!(&active_context, &key).transpose()
				.map(|expanded_property| (key, expanded_property, value)))
			.filter(|(_, key, _)| key.as_ref().map_or(true, |key| key.contains(':') || is_jsonld_keyword(&key))) {
		let expanded_property = expanded_property?;
		if is_jsonld_keyword(&expanded_property) {
			expand_keyword(result, &mut nests, &active_context, &type_scoped_context, active_property, expanded_property, value,
				base_url, &input_type, options).await?;
			continue;
		}
		let definition = active_context.term_definitions.get(key.as_str());
		let container_mapping = definition.and_then(|definition| definition.container_mapping.as_ref());
		let mut expanded_value = if definition.and_then(|definition| definition.type_mapping.as_deref()) == Some("@json") {
			Owned::Object(json!(T, {"@value": value, "@type": "@json"}))
		} else {
			match value.into_enum() {
				Owned::Object(obj) => {
					if container_mapping.map_or(false, |container| container.contains("@language")) {
						let direction = definition.and_then(|definition| definition.direction_mapping.as_ref())
							.or(active_context.default_base_direction.as_ref());
						Owned::Array(if options.inner.ordered {
							expand_language_map(active_context, obj.into_iter().collect::<BTreeMap<_, _>>(), direction)?
						} else {
							expand_language_map(active_context, obj, direction)?
						})
					} else if container_mapping.map_or(false, |container| container.contains("@index") ||
							container.contains("@type") || container.contains("@id")) {
						let map_context = if container_mapping.unwrap().contains("@index") {
							active_context
						} else {
							active_context.previous_context.as_deref().unwrap_or(active_context)
						};
						let index_key = definition.and_then(|definition| definition.index_mapping.as_deref()).unwrap_or("@index");
						let as_graph = container_mapping.unwrap().contains("@graph");
						let property_index = index_key != "@index" && container_mapping.unwrap().contains("@index");
						Owned::Array(if options.inner.ordered {
							expand_index_map(map_context, &key, obj.into_iter().collect::<BTreeMap<_, _>>(), index_key,
								as_graph, property_index, base_url, options).await?
						} else {
							expand_index_map(map_context, &key, obj, index_key, as_graph, property_index, base_url, options).await?
						})
					} else {
						expand_internal(&active_context, Some(&key), obj.into(), base_url, options, false).await?
					}
				},
				value => {
					expand_internal(&active_context, Some(&key), value.into_untyped(), base_url, options, false).await?
				}
			}
		};
		if let Owned::Null = expanded_value { continue; }
		if container_mapping.map_or(false, |container| container.contains("@list")) {
			if_chain! {
				if let Owned::Object(ref obj) = expanded_value;
				if obj.contains("@list");
				then {} else {
					expanded_value = Owned::Object(json!(T, {
						"@list": if let Owned::Array(array) = expanded_value {
							<_ as Into<T>>::into(array)
						} else {
							json!(T, [expanded_value.into_untyped()])
						}
					}));
				}
			}
		}
		if container_mapping.map_or(false, |container| container.contains("@graph") &&
				!container.contains("@id") && !container.contains("@index")) {
			let into_graph_object = |ev: T| {
				Owned::Object(json!(T, {"@graph": if let Some(_) = ev.as_array() { ev } else { json!(T, [ev]) }}))
			};
			expanded_value = if let Owned::Array(array) = expanded_value {
				Owned::Array(array.into_iter().map(into_graph_object).map(|ev| ev.into_untyped()).collect())
			} else {
				into_graph_object(expanded_value.into_untyped())
			};
		}
		if definition.map_or(false, |definition| definition.reverse_property) {
			let reverse_map = if let Some(reverse) = result.get_mut("@reverse") {
				reverse.as_object_mut().unwrap()
			} else {
				result.insert("@reverse".to_string(), T::empty_object().into());
				result.get_mut("@reverse").unwrap().as_object_mut().unwrap()
			};
			for item in if let Owned::Array(array) = expanded_value { array } else {
				expanded_value = Owned::Array(json!(T, [expanded_value.into_untyped()]));
				if let Owned::Array(array) = expanded_value { array } else { unreachable!() }
			} {
				if item.as_object().map_or(false, |item| item.contains("@value") || item.contains("@list")) {
					return Err(err!(InvalidReversePropertyValue));
				}
				if !reverse_map.contains(&expanded_property) {
					reverse_map.insert(expanded_property.clone(), T::empty_array().into());
				}
				add_value(reverse_map, &expanded_property, item, true);
			}
		} else {
			add_value(result, &expanded_property, expanded_value.into_untyped(), true);
		}
	}
	for (_, nested_values) in nests {
		match nested_values.as_enum() {
			Borrowed::Array(array) => {
				for nested_value in array.iter() {
					if let Some(nested_value) = nested_value.as_object() {
						expand_nested_value(result, nested_value, active_context, type_scoped_context,
							active_property, base_url, input_type, options).await?;
					} else {
						return Err(err!(InvalidNestValue));
					}
				}
			},
			Borrowed::Object(nested_value) => expand_nested_value(result, nested_value,
				active_context, type_scoped_context, active_property, base_url, input_type, options).await?,
			_ => return Err(err!(InvalidNestValue))
		}
	}
	return Ok(());
}

fn expand_language_map<T: ForeignMutableJson + BuildableJson>(active_context: &Context<'_, T>,
		language_map: impl MutableObject<T>, direction: Option<&Direction>) -> Result<T::Array> {
	let mut result = T::empty_array();
	for (language, language_value) in language_map {
		let language = if language != "@none" && expand_iri!(active_context, &language)?.as_deref() != Some("@none") {
			Some(language)
		} else {
			None
		};
		match language_value.into_enum() {
			Owned::Array(array) => {
				for item in array.into_iter().filter_map(|item| expand_language_value(language.as_deref(), item, direction).transpose()) {
					result.push_back(item?.into());
				}
			},
			language_value => {
				if let Some(expanded_item) = expand_language_value(language.as_deref(), language_value.into_untyped(), direction)? {
					result.push_back(expanded_item.into());
				}
			}
		}
	}
	Ok(result)
}

fn expand_language_value<T: ForeignMutableJson + BuildableJson>(language: Option<&str>, language_value: T,
		direction: Option<&Direction>) -> Result<Option<T::Object>> {
	match language_value.into_enum() {
		Owned::Null => Ok(None),
		Owned::String(language_value) => {
			let mut v: T::Object = json!(T, {"@value": language_value});
			if let Some(language) = language { v.insert("@language".to_string(), language.into()); }
			if let Some(direction) = direction { v.insert("@direction".to_string(), direction.as_ref().into()); }
			Ok(Some(v))
		},
		_ => Err(err!(InvalidLanguageMapValue))
	}
}

async fn expand_index_map<T, F, R>(map_context: &Context<'_, T>, key: &str, index_map: impl MutableObject<T>, index_key: &str,
		as_graph: bool, property_index: bool, base_url: Option<&Url>, options: &JsonLdOptionsImpl<'_, T, F, R>) -> Result<T::Array> where
	T: ForeignMutableJson + BuildableJson,
	F: Fn(&str, &Option<LoadDocumentOptions>) -> R + Clone,
	R: Future<Output = Result<RemoteDocument<T>>> + Clone
{
	let mut result = T::empty_array();
	for (index, index_value) in index_map {
		let map_context = if_chain! {
			if index_key == "@type";
			if let Some(definition) = map_context.term_definitions.get(index.as_str());
			if let Some(ref context) = definition.context;
			then {
				Cow::Owned(process_context(&map_context, vec![Some(context.clone())], definition.base_url.as_ref(),
					options, &FrozenSet::new(), false, true, true).await?)
			} else {
				Cow::Borrowed(map_context)
			}
		};

		let expanded_index = expand_iri!(&map_context, &index, index_key == "@id")?;
		let index_value = expand_internal(&map_context, Some(key), index_value, base_url, options, true).await?;
		if let Owned::Array(array) = index_value {
			for item in array.into_iter().map(|item| expand_index_value(&map_context, &index, expanded_index.as_deref(), item,
					index_key, as_graph, property_index)) {
				result.push_back(item?.into());
			}
		} else {
			result.push_back(expand_index_value(&map_context, &index, expanded_index.as_deref(), index_value.into_untyped(),
				index_key, as_graph, property_index)?.into());
		}
	}
	Ok(result)
}

fn expand_index_value<T: ForeignMutableJson + BuildableJson>(map_context: &Context<'_, T>, index: &str, expanded_index: Option<&str>,
		index_value: T, index_key: &str, as_graph: bool, property_index: bool) -> Result<T::Object> {
	let mut index_value = index_value.into_object().ok_or(err!(InvalidValueObject))?;
	if as_graph && !is_graph_object::<T>(&index_value) {
		index_value = json!(T, {"@graph": [index_value]});
	}
	if let Some(expanded_index) = expanded_index {
		if expanded_index != "@none" {
			if property_index {
				let reexpanded_index = expand_value(map_context, map_context.term_definitions.get(index_key),
					Owned::String(index.to_string()))?;
				if let Some(expanded_index_key) = expand_iri!(map_context, index_key)? {
					let mut array: T::Array = json!(T, [reexpanded_index]);
					if let Some(index_property_values) = index_value.remove(&expanded_index_key) {
						match index_property_values.into_enum() {
							Owned::Array(index_property_values) => array.extend(index_property_values),
							index_property_value => { array.push_back(index_property_value.into_untyped()); }
						}
					}
					index_value.insert(expanded_index_key, array.into());
				}
			} else {
				match index_key {
					"@index" if !index_value.contains("@index") => {
						index_value.insert(index_key.to_string(), index.into());
					}
					"@id" if !index_value.contains("@id") => {
						index_value.insert(index_key.to_string(), expanded_index.into());
					},
					"@type" => {
						let mut array: T::Array = json!(T, [expanded_index]);
						if let Some(ty) = index_value.remove("@type") {
							match ty.into_enum() {
								Owned::Array(ty) => array.extend(ty),
								ty => { array.push_back(ty.into_untyped()); }
							}
						}
						index_value.insert("@type".to_string(), array.into());
					},
					_ => {}
				}
			}
		}
	}
	Ok(index_value)
}

async fn expand_nested_value<'a, T, F, R>(result: &mut T::Object, nested_value: &T::Object,
		active_context: &'a Context<'a, T>, type_scoped_context: &Context<'a, T>, active_property: Option<&str>,
		base_url: Option<&Url>, input_type: &Option<String>, options: &'a JsonLdOptionsImpl<'a, T, F, R>) -> Result<()> where
	T: ForeignMutableJson + BuildableJson,
	F: Fn(&str, &Option<LoadDocumentOptions>) -> R + Clone,
	R: Future<Output = Result<RemoteDocument<T>>> + Clone
{
	for (key, _) in nested_value.iter() {
		if expand_iri!(active_context, key)?.as_deref() == Some("@value") { return Err(err!(InvalidNestValue)); }
	}
	expand_object(result, active_context, type_scoped_context, active_property, nested_value.clone(), base_url, input_type, options).await
}

async fn expand_keyword<'a, T, F, R>(result: &mut T::Object, nests: &mut BTreeMap<String, T>,
		active_context: &'a Context<'a, T>, type_scoped_context: &Context<'a, T>, active_property: Option<&str>,
		key: String, value: T, base_url: Option<&Url>, input_type: &Option<String>,
		options: &'a JsonLdOptionsImpl<'a, T, F, R>) -> Result<()> where
	T: ForeignMutableJson + BuildableJson,
	F: Fn(&str, &Option<LoadDocumentOptions>) -> R + Clone,
	R: Future<Output = Result<RemoteDocument<T>>> + Clone
{
	if active_property == Some("@reverse") { return Err(err!(InvalidReversePropertyMap)); }
	match key.as_str() {
		"@type" => {
			if options.inner.processing_mode == JsonLdProcessingMode::JsonLd1_0 && result.contains(&key) {
				return Err(err!(CollidingKeywords));
			}
			match value.into_enum() {
				Owned::String(iri) => {
					let iri = expand_iri!(type_scoped_context, &iri, true)?.map_or(T::null(), |iri| iri.into());
					add_value(result, &key, iri, false);
				},
				Owned::Array(array) => {
					let expanded = array.iter().map(|iri| iri.as_string().ok_or(err!(InvalidTypeValue))).flat_map(|iri| iri.map(|iri|
						Ok(expand_iri!(type_scoped_context, &iri, true)?.map_or(T::null(), |iri| iri.into()))));
					for iri in expanded { add_value(result, &key, iri?, false); }
				},
				Owned::Object(obj) if options.inner.frame_expansion => {
					result.insert(key, if obj.is_empty() {
						obj.into()
					} else if let Some(default) = obj.get("@default").and_then(|default| default.as_string()) {
						json!(T, {"@default": expand_iri!(type_scoped_context, default, true)?.map_or(T::null(), |default| default.into())})
					} else {
						return Err(err!(InvalidTypeValue));
					});
				},
				_ => return Err(err!(InvalidTypeValue))
			}
		},
		"@included" if options.inner.processing_mode != JsonLdProcessingMode::JsonLd1_0 => add_value(result, &key,
			expand_internal(active_context, None, value, base_url, options, false).await?.into_untyped(), true),
		_ if result.contains(&key) => return Err(err!(CollidingKeywords)),
		"@id" => {
			result.insert(key, match value.into_enum() {
				Owned::String(iri) => expand_iri!(active_context, &iri, true, false)?.map_or(T::null(), |iri| iri.into()),
				Owned::Array(array) if options.inner.frame_expansion => {
					array.iter().map(|iri| iri.as_string().ok_or(err!(InvalidIdValue))).flat_map(|iri| iri.map(|iri|
						Ok(expand_iri!(active_context, &iri, true, false)?.map_or(T::null(), |iri| iri.into()))))
						.collect::<Result<T::Array>>()?.into()
				},
				Owned::Object(obj) if options.inner.frame_expansion && obj.is_empty() => obj.into(),
				_ => return Err(err!(InvalidIdValue))
			});
		},
		"@graph" => {
			let expanded_value = expand_internal(active_context, Some("@graph"), value, base_url, options, false).await?;
			result.insert(key, if let Owned::Array(array) = expanded_value {
				array.into()
			} else {
				json!(T, [expanded_value.into_untyped()])
			});
		},
		"@value" => {
			result.insert(key, if input_type.as_deref() == Some("@json") {
				if options.inner.processing_mode == JsonLdProcessingMode::JsonLd1_0 {
					return Err(err!(InvalidValueObjectValue));
				}
				value
			} else {
				match value.into_enum() {
					Owned::Array(array) if options.inner.frame_expansion => {
						array.iter().map(|iri| iri.as_string().ok_or(err!(InvalidValueObjectValue)).map(|iri| iri.into()))
							.collect::<Result<T::Array>>()?.into()
					},
					Owned::Object(obj) if options.inner.frame_expansion && obj.is_empty() => obj.into(),
					Owned::Array(_) | Owned::Object(_) => return Err(err!(InvalidValueObjectValue)),
					value => value.into_untyped()
				}
			});
		},
		"@language" => {
			result.insert(key, match value.into_enum() {
				Owned::String(lang) => lang.into(),
				Owned::Array(array) if options.inner.frame_expansion => {
					array.iter().map(|lang| lang.as_string().ok_or(err!(InvalidLanguageTaggedString)).map(|lang| lang.into()))
						.collect::<Result<T::Array>>()?.into()
				},
				Owned::Object(obj) if options.inner.frame_expansion && obj.is_empty() => obj.into(),
				_ => return Err(err!(InvalidLanguageTaggedString))
			});
		},
		"@direction" => {
			result.insert(key, match value.into_enum() {
				Owned::String(dir) => {
					if dir != "ltr" && dir != "rtl" { return Err(err!(InvalidBaseDirection)); }
					dir.into()
				},
				Owned::Array(array) if options.inner.frame_expansion => {
					array.iter().map(|dir| dir.as_string().ok_or(err!(InvalidBaseDirection))).flat_map(|dir| dir.map(|dir| {
						if dir != "ltr" && dir != "rtl" { return Err(err!(InvalidBaseDirection)); }
						Ok(dir.into())
					})).collect::<Result<T::Array>>()?.into()
				},
				Owned::Object(obj) if options.inner.frame_expansion && obj.is_empty() => obj.into(),
				_ => return Err(err!(InvalidBaseDirection))
			});
		},
		"@index" => {
			if let Some(value) = value.as_string() {
				result.insert(key, value.into());
			} else {
				return Err(err!(InvalidIndexValue))
			}
		},
		"@list" => {
			match active_property {
				None | Some("@graph") => {},
				_ => add_value(result, &key, expand_internal(active_context, active_property, value, base_url, options, false)
					.await?.into_untyped(), true)
			}
		},
		"@set" => {
			result.insert(key, expand_internal(active_context, active_property, value, base_url, options, false).await?.into_untyped());
		}
		"@reverse" => {
			if value.as_object().is_some() {
				let expanded_value = expand_internal(active_context, Some("@reverse"), value, base_url, options, false).await?;
				if let Owned::Object(mut expanded_value) = expanded_value {
					if let Some(reverse) = expanded_value.remove("@reverse").map(|reverse| reverse.into_object().unwrap()) {
						for (property, item) in reverse.into_iter() { add_value(result, &property, item, true); }
					}
					if !expanded_value.is_empty() {
						let reverse_map = if let Some(reverse_map) = result.get_mut("@reverse") { reverse_map } else {
							result.insert("@reverse".to_string(), T::empty_object().into());
							result.get_mut("@reverse").unwrap()
						}.as_object_mut().unwrap();
						for (property, items) in expanded_value {
							for item in items.into_array().unwrap() {
								if let Some(item) = item.as_object() {
									if item.contains("@value") || item.contains("@list") {
										return Err(err!(InvalidReversePropertyValue));
									}
								}
								add_value(reverse_map, &property, item, true);
							}
						}
					}
				}
			} else {
				return Err(err!(InvalidReverseValue));
			}
		},
		"@nest" => {
			nests.insert(key, T::empty_array().into());
		},
		_ => {}
	}
	return Ok(());
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
	if is_jsonld_keyword(value) { return Ok(Some(value.to_string())) }
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

fn expand_value<T: ForeignMutableJson + BuildableJson>(
		active_context: &Context<'_, T>, definition: Option<&TermDefinition<T>>, value: typed_json::Owned<T>) -> Result<T::Object> {
	let type_mapping = definition.and_then(|definition| definition.type_mapping.as_deref());
	if let (Some(type_mapping @ ("@id" | "@vocab")), Owned::String(value)) = (type_mapping, &value) {
		return Ok(json!(T, {"@id": expand_iri!(active_context, value, true, type_mapping == "@vocab")?
			.map_or(T::null(), |iri| iri.into())}));
	}
	let mut result = T::empty_object();
	if let Some(type_mapping) = type_mapping {
		if type_mapping != "@none" {
			result.insert("@type".to_string(), type_mapping.into());
		}
	} else if let Owned::String(_) = value {
		if let Some(language) = definition.and_then(|definition| definition.language_mapping.as_ref().map(|lang| lang.as_deref()))
				.unwrap_or(active_context.default_language.as_deref()) {
			result.insert("@language".to_string(), language.into());
		}
		if let Some(direction) = definition.and_then(|definition| definition.direction_mapping.as_ref())
				.or(active_context.default_base_direction.as_ref()) {
			result.insert("@direction".to_string(), direction.as_ref().into());
		}
	}
	result.insert("@value".to_string(), value.into_untyped());
	Ok(result)
}