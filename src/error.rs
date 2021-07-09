use std::error::Error;
use std::fmt;
use std::fmt::Display;

#[derive(Debug)]
pub struct JsonLdError {
	pub code: JsonLdErrorCode,
	pub description: Option<String>,
	pub cause: Option<Box<dyn Error + Send + Sync + 'static>>
}

#[derive(Debug)]
#[non_exhaustive]
pub enum JsonLdErrorCode {
	CollidingKeywords,
	ContextOverflow,
	CyclicIRIMapping,
	InvalidBaseDirection,
	InvalidBaseIRI,
	InvalidContainerMapping,
	InvalidContextEntry,
	InvalidContextNullification,
	InvalidDefaultLanguage,
	InvalidIdValue,
	InvalidImportValue,
	InvalidIndexValue,
	InvalidIRIMapping,
	InvalidJsonLiteral,
	InvalidLanguageMapping,
	InvalidLanguageMapValue,
	InvalidLanguageTaggedString,
	InvalidLanguageTaggedValue,
	InvalidLocalContext,
	InvalidNestValue,
	InvalidPrefixValue,
	InvalidPropagateValue,
	InvalidProtectedValue,
	InvalidRemoteContext,
	InvalidReverseProperty,
	InvalidReversePropertyMap,
	InvalidReversePropertyValue,
	InvalidReverseValue,
	InvalidScopedContext,
	InvalidSetOrListObject,
	InvalidTermDefinition,
	InvalidTypedValue,
	InvalidTypeMapping,
	InvalidTypeValue,
	InvalidValueObject,
	InvalidValueObjectValue,
	InvalidVersionValue,
	InvalidVocabMapping,
	IRIConfusedWithPrefix,
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
			CollidingKeywords => write!(f, "{}", "colliding keywords"),
			ContextOverflow => write!(f, "{}", "context overflow"),
			CyclicIRIMapping => write!(f, "{}", "cyclic IRI mapping"),
			InvalidBaseDirection => write!(f, "{}", "invalid base direction"),
			InvalidBaseIRI => write!(f, "invalid base IRI"),
			InvalidContainerMapping => write!(f, "{}", "invalid container mapping"),
			InvalidContextEntry => write!(f, "{}", "invalid context entry"),
			InvalidContextNullification => write!(f, "{}", "invalid context nullification"),
			InvalidDefaultLanguage => write!(f, "{}", "invalid default language"),
			InvalidIdValue => write!(f, "{}", "invalid @id value"),
			InvalidImportValue => write!(f, "{}", "invalid @import value"),
			InvalidIndexValue => write!(f, "{}", "invalid @index value"),
			InvalidIRIMapping => write!(f, "{}", "invalid IRI mapping"),
			InvalidJsonLiteral => write!(f, "{}", "invalid JSON literal"),
			InvalidLanguageMapping => write!(f, "{}", "invalid language mapping"),
			InvalidLanguageMapValue => write!(f, "{}", "invalid language map value"),
			InvalidLanguageTaggedString => write!(f, "{}", "invalid language-tagged string"),
			InvalidLanguageTaggedValue => write!(f, "{}", "invalid language-tagged value"),
			InvalidLocalContext => write!(f, "{}", "invalid local context"),
			InvalidNestValue => write!(f, "{}", "invalid @nest value"),
			InvalidPrefixValue => write!(f, "{}", "invalid @prefix value"),
			InvalidPropagateValue => write!(f, "{}", "invalid @propagate value"),
			InvalidProtectedValue => write!(f, "{}", "invalid @protected value"),
			InvalidRemoteContext => write!(f, "{}", "invalid remote context"),
			InvalidReverseProperty => write!(f, "{}", "invalid reverse property"),
			InvalidReversePropertyMap => write!(f, "{}", "invalid reverse property map"),
			InvalidReversePropertyValue => write!(f, "{}", "invalid reverse property value"),
			InvalidReverseValue => write!(f, "{}", "invalid @reverse value"),
			InvalidScopedContext => write!(f, "{}", "invalid scoped context"),
			InvalidSetOrListObject => write!(f, "{}", "invalid set or list object"),
			InvalidTermDefinition => write!(f, "{}", "invalid term definition"),
			InvalidTypedValue => write!(f, "{}", "invalid typed value"),
			InvalidTypeMapping => write!(f, "{}", "invalid type mapping"),
			InvalidTypeValue => write!(f, "{}", "invalid type value"),
			InvalidValueObject => write!(f, "{}", "invalid value object"),
			InvalidValueObjectValue => write!(f, "{}", "invalid value object value"),
			InvalidVersionValue => write!(f, "{}", "invalid @version value"),
			InvalidVocabMapping => write!(f, "invalid vocab mapping"),
			IRIConfusedWithPrefix => write!(f, "IRI confused with prefix"),
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
