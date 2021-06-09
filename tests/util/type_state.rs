pub enum TypeState {
	Initial,
	Manifest,
	TestType(TestType),
	TestClass(TestClass),
	Test(TestType, TestClass)
}

#[derive(Clone, Copy)]
pub enum TestType {
	PositiveEvaluationTest,
	NegativeEvaluationTest
}

#[derive(Clone, Copy)]
pub enum TestClass {
	CompactTest,
	ExpandTest
}

impl TypeState {
	#[allow(unused_must_use)]
	pub fn register(&mut self, ty: &str) {
		let new_self = match self {
			Self::Initial => {
				match ty {
					"http://www.w3.org/2001/sw/DataAccess/tests/test-manifest#Manifest" => Self::Manifest,
					"https://w3c.github.io/json-ld-api/tests/vocab#PositiveEvaluationTest" => Self::TestType(TestType::PositiveEvaluationTest),
					"https://w3c.github.io/json-ld-api/tests/vocab#NegativeEvaluationTest" => Self::TestType(TestType::NegativeEvaluationTest),
					"https://w3c.github.io/json-ld-api/tests/vocab#CompactTest" => Self::TestClass(TestClass::CompactTest),
					_ => return
				}
			},
			Self::TestType(test_type) => {
				match ty {
					"https://w3c.github.io/json-ld-api/tests/vocab#CompactTest" => Self::Test(*test_type, TestClass::CompactTest),
					"https://w3c.github.io/json-ld-api/tests/vocab#ExpandTest" => Self::Test(*test_type, TestClass::ExpandTest),
					_ => return
				}
			},
			Self::TestClass(test_class) => {
				match ty {
					"https://w3c.github.io/json-ld-api/tests/vocab#PositiveEvaluationTest" => Self::Test(TestType::PositiveEvaluationTest, *test_class),
					"https://w3c.github.io/json-ld-api/tests/vocab#NegativeEvaluationTest" => Self::Test(TestType::NegativeEvaluationTest, *test_class),
					_ => return
				}
			},
			_ => panic!("unexpected @type types")
		};
		std::mem::replace(self, new_self);
	}
}