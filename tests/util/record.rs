use std::fmt::*;

pub enum TestRecordKind {
	Group(TestRecords),
	Pass,
	Fail(TestFailure),
	Skip
}

pub struct TestRecord {
	pub name: Option<String>,
	pub content: TestRecordKind
}

impl Display for TestRecord {
	fn fmt(&self, f: &mut Formatter) -> Result {
		self.print(f, "")
	}
}

impl TestRecord {
	fn print(&self, f: &mut Formatter, indent: &str) -> Result {
		match &self.content {
			TestRecordKind::Group(records) => {
				if let Some(name) = &self.name { writeln!(f, "{}\x1b[1m{}\x1b[0m", indent, name)? }
				else { writeln!(f, "{}\x1b[1m(Anonymous test group)\x1b[0m", indent)? }
				records.print(f, indent)
			},
			TestRecordKind::Fail(failure) => {
				write!(f, "{}", indent)?;
				match failure {
					TestFailure::AssertFailure(_, _) => write!(f, "Assert failed")?,
					TestFailure::Error(_) | TestFailure::TestError(_) => write!(f, "Error")?,
					TestFailure::UnknownPanic => write!(f, "A panic occurred")?
				}
				if let Some(name) = &self.name { write!(f, " at test {}", name)? }
				match failure {
					TestFailure::TestError(err) => write!(f, ": {}", err)?,
					TestFailure::AssertFailure(actual, expected) => write!(f, ": expected {}, {}", expected, actual)?,
					TestFailure::Error(err) => write!(f, ": {}", err)?,
					TestFailure::UnknownPanic => ()
				}
				writeln!(f)
			},
			_ => Ok(())
		}
	}
}

pub struct TestRecords {
	records: Vec<TestRecord>,
	pass: usize,
	fail: usize,
	skip: usize
}

impl TestRecords {
	pub fn new() -> Self {
		TestRecords {
			records: Vec::new(),
			pass: 0,
			fail: 0,
			skip: 0
		}
	}

	pub fn add(&mut self, record: TestRecord) {
		match &record.content {
			TestRecordKind::Group(records) => {
				self.pass += records.pass;
				self.fail += records.fail;
				self.skip += records.skip;
			},
			TestRecordKind::Pass => self.pass += 1,
			TestRecordKind::Fail(_) => self.fail += 1,
			TestRecordKind::Skip => self.skip += 1
		}
		self.records.push(record);
	}
}

impl Display for TestRecords {
	fn fmt(&self, f: &mut Formatter) -> Result {
		self.print(f, "")
	}
}

impl TestRecords {
	fn print(&self, f: &mut Formatter, indent: &str) -> Result {
		if !self.records.is_empty() {
			let indent = indent.to_string() + "    ";
			for record in &self.records {
				record.print(f, &indent)?
			}
		}
		writeln!(f, "{}    {} passed; {} failed; {} skipped", indent, self.pass, self.fail, self.skip)
	}
}

pub enum TestFailure {
	TestError(crate::JsonLdTestError),
	AssertFailure(String, String),
	Error(String),
	UnknownPanic
}