use std::error::Error;
use std::{fmt, fmt::Display};

#[derive(Debug)]
pub struct JsonLdError {
	pub code: JsonLdErrorCode,
	pub description: Option<String>,
	pub cause: Option<Box<dyn Error>>
}

#[derive(Debug)]
pub enum JsonLdErrorCode {
	ContextOverflow,
	CyclicIRIMapping,
	InvalidBaseDirection,
	InvalidBaseIRI,
	InvalidContainerMapping,
	InvalidContextEntry,
	InvalidContextNullification,
	InvalidDefaultLanguage,
	InvalidImportValue,
	InvalidIRIMapping,
	InvalidLanguageMapping,
	InvalidNestValue,
	InvalidPrefixValue,
	InvalidPropagateValue,
	InvalidProtectedValue,
	InvalidRemoteContext,
	InvalidReverseProperty,
	InvalidScopedContext,
	InvalidTermDefinition,
	InvalidTypeMapping,
	InvalidVersionValue,
	InvalidVocabMapping,
	KeywordRedefinition,
	LoadingDocumentFailed,
	LoadingRemoteContextFailed,
	MultipleContextLinkHeaders,
	ProcessingModeConflict,
	ProtectedTermRedefinition
}

impl Display for JsonLdErrorCode {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		use JsonLdErrorCode::*;
		match self {
			ContextOverflow => write!(f, "{}", "context overflow"),
			CyclicIRIMapping => write!(f, "{}", "cyclic IRI mapping"),
			InvalidBaseDirection => write!(f, "{}", "invalid base direction"),
			InvalidBaseIRI => write!(f, "invalid base IRI"),
			InvalidContainerMapping => write!(f, "{}", "invalid container mapping"),
			InvalidContextEntry => write!(f, "{}", "invalid @context entry"),
			InvalidContextNullification => write!(f, "{}", "invalid context nullification"),
			InvalidDefaultLanguage => write!(f, "{}", "invalid default language"),
			InvalidImportValue => write!(f, "{}", "invalid @import value"),
			InvalidIRIMapping => write!(f, "{}", "invalid IRI mapping"),
			InvalidLanguageMapping => write!(f, "{}", "invalid language mapping"),
			InvalidNestValue => write!(f, "{}", "invalid @nest value"),
			InvalidPrefixValue => write!(f, "{}", "invalid @prefix value"),
			InvalidPropagateValue => write!(f, "{}", "invalid @propagate value"),
			InvalidProtectedValue => write!(f, "{}", "invalid @protected value"),
			InvalidRemoteContext => write!(f, "{}", "invalid remote context"),
			InvalidReverseProperty =>  write!(f, "{}", "invalid reverse property"),
			InvalidScopedContext =>  write!(f, "{}", "invalid scoped context"),
			InvalidTermDefinition => write!(f, "{}", "invalid term definition"),
			InvalidTypeMapping => write!(f, "{}", "invalid type mapping"),
			InvalidVersionValue => write!(f, "{}", "invalid @version value"),
			InvalidVocabMapping => write!(f, "invalid vocab mapping"),
			KeywordRedefinition => write!(f, "{}", "keyword redefinition"),
			LoadingDocumentFailed => write!(f, "loading document failed"),
			LoadingRemoteContextFailed => write!(f, "loading remote context failed"),
			MultipleContextLinkHeaders => write!(f, "{}", "multiple context link headers"),
			ProcessingModeConflict => write!(f, "{}", "processing mode conflict"),
			ProtectedTermRedefinition => write!(f, "{}", "protected term redefinition")
		}
	}
}

impl Display for JsonLdError {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		write!(f, "{}", self.code)?;
		if let Some(ref description) = self.description {
			write!(f, ": {}", description)?;
		}
		if let Some(ref cause) = self.cause {
			write!(f, "\ncaused by: {}", cause)?;
		}
		Ok(())
	}
}

impl Error for JsonLdError {}

pub type Result<T> = std::result::Result<T, JsonLdError>;