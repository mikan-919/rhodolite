// 構文木のフィールドは module loader とその単体テストが読む。処理系の意味は
// HIR が持つので、AST 側で使われないままの読み取り口が残る
#[allow(dead_code)]
mod ast;
// ambient を単相化で消す計画。Wasm 生成の入力になる内部表現。インタプリタ経路は
// 読まないので、生成側だけが触る要素が残る
#[allow(dead_code)]
mod ambient_abi;
mod diag;
#[cfg(test)]
mod differential;
mod eval;
// HIR は宣言 span と所属を語彙として全部持つ。診断と Wasm 下ろしが読むもの、
// そして `dump` のように下ろしのテストだけが使うものがあるので、いまの3つの
// 利用者(検査・要求解析・評価)が触らない要素も残す
#[allow(dead_code)]
mod hir;
mod lex;
mod module;
mod ownership;
mod parse;
mod render;
mod requirement;
mod typecheck;
mod wasm;
mod wasm_abi;
// ambient record の物理表現は段階的に wasm emitter へ接続する。土台の local
// allocator は先に単体テストで固定するので、接続前の unused 警告を抑える。
#[allow(dead_code)]
mod wasm_ambient;
mod wasm_data;
mod wasm_layout;
mod wasm_runtime;
mod wasm_wire;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// CLI が受ける形。位置引数だけの従来形は、そのままインタプリタ実行
/// (design.md 決定10)
enum Command {
    Interpret {
        path: String,
    },
    Build {
        entry: PathBuf,
        output: Option<PathBuf>,
    },
}

const USAGE: &str = "使い方:\n  \
                     rhodolite [<entry.rd>]\n  \
                     rhodolite build <entry.rd> --target wasm [-o <output.wasm>]";

/// 引数を1本の形へ畳む。`build` を名乗ったときだけ選択肢を読む
fn parse_args(args: Vec<String>) -> Result<Command, String> {
    if args.first().map(String::as_str) != Some("build") {
        return match args.len() {
            0 => Ok(Command::Interpret {
                path: "examples/canonical.rd".to_string(),
            }),
            1 => Ok(Command::Interpret {
                path: args.into_iter().next().unwrap(),
            }),
            _ => Err(format!("引数が多すぎます\n{USAGE}")),
        };
    }

    let mut rest = args.into_iter().skip(1);
    let Some(entry) = rest.next() else {
        return Err(format!("`build` にはエントリーファイルが要ります\n{USAGE}"));
    };
    let mut target = None;
    let mut output: Option<PathBuf> = None;
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--target" => {
                let Some(value) = rest.next() else {
                    return Err(format!("`--target` に値がありません\n{USAGE}"));
                };
                if target.is_some() {
                    return Err(format!("`--target` が二度指定されています\n{USAGE}"));
                }
                if value != "wasm" {
                    return Err(format!("知らないターゲット `{value}` です\n{USAGE}"));
                }
                target = Some(value);
            }
            "-o" => {
                let Some(value) = rest.next() else {
                    return Err(format!("`-o` に値がありません\n{USAGE}"));
                };
                if output.is_some() {
                    return Err(format!("`-o` が二度指定されています\n{USAGE}"));
                }
                output = Some(PathBuf::from(value));
            }
            other => return Err(format!("知らない引数 `{other}` です\n{USAGE}")),
        }
    }
    if target.is_none() {
        return Err(format!("`build` には `--target wasm` が要ります\n{USAGE}"));
    }
    Ok(Command::Build {
        entry: PathBuf::from(entry),
        output,
    })
}

/// Wasm ビルド。
///
/// 読み込み・型検査・要求検査・計画・対応検査・生成・検証を全部通してから、
/// 最後に一度だけ出力へ触る。途中で失敗したら既にある成果物は変えない
fn build(entry: &Path, output: Option<&Path>) -> ExitCode {
    let loaded = match module::load(entry) {
        Ok(loaded) => loaded,
        Err(failure) => {
            render::report(&failure.diagnostics, &failure.sources);
            return ExitCode::FAILURE;
        }
    };
    let sources = loaded.sources;

    let lowered = match typecheck::check_and_lower(&loaded.program) {
        Ok(lowered) => lowered,
        Err(errors) => {
            render::report(&errors, &sources);
            return ExitCode::FAILURE;
        }
    };
    let checked = match ownership::check(lowered) {
        Ok(checked) => checked,
        Err(errors) => {
            render::report(&errors, &sources);
            return ExitCode::FAILURE;
        }
    };

    let Some(entry_callable) = checked.hir.free_callable(&loaded.entry) else {
        eprintln!("エントリー `{}` がありません", loaded.entry);
        return ExitCode::FAILURE;
    };

    // 予約名は `pub use` の綴りで決まるので、原因の位置もその宣言に置く
    let reserved: Vec<diag::Diag> = loaded
        .public_exports
        .iter()
        .filter(|export| export.name == wasm::ENTRY_EXPORT)
        .map(|export| {
            diag::Diag::at(
                export.span,
                format!("`{}` は ABI v0 の予約名です", wasm::ENTRY_EXPORT),
            )
            .label("この公開名は使えません")
            .help("`pub use` の別名を変えてください")
        })
        .collect();
    if !reserved.is_empty() {
        render::report(&reserved, &sources);
        return ExitCode::FAILURE;
    }

    // 公開名からホスト呼び出し可能な関数だけを取る。struct や enum を明示選択
    // していても、それは言語側の公開名前空間に留まる
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

    // 生産の根は `main` と公開関数。test の要求はここでは見ない
    let analysis = requirement::analyze(&checked);
    let mut roots = vec![loaded.entry.clone()];
    roots.extend(
        exports
            .iter()
            .map(|(_, id)| checked.hir.callables[*id].name.clone()),
    );
    let errors = analysis.errors_for_roots(&roots);
    if !errors.is_empty() {
        render::report(&errors, &sources);
        return ExitCode::FAILURE;
    }

    let production =
        match ambient_abi::plan_production(&checked, &analysis, entry_callable, &exports) {
            Ok(production) => production,
            Err(error) => {
                eprintln!("{}", error.show(&checked.hir));
                return ExitCode::FAILURE;
            }
        };

    let unsupported = wasm::check_support(&checked, &production.plan);
    if !unsupported.is_empty() {
        render::report(&unsupported, &sources);
        return ExitCode::FAILURE;
    }

    let bytes = match wasm::emit(&checked, &production) {
        Ok(bytes) => bytes,
        Err(errors) => {
            render::report(&errors, &sources);
            return ExitCode::FAILURE;
        }
    };
    if let Err(message) = wasm::validate(&bytes) {
        eprintln!("生成した Wasm が検証を通りませんでした: {message}");
        return ExitCode::FAILURE;
    }

    let destination = match output {
        Some(path) => path.to_path_buf(),
        // 依存の隣でもエントリーの親でもなく、起動したディレクトリを基準にする
        None => {
            let Some(stem) = entry.file_stem().and_then(|s| s.to_str()) else {
                eprintln!("エントリーファイル名を UTF-8 として読めません");
                return ExitCode::FAILURE;
            };
            PathBuf::from("target/wasm").join(format!("{stem}.wasm"))
        }
    };
    match publish(&destination, &bytes) {
        Ok(()) => {
            println!("{}", destination.display());
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{} を書けませんでした: {error}", destination.display());
            ExitCode::FAILURE
        }
    }
}

/// 同じディレクトリの一時ファイルへ書いてから名前を付け替える。
/// 途中で失敗しても、既にある成果物が半端な中身に置き換わらない
fn publish(destination: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = destination.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }
    let temporary = destination.with_extension(format!("wasm.tmp{}", std::process::id()));
    std::fs::write(&temporary, bytes)?;
    std::fs::rename(&temporary, destination)
}

fn main() -> ExitCode {
    let path = match parse_args(std::env::args().skip(1).collect()) {
        Ok(Command::Interpret { path }) => path,
        Ok(Command::Build { entry, output }) => return build(&entry, output.as_deref()),
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::FAILURE;
        }
    };

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
    let lowered = match typecheck::check_and_lower(&program) {
        Ok(lowered) => lowered,
        Err(errors) => {
            eprintln!();
            render::report(&errors, &sources);
            return ExitCode::FAILURE;
        }
    };
    let checked = match ownership::check(lowered) {
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
fn run(
    checked: &ownership::CheckedProgram,
    entry: &str,
    sources: &[module::SourceFile],
) -> ExitCode {
    let interp = eval::Interp::new_checked(checked);

    if checked.hir.tests.is_empty() {
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
    for (id, declared) in checked.hir.tests.iter() {
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

    let total = checked.hir.tests.len();
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
