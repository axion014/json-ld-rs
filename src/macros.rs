macro_rules! err {
	($code:expr) => {
		crate::error::JsonLdError { code: $code, description: None, cause: None }
	};
	($code:expr, $desc:expr) => {
		crate::error::JsonLdError { code: $code, description: Some($desc.into()), cause: None }
	};
	($code:expr, , $cause:expr) => {
		crate::error::JsonLdError { code: $code, description: None, cause: Some(Box::new($cause)) }
	};
	($code:expr, $desc:expr, $cause:expr) => {
		crate::error::JsonLdError { code: $code, description: Some($desc.into()), cause: Some(Box::new($cause)) }
	};
}

macro_rules! expand_iri {
	($active_context:expr,$value:expr) => {
		expand_iri!($active_context, $value, false)
	};
	($active_context:expr,$value:expr,$document_relative:expr) => {
		expand_iri!($active_context, $value, $document_relative, true)
	};
	($active_context:expr,$value:expr,$document_relative:expr,$vocab:expr) => {
		// FIXME: Waiting for Never type to arrive
		expand_iri::<_, fn(&_, &_) -> _, std::future::Pending<_>>(
			crate::expand::IRIExpansionArguments::Normal($active_context), $value, $document_relative, $vocab)
	};
}

macro_rules! process_context_iri {
	($active_context:expr,$value:expr,$local_context:expr,$defined:expr,$options:expr) => {
		process_context_iri!($active_context, $value, false, true, $local_context, $defined, $options)
	};
	($active_context:expr,$value:expr,$document_relative:expr,$vocab:expr,$local_context:expr,$defined:expr,$options:expr) => {
		expand_iri(crate::expand::IRIExpansionArguments::DefineTerms {
			active_context: $active_context,
			local_context: $local_context,
			defined: $defined,
			options: $options
		}, $value, $document_relative, $vocab)
	};
}