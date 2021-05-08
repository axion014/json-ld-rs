use reqwest::{Client, Response, header, redirect::Policy, StatusCode};
use http_link::{Link, parse_link_header};
use async_recursion::async_recursion;

use json_trait::{ForeignMutableJson, BuildableJson};

use crate::{RemoteDocument, Document};
use crate::error::{JsonLdError, JsonLdErrorCode::*};
use crate::util::OneError;
use super::LoadDocumentOptions;

lazy_static::lazy_static! {
	static ref IRI_CLIENT: Client = Client::builder().redirect(Policy::custom(|attempt| {
		match attempt.status() {
			StatusCode::MULTIPLE_CHOICES | StatusCode::SEE_OTHER => attempt.stop(),
			_ => Policy::default().redirect(attempt)
		}
	})).build().unwrap();
	static ref REPRESENTATION_CLIENT: Client = Client::new();
}

fn process_link_headers<'a>(response: &'a Response, url: &'a url::Url) -> impl Iterator<Item = Result<Link, JsonLdError>> + 'a {
	response.headers().get_all(header::LINK).iter().flat_map(move |links| {
		OneError::new(match links.to_str() {
			Ok(links) => parse_link_header(links, url).map_err(|e| LoadingDocumentFailed.to_error(Some(Box::new(e)))),
			Err(e) => Err(LoadingDocumentFailed.to_error(Some(Box::new(e))))
		})
	})
}

#[async_recursion(?Send)]
pub async fn default_document_loader<T>(url_str: &str, options: &Option<LoadDocumentOptions>) ->
		Result<RemoteDocument<T>, JsonLdError> where
	T: ForeignMutableJson + BuildableJson
{
	let mut accept = String::from("application/ld+json");

	if let Some(LoadDocumentOptions { request_profile, .. }) = options {
		if !request_profile.is_empty() {
			accept += ";profile=\"";
			for request_profile in request_profile {
				accept += " ";
				accept += &request_profile;
			}
			accept += "\", application/ld+json";
		}
	}
	accept += ", application/json";

	let url = url::Url::parse(url_str).map_err(|e| LoadingDocumentFailed.to_error(Some(Box::new(e))))?;

	let intermediate_response = IRI_CLIENT.get(url.clone())
		.header(header::ACCEPT, accept.clone())
		.send().await
		.map_err(|e| LoadingDocumentFailed.to_error(Some(Box::new(e))))?;
	let document_url = intermediate_response.url().to_string();
	let response = if intermediate_response.status().is_redirection() {
		REPRESENTATION_CLIENT.get(intermediate_response.url().clone())
			.header(header::ACCEPT, accept)
			.send().await
			.map_err(|e| LoadingDocumentFailed.to_error(Some(Box::new(e))))?
	} else { intermediate_response };

	let mut context_url = None;
	let content_type = response.headers().get(header::CONTENT_TYPE)
		.map(|s| s.to_str().map_err(|e| LoadingDocumentFailed.to_error(Some(Box::new(e))))?
			.parse::<mime::Mime>().map_err(|e| LoadingDocumentFailed.to_error(Some(Box::new(e))))
		).ok_or(LoadingDocumentFailed.with_description("Content-Type header is missing", None))??;

	match content_type.essence_str() {
		"application/ld+json" => {},
		t if t == "application/json" || t.ends_with("+json") => {
			for link in process_link_headers(&response, &url) {
				let link = link?;
				if link.rel == "http://www.w3.org/ns/json-ld#context" {
					if context_url.is_some() { return Err(MultipleContextLinkHeaders.to_error(None)); }
					context_url = Some(link.target);
				}
			}
		},
		t => {
			if !(t == "text/html" || t == "application/xhtml+xml") {
				for link in process_link_headers(&response, &url) {
					let link = link?;
					if link.rel == "alternate" {
						for attribute in link.attributes {
							if attribute.name == "type" && attribute.value == "application/ld+json" {
								return default_document_loader(link.target.as_str(), options).await;
							}
						}
					}
				}
			}
			return Err(LoadingDocumentFailed.with_description("No JSON representation of resource found", None));
		}
	}
	let content_text = &response.text().await.map_err(|e| LoadingDocumentFailed.to_error(Some(Box::new(e))))?;
	let content_json = T::from_str(content_text).map_err(|e| LoadingDocumentFailed.to_error(Some(Box::new(e))))?;

	Ok(RemoteDocument {
		content_type: content_type.essence_str().to_string(),
		context_url: context_url.map(|url| url.to_string()),
		document: Document::ParsedJson(content_json),
		document_url,
		profile: content_type.get_param("profile").map(|mime| mime.as_str().to_string())
	})
}