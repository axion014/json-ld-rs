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
					"mf:Manifest" => Self::Manifest,
					"jld:PositiveEvaluationTest" => Self::TestType(TestType::PositiveEvaluationTest),
					"jld:NegativeEvaluationTest" => Self::TestType(TestType::NegativeEvaluationTest),
					"jld:CompactTest" => Self::TestClass(TestClass::CompactTest),
					_ => return
				}
			},
			Self::TestType(test_type) => {
				match ty {
					"jld:CompactTest" => Self::Test(*test_type, TestClass::CompactTest),
					"jld:ExpandTest" => Self::Test(*test_type, TestClass::ExpandTest),
					_ => return
				}
			},
			Self::TestClass(test_class) => {
				match ty {
					"jld:PositiveEvaluationTest" => Self::Test(TestType::PositiveEvaluationTest, *test_class),
					"jld:NegativeEvaluationTest" => Self::Test(TestType::NegativeEvaluationTest, *test_class),
					_ => return
				}
			},
			_ => panic!("unexpected @type types")
		};
		std::mem::replace(self, new_self);
	}
}