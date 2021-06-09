#![feature(iter_intersperse)]

use std::error::Error;
use std::borrow::Cow;

use futures::future::FutureExt;
use json_ld_rs_stable as stable;
use json_ld_rs::JsonLdProcessor::*;
use json_ld_rs::{JsonLdInput, JsonOrReference, JsonLdOptions, JsonLdOptionsWithoutDocumentLoader, JsonLdProcessingMode, load_remote};
use json_ld_rs::error::JsonLdErrorCode;
use serde_json::{Value, Map};
use url::Url;

use async_recursion::async_recursion;

mod util;

use crate::util::json_ld_eq;
use crate::util::type_state::*;
use crate::util::record::TestRecord;

#[derive(Debug)]
enum JsonLdTestError {
	InvalidManifest,
	JsonLdError(JsonLdErrorCode)
}

impl std::fmt::Display for JsonLdTestError {
	fn fmt(&self, _: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		Ok(())
	}
}

impl Error for JsonLdTestError {}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
	println!();
	evaluate_json_ld(&stable::JsonLdInput::Reference("https://w3c.github.io/json-ld-api/tests/manifest.jsonld".to_string()),
		&mut TestRecord { pass: 0, fail: 0 }, 0).await?;
	println!();
	Ok(())
}

#[async_recursion(?Send)]
async fn evaluate_json_ld(value: &stable::JsonLdInput<Value>, record: &mut TestRecord, depth: usize) -> Result<(), Box<dyn Error>> {
	let base_iri = (if let stable::JsonLdInput::Reference(ref iri) = value { Some(iri.clone()) } else { None })
		.map(|base| Url::parse(&base)).transpose()?;
	let value = stable::JsonLdProcessor::expand(value, &stable::JsonLdOptions::default()).await?;
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
			evaluate_typed_object(value, &types[1..], state, record, base, depth).await?;
		} else {
			panic!("invalid @type value: {}", ty);
		}
	} else {
		match state {
			TypeState::Manifest => evaluate_manifest(value, record, base, depth).await?,
			TypeState::Test(test_type, test_class) => evaluate_test(value, test_type, test_class, record, base, depth).await?,
			TypeState::Initial => (),
			_ => panic!("unexpected end of @type types")
		}
	}
	Ok(())
}

#[async_recursion(?Send)]
async fn evaluate_manifest(mut value: Map<String, Value>, parent_record: &mut TestRecord,
		base: &Option<Url>, depth: usize) -> Result<(), Box<dyn Error>> {
	if let Some(name) = value.get("mf:name") {
		println!("{}Evaluating {}", "    ".repeat(depth),
			name.as_array().unwrap().iter()
				.map(|name| name["@value"].to_string()).intersperse_with(|| ", ".to_string()).collect::<String>());
	}
	if let Some(Value::Array(sequence)) = value.remove("mf:entries") {
		let mut record = TestRecord { pass: 0, fail: 0 };
		for item in sequence {
			if let Value::Object(mut value) = item {
				if let Some(url) = value.remove("@value")
						.map(|value| if let Value::String(value) = value { value } else { unreachable!() }) {
					evaluate_json_ld(&stable::JsonLdInput::Reference(Url::options().base_url(base.as_ref()).parse(&url)?.to_string()),
						&mut record, depth + 1).await?;
				} else {
					evaluate_object(value, &mut record, base, depth + 1).await?;
				}
			} else {
				panic!("invalid item in sequence");
			}
		}
		println!("{}{} passed; {} failed", "    ".repeat(depth), record.pass, record.fail);
		parent_record.pass += record.pass;
		parent_record.fail += record.fail;
	}
	Ok(())
}

async fn evaluate_test(value: Map<String, Value>, test_type: TestType, test_class: TestClass,
		record: &mut TestRecord, base: &Option<Url>, depth: usize) -> Result<(), JsonLdTestError> {
	let input = value.get("mf:action").and_then(|v| v.pointer("/0/@value")).and_then(|input| input.as_str())
		.ok_or(JsonLdTestError::InvalidManifest)?;
	let input = JsonLdInput::<Value>::Reference(Url::options().base_url(
		base.as_ref()).parse(input).map_err(|_| JsonLdTestError::InvalidManifest)?.to_string());
	let target = value.get("mf:result").and_then(|v| v.pointer("/0/@value")).and_then(|input| input.as_str())
		.ok_or(JsonLdTestError::InvalidManifest)?;
	let options = evaluate_option(value.get("jld:Option"))?;
	let output = match test_class {
		TestClass::CompactTest => {
			let context = value.get("https://w3c.github.io/json-ld-api/tests/vocab#context")
				.and_then(|v| v.pointer("/0/@value")).and_then(|input| input.as_str())
				.ok_or(JsonLdTestError::InvalidManifest)?;
			let context = load_remote(
				Url::options().base_url(base.as_ref()).parse(context).map_err(|_| JsonLdTestError::InvalidManifest)?.as_str(),
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
		}
	};
	match output {
		Ok(output) => match output {
			Ok(output) => {
				if let TestType::PositiveEvaluationTest = test_type {
					let target = load_remote(
						Url::options().base_url(base.as_ref()).parse(target).map_err(|_| JsonLdTestError::InvalidManifest)?.as_str(),
						&JsonLdOptions::default(), None, vec![]).await
						.map_err(|_| JsonLdTestError::JsonLdError(JsonLdErrorCode::LoadingDocumentFailed))?.document.to_parsed()
						.map_err(|_| JsonLdTestError::JsonLdError(JsonLdErrorCode::LoadingDocumentFailed))?;
					if json_ld_eq(&output, &target, false) {
						record.pass += 1;
					} else {
						eprintln!("{}Assert Failed: {} != {}", "    ".repeat(depth), output, target);
						record.fail += 1;
					}
				} else {
					eprintln!("{}Assert Failed: expected {}, {}", "    ".repeat(depth), target, output);
					record.fail += 1;
				}
			},
			Err(err) => {
				if let TestType::NegativeEvaluationTest = test_type {
					if &err.code.to_string() == target {
						record.pass += 1;
					} else {
						eprintln!("{}Assert Failed: expected {}, {}", "    ".repeat(depth), target, err.code);
						record.fail += 1;
					}
				} else {
					eprintln!("{}Error: {}", "    ".repeat(depth), err);
					record.fail += 1;
				}
			}
		},
		Err(panic) => {
			if let Some(s) = panic.downcast_ref::<&str>() {
				eprintln!("{}{}", "    ".repeat(depth), s);
			} else {
				eprintln!("panic occurred");
			}
			record.fail += 1;
		}
	}
	Ok(())
}

fn evaluate_option(options: Option<&'_ Value>) -> Result<JsonLdOptionsWithoutDocumentLoader<'_, Value>, JsonLdTestError> {
	Ok(JsonLdOptions {
		base: options.and_then(|options| options.get("jld:base")
			.map(|base| base.as_str().map(|base| base.to_string())
			.ok_or(JsonLdTestError::InvalidManifest))).transpose()?,
		expand_context: options.and_then(|options| options.get("jld:expandContext")
			.map(|context| context.as_str().map(|context| JsonOrReference::Reference(Cow::Borrowed(context)))
			.ok_or(JsonLdTestError::InvalidManifest))).transpose()?,
		processing_mode: options.and_then(|options| options.get("jld:processingMode")
			.map(|mode| match mode.as_str() {
				Some("json-ld-1.1") => Ok(JsonLdProcessingMode::JsonLd1_1),
				Some("json-ld-1.0") => Ok(JsonLdProcessingMode::JsonLd1_0),
				_ => Err(JsonLdTestError::InvalidManifest)
			})).transpose()?.unwrap_or(JsonLdProcessingMode::JsonLd1_1),
		..JsonLdOptions::default()
	})
}