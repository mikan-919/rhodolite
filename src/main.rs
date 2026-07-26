// 構文木のフィールドは次の段(要求推論)で読む。それまでは未使用になる。
#[allow(dead_code)]
mod ast;
mod eval;
mod lex;
mod parse;
mod requirement;

use std::process::ExitCode;

fn main() -> ExitCode {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "examples/canonical.rd".to_string());

    let src = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{path} を読めません: {e}");
            return ExitCode::FAILURE;
        }
    };

    let tokens = match lex::lex(&src) {
        Ok(t) => lex::join(t),
        Err(e) => {
            eprintln!("{path}: 字句解析エラー: {e}");
            return ExitCode::FAILURE;
        }
    };

    let program = match parse::parse(&tokens) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{path}: {e}");
            return ExitCode::FAILURE;
        }
    };

    println!("{path}: {} 個の宣言", program.items.len());
    for item in &program.items {
        println!("  {}", summary(item));
    }

    let analysis = requirement::analyze(&program);

    println!("\nスロット:");
    for slot in analysis.slots.names() {
        let trait_name = analysis.slots.trait_of(slot).unwrap_or("?");
        println!("  effect {slot}: {trait_name}");
    }

    // 「書かせない、だが見える」— 誰も書いていない要求を推論して見せる
    println!("\n推論された要求:");
    print!("{}", analysis.render());

    let errors = analysis.errors();
    if !errors.is_empty() {
        eprintln!();
        for e in &errors {
            eprintln!("{e}");
        }
        return ExitCode::FAILURE;
    }

    // 検査を通ったので走らせる
    run(&program)
}

/// `test` があれば全部走らせる。無ければ `main` を走らせる。
fn run(program: &ast::Program) -> ExitCode {
    let interp = eval::Interp::new(program);

    let tests: Vec<(&str, &[ast::Expr])> = program
        .items
        .iter()
        .filter_map(|i| match i {
            ast::Item::Test { name, body, .. } => Some((name.as_str(), body.as_slice())),
            _ => None,
        })
        .collect();

    if tests.is_empty() {
        println!("\n実行:");
        return match interp.run("main") {
            Ok(v) => {
                println!("  main -> {}", v.show());
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("  main で失敗: {e}");
                ExitCode::FAILURE
            }
        };
    }

    println!("\nテスト:");
    let mut failed = 0;
    for (name, body) in &tests {
        match interp.run_body(body) {
            Ok(_) => println!("  ok   {name}"),
            Err(e) => {
                failed += 1;
                println!("  FAIL {name}");
                println!("       {e}");
            }
        }
    }

    println!("\n{} 件中 {} 件成功", tests.len(), tests.len() - failed);
    if failed == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
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
