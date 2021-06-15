#![feature(iter_intersperse)]

use std::error::Error;
use std::borrow::Cow;

use futures::future::FutureExt;
use json_ld_rs_stable as stable;
use json_ld_rs::{
	compact, expand,
	JsonLdInput, JsonOrReference, JsonLdOptions, JsonLdOptionsWithoutDocumentLoader, JsonLdProcessingMode, load_remote
};
use json_ld_rs::error::JsonLdErrorCode;
use serde_json::{Value, Map};
use url::Url;
use regex::Regex;
use lazy_static::lazy_static;

use async_recursion::async_recursion;

mod util;

use crate::util::json_ld_eq;
use crate::util::type_state::*;
use crate::util::record::TestRecord;

#[derive(Debug)]
enum JsonLdTestError {
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
	static ref FILTER: Regex = Regex::new(std::env::args().nth(2).as_deref().unwrap_or("")).unwrap();
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
	println!();
	evaluate_json_ld(&stable::JsonLdInput::Reference("https://w3c.github.io/json-ld-api/tests/manifest.jsonld".to_string()),
		&mut TestRecord { pass: 0, fail: 0, skip: 0 }, 0).await?;
	println!();
	Ok(())
}

#[async_recursion(?Send)]
async fn evaluate_json_ld(value: &stable::JsonLdInput<Value>, record: &mut TestRecord, depth: usize) -> Result<(), Box<dyn Error>> {
	let base_iri = (if let stable::JsonLdInput::Reference(ref iri) = value { Some(iri.clone()) } else { None })
		.map(|base| Url::parse(&base)).transpose()?;
	let value = stable::expand(value, &stable::JsonLdOptions::default()).await?;
	for item in value {
		if let Value::Object(item) = item {
			evaluate_object(item, record, &base_iri, depth).await?;
		} else {
			unreachable!();
		}
	}
	Ok(())
}

#[async_recursion(?Send)]
async fn evaluate_object(mut value: Map<String, Value>, record: &mut TestRecord,
		base: &Option<Url>, depth: usize) -> Result<(), Box<dyn Error>> {
	// println!("Evaluating {:#?}", value);
	if let Some(types) = value.remove("@type") {
		evaluate_typed_object(value, types.as_array().unwrap(), TypeState::Initial, record, base, depth).await?;
	}
	Ok(())
}

#[async_recursion(?Send)]
async fn evaluate_typed_object(value: Map<String, Value>, types: &[Value], mut state: TypeState, record: &mut TestRecord,
		base: &Option<Url>, depth: usize) -> Result<(), Box<dyn Error>> {
	if let Some(ty) = types.get(0) {
		if let Some(ty) = ty.as_str() {
			state.register(ty);
			evaluate_typed_object(value, &types[1..], state, record, base, depth).await
		} else {
			panic!("invalid @type value: {}", ty)
		}
	} else {
		match state {
			TypeState::Manifest => evaluate_manifest(value, record, base, depth).await,
			TypeState::Test(Some(test_type), Some(test_class), is_html) =>
				Ok(evaluate_test(value, test_type, test_class, is_html, record, base, depth).await?),
			_ => panic!("unexpected end of @type types")
		}
	}
}

#[async_recursion(?Send)]
async fn evaluate_manifest(mut value: Map<String, Value>, parent_record: &mut TestRecord,
		base: &Option<Url>, depth: usize) -> Result<(), Box<dyn Error>> {
	if let Some(name) = value.get("http://www.w3.org/2001/sw/DataAccess/tests/test-manifest#name") {
		println!("{}Evaluating {}", "    ".repeat(depth),
			name.as_array().unwrap().iter()
				.map(|name| name["@value"].to_string()).intersperse_with(|| ", ".to_string()).collect::<String>());
	}
	if let Some(Value::Array(sequence)) = value.remove("http://www.w3.org/2001/sw/DataAccess/tests/test-manifest#entries")
	 		.map(|mut sequence| sequence.as_array_mut().unwrap().remove(0).as_object_mut().unwrap().remove("@list").unwrap()){
		let mut record = TestRecord { pass: 0, fail: 0, skip: 0 };
		for item in sequence {
			if let Value::Object(value) = item {
				if let Some(url) = value.get("@id").and_then(|url| url.as_str())
						.map(|url| Url::options().base_url(base.as_ref()).parse(&url)).transpose()? {
					if base.as_ref().map_or(true, |base| base.as_str() != &url[..url::Position::AfterQuery]) {
						evaluate_json_ld(&stable::JsonLdInput::Reference(url.to_string()),
							&mut record, depth + 1).await?;
						continue;
					}
				}
				evaluate_object(value, &mut record, base, depth + 1).await?;
			} else {
				panic!("invalid item in sequence");
			}
		}
		println!("{}{} passed; {} failed; {} skipped", "    ".repeat(depth), record.pass, record.fail, record.skip);
		parent_record.pass += record.pass;
		parent_record.fail += record.fail;
		parent_record.skip += record.skip;
	}
	Ok(())
}

async fn evaluate_test(value: Map<String, Value>, test_type: TestType, test_class: TestClass, _is_html: bool,
		record: &mut TestRecord, base: &Option<Url>, depth: usize) -> Result<(), JsonLdTestError> {
	if value.get("@id").and_then(|url| url.as_str()).map_or(true, |url| !FILTER.is_match(url)) {
		record.skip += 1;
		return Ok(());
	}
	let options = value.get("https://w3c.github.io/json-ld-api/tests/vocab#option").and_then(|options| options.get(0));
	let options = evaluate_option(options)?;
	let name = value.get("http://www.w3.org/2001/sw/DataAccess/tests/test-manifest#name")
		.and_then(|v| v.pointer("/0/@value")).ok_or(JsonLdTestError::InvalidManifest("no name found"))?;
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
				&JsonLdOptions::default(), None, vec![]).await
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
		_ => {
			record.skip += 1;
			return Ok(())
		}
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
							&JsonLdOptions::default(), None, vec![]).await
							.map_err(|_| JsonLdTestError::JsonLdError(JsonLdErrorCode::LoadingDocumentFailed))?.document.to_parsed()
							.map_err(|_| JsonLdTestError::JsonLdError(JsonLdErrorCode::LoadingDocumentFailed))?;
						if json_ld_eq(&output, &target, false) {
							record.pass += 1;
						} else {
							eprintln!("{}Assert failed at test {}: {} != {}", "    ".repeat(depth), name, output, target);
							record.fail += 1;
						}
					},
					TestType::PositiveSyntaxTest => record.pass += 1,
					TestType::NegativeEvaluationTest => {
						let target = value.get("http://www.w3.org/2001/sw/DataAccess/tests/test-manifest#result")
							.and_then(|v| v.pointer("/0/@value"))
							.and_then(|input| input.as_str()).ok_or(JsonLdTestError::InvalidManifest("invalid target"))?;
						eprintln!("{}Assert failed at test {}: expected {}, {}", "    ".repeat(depth), name, target, output);
						record.fail += 1;
					},
					TestType::NegativeSyntaxTest => {
						eprintln!("{}Assert failed at test {}: expected an error, {}", "    ".repeat(depth), name, output);
						record.fail += 1;
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
							record.pass += 1;
						} else {
							eprintln!("{}Assert failed at test {}: expected {}, {}", "    ".repeat(depth), name, target, err.code);
							record.fail += 1;
						}
					},
					TestType::NegativeSyntaxTest => record.pass += 1,
					_ => {
						eprintln!("{}Error at test {}: {}", "    ".repeat(depth), name, err);
						record.fail += 1;
					}
				}
			}
		},
		Err(panic) => {
			if let Some(s) = panic.downcast_ref::<&str>() {
				eprintln!("{}Testing {}: {}", "    ".repeat(depth), name, s);
			} else {
				eprintln!("A panic occurred at test {}", name);
			}
			record.fail += 1;
		}
	}
	Ok(())
}

fn evaluate_option(options: Option<&'_ Value>) -> Result<JsonLdOptionsWithoutDocumentLoader<'_, Value>, JsonLdTestError> {
	Ok(JsonLdOptions {
		base: options.and_then(|options| options.get("https://w3c.github.io/json-ld-api/tests/vocab#base")
			.and_then(|v| v.pointer("/0/@value")).map(|base| base.as_str().map(|base| base.to_string())
			.ok_or(JsonLdTestError::InvalidManifest("invalid base iri")))).transpose()?,
		expand_context: options.and_then(|options| options.get("https://w3c.github.io/json-ld-api/tests/vocab#expandContext")
			.and_then(|v| v.pointer("/0/@value"))
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