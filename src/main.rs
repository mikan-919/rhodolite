// 構文木のフィールドは module loader とその単体テストが読む。処理系の意味は
// HIR が持つので、AST 側で使われないままの読み取り口が残る
#[allow(dead_code)]
mod ast;
mod diag;
mod eval;
// HIR は宣言 span と所属を語彙として全部持つ。診断と次段(C 下ろし)が読むもの、
// そして `dump` のように下ろしのテストだけが使うものがあるので、いまの3つの
// 利用者(検査・要求解析・評価)が触らない要素も残す
#[allow(dead_code)]
mod hir;
mod lex;
mod module;
mod parse;
mod render;
mod requirement;
mod typecheck;

use std::path::Path;
use std::process::ExitCode;

fn main() -> ExitCode {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "examples/canonical.rd".to_string());

    let loaded = match module::load(Path::new(&path)) {
        Ok(program) => program,
        Err(failure) => {
            render::report(&failure.diagnostics, &failure.sources);
            return ExitCode::FAILURE;
        }
    };
    let program = loaded.program;
    let entry = loaded.entry;
    let sources = loaded.sources;

    println!("{path}: {} 個の宣言", program.items.len());
    for item in &program.items {
        println!("  {}", summary(item));
    }

    // 検査と下ろしはひとつ。ここを通れば、後段が受け取るのは型の付いた
    // 参照解決済みの HIR で、名前を引き直す必要がない(src/hir.rs)
    let checked = match typecheck::check_and_lower(&program) {
        Ok(checked) => checked,
        Err(errors) => {
            eprintln!();
            render::report(&errors, &sources);
            return ExitCode::FAILURE;
        }
    };

    let analysis = requirement::analyze(&checked);

    println!("\nスロット:");
    for slot in analysis.slots.names() {
        let trait_name = analysis.slots.trait_of(slot).unwrap_or("?");
        println!("  effect {slot}: {trait_name}");
    }

    // 「書かせない、だが見える」— 誰も書いていない要求を推論して見せる
    println!("\n推論された要求:");
    print!("{}", analysis.render());

    let errors = analysis.errors_for(&entry);
    if !errors.is_empty() {
        eprintln!();
        render::report(&errors, &sources);
        return ExitCode::FAILURE;
    }

    // 検査を通ったので走らせる
    run(&checked, &entry, &sources)
}

/// `test` があれば全部走らせる。無ければ `main` を走らせる。
fn run(checked: &hir::Program, entry: &str, sources: &[module::SourceFile]) -> ExitCode {
    let interp = eval::Interp::new(checked);

    if checked.tests.is_empty() {
        println!("\n実行:");
        return match interp.run(entry) {
            Ok(v) => {
                println!("  main -> {}", interp.show(&v));
                ExitCode::SUCCESS
            }
            Err(e) => {
                println!("  main で失敗:");
                report_runtime(&e, sources);
                ExitCode::FAILURE
            }
        };
    }

    println!("\nテスト:");
    let mut failed = 0;
    for (id, declared) in checked.tests.iter() {
        let name = &declared.name;
        match interp.run_test(id) {
            Ok(_) => println!("  ok   {name}"),
            Err(e) => {
                failed += 1;
                println!("  FAIL {name}");
                report_runtime(&e, sources);
            }
        }
    }

    let total = checked.tests.len();
    println!("\n{total} 件中 {} 件成功", total - failed);
    if failed == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

/// 実行時の失敗を、実行前の診断と同じ描画で出す。
///
/// 見出しは stdout、描画は stderr なので、パイプで別々に溜まると順が入れ替わる。
/// 描く前に stdout を流して見出しを先に出す。
fn report_runtime(flow: &eval::Flow, sources: &[module::SourceFile]) {
    use std::io::Write;
    let _ = std::io::stdout().flush();
    match flow {
        eval::Flow::Error(diagnostic) => render::report(std::slice::from_ref(diagnostic), sources),
        // `return` は関数の境界で受け止まるので、ここへ来るのは評価器の不具合
        other => eprintln!("       {other}"),
    }
}

fn summary(item: &ast::Item) -> String {
    use ast::Item::*;
    match item {
        Trait { name, methods, .. } => {
            format!("trait  {name} ({} メソッド)", methods.len())
        }
        Struct { name, fields, .. } => {
            format!("struct {name} ({} フィールド)", fields.len())
        }
        Enum { name, variants, .. } => {
            format!("enum   {name} ({} variant)", variants.len())
        }
        Impl {
            trait_name,
            type_name,
            methods,
            ..
        } => match trait_name {
            Some(t) => format!("impl   {t} for {type_name} ({} メソッド)", methods.len()),
            None => format!("impl   {type_name} ({} メソッド)", methods.len()),
        },
        Effect {
            slot, trait_name, ..
        } => format!("effect {slot}: {trait_name}"),
        Fn { sig, body, .. } => format!("fn     {} ({} 式)", sig.name, body.len()),
        Test { name, body, .. } => format!("test   {name:?} ({} 式)", body.len()),
    }
}
