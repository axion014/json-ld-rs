// Compaction Algorithms.
//
// Spec correspondence:
// compact_internal - the entire algorithm
// compact_map - 12.*
// compact_item - 12.8.*
// compact_node_or_set - 12.8.9~12.8.10
// get_nest_result - 12.7.2~12.7.3,12.8.2~12.8.3
//
// There are some large number of unwraps used.
// It is assumed that the input is passed through the expansion algorithm beforehand,
// which validates the input.

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};

use futures::future::BoxFuture;

use cc_traits::{Get, GetMut, Len, MapInsert, PushBack, Remove};
use json_trait::typed_json::*;
use json_trait::{json, Array, BuildableJson, ForeignMutableJson, MutableObject, Object};

use elsa::FrozenSet;
use maybe_owned::MaybeOwned;

use async_recursion::async_recursion;

use if_chain::if_chain;

use crate::container::{Container, ContainerKind, GraphContainer, UnorderedContainer};
use crate::context::{create_inverse_context, process_context, select_term};
use crate::error::JsonLdErrorCode::{IRIConfusedWithPrefix, InvalidNestValue};
use crate::error::Result;
use crate::expand::expand_iri;
use crate::remote::LoadDocumentOptions;
use crate::util::{add_value, is_graph_object, make_lang_dir, resolve};
use crate::{Context, Direction, JsonLdOptions, JsonLdOptionsImpl, JsonLdProcessingMode, RemoteDocument, TermDefinition, TypeOrLanguage};

#[async_recursion(?Send)]
pub(crate) async fn compact_internal<'a, T, F>(active_context: &Context<'a, T>, active_property: Option<&'a str>, element: T, options: &JsonLdOptionsImpl<T, F>) -> Result<T>
where
	T: ForeignMutableJson + BuildableJson,
	F: for<'b> Fn(&'b str, &'b Option<LoadDocumentOptions>) -> BoxFuture<'b, Result<RemoteDocument<T>>> + Clone
{
	match element.into_enum() {
		Owned::Array(array) => {
			let mut result = T::empty_array();
			for item in array.into_iter() {
				let compacted_item = compact_internal(active_context, active_property, item, options).await?;
				if !compacted_item.is_null() {
					result.push_back(compacted_item);
				}
			}
			if result.len() != 1 || !options.inner.compact_arrays {
				return Ok(result.into());
			}
			if let Some(active_property) = active_property {
				match active_property {
					"@graph" | "@set" => return Ok(result.into()),
					_ => {}
				}
				let container = active_context.term_definitions.get(active_property).map(|definition| &definition.container_mapping);
				if let Some(Container::List | Container::Unordered(UnorderedContainer { is_set: true, .. })) = container {
					return Ok(result.into());
				}
			}
			Ok(result.remove(0).unwrap())
		}
		Owned::Object(mut obj) => {
			let type_scoped_context = active_context;
			let active_context = if_chain! {
				if let Some(previous_context) = active_context.previous_context.as_ref();
				if !obj.contains("@value") && (obj.len() != 1 || !obj.contains("@id"));
				then {
					previous_context
				} else {
					active_context
				}
			};
			let mut active_context = if_chain! {
				if let Some(term_definition) = active_property
					.and_then(|active_property| active_context.term_definitions.get(active_property));
				if !term_definition.context.is_empty();
				then {
					Cow::Owned(process_context(active_context, &term_definition.context, term_definition.base_url.as_ref(),
						options, &FrozenSet::new(), true, false, true).await?)
				} else {
					Cow::Borrowed(active_context)
				}
			};
			if obj.contains("@value") || (obj.contains("@id") && obj.len() == 1) {
				return compact_value(&active_context, active_property, obj, options.inner);
			}
			if_chain! {
				if let Some(list) = obj.remove("@list");
				if let Some(Container::List) = active_property
					.and_then(|property| active_context.term_definitions.get(property))
					.map(|definition| &definition.container_mapping);
				then { return compact_internal(&mut active_context, active_property, list, options).await; }
			}
			if let Some(expanded_types) = obj.get("@type") {
				// Collecting into a BTreeSet applies lexicographic sort implicitly
				let compacted_types = expanded_types
					.as_array()
					.unwrap()
					.iter()
					.map(|expanded_type| expanded_type.as_string().unwrap())
					.map(|expanded_type| Ok(compact_iri(&active_context, expanded_type, options.inner, None, true, false)?))
					.collect::<Result<BTreeSet<String>>>()?;
				for term in compacted_types {
					if let Some(TermDefinition {
						context: local_context, base_url, ..
					}) = type_scoped_context.term_definitions.get(term.as_str())
					{
						if !local_context.is_empty() {
							active_context =
								Cow::Owned(process_context(&mut active_context, local_context, base_url.as_ref(), options, &FrozenSet::new(), false, false, true).await?);
						}
					}
				}
			}
			Ok(if options.inner.ordered {
				compact_map(&active_context, type_scoped_context, active_property, obj.into_iter().collect::<BTreeMap<_, _>>(), options).await?
			} else {
				compact_map(&active_context, type_scoped_context, active_property, obj, options).await?
			}
			.into())
		}
		element => Ok(element.into_untyped())
	}
}

async fn compact_map<T, F>(
	active_context: &Context<'_, T>,
	type_scoped_context: &Context<'_, T>,
	active_property: Option<&str>,
	expanded_map: impl MutableObject<T>,
	options: &JsonLdOptionsImpl<'_, T, F>
) -> Result<T::Object>
where
	T: ForeignMutableJson + BuildableJson,
	F: for<'a> Fn(&'a str, &'a Option<LoadDocumentOptions>) -> BoxFuture<'a, Result<RemoteDocument<T>>> + Clone
{
	let mut result = T::empty_object();
	for (expanded_property, expanded_value) in expanded_map {
		match expanded_property.as_str() {
			"@id" => {
				let compacted_value = if let Some(expanded_value) = expanded_value.as_string() {
					compact_iri(active_context, expanded_value, options.inner, None, false, false)?.into()
				} else {
					T::null()
				};
				let alias = compact_iri(active_context, "@id", options.inner, None, true, false)?;
				result.insert(alias, compacted_value);
			}
			"@type" => {
				let compacted_value: T = match expanded_value.as_enum() {
					Borrowed::String(expanded_type) => compact_iri(type_scoped_context, expanded_type, options.inner, None, true, false)?.into(),
					Borrowed::Array(type_array) => {
						let mut compacted_value = T::empty_array();
						for expanded_type in type_array.iter() {
							compacted_value.push_back(compact_iri(type_scoped_context, expanded_type.as_string().unwrap(), options.inner, None, true, false)?.into());
						}
						compacted_value.into()
					}
					_ => panic!()
				};
				let alias = compact_iri(active_context, "@type", options.inner, None, true, false)?;
				let as_array = options.inner.processing_mode == JsonLdProcessingMode::JsonLd1_1
					&& active_context.term_definitions.get(alias.as_str()).map_or(false, |def| def.container_mapping.is_set())
					|| !options.inner.compact_arrays;
				add_value(&mut result, &alias, compacted_value, as_array);
			}
			"@reverse" => {
				let mut compacted_value = compact_internal(active_context, Some("@reverse"), expanded_value, options).await?.into_object().unwrap();
				let keys = compacted_value.iter().map(|(property, _)| property.to_string()).collect::<Vec<_>>();
				for property in keys {
					if let Some(term_definition) = active_context.term_definitions.get(property.as_str()) {
						if term_definition.reverse_property {
							let as_array = term_definition.container_mapping.is_set() || !options.inner.compact_arrays;
							add_value(&mut result, &property, compacted_value.remove(&property).unwrap(), as_array);
						}
					}
				}
				if !compacted_value.is_empty() {
					let alias = compact_iri(active_context, "@reverse", options.inner, None, true, false)?;
					result.insert(alias, compacted_value.into());
				}
			}
			"@preserve" => {
				let compacted_value = compact_internal(active_context, active_property, expanded_value, options).await?;
				if compacted_value.as_array().map_or(true, |array| !array.is_empty()) {
					result.insert("@preserve".to_string(), compacted_value.into());
				}
			}
			"@index"
				if active_property
					.and_then(|active_property| active_context.term_definitions.get(active_property))
					.map(|definition| &definition.container_mapping)
					.map_or(false, |container| container.is_index()) => {}
			"@direction" | "@index" | "@language" | "@value" => {
				let alias = compact_iri(active_context, expanded_property.as_str(), options.inner, None, true, false)?;
				result.insert(alias, expanded_value.into());
			}
			_ => {
				let expanded_value_array = expanded_value.into_array().unwrap();
				if expanded_value_array.is_empty() {
					let expanded_value = expanded_value_array.into();
					let item_active_property = compact_iri(
						active_context,
						expanded_property.as_str(),
						options.inner,
						Some(&expanded_value),
						true,
						active_property == Some("@reverse")
					)?;
					let nest_result = get_nest_result(active_context, &item_active_property, &mut result)?;
					add_value::<T>(nest_result, &item_active_property, expanded_value, true);
				} else {
					for expanded_item in expanded_value_array.into_iter() {
						let item_active_property = compact_iri(
							active_context,
							expanded_property.as_str(),
							options.inner,
							Some(&expanded_item),
							true,
							active_property == Some("@reverse")
						)?;
						let nest_result = get_nest_result(active_context, &item_active_property, &mut result)?;
						compact_item(active_context, item_active_property, nest_result, expanded_item, options).await?;
					}
				}
			}
		}
	}
	Ok(result)
}

async fn compact_item<T, F>(
	active_context: &Context<'_, T>,
	item_active_property: String,
	nest_result: &mut T::Object,
	expanded_item: T,
	options: &JsonLdOptionsImpl<'_, T, F>
) -> Result<()>
where
	T: ForeignMutableJson + BuildableJson,
	F: for<'a> Fn(&'a str, &'a Option<LoadDocumentOptions>) -> BoxFuture<'a, Result<RemoteDocument<T>>> + Clone
{
	let container = active_context
		.term_definitions
		.get(item_active_property.as_str())
		.map_or(&container!(None), |term_definition| &term_definition.container_mapping);
	let as_array = container.is_set() || item_active_property == "@graph" || item_active_property == "@list" || !options.inner.compact_arrays;
	if expanded_item.as_object().is_some() {
		let mut expanded_item = expanded_item.into_object().unwrap();
		if let Some(list) = expanded_item.remove("@list") {
			let compacted_item = compact_internal(active_context, Some(&item_active_property), list, options).await?;
			let compacted_item = if compacted_item.as_array().is_some() {
				compacted_item
			} else {
				json!(T, [compacted_item])
			};
			if let Container::List = container {
				nest_result.insert(item_active_property, compacted_item);
			} else {
				let mut obj = T::empty_object();
				obj.insert(compact_iri(active_context, "@list", options.inner, None, true, false)?, compacted_item);
				if let Some(value) = expanded_item.remove("@index") {
					obj.insert(compact_iri(active_context, "@index", options.inner, None, true, false)?, value);
				}
				add_value::<T>(nest_result, &item_active_property, obj.into(), as_array);
			}
		} else if is_graph_object::<T>(&expanded_item) {
			let compacted_item = compact_internal(active_context, Some(&item_active_property), expanded_item.remove("@graph").unwrap(), options).await?;
			if container.is_graph() && container.is_id() {
				let map_object = nest_result.get_mut(&item_active_property);
				let map_object = if let Some(map_object) = map_object {
					map_object.as_object_mut().unwrap()
				} else {
					nest_result.insert(item_active_property.clone(), T::empty_object().into());
					nest_result.get_mut(&item_active_property).unwrap().as_object_mut().unwrap()
				};
				let id = expanded_item.get("@id").map(|id| id.as_string().unwrap());
				let map_key = compact_iri(active_context, id.unwrap_or("@none"), options.inner, None, id.is_none(), false)?;
				add_value(map_object, &map_key, compacted_item, as_array);
			} else if container.is_graph() && !expanded_item.contains("@id") {
				if container.is_index() {
					let map_object = nest_result.get_mut(&item_active_property);
					let map_object = if let Some(map_object) = map_object {
						map_object.as_object_mut().unwrap()
					} else {
						nest_result.insert(item_active_property.clone(), T::empty_object().into());
						nest_result.get_mut(&item_active_property).unwrap().as_object_mut().unwrap()
					};
					let map_key = expanded_item.get("@index").map_or("@none", |index| index.as_string().unwrap());
					add_value(map_object, map_key, compacted_item, as_array);
				} else {
					let compacted_item = if_chain! {
						if let Some(array) = compacted_item.as_array();
						if array.len() > 1;
						then { json!(T, {"@included": compacted_item}) } else { compacted_item }
					};
					add_value(nest_result, &item_active_property, compacted_item, as_array);
				}
			} else {
				let mut obj = T::empty_object();
				obj.insert(compact_iri(active_context, "@graph", options.inner, None, true, false)?, compacted_item);
				if let Some(id) = expanded_item.get("@id").map(|id| id.as_string().unwrap()) {
					obj.insert(
						compact_iri(active_context, "@id", options.inner, None, true, false)?,
						compact_iri(active_context, id, options.inner, None, false, false)?.into()
					);
				}
				if let Some(index) = expanded_item.remove("@index") {
					obj.insert(compact_iri(active_context, "@index", options.inner, None, true, false)?, index);
				}
				add_value::<T>(nest_result, &item_active_property, obj.into(), as_array);
			}
		} else {
			let compacted_item = compact_internal(active_context, Some(&item_active_property), expanded_item.clone().into(), options).await?;
			compact_node_or_set(
				active_context,
				item_active_property,
				nest_result,
				expanded_item.into(),
				compacted_item,
				container,
				options,
				as_array
			)
			.await?;
		}
	} else {
		let compacted_item = compact_internal(active_context, Some(&item_active_property), expanded_item.clone(), options).await?;
		compact_node_or_set(
			active_context,
			item_active_property,
			nest_result,
			expanded_item,
			compacted_item,
			container,
			options,
			as_array
		)
		.await?;
	}
	Ok(())
}

async fn compact_node_or_set<T, F>(
	active_context: &Context<'_, T>,
	item_active_property: String,
	nest_result: &mut T::Object,
	mut expanded_item: T,
	mut compacted_item: T,
	container: &Container,
	options: &JsonLdOptionsImpl<'_, T, F>,
	as_array: bool
) -> Result<()>
where
	T: ForeignMutableJson + BuildableJson,
	F: for<'a> Fn(&'a str, &'a Option<LoadDocumentOptions>) -> BoxFuture<'a, Result<RemoteDocument<T>>> + Clone
{
	if_chain! {
		if let Container::Unordered(UnorderedContainer {
			kind: ContainerKind::GraphContainer(GraphContainer { kind: Some(_), is_graph: false }) |
				ContainerKind::Language |
				ContainerKind::Type,
			..
		}) = container;
		then {
			let map_object = nest_result.get_mut(&item_active_property);
			let map_object = if let Some(map_object) = map_object { map_object.as_object_mut().unwrap() } else {
				nest_result.insert(item_active_property.clone(), T::empty_object().into());
				nest_result.get_mut(&item_active_property).unwrap().as_object_mut().unwrap()
			};
			let container_key = compact_iri(active_context, container.get_kind_str().unwrap(), options.inner, None, true, false)?;
			let map_key = Ok(match container {
				LanguageContainer!() => if_chain! {
					if let Some(expanded_item) = expanded_item.as_object_mut();
					if let Some(value) = expanded_item.remove("@value");
					then {
						compacted_item = value;
						expanded_item.remove("@language").map(|lang| lang.into_string().unwrap())
					} else {
						None
					}
				},
				IndexContainer!() => if let Some(index_key) = active_context.term_definitions.get(item_active_property.as_str())
								.and_then(|definition| definition.index_mapping.as_ref()) {
					let container_key = compact_iri(active_context, index_key, options.inner, None, true, false)?;
					compacted_item.as_object_mut()
						.and_then(|compacted_item| compacted_item.remove(&container_key).map(|index| (compacted_item, index)))
						.and_then(|(compacted_item, index)| {
							if index.as_array().is_some() {
								let mut index = index.into_array().unwrap().into_iter();
								let ret = index.next().map(|map_key| map_key.into_string().unwrap());
								for value in index { add_value(compacted_item, &container_key, value, false); }
								ret
							} else {
								index.into_string()
							}
						})
				} else {
					compacted_item.as_object_mut().and_then(|compacted_item| compacted_item.remove(&container_key));
					expanded_item.as_object_mut().and_then(|expanded_item| expanded_item.remove("@index"))
						.map(|index| index.into_string().unwrap())
				},
				IdContainer!() => compacted_item.as_object_mut()
					.and_then(|compacted_item| compacted_item.remove(&container_key))
					.map(|map_key| map_key.into_string().unwrap()),
				TypeContainer!() => {
					let map_key = compacted_item.as_object_mut()
						.and_then(|compacted_item| compacted_item.remove(&container_key).map(|ty| (compacted_item, ty)))
						.and_then(|(compacted_item, ty)| {
							if ty.as_array().is_some() {
								let mut ty = ty.into_array().unwrap().into_iter();
								let ret = ty.next().map(|map_key| map_key.into_string().unwrap());
								for value in ty { add_value(compacted_item, &container_key, value, false); }
								ret
							} else {
								ty.into_string()
							}
						});
					if compacted_item.as_object()
							.map_or(Ok(false), |compacted_item| Ok(compacted_item.len() == 1 &&
								expand_iri!(active_context, compacted_item.iter().next().unwrap().0)?.as_deref() == Some("@id")))? {
						let element = json!(T, {"@id": expanded_item.into_object().unwrap().remove("@id").unwrap_or_else(|| T::null())});
						compacted_item = compact_internal(active_context, Some(&item_active_property), element,
							&JsonLdOptionsImpl {
								inner: &JsonLdOptions { compact_arrays: false, ordered: false, ..(*options.inner).clone() },
								loaded_contexts: MaybeOwned::Borrowed(&options.loaded_contexts)
							}).await?;
					}
					map_key
				},
				_ => { unreachable!() }
			}).transpose().unwrap_or_else(|| compact_iri(active_context, "@none", options.inner, None, true, false))?;
			add_value(map_object, &map_key, compacted_item, as_array);
		} else {
			add_value(nest_result, &item_active_property, compacted_item, as_array);
		}
	}
	Ok(())
}

fn get_nest_result<'a, T>(active_context: &Context<T>, item_active_property: &str, result: &'a mut T::Object) -> Result<&'a mut T::Object>
where
	T: ForeignMutableJson + BuildableJson
{
	if let Some(nest_term) = active_context
		.term_definitions
		.get(item_active_property)
		.and_then(|definition| definition.nest_value.as_ref())
	{
		if nest_term != "@nest" && expand_iri!(active_context, nest_term)?.as_deref() != Some("@nest") {
			return Err(err!(InvalidNestValue));
		}
		if !result.contains(nest_term) {
			result.insert(nest_term.clone(), T::empty_object().into());
		}
		Ok(result.get_mut(nest_term).unwrap().as_object_mut().unwrap())
	} else {
		Ok(result)
	}
}

pub fn compact_iri<T, F>(active_context: &Context<T>, var: &str, options: &JsonLdOptions<T, F>, mut value: Option<&T>, vocab: bool, reverse: bool) -> Result<String>
where
	T: ForeignMutableJson + BuildableJson,
	F: for<'a> Fn(&'a str, &'a Option<LoadDocumentOptions>) -> BoxFuture<'a, Result<RemoteDocument<T>>>
{
	let inverse_context = active_context.inverse_context.get_or_init(|| create_inverse_context(&active_context));
	if vocab && inverse_context.contains_key(var) {
		let default_language = make_lang_dir(
			active_context.default_language.as_ref().cloned().or_else(|| Some("@none".to_string())),
			active_context.default_base_direction.as_ref()
		);
		if let Some(preserve) = value.and_then(|value| value.get_attr("@preserve")) {
			value = Some(preserve.get_index(0).unwrap());
		}
		let mut containers = Vec::new();
		let mut type_language = TypeOrLanguage::Language;
		let mut type_language_value = "@null".to_string();

		macro_rules! add_containers { ($($kind:ident),+) => { $(containers.push(container!($kind)));+ } }

		if let Some(value) = value.and_then(|value| value.as_object()) {
			if value.contains("@index") && !is_graph_object::<T>(value) {
				add_containers!(index, indexes);
			}
		}
		let mut set_default = || {
			type_language = TypeOrLanguage::Type;
			type_language_value = "@id".to_string();
			add_containers!(id, ids, type, types);
		};
		if reverse {
			type_language = TypeOrLanguage::Type;
			type_language_value = "@reverse".to_string();
			containers.push(container!(set));
		} else if let Some(value) = value {
			if let Some(value) = value.as_object() {
				if let Some(list) = value.get("@list") {
					let list = list.as_array().unwrap();
					if !value.contains("@index") {
						containers.push(container!(list));
					}
					let mut common_type = None;
					let mut common_language = if list.len() == 0 { Some(default_language) } else { None };
					for item in list.iter() {
						let mut item_language = "@none".to_string();
						let mut item_type = "@none";
						if let Some(item) = item.as_object() {
							if item.contains("@value") {
								let lang_dir = make_lang_dir(
									item.get("@language").map(|lang| lang.as_string().unwrap()).map(|lang| lang.to_string()),
									item.get("@direction").map(|dir| dir.as_string().unwrap())
								);
								if lang_dir != "" {
									item_language = lang_dir;
								} else if let Some(ty) = item.get("@type") {
									item_type = ty.as_string().unwrap();
								} else {
									item_language = "@null".to_string();
								}
							} else {
								item_type = "@id";
							}
						}
						if common_language.is_none() {
							common_language = Some(item_language);
						} else if Some(item_language) != common_language && item.as_object().map_or(false, |item| item.contains("@value")) {
							common_language = Some("@none".to_string());
						}
						if common_type.is_none() {
							common_type = Some(item_type);
						} else if Some(item_type) != common_type {
							common_type = Some("@none");
						}
						if common_language.as_deref() == Some("@none") && common_type == Some("@none") {
							break;
						}
					}
					let common_language = common_language.unwrap_or_else(|| "@none".to_string());
					let common_type = common_type.unwrap_or("@none");
					if common_type != "@none" {
						type_language = TypeOrLanguage::Type;
						type_language_value = common_type.to_string();
					} else {
						type_language_value = common_language;
					}
				} else if is_graph_object::<T>(value) {
					if value.contains("@index") {
						add_containers!(index_graph, indexes_graph);
					}
					if value.contains("@id") {
						add_containers!(id_graph, ids_graph);
					}
					add_containers!(graph, set_graph, set);
					if !value.contains("@index") {
						add_containers!(index_graph, indexes_graph);
					}
					if !value.contains("@id") {
						add_containers!(id_graph, ids_graph);
					}
					add_containers!(index, indexes);
					type_language = TypeOrLanguage::Type;
					type_language_value = "@id".to_string();
				} else {
					if value.contains("@value") {
						if_chain! {
							if !value.contains("@index");
							let lang_dir = make_lang_dir(
								value.get("@language").map(|lang| lang.as_string().unwrap()).map(|lang| lang.to_string()),
								value.get("@direction").map(|dir| dir.as_string().unwrap()));
							if lang_dir != "";
							then {
								type_language_value = lang_dir;
								add_containers!(language, languages);
							} else {
								if let Some(ty) = value.get("@type") {
									type_language_value = ty.as_string().unwrap().to_string();
									type_language = TypeOrLanguage::Type;
								}
							}
						}
					} else {
						set_default();
					}
					containers.push(container!(set));
				}
			} else {
				set_default();
				containers.push(container!(set));
			}
		} else {
			set_default();
			containers.push(container!(set));
		}
		containers.push(container!(None));
		if options.processing_mode != JsonLdProcessingMode::JsonLd1_0 {
			if let Some(value) = value.and_then(|value| value.as_object()) {
				if !value.contains("@index") {
					add_containers!(index, indexes);
				}
				if value.len() == 1 && value.contains("@value") {
					add_containers!(language, languages);
				}
			} else {
				add_containers!(index, indexes);
			}
		}
		let mut preferred_values = Vec::new();
		if type_language_value == "@reverse" {
			preferred_values.push("@reverse");
		}
		if_chain! {
			if let "@id" | "@reverse" = type_language_value.as_str();
			if let Some(id) = value.and_then(|value| value.get_attr("@id")).map(|id| id.as_string().unwrap());
			then {
				let result = compact_iri(active_context, id, options, None, true, false)?;
				if_chain! {
					if let Some(term_definition) = active_context.term_definitions.get(result.as_str());
					if term_definition.iri.as_deref() == Some(id);
					then {
						preferred_values.push("@vocab");
						preferred_values.push("@id");
					} else {
						preferred_values.push("@id");
						preferred_values.push("@vocab");
					}
				}
			} else {
				preferred_values.push(&type_language_value);
			}
		}
		preferred_values.push("@none");
		if value.and_then(|value| value.get_attr("@list")).and_then(|list| list.as_array()).map(|list| list.len()) == Some(0) {
			type_language = TypeOrLanguage::Any;
		}
		preferred_values.push("@any");
		for i in 0..preferred_values.len() {
			let value = preferred_values[i];
			if let Some(index) = value.find('_') {
				preferred_values.push(&value[index..]);
			}
		}
		let term = select_term(active_context, var, containers, type_language, preferred_values);
		if let Some(term) = term {
			return Ok(term.to_string());
		}
	}
	if_chain! {
		if vocab;
		if let Some(vocabulary_mapping) = active_context.vocabulary_mapping.as_ref();
		if var.starts_with(vocabulary_mapping);
		let suffix = &var[vocabulary_mapping.len()..];
		if !active_context.term_definitions.contains(suffix);
		then { return Ok(suffix.to_string()); }
	}
	let mut compact_iri: Option<String> = None;
	for (key, definition) in active_context.term_definitions.iter() {
		let iri = definition.iri.as_ref();
		if iri.is_none() || iri.unwrap() == var || !var.starts_with(iri.unwrap()) || !definition.prefix {
			continue;
		}
		let candidate: String = key.0.clone() + ":" + &var[iri.unwrap().len()..];
		if (compact_iri.is_none() || candidate.as_str() < compact_iri.as_deref().unwrap())
			&& (active_context
				.term_definitions
				.get(candidate.as_str())
				.map_or(true, |definition| definition.iri.as_deref() == Some(var) && value == None))
		{
			compact_iri = Some(candidate);
		}
	}
	if let Some(compact_iri) = compact_iri {
		return Ok(compact_iri);
	}
	if var
		.find(':')
		.filter(|scheme| &var[(scheme + 1)..(scheme + 3)] != "//")
		.and_then(|scheme| active_context.term_definitions.get(&var[..scheme]))
		.map_or(false, |definition| definition.prefix)
	{
		return Err(err!(IRIConfusedWithPrefix));
	}
	if !vocab {
		if let Some(base_iri) = active_context.base_iri.as_ref() {
			let var = resolve(&var, Some(&base_iri)).unwrap();
			if *base_iri == var {
				let mut base_iri = base_iri.clone();
				{
					let mut path = base_iri.path_segments_mut().unwrap();
					path.pop();
					path.push("");
				}
				return Ok(base_iri.make_relative(&var).unwrap());
			}
			return Ok(base_iri.make_relative(&var).unwrap_or(var.to_string()));
		}
	}
	Ok(var.to_string())
}

fn compact_value<T, F>(active_context: &Context<T>, active_property: Option<&str>, mut value: T::Object, options: &JsonLdOptions<T, F>) -> Result<T>
where
	T: ForeignMutableJson + BuildableJson,
	F: for<'a> Fn(&'a str, &'a Option<LoadDocumentOptions>) -> BoxFuture<'a, Result<RemoteDocument<T>>>
{
	let term_definition = active_property.and_then(|active_property| active_context.term_definitions.get(active_property));
	let type_mapping = term_definition.and_then(|definition| definition.type_mapping.as_deref());
	let value = (|| {
		if value.len() == (if value.contains("@index") { 2 } else { 1 }) {
			if let Some(id) = value.remove("@id").map(|id| id.into_string().unwrap()) {
				// Contrary to spec, we compact the IRI regardless of the type
				return Ok(match type_mapping {
					Some("@id") => compact_iri(active_context, &id, options, None, false, false)?.into(),
					Some("@vocab") => compact_iri(active_context, &id, options, None, true, false)?.into(),
					_ => {
						value.insert("@id".to_string(), compact_iri(active_context, &id, options, None, false, false)?.into());
						value.into()
					}
				});
			}
		}
		if let Some(ty) = value.remove("@type").map(|ty| ty.into_string().unwrap()) {
			if Some(ty.as_str()) == type_mapping {
				return Ok(value.remove("@value").unwrap());
			} else {
				value.insert("@type".to_string(), compact_iri(active_context, &ty, options, None, true, false)?.into());
			}
		} else if type_mapping.as_deref() != Some("@none") && (!value.contains("@index") || term_definition.map_or(false, |definition| definition.container_mapping.is_index())) {
			let language = term_definition
				.and_then(|definition| definition.language_mapping.as_ref().map(|lang| lang.as_deref()))
				.unwrap_or(active_context.default_language.as_deref());
			let direction = term_definition
				.and_then(|definition| definition.direction_mapping.as_ref())
				.unwrap_or(active_context.default_base_direction.as_ref().unwrap_or(&Direction::None));
			if value.get("@value").unwrap().as_string().is_none()
				|| (value.get("@language").map(|lang| lang.as_string().unwrap()) == language.as_deref()
					&& value.get("@direction").map(|lang| lang.as_string().unwrap()).unwrap_or("@none") == direction.as_ref())
			{
				return Ok(value.remove("@value").unwrap());
			}
		}
		Ok(value.into())
	})()?;
	Ok(if value.as_object().is_some() {
		value
			.into_object()
			.unwrap()
			.into_iter()
			.map(|(key, value)| Ok((compact_iri(active_context, &key, options, None, true, false)?, value)))
			.collect::<Result<T::Object>>()?
			.into()
	} else {
		value
	})
}
