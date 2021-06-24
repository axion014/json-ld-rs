use async_recursion::async_recursion;
use http_link::{parse_link_header, Link};
use reqwest::redirect::Policy;
use reqwest::{header, Client, Response, StatusCode};

use json_trait::{BuildableJson, ForeignMutableJson};

use super::LoadDocumentOptions;
use crate::error::JsonLdError;
use crate::error::JsonLdErrorCode::*;
use crate::util::OneError;
use crate::{Document, RemoteDocument};

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
			Ok(links) => parse_link_header(links, url).map_err(|e| err!(LoadingDocumentFailed, , e)),
			Err(e) => Err(err!(LoadingDocumentFailed, , e))
		})
	})
}

#[async_recursion]
pub async fn default_document_loader<T>(url_str: &str, options: &Option<LoadDocumentOptions>) -> Result<RemoteDocument<T>, JsonLdError>
where
	T: ForeignMutableJson + BuildableJson
{
	let mut accept = String::from("application/ld+json");

	if let Some(LoadDocumentOptions { request_profile, .. }) = options {
		if !request_profile.is_empty() {
			accept += ";profile=\"";
			let mut iter = request_profile.iter();
			accept += iter.next().unwrap();
			for request_profile in iter {
				accept += " ";
				accept += &request_profile;
			}
		}
	}
	accept += ", application/json";

	let url = url::Url::parse(url_str).map_err(|e| err!(LoadingDocumentFailed, , e))?;

	let intermediate_response = IRI_CLIENT
		.get(url.clone())
		.header(header::ACCEPT, accept.clone())
		.send()
		.await
		.map_err(|e| err!(LoadingDocumentFailed, , e))?;
	let document_url = intermediate_response.url().to_string();
	let response = if intermediate_response.status().is_redirection() {
		REPRESENTATION_CLIENT
			.get(intermediate_response.url().clone())
			.header(header::ACCEPT, accept)
			.send()
			.await
			.map_err(|e| err!(LoadingDocumentFailed, , e))?
	} else {
		intermediate_response
	};

	let mut context_url = None;
	let content_type = response
		.headers()
		.get(header::CONTENT_TYPE)
		.map(|s| {
			s.to_str()
				.map_err(|e| err!(LoadingDocumentFailed, , e))?
				.parse::<mime::Mime>()
				.map_err(|e| err!(LoadingDocumentFailed, , e))
		})
		.ok_or(err!(LoadingDocumentFailed, "Content-Type header is missing"))??;

	match content_type.essence_str() {
		"application/ld+json" => {}
		t if t == "application/json" || t.ends_with("+json") => {
			for link in process_link_headers(&response, &url) {
				let link = link?;
				if link.rel == "http://www.w3.org/ns/json-ld#context" {
					if context_url.is_some() {
						return Err(err!(MultipleContextLinkHeaders));
					}
					context_url = Some(link.target);
				}
			}
		}
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
			return Err(err!(LoadingDocumentFailed, "No JSON representation of resource found"));
		}
	}
	let content_text = &response.text().await.map_err(|e| err!(LoadingDocumentFailed, , e))?;
	let content_json = T::from_str(content_text).map_err(|e| err!(LoadingDocumentFailed, , e))?;

	Ok(RemoteDocument {
		content_type: content_type.essence_str().to_string(),
		context_url: context_url.map(|url| url.to_string()),
		document: Document::ParsedJson(content_json),
		document_url,
		profile: content_type.get_param("profile").map(|mime| mime.as_str().to_string())
	})
}
