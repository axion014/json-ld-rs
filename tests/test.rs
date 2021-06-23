#![feature(iter_intersperse)]

use std::error::Error;
use std::borrow::Cow;
use std::sync::Arc;

use futures::{FutureExt, stream, StreamExt, TryStreamExt};
use tokio::sync::Semaphore;
use tokio_util::sync::PollSemaphore;

use json_ld_rs_stable as stable;
use json_ld_rs::{
	compact, expand,
	JsonLdInput, JsonOrReference, JsonLdOptions, JsonLdProcessingMode, RemoteDocument
};
use json_ld_rs::remote::{load_remote, LoadDocumentOptions, default_document_loader};
use json_ld_rs::error::{JsonLdError, JsonLdErrorCode};
use serde_json::{Value, Map};
use url::Url;
use regex::Regex;
use lazy_static::lazy_static;

use async_recursion::async_recursion;

mod util;

use crate::util::json_ld_eq;
use crate::util::type_state::*;
use crate::util::record::{*, TestRecordKind::*, TestFailure::*};

#[derive(Debug)]
pub enum JsonLdTestError {
	InvalidManifest(&'static str),
	JsonLdError(JsonLdErrorCode)
}

impl std::fmt::Display for JsonLdTestError {
	fn fmt(&self, _: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		Ok(())
	}
}

impl Error for JsonLdTestError {}

lazy_static! {
	static ref FILTER: Regex = Regex::new(std::env::args().nth(1).as_deref().unwrap_or("")).unwrap();
	static ref REQUESTS: Arc<Semaphore> = Arc::new(Semaphore::new(20));
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
	println!();
	print!("{}", evaluate_json_ld(
		&stable::JsonLdInput::Reference("https://w3c.github.io/json-ld-api/tests/manifest.jsonld".to_string())
	).await?.unwrap());
	Ok(())
}

#[async_recursion(?Send)]
async fn evaluate_json_ld(value: &stable::JsonLdInput<Value>) -> Result<Option<TestRecord>, Box<dyn Error>> {
	let base_iri = (if let stable::JsonLdInput::Reference(ref iri) = value { Some(iri) } else { None })
		.map(|base| Url::parse(&base)).transpose()?;
	let value = stable::expand(value, &stable::JsonLdOptions::<_>::default()).await?;
	for item in value {
		if let Value::Object(item) = item {
			return evaluate_object(item, &base_iri).await;
		} else {
			unreachable!();
		}
	}
	Ok(None)
}

#[async_recursion(?Send)]
async fn evaluate_object(mut value: Map<String, Value>, base: &Option<Url>) -> Result<Option<TestRecord>, Box<dyn Error>> {
	// println!("Evaluating {:#?}", value);
	if let Some(types) = value.remove("@type") {
		return Some(evaluate_typed_object(value, types.as_array().unwrap(), TypeState::Initial, base).await).transpose();
	}
	Err(JsonLdTestError::InvalidManifest("found an untyped object").into())
}

#[async_recursion(?Send)]
async fn evaluate_typed_object(value: Map<String, Value>, types: &[Value], mut state: TypeState,
		base: &Option<Url>) -> Result<TestRecord, Box<dyn Error>> {
	if let Some(ty) = types.get(0) {
		if let Some(ty) = ty.as_str() {
			state.register(ty);
			evaluate_typed_object(value, &types[1..], state, base).await
		} else {
			panic!("invalid @type value: {}", ty)
		}
	} else {
		match state {
			TypeState::Manifest => evaluate_manifest(value, base).await,
			TypeState::Test(Some(test_type), Some(test_class), is_html) => {
				match evaluate_test(value, test_type, test_class, is_html, base).await {
					Ok(record) => Ok(record),
					Err(err) => Ok(TestRecord { name: None, content: Fail(TestError(err)) })
				}
			},
			_ => panic!("unexpected end of @type types")
		}
	}
}

#[async_recursion(?Send)]
async fn evaluate_manifest(mut value: Map<String, Value>, base: &Option<Url>) -> Result<TestRecord, Box<dyn Error>> {
	let name = value.get("http://www.w3.org/2001/sw/DataAccess/tests/test-manifest#name")
			.map(|v| v.pointer("/0/@value").and_then(|name| name.as_str())
			.ok_or(JsonLdTestError::InvalidManifest("invalid name"))).transpose()?.map(|name| name.to_string());
	let mut records = TestRecords::new();
	if let Some(Value::Array(sequence)) = value.remove("http://www.w3.org/2001/sw/DataAccess/tests/test-manifest#entries")
	 		.map(|mut sequence| sequence.as_array_mut().unwrap().remove(0).as_object_mut().unwrap().remove("@list").unwrap()) {
		records = sequence.into_iter().map(|item| evaluate_manifest_entry(item, base))
			.collect::<stream::FuturesOrdered<_>>()
			.try_fold(records, |mut records, record| async {
				if let Some(record) = record { records.add(record); }
				Ok(records)
			}).await?;
	}
	Ok(TestRecord { name, content: Group(records) })
}

async fn evaluate_manifest_entry(entry: Value, base: &Option<Url>) -> Result<Option<TestRecord>, Box<dyn Error>> {
	Ok(if let Value::Object(value) = entry {
		if let Some(url) = value.get("@id").and_then(|url| url.as_str())
				.map(|url| Url::options().base_url(base.as_ref()).parse(&url)).transpose()? {
			if base.as_ref().map_or(true, |base| base.as_str() != &url[..url::Position::AfterQuery]) {
				if let Some(record) = evaluate_json_ld(&stable::JsonLdInput::Reference(url.to_string())).await? {
					return Ok(Some(record));
				} else {
					return Ok(None);
				}
			}
		}
		if let Some(record) = evaluate_object(value, base).await? {
			Some(record)
		} else {
			None
		}
	} else {
		None
	})
}

async fn evaluate_test(value: Map<String, Value>, test_type: TestType, test_class: TestClass, is_html: bool,
		base: &Option<Url>) -> Result<TestRecord, JsonLdTestError> {
	let name = value.get("http://www.w3.org/2001/sw/DataAccess/tests/test-manifest#name")
			.map(|v| v.pointer("/0/@value").and_then(|name| name.as_str())
			.ok_or(JsonLdTestError::InvalidManifest("invalid name"))).transpose()?.map(|name| name.to_string());
	if is_html {
		return Ok(TestRecord { name, content: Skip });
	}
	if !FILTER.is_match(value["@id"].as_str().unwrap()) && name.as_ref().map_or(true, |name| !FILTER.is_match(name)) {
		return Ok(TestRecord { name, content: Skip });
	}
	let options = value.get("https://w3c.github.io/json-ld-api/tests/vocab#option").and_then(|options| options.get(0));
	if let Some(options) = options {
		if options.get("https://w3c.github.io/json-ld-api/tests/vocab#httpLink").is_some() ||
		 		options.get("https://w3c.github.io/json-ld-api/tests/vocab#contentType").is_some() ||
				options.get("https://w3c.github.io/json-ld-api/tests/vocab#redirectTo").is_some() ||
				options.get("https://w3c.github.io/json-ld-api/tests/vocab#httpStatus").is_some() {
			return Ok(TestRecord { name, content: Skip });
		}
	}
	let options = evaluate_option(options)?;
	let input = value.get("http://www.w3.org/2001/sw/DataAccess/tests/test-manifest#action")
		.and_then(|v| v.pointer("/0/@id")).and_then(|input| input.as_str()).ok_or(JsonLdTestError::InvalidManifest("invalid input"))?;
	let input = JsonLdInput::<Value>::Reference(Url::options().base_url(
		base.as_ref()).parse(input).map_err(|_| JsonLdTestError::InvalidManifest("invalid input url"))?.to_string());
	let output = match test_class {
		TestClass::CompactTest => {
			let context = value.get("https://w3c.github.io/json-ld-api/tests/vocab#context")
				.and_then(|v| v.pointer("/0/@id")).and_then(|context| context.as_str())
				.map(|context| Url::options().base_url(base.as_ref()).parse(context))
				.ok_or(JsonLdTestError::InvalidManifest("invalid context"))?.map_err(|_| JsonLdTestError::InvalidManifest("invalid context url"))?;
			let context = load_remote(context.as_str(),
				&options, None, vec![]).await
				.map_err(|_| JsonLdTestError::JsonLdError(JsonLdErrorCode::LoadingDocumentFailed))?.document.to_parsed()
				.map_err(|_| JsonLdTestError::JsonLdError(JsonLdErrorCode::LoadingDocumentFailed))?;
			let context = match context {
				Value::Array(ctx) => ctx.into_iter().map(|value| Ok(match value {
					// Only one level of recursion, I think
					Value::Object(obj) => JsonOrReference::JsonObject(Cow::Owned(obj)),
					Value::String(reference) => JsonOrReference::Reference(Cow::Owned(reference)),
					_ => return Err(JsonLdTestError::JsonLdError(JsonLdErrorCode::InvalidContextEntry))
				})).collect(),
				Value::Object(ctx) => Ok(vec![JsonOrReference::JsonObject(Cow::Owned(ctx))]),
				Value::String(reference) => Ok(vec![JsonOrReference::Reference(Cow::Owned(reference))]),
				_ => Err(JsonLdTestError::JsonLdError(JsonLdErrorCode::InvalidContextEntry))
			}?;
			std::panic::AssertUnwindSafe(compact(&input, Some(context), &options))
				.catch_unwind().await.map(|output| output.map(|output| Value::Object(output)))
		},
		TestClass::ExpandTest => {
			std::panic::AssertUnwindSafe(expand(&input, &options)).catch_unwind().await
				.map(|output| output.map(|output| Value::Array(output)))
		},
		_ => return Ok(TestRecord { name, content: Skip })
	};
	match output {
		Ok(output) => match output {
			Ok(output) => {
				match test_type {
					TestType::PositiveEvaluationTest => {
						let target = value.get("http://www.w3.org/2001/sw/DataAccess/tests/test-manifest#result")
							.and_then(|v| v.pointer("/0/@id"))
							.and_then(|input| input.as_str()).ok_or(JsonLdTestError::InvalidManifest("invalid target"))?;
						let target = load_remote(
							Url::options().base_url(base.as_ref()).parse(target).map_err(|_| JsonLdTestError::InvalidManifest("invalid target url"))?.as_str(),
							&options, None, vec![]).await
							.map_err(|_| JsonLdTestError::JsonLdError(JsonLdErrorCode::LoadingDocumentFailed))?.document.to_parsed()
							.map_err(|_| JsonLdTestError::JsonLdError(JsonLdErrorCode::LoadingDocumentFailed))?;
						if json_ld_eq(&output, &target, false) {
							Ok(TestRecord { name, content: Pass })
						} else {
							Ok(TestRecord { name, content: Fail(AssertFailure(output.to_string(), target.to_string()))})
						}
					},
					TestType::PositiveSyntaxTest => Ok(TestRecord { name, content: Pass }),
					TestType::NegativeEvaluationTest => {
						let target = value.get("http://www.w3.org/2001/sw/DataAccess/tests/test-manifest#result")
							.and_then(|v| v.pointer("/0/@value"))
							.and_then(|input| input.as_str()).ok_or(JsonLdTestError::InvalidManifest("invalid target"))?;
						Ok(TestRecord { name, content: Fail(AssertFailure(output.to_string(), target.to_string()))})
					},
					TestType::NegativeSyntaxTest => {
						Ok(TestRecord { name, content: Fail(AssertFailure(output.to_string(), "an error".to_string()))})
					}
				}
			},
			Err(err) => {
				match test_type {
					TestType::NegativeEvaluationTest => {
						let target = value.get("http://www.w3.org/2001/sw/DataAccess/tests/test-manifest#result")
							.and_then(|v| v.pointer("/0/@value"))
							.and_then(|input| input.as_str()).ok_or(JsonLdTestError::InvalidManifest("invalid target"))?;
						if &err.code.to_string() == target {
							Ok(TestRecord { name, content: Pass })
						} else {
							Ok(TestRecord { name, content: Fail(AssertFailure(err.code.to_string(), target.to_string()))})
						}
					},
					TestType::NegativeSyntaxTest => Ok(TestRecord { name, content: Pass }),
					_ => Ok(TestRecord { name, content: Fail(Error(err.to_string()))})
				}
			}
		},
		Err(panic) => {
			if let Some(s) = panic.downcast_ref::<&str>() {
				Ok(TestRecord { name, content: Fail(Error(s.to_string()))})
			} else {
				Ok(TestRecord { name, content: Fail(UnknownPanic)})
			}
		}
	}
}

fn evaluate_option(options: Option<&'_ Value>) -> Result<JsonLdOptions<'_, Value>, JsonLdTestError> {
	Ok(JsonLdOptions {
		base: options.and_then(|options| options.get("https://w3c.github.io/json-ld-api/tests/vocab#base")
			.and_then(|v| v.pointer("/0/@id")).map(|base| base.as_str().map(|base| base.to_string())
			.ok_or(JsonLdTestError::InvalidManifest("invalid base iri")))).transpose()?,
		compact_arrays: options.and_then(|options| options.get("https://w3c.github.io/json-ld-api/tests/vocab#compactArrays"))
			.map(|v| v.pointer("/0/@value").and_then(|base| base.as_bool())
			.ok_or(JsonLdTestError::InvalidManifest("invalid compact arrays flag"))).transpose()?.unwrap_or(true),
		document_loader: Some(|url, options| limited_concurrency_loader(url, options)),
		expand_context: options.and_then(|options| options.get("https://w3c.github.io/json-ld-api/tests/vocab#expandContext")
			.and_then(|v| v.pointer("/0/@id"))
			.map(|context| context.as_str().map(|context| JsonOrReference::Reference(Cow::Borrowed(context)))
			.ok_or(JsonLdTestError::InvalidManifest("invalid expand context")))).transpose()?,
		processing_mode: options.and_then(|options| options.get("https://w3c.github.io/json-ld-api/tests/vocab#processingMode")
			.and_then(|v| v.pointer("/0/@value")).map(|mode| match mode.as_str() {
				Some("json-ld-1.1") => Ok(JsonLdProcessingMode::JsonLd1_1),
				Some("json-ld-1.0") => Ok(JsonLdProcessingMode::JsonLd1_0),
				_ => Err(JsonLdTestError::InvalidManifest("invalid processing mode"))
			})).transpose()?.unwrap_or(JsonLdProcessingMode::JsonLd1_1),
		..JsonLdOptions::default()
	})
}

#[async_recursion]
async fn limited_concurrency_loader(url: &str, options: &Option<LoadDocumentOptions>) -> Result<RemoteDocument<Value>, JsonLdError> {
	let _lock = PollSemaphore::new(REQUESTS.clone()).into_future().await;
	default_document_loader(url, options).await
}
