#[cfg(feature = "reqwest-loader")]
mod defaultdocumentloader;

use std::future::Future;
use json_trait::{ForeignMutableJson, BuildableJson};

use crate::{JsonLdOptions, RemoteDocument};
use crate::error::JsonLdError;

pub struct LoadDocumentOptions {
	pub extract_all_scripts: bool,
	pub profile: Option<String>,
	pub request_profile: Vec<String>
}

pub async fn load_remote<'a, T, F, R>(iri: &str, options: &JsonLdOptions<'a, T, F, R>,
	profile: Option<String>, request_profile: Vec<String>) ->
		Result<RemoteDocument<'a, T>, JsonLdError> where
	T: ForeignMutableJson<'a> + BuildableJson<'a>,
	F: Fn(&str, &Option<LoadDocumentOptions>) -> R,
	R: Future<Output = Result<RemoteDocument<'a, T>, JsonLdError>>
{
	let load_document_options = Some(LoadDocumentOptions {
		extract_all_scripts: options.extract_all_scripts,
		profile,
		request_profile
	});
	if let Some(ref document_loader) = options.document_loader {
		Ok(document_loader(iri, &load_document_options).await?)
	} else {
		#[cfg(feature = "reqwest-loader")]
		return Ok(self::defaultdocumentloader::default_document_loader(iri, &load_document_options).await?);
		#[cfg(not(feature = "reqwest-loader"))]
		Err(crate::error::JsonLdErrorCode::LoadingDocumentFailed.with_description("Default Document Loader is not specified", None))
	}
}