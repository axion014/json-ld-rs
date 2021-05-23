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
