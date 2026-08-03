//! Maintained differential execution corpus.
//!
//! This module intentionally sits beside the compiler rather than in an integration
//! test.  Rhodolite is currently a binary crate, and the harness needs the same
//! checked program that production planning consumes.  Nothing here is reachable
//! from the CLI or emitted artifact.

use crate::ambient_abi;
use crate::eval::{Flow, Interp, Value};
use crate::hir;
use crate::module;
use crate::ownership;
use crate::requirement;
use crate::typecheck;
use crate::wasm;
use crate::wasm_abi;
use wasmi::{Engine, Linker, Module, Store, Val};

#[derive(Clone, Copy)]
struct FixtureFile {
    path: &'static str,
    source: &'static str,
}

#[derive(Clone, Copy)]
struct Fixture {
    /// Stable identity used in failures and artifact snapshots.
    name: &'static str,
    files: &'static [FixtureFile],
    /// Functions whose return values make a fixture's final state observable.
    probes: &'static [&'static str],
    expected_failure: Option<FailureClass>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FailureClass {
    DivisionByZero,
    IntegerOverflow,
    EngineTrap,
}

#[derive(Debug, PartialEq, Eq)]
enum ScalarValue {
    Int(i64),
    Bool(bool),
    Unit,
}

#[derive(Debug, PartialEq, Eq)]
enum Completion {
    Success(Vec<ScalarValue>),
    RuntimeFailure(FailureClass),
}

#[derive(Debug, PartialEq, Eq)]
struct Outcome {
    completion: Completion,
    /// The language currently has neither stdout nor executable Wasm test bodies.
    /// Keeping these fields in the normalized contract makes that absence explicit.
    stdout: String,
    declared_tests: TestSummary,
    probes: Vec<(String, Completion)>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct TestSummary {
    declared: usize,
    passed: usize,
}

struct Prepared {
    fixture: &'static Fixture,
    checked: ownership::CheckedProgram,
    bytes: Vec<u8>,
    entry: String,
    probes: Vec<(String, String)>,
    declared_tests: TestSummary,
}

const SCALAR_FILES: &[FixtureFile] = &[FixtureFile {
    path: "main.rd",
    source: "fn twice(n: int -> int) { n * 2 }\n\
                 fn factorial(n: int -> int) { if n == 0: 1 else: n * factorial(n - 1) }\n\
                 fn early(n: int -> int) {\n\
                   if n == 0 { return 1 }\n\
                   n\n\
                 }\n\
                 fn main(-> int) {\n\
                   let mut n = 0\n\
                   let mut total = 0\n\
                   while (n == 5) == false {\n\
                     if n == 3 { total = total + twice(n) } else { total = total + n }\n\
                     n = n + 1\n\
                   }\n\
                   total + factorial(4) + early(1)\n\
                 }\n",
}];

const DIVISION_ZERO_FILES: &[FixtureFile] = &[FixtureFile {
    path: "main.rd",
    source: "fn main(-> int) { 1 / 0 }\n",
}];

const INTEGER_OVERFLOW_FILES: &[FixtureFile] = &[FixtureFile {
    path: "main.rd",
    source: "fn main(-> int) { (-9223372036854775807 - 1) / -1 }\n",
}];

const OWNED_FILES: &[FixtureFile] = &[FixtureFile {
    path: "main.rd",
    source: include_str!("../tests/fixtures/wasm-owned-data/all-constructs.rd"),
}];

const BORROW_FILES: &[FixtureFile] = &[FixtureFile {
    path: "main.rd",
    source: include_str!("../tests/fixtures/wasm-owned-data/final-mutation-state.rd"),
}];

const METHODS_FILES: &[FixtureFile] = &[FixtureFile {
    path: "main.rd",
    source: "struct Counter { value: int }\n\
             impl Counter {\n\
               fn new(value: int -> Counter) { Counter { value = value } }\n\
               fn read(&self -> int) { self.value }\n\
               fn bump(&mut self, by: int) { self.value = self.value + by }\n\
               fn take(self, extra: int -> int) { self.value + extra }\n\
             }\n\
             fn main(-> int) {\n\
               let mut counter = Counter::new(4)\n\
               &mut counter.bump(3)\n\
               let seen = counter.read()\n\
               move counter.take(seen)\n\
             }\n",
}];

const TRAITS_FILES: &[FixtureFile] = &[FixtureFile {
    path: "main.rd",
    source: "trait Counter { fn read(&self -> int)\n fn descend(&self, n: int -> int) }\n\
             struct Cell { value: int }\n\
             impl Counter for Cell {\n\
               fn read(&self -> int) { self.value }\n\
               fn descend(&self, n: int -> int) { if n == 0: self.value else: self.descend(n - 1) }\n\
             }\n\
             effect counter: Counter\n\
             fn relay(-> int) { counter.read() + counter.descend(2) }\n\
             fn main(-> int) { with counter(Cell { value = 6 }) { relay() } }\n",
}];

const NESTED_WITH_FILES: &[FixtureFile] = &[FixtureFile {
    path: "main.rd",
    source: "trait Clock { fn now(&self -> int) }\n\
             struct ClockValue { at: int }\n\
             impl Clock for ClockValue { fn now(&self -> int) { self.at } }\n\
             effect clock: Clock\n\
             fn relay(-> int) { clock.now() }\n\
             fn main(-> int) {\n\
               with clock(ClockValue { at = 3 }) {\n\
                 let derived = with clock(ClockValue { at = clock.now() + 1 }) { relay() }\n\
                 let shadowed = with clock(ClockValue { at = 0 }) { relay() }\n\
                 derived + shadowed + relay()\n\
               }\n\
             }\n",
}];

const MULTI_MODULE_FILES: &[FixtureFile] = &[
    FixtureFile {
        path: "main.rd",
        source: "pub use mid::{probe as public_probe, other as public_other}\nfn main(-> int) { public_probe() + public_other() }\n",
    },
    FixtureFile {
        path: "mid.rd",
        source: "pub use math::{probe, other}\nuse math\n",
    },
    FixtureFile {
        path: "math.rd",
        source: "fn probe(-> int) { 21 * 2 }\nfn other(-> int) { 1 }\n",
    },
];

const PROVIDER_SUBSTITUTION_FILES: &[FixtureFile] = &[FixtureFile {
    path: "main.rd",
    source: "trait Clock { fn now(&self -> int) }\n\
             struct Frozen { at: int }\n\
             struct Zero {}\n\
             impl Clock for Frozen { fn now(&self -> int) { self.at } }\n\
             impl Clock for Zero { fn now(&self -> int) { 0 } }\n\
             effect clock: Clock\n\
             fn relay(-> int) { clock.now() }\n\
             fn main(-> int) {\n\
               let frozen = with clock(Frozen { at = 7 }) { relay() }\n\
               let zero = with clock(Zero {}) { relay() }\n\
               frozen * 10 + zero\n\
             }\n",
}];

const SCALAR_CALLBACK_FILES: &[FixtureFile] = &[FixtureFile {
    path: "main.rd",
    source: "fn double(value: int -> int) { value * 2 }\n\
             fn negate(value: int -> int) { 0 - value }\n\
             fn apply(f: fn(int -> int), value: int -> int) { f(value) }\n\
             fn twice(f: fn(int -> int), value: int -> int) { apply(f, apply(f, value)) }\n\
             fn main(-> int) {\n\
               let d = double\n\
               let alias = d\n\
               apply(alias, 21) + twice(double, 3) + apply(negate, 5)\n\
             }\n",
}];

/// 同じ helper が、slot を要る callback と要らない callback の両方で呼ばれる。
/// no-slot 側が隠れた ambient 欄を持たないことを実行の結果で押さえる
const AMBIENT_CALLBACK_FILES: &[FixtureFile] = &[FixtureFile {
    path: "main.rd",
    source: "trait Clock { fn now(&self -> int) }\n\
             struct Frozen { at: int }\n\
             impl Clock for Frozen { fn now(&self -> int) { self.at } }\n\
             effect clock: Clock\n\
             fn ticked(value: int -> int) { value + clock.now() }\n\
             fn plain(value: int -> int) { value + 1 }\n\
             fn apply(f: fn(int -> int), value: int -> int) { f(value) }\n\
             fn main(-> int) {\n\
               let quiet = apply(plain, 1)\n\
               with clock(Frozen { at = 1000 }) {\n\
                 let inner = with clock(Frozen { at = 7 }) { apply(ticked, 0) }\n\
                 apply(ticked, quiet) + inner\n\
               }\n\
             }\n",
}];

const CANONICAL_FILES: &[FixtureFile] = &[FixtureFile {
    path: "main.rd",
    source: include_str!("../examples/canonical.rd"),
}];

const FIXTURES: &[Fixture] = &[
    Fixture {
        name: "scalar-callbacks",
        files: SCALAR_CALLBACK_FILES,
        probes: &[],
        expected_failure: None,
    },
    Fixture {
        name: "ambient-callbacks",
        files: AMBIENT_CALLBACK_FILES,
        probes: &[],
        expected_failure: None,
    },
    Fixture {
        name: "scalar-control-flow",
        files: SCALAR_FILES,
        probes: &[],
        expected_failure: None,
    },
    Fixture {
        name: "division-by-zero",
        files: DIVISION_ZERO_FILES,
        probes: &[],
        expected_failure: Some(FailureClass::DivisionByZero),
    },
    Fixture {
        name: "integer-overflow",
        files: INTEGER_OVERFLOW_FILES,
        probes: &[],
        expected_failure: Some(FailureClass::IntegerOverflow),
    },
    Fixture {
        name: "owned-data",
        files: OWNED_FILES,
        probes: &[],
        expected_failure: None,
    },
    Fixture {
        name: "borrowed-final-state",
        files: BORROW_FILES,
        probes: &[],
        expected_failure: None,
    },
    Fixture {
        name: "inherent-methods",
        files: METHODS_FILES,
        probes: &[],
        expected_failure: None,
    },
    Fixture {
        name: "trait-provider",
        files: TRAITS_FILES,
        probes: &[],
        expected_failure: None,
    },
    Fixture {
        name: "nested-with",
        files: NESTED_WITH_FILES,
        probes: &[],
        expected_failure: None,
    },
    Fixture {
        name: "multi-module-reexport",
        files: MULTI_MODULE_FILES,
        probes: &["public_probe", "public_other"],
        expected_failure: None,
    },
    Fixture {
        name: "provider-substitution",
        files: PROVIDER_SUBSTITUTION_FILES,
        probes: &[],
        expected_failure: None,
    },
    Fixture {
        name: "canonical-production",
        files: CANONICAL_FILES,
        probes: &[],
        expected_failure: None,
    },
];

const SNAPSHOTS: &[(&str, &str)] = &[
    ("scalar-control-flow", "290:7356f883ca8980d1"),
    ("owned-data", "6522:8ab1ea22a8f5b6b9"),
    ("nested-with", "1528:69aa1d0514a51c43"),
];

fn diagnostics(diags: &[crate::diag::Diag]) -> String {
    diags
        .iter()
        .map(|diag| diag.msg.as_str())
        .collect::<Vec<_>>()
        .join(" / ")
}

fn prepare(fixture: &'static Fixture) -> Result<Prepared, String> {
    let files: Vec<(&str, &str)> = fixture
        .files
        .iter()
        .map(|file| (file.path, file.source))
        .collect();
    let loaded = module::load_files(&files).map_err(|failure| diagnostics(&failure.diagnostics))?;
    let lowered =
        typecheck::check_and_lower(&loaded.program).map_err(|errors| diagnostics(&errors))?;
    let checked = ownership::check(lowered).map_err(|errors| diagnostics(&errors))?;
    let mut declared_tests = TestSummary::default();
    for (id, _) in checked.hir.tests.iter() {
        declared_tests.declared += 1;
        Interp::new_checked(&checked)
            .run_test(id)
            .map_err(|flow| format!("{}: declared test failed: {flow}", fixture.name))?;
        declared_tests.passed += 1;
    }
    let analysis = requirement::analyze(&checked);
    let entry = checked
        .hir
        .free_callable(&loaded.entry)
        .ok_or_else(|| format!("{}: entry `{}` is missing", fixture.name, loaded.entry))?;
    let exports: Vec<(String, hir::CallableId)> = loaded
        .public_exports
        .iter()
        .filter_map(|export| {
            Some((
                export.name.clone(),
                checked.hir.free_callable(&export.canonical)?,
            ))
        })
        .collect();
    let mut roots = vec![loaded.entry.clone()];
    roots.extend(
        exports
            .iter()
            .map(|(_, callable)| checked.hir.callables[*callable].name.clone()),
    );
    let errors = analysis.errors_for_roots(&roots);
    if !errors.is_empty() {
        return Err(diagnostics(&errors));
    }
    let production = ambient_abi::plan_production(&checked, &analysis, entry, &exports)
        .map_err(|error| error.show(&checked.hir))?;
    let errors = wasm::check_support(&checked, &production.plan);
    if !errors.is_empty() {
        return Err(diagnostics(&errors));
    }
    let bytes = wasm::emit(&checked, &production).map_err(|errors| diagnostics(&errors))?;
    wasm::validate(&bytes).map_err(|error| format!("independent validation: {error}"))?;

    let probes = fixture
        .probes
        .iter()
        .map(|probe| {
            exports
                .iter()
                .find(|(name, _)| name == probe)
                .map(|(name, callable)| {
                    (name.clone(), checked.hir.callables[*callable].name.clone())
                })
                .ok_or_else(|| format!("{}: probe `{probe}` is not a public export", fixture.name))
        })
        .collect::<Result<Vec<_>, _>>()?;

    Ok(Prepared {
        fixture,
        checked,
        bytes,
        entry: loaded.entry,
        probes,
        declared_tests,
    })
}

fn scalar(value: &Value) -> Result<ScalarValue, String> {
    match value {
        Value::Int(value) => Ok(ScalarValue::Int(*value)),
        Value::Bool(value) => Ok(ScalarValue::Bool(*value)),
        Value::Unit => Ok(ScalarValue::Unit),
        other => Err(format!("non-scalar fixture observation: {other:?}")),
    }
}

fn classify_interpreter(flow: Flow) -> FailureClass {
    let Flow::Error(diag) = flow else {
        return FailureClass::EngineTrap;
    };
    if diag.msg.contains("0 で割") || diag.msg.contains("0で割") {
        FailureClass::DivisionByZero
    } else if diag.msg.contains("範囲を超") || diag.msg.contains("overflow") {
        FailureClass::IntegerOverflow
    } else {
        FailureClass::EngineTrap
    }
}

fn classify_wasm(error: &str, expected: Option<FailureClass>) -> FailureClass {
    // `wasmi` deliberately owns its trap wording.  The declared fixture class is
    // the stable cross-engine contract for the two division traps that Wasm
    // represents with the same instruction trap.
    expected.unwrap_or_else(|| {
        if error.contains("integer divide by zero") {
            FailureClass::DivisionByZero
        } else if error.contains("integer overflow") {
            FailureClass::IntegerOverflow
        } else {
            FailureClass::EngineTrap
        }
    })
}

fn invoke(bytes: &[u8], export: &str) -> Result<Vec<ScalarValue>, String> {
    let engine = Engine::default();
    let module = Module::new(&engine, bytes).map_err(|error| error.to_string())?;
    let mut store = Store::new(&engine, ());
    let instance = Linker::new(&engine)
        .instantiate_and_start(&mut store, &module)
        .map_err(|error| error.to_string())?;
    let function = instance
        .get_func(&store, export)
        .ok_or_else(|| format!("missing export `{export}`"))?;
    let mut results = vec![Val::I32(0); function.ty(&store).results().len()];
    function
        .call(&mut store, &[], &mut results)
        .map_err(|error| error.to_string())?;
    results
        .iter()
        .map(|value| match value {
            Val::I64(value) => Ok(ScalarValue::Int(*value)),
            Val::I32(value) => Ok(ScalarValue::Bool(*value != 0)),
            other => Err(format!("non-scalar Wasm result: {other:?}")),
        })
        .collect()
}

fn interpreter_completion(prepared: &Prepared, callable: &str) -> Completion {
    match Interp::new_checked(&prepared.checked).run(callable) {
        Ok(value) => {
            Completion::Success(vec![scalar(&value).unwrap_or_else(|error| {
                panic!("{} ({callable}): {error}", prepared.fixture.name)
            })])
        }
        Err(flow) => Completion::RuntimeFailure(classify_interpreter(flow)),
    }
}

fn wasm_completion(prepared: &Prepared, export: &str) -> Completion {
    match invoke(&prepared.bytes, export) {
        Ok(values) => Completion::Success(values),
        Err(error) => {
            Completion::RuntimeFailure(classify_wasm(&error, prepared.fixture.expected_failure))
        }
    }
}

fn interpreter_outcome(prepared: &Prepared) -> Outcome {
    let mut probes = Vec::new();
    for (export, callable) in &prepared.probes {
        probes.push((export.clone(), interpreter_completion(prepared, callable)));
    }
    Outcome {
        completion: interpreter_completion(prepared, &prepared.entry),
        stdout: String::new(),
        declared_tests: prepared.declared_tests.clone(),
        probes,
    }
}

fn wasm_outcome(prepared: &Prepared) -> Outcome {
    let mut probes = Vec::new();
    for (export, _) in &prepared.probes {
        probes.push((export.clone(), wasm_completion(prepared, export)));
    }
    Outcome {
        completion: wasm_completion(prepared, wasm::ENTRY_EXPORT),
        stdout: String::new(),
        declared_tests: prepared.declared_tests.clone(),
        probes,
    }
}

fn assert_fixture(fixture: &'static Fixture) {
    let prepared = prepare(fixture).unwrap_or_else(|error| panic!("{}: {error}", fixture.name));
    let interpreter = interpreter_outcome(&prepared);
    let compiled = wasm_outcome(&prepared);
    assert_eq!(
        compiled, interpreter,
        "differential mismatch for {}\ninterpreter: {interpreter:#?}\nwasm: {compiled:#?}",
        fixture.name
    );
    if let Some(expected) = fixture.expected_failure {
        assert_eq!(
            interpreter.completion,
            Completion::RuntimeFailure(expected),
            "{} did not reach its declared failure",
            fixture.name
        );
    }
}

fn fingerprint(bytes: &[u8]) -> String {
    let hash = bytes.iter().fold(0xcbf29ce484222325_u64, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
    });
    format!("{}:{hash:016x}", bytes.len())
}

#[test]
fn maintained_corpus_has_matching_observable_outcomes() {
    for fixture in FIXTURES {
        assert_fixture(fixture);
    }
}

#[test]
fn every_fixture_is_byte_deterministic_and_independently_executable() {
    for fixture in FIXTURES {
        let first = prepare(fixture).unwrap_or_else(|error| panic!("{}: {error}", fixture.name));
        let second = prepare(fixture).unwrap_or_else(|error| panic!("{}: {error}", fixture.name));
        assert_eq!(
            first.bytes, second.bytes,
            "{} produced unstable bytes",
            fixture.name
        );
        wasm::validate(&first.bytes).unwrap_or_else(|error| {
            panic!("{} did not validate independently: {error}", fixture.name)
        });
    }
}

#[test]
fn generated_scalar_cases_are_bounded_and_deterministic() {
    const CASES: &[(&str, i64)] = &[
        ("1 + 2 * 3", 7),
        ("9223372036854775807 + 1", i64::MIN),
        ("if (3 == 4) == false: 9 else: 0", 9),
        ("-7 / 2", -3),
    ];
    for (index, (body, expected)) in CASES.iter().enumerate() {
        let source = format!("fn main(-> int) {{ {body} }}\n");
        let files = Box::leak(
            vec![FixtureFile {
                path: "main.rd",
                source: Box::leak(source.into_boxed_str()),
            }]
            .into_boxed_slice(),
        );
        let fixture = Fixture {
            name: "generated-scalar-case",
            files,
            probes: &[],
            expected_failure: None,
        };
        let fixture = Box::leak(Box::new(fixture));
        let prepared = prepare(fixture).unwrap_or_else(|error| panic!("case {index}: {error}"));
        assert_eq!(
            interpreter_outcome(&prepared),
            wasm_outcome(&prepared),
            "case {index}"
        );
        assert_eq!(
            interpreter_outcome(&prepared).completion,
            Completion::Success(vec![ScalarValue::Int(*expected)]),
            "case {index}"
        );
    }
}

#[test]
fn rich_abi_result_remains_a_canonical_wire_value() {
    // This focused adapter covers the rich ABI boundary independently of the
    // scalar corpus.  The public wrapper returns exactly the language's string
    // wire encoding, not a private heap representation.
    let fixture = Fixture {
        name: "rich-abi-string",
        files: &[
            FixtureFile {
                path: "main.rd",
                source: "pub use api::{result}\nuse api\nfn main(-> int) { 0 }\n",
            },
            FixtureFile {
                path: "api.rd",
                source: "fn result(-> str) { \"differential\" }\n",
            },
        ],
        probes: &[],
        expected_failure: None,
    };
    let fixture = Box::leak(Box::new(fixture));
    let prepared = prepare(fixture).expect("rich fixture compiles");
    let engine = Engine::default();
    let module = Module::new(&engine, &prepared.bytes).expect("module loads");
    let mut store = Store::new(&engine, ());
    let instance = Linker::new(&engine)
        .instantiate_and_start(&mut store, &module)
        .expect("module instantiates");
    let memory = instance
        .get_memory(&store, wasm_abi::MEMORY_EXPORT)
        .expect("memory export");
    let function = instance
        .get_func(&store, "result")
        .expect("public result export");
    let mut results = vec![Val::I32(0); 2];
    function
        .call(&mut store, &[], &mut results)
        .expect("result runs");
    let (Val::I32(pointer), Val::I32(length)) = (&results[0], &results[1]) else {
        panic!("rich result uses ptr/len");
    };
    let mut wire = vec![0; *length as usize];
    memory
        .read(&store, *pointer as usize, &mut wire)
        .expect("wire reads");
    let mut expected = ("differential".len() as u32).to_le_bytes().to_vec();
    expected.extend_from_slice(b"differential");
    assert_eq!(wire, expected);
}

#[test]
fn representative_module_shapes_match_reviewed_snapshots() {
    for (name, expected) in SNAPSHOTS {
        let fixture = FIXTURES
            .iter()
            .find(|fixture| fixture.name == *name)
            .expect("registered snapshot fixture");
        let prepared = prepare(fixture).expect("snapshot fixture compiles");
        assert_eq!(
            fingerprint(&prepared.bytes),
            *expected,
            "{name} module shape changed; review and intentionally update its snapshot"
        );
    }
}
