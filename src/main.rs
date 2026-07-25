// 構文木のフィールドは次の段(要求推論)で読む。それまでは未使用になる。
#[allow(dead_code)]
mod ast;
mod lex;
mod parse;

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
    ExitCode::SUCCESS
}

fn summary(item: &ast::Item) -> String {
    use ast::Item::*;
    match item {
        Trait { name, methods, .. } => {
            format!("trait  {name} ({} メソッド)", methods.len())
        }
        Effect {
            slot, trait_name, ..
        } => format!("effect {slot}: {trait_name}"),
        Fn { sig, body, .. } => format!("fn     {} ({} 式)", sig.name, body.len()),
        Test { name, body, .. } => format!("test   {name:?} ({} 式)", body.len()),
    }
}
