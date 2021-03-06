// Expansion Algorithms.
//
// Spec correspondence:
// expand_internal - the entire algorithm
// expand_object - 6~
// expand_object_properties - (part of 12,)13~14
// expand_language_map - 13.7
// expand_language_value - 13.7.4.2.*
// expand_index_map - 13.8
// expand_index_value - 13.8.3.7.*
// expand_nested_value - 14.1.*
// expand_keyword - 13.4.*

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet, HashMap};

use futures::future::BoxFuture;

use cc_traits::{Get, GetMut, Len, MapInsert, PushBack, Remove};
use json_trait::typed_json::{self, *};
use json_trait::{json, Array, BuildableJson, ForeignMutableJson, MutableObject, Object};

use elsa::FrozenSet;
use maybe_owned::MaybeOwned;

use async_recursion::async_recursion;

use url::Url;

use if_chain::if_chain;

use crate::container::Container;
use crate::context::{create_term_definition, process_context};
use crate::error::JsonLdErrorCode::*;
use crate::error::Result;
use crate::util::{add_value, as_compact_iri, is_graph_object, is_iri, is_jsonld_keyword, looks_like_a_jsonld_keyword, resolve, ContextJson};
use crate::{Context, Direction, JsonLdOptions, JsonLdOptionsImpl, JsonLdProcessingMode, LoadDocumentOptions, OptionalContexts, RemoteDocument, TermDefinition};

#[async_recursion(?Send)]
pub(crate) async fn expand_internal<'a, T, F>(
	active_context: &Context<'a, T>,
	active_property: Option<&'a str>,
	element: &T,
	base_url: Option<&'a Url>,
	options: &JsonLdOptionsImpl<T, F>,
	from_map: bool
) -> Result<Owned<T>>
where
	T: ForeignMutableJson + BuildableJson,
	F: for<'b> Fn(&'b str, &'b Option<LoadDocumentOptions>) -> BoxFuture<'b, Result<RemoteDocument<T>>> + Clone
{
	match element.as_enum() {
		Borrowed::Null => Ok(Owned::Null),
		Borrowed::Array(array) => {
			let frame_expansion = options.inner.frame_expansion && active_property != Some("@default");
			let definition = active_property.and_then(|active_property| active_context.term_definitions.get(active_property));
			let mut result = T::empty_array();
			for item in array.iter() {
				let expanded_item = expand_internal(
					active_context,
					active_property,
					item,
					base_url,
					&JsonLdOptionsImpl {
						inner: &JsonLdOptions {
							frame_expansion,
							..(*options.inner).clone()
						},
						loaded_contexts: MaybeOwned::Borrowed(&options.loaded_contexts)
					},
					from_map
				)
				.await?;
				match expanded_item {
					Owned::Array(array) => {
						if let Some(Container::List) = definition.map(|definition| &definition.container_mapping) {
							result.push_back(json!(T, { "@list": array }));
						} else {
							result.extend(array);
						}
					}
					Owned::Null => {}
					_ => {
						result.push_back(expanded_item.into_untyped());
					}
				}
			}
			Ok(Owned::Array(result))
		}
		Borrowed::Object(obj) => expand_object(active_context, active_property, obj, base_url, options, from_map).await,
		value => {
			if active_property.is_none() || active_property == Some("@graph") {
				return Ok(Owned::Null);
			}
			let definition = active_property.and_then(|active_property| active_context.term_definitions.get(active_property));
			let property_scoped_context = definition.map_or(&[] as &[_], |definition| &definition.context);
			Ok(Owned::Object(if !property_scoped_context.is_empty() {
				expand_value(
					&process_context(
						active_context,
						property_scoped_context,
						definition.unwrap().base_url.as_ref(),
						options,
						&FrozenSet::new(),
						false,
						true,
						true
					)
					.await?,
					definition,
					value
				)?
			} else {
				expand_value(active_context, definition, value)?
			}))
		}
	}
}

#[async_recursion(?Send)]
pub(crate) async fn expand_object<'a, T, F>(
	active_context: &Context<'a, T>,
	active_property: Option<&'a str>,
	obj: &'a impl MutableObject<T>,
	base_url: Option<&'a Url>,
	options: &JsonLdOptionsImpl<T, F>,
	from_map: bool
) -> Result<Owned<T>>
where
	T: ForeignMutableJson + BuildableJson,
	F: for<'b> Fn(&'b str, &'b Option<LoadDocumentOptions>) -> BoxFuture<'b, Result<RemoteDocument<T>>> + Clone
{
	let definition = active_property.and_then(|active_property| active_context.term_definitions.get(active_property));
	let property_scoped_context = definition.map_or(&[] as &[_], |definition| &definition.context);
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
	if !property_scoped_context.is_empty() {
		active_context = Cow::Owned(
			process_context(
				&active_context,
				property_scoped_context,
				definition.unwrap().base_url.as_ref(),
				options,
				&FrozenSet::new(),
				true,
				true,
				true
			)
			.await?
		);
	}
	let obj = obj.iter().collect::<BTreeMap<_, _>>();
	if let Some(context) = obj.get("@context") {
		active_context = Cow::Owned(
			process_context(
				&active_context,
				&OptionalContexts::from_json(Cow::Borrowed(*context))?,
				base_url,
				options,
				&FrozenSet::new(),
				false,
				true,
				true
			)
			.await?
		);
	}
	let type_scoped_context = active_context.clone();
	let mut input_type = None;
	for (key, value) in obj.iter() {
		if expand_iri!(&active_context, key)?.as_deref() == Some("@type") {
			if let Some(value) = value.as_array() {
				for (term, context) in value
					.iter()
					.filter_map(|term| term.as_string())
					.collect::<BTreeSet<_>>()
					.iter()
					.map(|term| {
						input_type = expand_iri!(&active_context, term)?;
						Ok(term)
					})
					.filter_map(|term| {
						term.map(|term| type_scoped_context.term_definitions.get(*term).map(|definition| (term, &definition.context)))
							.transpose()
					})
					.collect::<Result<BTreeMap<_, _>>>()?
				{
					active_context = Cow::Owned(
						process_context(
							&active_context,
							&context,
							type_scoped_context.term_definitions.get(*term).unwrap().base_url.as_ref(),
							options,
							&FrozenSet::new(),
							false,
							false,
							true
						)
						.await?
					);
				}
			} else if let Some(term) = value.as_string() {
				input_type = expand_iri!(&active_context, term)?;
				if let Some(context) = type_scoped_context.term_definitions.get(term).map(|definition| &definition.context) {
					active_context = Cow::Owned(
						process_context(
							&active_context,
							&context,
							type_scoped_context.term_definitions.get(term).unwrap().base_url.as_ref(),
							options,
							&FrozenSet::new(),
							false,
							false,
							true
						)
						.await?
					);
				}
			}
		}
	}
	let mut result = T::empty_object();
	expand_object_properties(&mut result, &active_context, &type_scoped_context, active_property, obj, base_url, &input_type, options).await?;
	if let Some(value) = result.get("@value") {
		let mut count = 1;
		let mut literal = false;
		let mut invalid_typed_value = false;
		if let Some(ty) = result.get("@type") {
			count += 1;
			if result.contains("@language") || result.contains("@direction") {
				return Err(err!(InvalidValueObject));
			}
			if ty.as_string() == Some("@json") {
				literal = true;
			} else {
				invalid_typed_value = !ty.as_string().map_or(false, |ty| is_iri(ty));
			}
		} else {
			if result.contains("@language") {
				count += 1;
			}
			if result.contains("@direction") {
				count += 1;
			}
		}
		if result.contains("@index") {
			count += 1;
		}
		if result.len() != count {
			return Err(err!(InvalidValueObject));
		}
		if !literal {
			if value.is_null() || value.as_array().map_or(false, |array| array.is_empty()) {
				return Ok(Owned::Null);
			}
			if value.as_string().is_none() && result.contains("@language") {
				return Err(err!(InvalidLanguageTaggedValue));
			}
			if invalid_typed_value {
				return Err(err!(InvalidTypedValue));
			}
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
	if (result.len() == 1 && result.contains("@language"))
		|| (active_property.is_none() || active_property == Some("@graph"))
			&& (result.is_empty() || result.contains("@value") || result.contains("@list") || (!options.inner.frame_expansion && result.len() == 1 && result.contains("@id")))
	{
		return Ok(Owned::Null);
	}
	Ok(Owned::Object(result))
}

#[async_recursion(?Send)]
async fn expand_object_properties<'a, T, F>(
	result: &mut T::Object,
	active_context: &Context<T>,
	type_scoped_context: &Context<T>,
	active_property: Option<&'a str>,
	element: impl IntoIterator<Item = (&'a str, &'a T)> + 'a,
	base_url: Option<&'a Url>,
	input_type: &Option<String>,
	options: &JsonLdOptionsImpl<'a, T, F>
) -> Result<()>
where
	T: ForeignMutableJson + BuildableJson,
	F: for<'b> Fn(&'b str, &'b Option<LoadDocumentOptions>) -> BoxFuture<'b, Result<RemoteDocument<T>>> + Clone
{
	let mut nests = BTreeMap::new();

	// Expand all keys, drop any that could not be expanded
	for (key, expanded_property, value) in element
		.into_iter()
		.filter_map(|(key, value)| expand_iri!(&active_context, &key).transpose().map(|expanded_property| (key, expanded_property, value)))
		.filter(|(_, key, _)| key.as_ref().map_or(true, |key| key.contains(':') || is_jsonld_keyword(&key)))
	{
		let expanded_property = expanded_property?;
		if is_jsonld_keyword(&expanded_property) {
			expand_keyword(
				result,
				&mut nests,
				&active_context,
				&type_scoped_context,
				active_property,
				key,
				expanded_property,
				value,
				base_url,
				&input_type,
				options
			)
			.await?;
			continue;
		}
		let definition = active_context.term_definitions.get(key);
		let container_mapping = definition.map_or(&container!(None), |definition| &definition.container_mapping);
		let mut expanded_value = if definition.and_then(|definition| definition.type_mapping.as_deref()) == Some("@json") {
			Owned::Object(json!(T, {"@value": value.clone(), "@type": "@json"}))
		} else {
			if let Some(obj) = value.as_object() {
				if let LanguageContainer!() = container_mapping {
					let direction = definition
						.and_then(|definition| definition.direction_mapping.as_ref())
						.or(active_context.default_base_direction.as_ref());
					Owned::Array(if options.inner.ordered {
						expand_language_map(active_context, obj.iter().collect::<BTreeMap<_, _>>(), direction)?
					} else {
						expand_language_map(active_context, obj.iter(), direction)?
					})
				} else if let Some(index_key) = match container_mapping {
					IndexContainer!() => Some(definition.and_then(|definition| definition.index_mapping.as_deref()).unwrap_or("@index")),
					TypeContainer!() => Some("@type"),
					IdContainer!() => Some("@id"),
					_ => None
				} {
					let map_context = if let IndexContainer!() = container_mapping {
						active_context
					} else {
						active_context.previous_context.as_deref().unwrap_or(active_context)
					};
					let as_graph = container_mapping.is_graph();
					let property_index = index_key != "@index" && container_mapping.is_index();
					Owned::Array(if options.inner.ordered {
						expand_index_map(
							map_context,
							&key,
							obj.iter().collect::<BTreeMap<_, _>>(),
							index_key,
							as_graph,
							property_index,
							base_url,
							options
						)
						.await?
					} else {
						expand_index_map(map_context, &key, obj.iter(), index_key, as_graph, property_index, base_url, options).await?
					})
				} else {
					expand_object(&active_context, Some(&key), obj, base_url, options, false).await?
				}
			} else {
				expand_internal(&active_context, Some(&key), value, base_url, options, false).await?
			}
		};
		if let Owned::Null = expanded_value {
			continue;
		}
		if let Container::List = container_mapping {
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
		if container_mapping.is_graph() && !container_mapping.is_id() && !container_mapping.is_index() {
			let into_graph_object = |ev: T| Owned::Object(json!(T, {"@graph": if let Some(_) = ev.as_array() { ev } else { json!(T, [ev]) }}));
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
			for item in if let Owned::Array(array) = expanded_value {
				array
			} else {
				expanded_value = Owned::Array(json!(T, [expanded_value.into_untyped()]));
				if let Owned::Array(array) = expanded_value {
					array
				} else {
					unreachable!()
				}
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
		match nested_values.into_enum() {
			Owned::Array(array) => {
				for nested_value in array.into_iter() {
					if let Some(nested_value) = nested_value.into_object() {
						expand_nested_value(result, nested_value, active_context, type_scoped_context, active_property, base_url, input_type, options).await?;
					} else {
						return Err(err!(InvalidNestValue));
					}
				}
			}
			Owned::Object(nested_value) => expand_nested_value(result, nested_value, active_context, type_scoped_context, active_property, base_url, input_type, options).await?,
			_ => return Err(err!(InvalidNestValue))
		}
	}
	return Ok(());
}

fn expand_language_map<'a, T: ForeignMutableJson + BuildableJson>(
	active_context: &Context<T>,
	language_map: impl IntoIterator<Item = (&'a str, &'a T)>,
	direction: Option<&Direction>
) -> Result<T::Array> {
	let mut result = T::empty_array();
	for (language, language_value) in language_map.into_iter() {
		let language = if language != "@none" && expand_iri!(active_context, &language)?.as_deref() != Some("@none") {
			Some(language)
		} else {
			None
		};
		if let Some(array) = language_value.as_array() {
			for item in array.iter().filter_map(|item| expand_language_value(language.as_deref(), item, direction).transpose()) {
				result.push_back(item?.into());
			}
		} else if let Some(expanded_item) = expand_language_value(language.as_deref(), language_value, direction)? {
			result.push_back(expanded_item.into());
		}
	}
	Ok(result)
}

fn expand_language_value<T: ForeignMutableJson + BuildableJson>(language: Option<&str>, language_value: &T, direction: Option<&Direction>) -> Result<Option<T::Object>> {
	match language_value.as_enum() {
		Borrowed::Null => Ok(None),
		Borrowed::String(language_value) => {
			let mut v: T::Object = json!(T, { "@value": language_value });
			if let Some(language) = language {
				v.insert("@language".to_string(), language.into());
			}
			if let Some(direction) = direction {
				if *direction != Direction::None {
					v.insert("@direction".to_string(), direction.as_ref().into());
				}
			}
			Ok(Some(v))
		}
		_ => Err(err!(InvalidLanguageMapValue))
	}
}

async fn expand_index_map<'a, T, F>(
	map_context: &Context<'_, T>,
	key: &str,
	index_map: impl IntoIterator<Item = (&'a str, &'a T)>,
	index_key: &str,
	as_graph: bool,
	property_index: bool,
	base_url: Option<&Url>,
	options: &JsonLdOptionsImpl<'_, T, F>
) -> Result<T::Array>
where
	T: ForeignMutableJson + BuildableJson,
	F: for<'b> Fn(&'b str, &'b Option<LoadDocumentOptions>) -> BoxFuture<'b, Result<RemoteDocument<T>>> + Clone
{
	let mut result = T::empty_array();
	for (index, index_value) in index_map.into_iter() {
		let map_context = if_chain! {
			if index_key == "@type";
			if let Some(definition) = map_context.term_definitions.get(index);
			if !definition.context.is_empty();
			then {
				Cow::Owned(process_context(&map_context, &definition.context, definition.base_url.as_ref(),
					options, &FrozenSet::new(), false, true, true).await?)
			} else {
				Cow::Borrowed(map_context)
			}
		};

		let expanded_index = expand_iri!(&map_context, &index, index_key == "@id", index_key != "@id")?;
		let index_value = expand_internal(&map_context, Some(key), index_value, base_url, options, true).await?;
		if let Owned::Array(array) = index_value {
			for item in array
				.into_iter()
				.map(|item| expand_index_value(&map_context, &index, expanded_index.as_deref(), item, index_key, as_graph, property_index))
			{
				result.push_back(item?.into());
			}
		} else {
			result.push_back(
				expand_index_value(
					&map_context,
					&index,
					expanded_index.as_deref(),
					index_value.into_untyped(),
					index_key,
					as_graph,
					property_index
				)?
				.into()
			);
		}
	}
	Ok(result)
}

fn expand_index_value<T: ForeignMutableJson + BuildableJson>(
	map_context: &Context<T>,
	index: &str,
	expanded_index: Option<&str>,
	index_value: T,
	index_key: &str,
	as_graph: bool,
	property_index: bool
) -> Result<T::Object> {
	let mut index_value = index_value.into_object().ok_or(err!(InvalidValueObject))?;
	if as_graph && !is_graph_object::<T>(&index_value) {
		index_value = json!(T, { "@graph": [index_value] });
	}
	if let Some(expanded_index) = expanded_index {
		if expanded_index != "@none" {
			if property_index {
				let reexpanded_index = expand_value(map_context, map_context.term_definitions.get(index_key), Borrowed::String(index))?;
				if let Some(expanded_index_key) = expand_iri!(map_context, index_key)? {
					let mut array: T::Array = json!(T, [reexpanded_index]);
					if let Some(index_property_values) = index_value.remove(&expanded_index_key) {
						match index_property_values.into_enum() {
							Owned::Array(index_property_values) => array.extend(index_property_values),
							index_property_value => {
								array.push_back(index_property_value.into_untyped());
							}
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
					}
					"@type" => {
						let mut array: T::Array = json!(T, [expanded_index]);
						if let Some(ty) = index_value.remove("@type") {
							match ty.into_enum() {
								Owned::Array(ty) => array.extend(ty),
								ty => {
									array.push_back(ty.into_untyped());
								}
							}
						}
						index_value.insert("@type".to_string(), array.into());
					}
					_ => {}
				}
			}
		}
	}
	Ok(index_value)
}

async fn expand_nested_value<T, F>(
	result: &mut T::Object,
	nested_value: T::Object,
	active_context: &Context<'_, T>,
	type_scoped_context: &Context<'_, T>,
	active_property: Option<&str>,
	base_url: Option<&Url>,
	input_type: &Option<String>,
	options: &JsonLdOptionsImpl<'_, T, F>
) -> Result<()>
where
	T: ForeignMutableJson + BuildableJson,
	F: for<'a> Fn(&'a str, &'a Option<LoadDocumentOptions>) -> BoxFuture<'a, Result<RemoteDocument<T>>> + Clone
{
	for (key, _) in nested_value.iter() {
		if expand_iri!(active_context, key)?.as_deref() == Some("@value") {
			return Err(err!(InvalidNestValue));
		}
	}
	expand_object_properties(
		result,
		active_context,
		type_scoped_context,
		active_property,
		nested_value.iter(),
		base_url,
		input_type,
		options
	)
	.await
}

async fn expand_keyword<T, F>(
	result: &mut T::Object,
	nests: &mut BTreeMap<String, T>,
	active_context: &Context<'_, T>,
	type_scoped_context: &Context<'_, T>,
	active_property: Option<&str>,
	key: &str,
	expanded_property: String,
	value: &T,
	base_url: Option<&Url>,
	input_type: &Option<String>,
	options: &JsonLdOptionsImpl<'_, T, F>
) -> Result<()>
where
	T: ForeignMutableJson + BuildableJson,
	F: for<'a> Fn(&'a str, &'a Option<LoadDocumentOptions>) -> BoxFuture<'a, Result<RemoteDocument<T>>> + Clone
{
	if active_property == Some("@reverse") {
		return Err(err!(InvalidReversePropertyMap));
	}
	match expanded_property.as_str() {
		"@type" => {
			if options.inner.processing_mode == JsonLdProcessingMode::JsonLd1_0 && result.contains(&key) {
				return Err(err!(CollidingKeywords));
			}
			match value.as_enum() {
				Borrowed::String(iri) => {
					let iri = expand_iri!(type_scoped_context, &iri, true)?.map_or(T::null(), |iri| iri.into());
					add_value(result, &expanded_property, iri, false);
				}
				Borrowed::Array(array) => {
					let expanded = array
						.iter()
						.map(|iri| iri.as_string().ok_or(err!(InvalidTypeValue)))
						.flat_map(|iri| iri.map(|iri| Ok(expand_iri!(type_scoped_context, &iri, true)?.map_or(T::null(), |iri| iri.into()))));
					for iri in expanded {
						add_value(result, &expanded_property, iri?, false);
					}
				}
				Borrowed::Object(obj) if options.inner.frame_expansion => {
					result.insert(
						expanded_property,
						if obj.is_empty() {
							T::empty_object().into()
						} else if let Some(default) = obj.get("@default").and_then(|default| default.as_string()) {
							json!(T, {"@default": expand_iri!(type_scoped_context, default, true)?.map_or(T::null(), |default| default.into())})
						} else {
							return Err(err!(InvalidTypeValue));
						}
					);
				}
				_ => return Err(err!(InvalidTypeValue))
			}
		}
		"@included" if options.inner.processing_mode != JsonLdProcessingMode::JsonLd1_0 => {
			let expanded_value = expand_internal(active_context, None, value, base_url, options, false).await?;
			match expanded_value {
				Owned::Array(expanded_value) => {
					for value in expanded_value.iter() {
						if let Some(obj) = value.as_object() {
							if obj.contains("@value") || obj.contains("@list") || obj.contains("@set") || obj.contains("@graph") {
								return Err(err!(InvalidIncludedValue));
							}
						} else {
							return Err(err!(InvalidIncludedValue));
						}
					}
					add_value::<T>(result, &expanded_property, expanded_value.into(), true)
				}
				Owned::Object(obj) => {
					if obj.contains("@value") || obj.contains("@list") || obj.contains("@set") || obj.contains("@graph") {
						return Err(err!(InvalidIncludedValue));
					}
					add_value::<T>(result, &expanded_property, obj.into(), true)
				}
				_ => return Err(err!(InvalidIncludedValue))
			}
		}
		_ if result.contains(&expanded_property) => return Err(err!(CollidingKeywords)),
		"@id" => {
			result.insert(expanded_property, match value.as_enum() {
				Borrowed::String(iri) => expand_iri!(active_context, &iri, true, false)?.map_or(T::null(), |iri| iri.into()),
				Borrowed::Array(array) if options.inner.frame_expansion => array
					.iter()
					.map(|iri| iri.as_string().ok_or(err!(InvalidIdValue)))
					.flat_map(|iri| iri.map(|iri| Ok(expand_iri!(active_context, &iri, true, false)?.map_or(T::null(), |iri| iri.into()))))
					.collect::<Result<T::Array>>()?
					.into(),
				Borrowed::Object(obj) if options.inner.frame_expansion && obj.is_empty() => T::empty_object().into(),
				_ => return Err(err!(InvalidIdValue))
			});
		}
		"@graph" => {
			let expanded_value = expand_internal(active_context, Some("@graph"), value, base_url, options, false).await?;
			result.insert(
				expanded_property,
				if let Owned::Array(array) = expanded_value {
					array.into()
				} else {
					json!(T, [expanded_value.into_untyped()])
				}
			);
		}
		"@value" => {
			result.insert(
				expanded_property,
				if input_type.as_deref() == Some("@json") {
					if options.inner.processing_mode == JsonLdProcessingMode::JsonLd1_0 {
						return Err(err!(InvalidValueObjectValue));
					}
					value.clone()
				} else {
					match value.as_enum() {
						Borrowed::Array(array) if options.inner.frame_expansion => array
							.iter()
							.map(|iri| iri.as_string().ok_or(err!(InvalidValueObjectValue)).map(|iri| iri.into()))
							.collect::<Result<T::Array>>()?
							.into(),
						Borrowed::Object(obj) if options.inner.frame_expansion && obj.is_empty() => value.clone(),
						Borrowed::Array(_) | Borrowed::Object(_) => return Err(err!(InvalidValueObjectValue)),
						_ => value.clone()
					}
				}
			);
		}
		"@language" => {
			result.insert(expanded_property, match value.as_enum() {
				Borrowed::String(lang) => lang.into(),
				Borrowed::Array(array) if options.inner.frame_expansion => array
					.iter()
					.map(|lang| lang.as_string().ok_or(err!(InvalidLanguageTaggedString)).map(|lang| lang.into()))
					.collect::<Result<T::Array>>()?
					.into(),
				Borrowed::Object(obj) if options.inner.frame_expansion && obj.is_empty() => T::empty_object().into(),
				_ => return Err(err!(InvalidLanguageTaggedString))
			});
		}
		"@direction" => {
			result.insert(expanded_property, match value.as_enum() {
				Borrowed::String(dir) => {
					if dir != "ltr" && dir != "rtl" {
						return Err(err!(InvalidBaseDirection));
					}
					dir.into()
				}
				Borrowed::Array(array) if options.inner.frame_expansion => array
					.iter()
					.map(|dir| dir.as_string().ok_or(err!(InvalidBaseDirection)))
					.flat_map(|dir| {
						dir.map(|dir| {
							if dir != "ltr" && dir != "rtl" {
								return Err(err!(InvalidBaseDirection));
							}
							Ok(dir.into())
						})
					})
					.collect::<Result<T::Array>>()?
					.into(),
				Borrowed::Object(obj) if options.inner.frame_expansion && obj.is_empty() => T::empty_object().into(),
				_ => return Err(err!(InvalidBaseDirection))
			});
		}
		"@index" => {
			if let Some(value) = value.as_string() {
				result.insert(expanded_property, value.into());
			} else {
				return Err(err!(InvalidIndexValue));
			}
		}
		"@list" => match active_property {
			None | Some("@graph") => {}
			_ => add_value(
				result,
				&expanded_property,
				expand_internal(active_context, active_property, value, base_url, options, false).await?.into_untyped(),
				true
			)
		},
		"@set" => {
			result.insert(
				expanded_property,
				expand_internal(active_context, active_property, value, base_url, options, false).await?.into_untyped()
			);
		}
		"@reverse" => {
			if value.as_object().is_some() {
				let expanded_value = expand_internal(active_context, Some("@reverse"), value, base_url, options, false).await?;
				if let Owned::Object(mut expanded_value) = expanded_value {
					if let Some(reverse) = expanded_value.remove("@reverse").map(|reverse| reverse.into_object().unwrap()) {
						for (property, item) in reverse.into_iter() {
							add_value(result, &property, item, true);
						}
					}
					if !expanded_value.is_empty() {
						let reverse_map = if let Some(reverse_map) = result.get_mut("@reverse") {
							reverse_map
						} else {
							result.insert("@reverse".to_string(), T::empty_object().into());
							result.get_mut("@reverse").unwrap()
						}
						.as_object_mut()
						.unwrap();
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
		}
		"@nest" => {
			nests.insert(key.to_string(), value.clone());
		}
		_ => {}
	}
	return Ok(());
}

pub enum IRIExpansionArguments<'a, 'b, T, F>
where
	T: ForeignMutableJson + BuildableJson,
	F: for<'c> Fn(&'c str, &'c Option<LoadDocumentOptions>) -> BoxFuture<'c, Result<RemoteDocument<T>>>
{
	DefineTerms {
		active_context: &'a mut Context<'b, T>,
		local_context: &'a T::Object,
		defined: &'a mut HashMap<String, bool>,
		options: &'a JsonLdOptions<'b, T, F>
	},
	Normal(&'a Context<'b, T>)
}

impl<T, F> IRIExpansionArguments<'_, '_, T, F>
where
	T: ForeignMutableJson + BuildableJson,
	F: for<'a> Fn(&'a str, &'a Option<LoadDocumentOptions>) -> BoxFuture<'a, Result<RemoteDocument<T>>>
{
	fn active_context(&self) -> &Context<T> {
		match self {
			Self::DefineTerms { active_context, .. } => active_context,
			Self::Normal(active_context) => active_context
		}
	}
}

pub fn expand_iri<T, F>(mut args: IRIExpansionArguments<T, F>, value: &str, document_relative: bool, vocab: bool) -> Result<Option<String>>
where
	T: ForeignMutableJson + BuildableJson,
	F: for<'a> Fn(&'a str, &'a Option<LoadDocumentOptions>) -> BoxFuture<'a, Result<RemoteDocument<T>>>
{
	if is_jsonld_keyword(value) {
		return Ok(Some(value.to_string()));
	}
	if looks_like_a_jsonld_keyword(value) {
		return Ok(None);
	}
	if_chain! {
		if let IRIExpansionArguments::DefineTerms { ref mut active_context, local_context, ref mut defined, options } = args;
		if let Some(value_definition) = local_context.get(value);
		if defined.get(value).map_or(false, |v| !v);
		then {
			create_term_definition(active_context, local_context, value, value_definition.as_enum(),
				defined, options, None, false, false)?;
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
		if_chain! {
			if let IRIExpansionArguments::DefineTerms { ref mut active_context, local_context, ref mut defined, options } = args;
			if let Some(prefix_definition) = local_context.get(prefix);
			if defined.get(prefix).map_or(true, |v| !v);
			then {
				create_term_definition(active_context, local_context, prefix, prefix_definition.as_enum(),
					defined, options, None, false, false)?;
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
		if let Some(base) = args.active_context().base_iri.as_ref() {
			return Ok(Some(resolve(value, Some(base)).map_err(|e| err!(InvalidBaseIRI, , e))?.to_string()));
		}
	}
	Ok(Some(value.to_string()))
}

fn expand_value<T: ForeignMutableJson + BuildableJson>(active_context: &Context<T>, definition: Option<&TermDefinition<T>>, value: typed_json::Borrowed<T>) -> Result<T::Object> {
	let type_mapping = definition.and_then(|definition| definition.type_mapping.as_deref());
	if let (Some(type_mapping @ ("@id" | "@vocab")), Borrowed::String(value)) = (type_mapping, &value) {
		return Ok(json!(T, {"@id": expand_iri!(active_context, value, true, type_mapping == "@vocab")?
			.map_or(T::null(), |iri| iri.into())}));
	}
	let mut result = T::empty_object();
	if let Some(type_mapping) = type_mapping {
		if type_mapping != "@id" && type_mapping != "@vocab" && type_mapping != "@none" {
			result.insert("@type".to_string(), type_mapping.into());
		}
	} else if let Borrowed::String(_) = value {
		if let Some(language) = definition
			.and_then(|definition| definition.language_mapping.as_ref().map(|lang| lang.as_deref()))
			.unwrap_or(active_context.default_language.as_deref())
		{
			result.insert("@language".to_string(), language.into());
		}
		if let Some(direction) = definition
			.and_then(|definition| definition.direction_mapping.as_ref())
			.or(active_context.default_base_direction.as_ref())
		{
			if *direction != Direction::None {
				result.insert("@direction".to_string(), direction.as_ref().into());
			}
		}
	}
	result.insert("@value".to_string(), match value {
		Borrowed::Number(n) => n.map(|n| n.into()).ok_or(err!(InvalidJsonLiteral))?,
		Borrowed::String(s) => s.into(),
		Borrowed::Null => T::null(),
		Borrowed::Bool(b) => b.into(),
		_ => panic!("a compound value were passed into expand_value")
	});
	Ok(result)
}
