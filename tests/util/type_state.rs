pub enum TypeState {
	Initial,
	Manifest,
	Test(Option<TestType>, Option<TestClass>, bool)
}

#[derive(Clone, Copy)]
pub enum TestType {
	PositiveEvaluationTest,
	NegativeEvaluationTest,
	PositiveSyntaxTest,
	NegativeSyntaxTest
}

impl TestType {
	fn from_iri(iri: &str) -> Option<Self> {
		match iri {
			"https://w3c.github.io/json-ld-api/tests/vocab#PositiveEvaluationTest" => Some(TestType::PositiveEvaluationTest),
			"https://w3c.github.io/json-ld-api/tests/vocab#NegativeEvaluationTest" => Some(TestType::NegativeEvaluationTest),
			"https://w3c.github.io/json-ld-api/tests/vocab#PositiveSyntaxTest" => Some(TestType::PositiveSyntaxTest),
			"https://w3c.github.io/json-ld-api/tests/vocab#NegativeSyntaxTest" => Some(TestType::NegativeSyntaxTest),
			_ => None
		}
	}
}

#[derive(Clone, Copy)]
pub enum TestClass {
	CompactTest,
	ExpandTest,
	FlattenTest,
	FrameTest,
	FromRDFTest,
	ToRDFTest,
	HttpTest
}

impl TestClass {
	fn from_iri(iri: &str) -> Option<Self> {
		match iri {
			"https://w3c.github.io/json-ld-api/tests/vocab#CompactTest" => Some(TestClass::CompactTest),
			"https://w3c.github.io/json-ld-api/tests/vocab#ExpandTest" => Some(TestClass::ExpandTest),
			"https://w3c.github.io/json-ld-api/tests/vocab#FlattenTest" => Some(TestClass::FlattenTest),
			"https://w3c.github.io/json-ld-api/tests/vocab#FrameTest" => Some(TestClass::FrameTest),
			"https://w3c.github.io/json-ld-api/tests/vocab#FromRDFTest" => Some(TestClass::FromRDFTest),
			"https://w3c.github.io/json-ld-api/tests/vocab#ToRDFTest" => Some(TestClass::ToRDFTest),
			"https://w3c.github.io/json-ld-api/tests/vocab#HttpTest" => Some(TestClass::HttpTest),
			_ => None
		}
	}
}

fn option_try_xor<T>(a: Option<T>, b: Option<T>) -> Option<T> {
	if let Some(a) = a {
		if b.is_some() {
			panic!("unexpected @type types");
		}
		Some(a)
	} else {
		b
	}
}

impl TypeState {
	#[allow(unused_must_use)]
	pub fn register(&mut self, ty: &str) {
		*self = match self {
			Self::Initial => TestType::from_iri(ty)
				.map(|test_type| Self::Test(Some(test_type), None, false))
				.or_else(|| TestClass::from_iri(ty).map(|test_class| Self::Test(None, Some(test_class), false)))
				.unwrap_or_else(|| match ty {
					"http://www.w3.org/2001/sw/DataAccess/tests/test-manifest#Manifest" => Self::Manifest,
					"https://w3c.github.io/json-ld-api/tests/vocab#HtmlTest" => Self::Test(None, None, true),
					_ => Self::Initial
				}),
			Self::Test(test_type, test_class, false) => Self::Test(
				option_try_xor(*test_type, TestType::from_iri(ty)),
				option_try_xor(*test_class, TestClass::from_iri(ty)),
				ty == "https://w3c.github.io/json-ld-api/tests/vocab#HtmlTest"
			),
			Self::Test(test_type, test_class, true) => {
				if ty == "https://w3c.github.io/json-ld-api/tests/vocab#HtmlTest" {
					panic!("unexpected @type types");
				}
				Self::Test(
					option_try_xor(*test_type, TestType::from_iri(ty)),
					option_try_xor(*test_class, TestClass::from_iri(ty)),
					true
				)
			}
			_ => panic!("unexpected @type types")
		};
	}
}
