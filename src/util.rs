use url::{Url, ParseError};

pub fn is_jsonld_keyword(value: &str) -> bool {
	todo!()
}

pub fn looks_like_a_jsonld_keyword(value: &str) -> bool {
	todo!()
}

pub fn resolve(r: &str, base: Option<&Url>) -> Result<Url, ParseError> {
	Url::options().base_url(base).parse(r)
}

pub fn resolve_with_str<T: std::borrow::Borrow<str>>(r: &str, base: Option<T>) -> Result<Url, ParseError> {
	resolve(r, base.map(|base| Url::parse(base.borrow())).transpose()?.as_ref())
}

pub fn is_iri(value: &str) -> bool {
	todo!()
}

pub enum OneError<T: Iterator<Item = I>, I, E> {
	Ok(T),
	Err(Option<E>)
}

impl <T: Iterator<Item = I>, I, E> OneError<T, I, E> {
	pub fn new<O>(value: Result<O, E>) -> Self where O: IntoIterator<IntoIter = T> {
		match value {
			Ok(o) => OneError::Ok(o.into_iter()),
			Err(e) => OneError::Err(Some(e))
		}
	}
}

impl <T: Iterator<Item = I>, I, E> Iterator for OneError<T, I, E> {
	type Item = Result<I, E>;

	fn next(&mut self) -> Option<Self::Item> {
		match self {
			OneError::Ok(iter) => Ok(iter.next()).transpose(),
			OneError::Err(e) => e.take().map(|e| Err(e))
		}
	}
}