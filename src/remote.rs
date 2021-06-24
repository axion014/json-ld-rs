#[cfg(feature = "reqwest-loader")]
mod defaultdocumentloader;

use futures::future::BoxFuture;
use json_trait::{BuildableJson, ForeignMutableJson};

use crate::error::JsonLdError;
use crate::{JsonLdOptions, RemoteDocument};

pub use defaultdocumentloader::default_document_loader;

pub struct LoadDocumentOptions {
	pub extract_all_scripts: bool,
	pub profile: Option<String>,
	pub request_profile: Vec<String>
}

pub async fn load_remote<T, F>(iri: &str, options: &JsonLdOptions<'_, T, F>, profile: Option<String>, request_profile: Vec<String>) -> Result<RemoteDocument<T>, JsonLdError>
where
	T: ForeignMutableJson + BuildableJson,
	F: for<'a> Fn(&'a str, &'a Option<LoadDocumentOptions>) -> BoxFuture<'a, Result<RemoteDocument<T>, JsonLdError>>
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
		return Ok(default_document_loader(iri, &load_document_options).await?);
		#[cfg(not(feature = "reqwest-loader"))]
		Err(err!(crate::error::JsonLdErrorCode::LoadingDocumentFailed, "Default Document Loader is not specified"))
	}
}
