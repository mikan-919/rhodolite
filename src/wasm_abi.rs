//! Rhodolite Wasm ABI v0(rhodolite-wasm-abi spec)。
//!
//! ホストと Core Wasm の境界の取り決めだけを持つ。生成した実装関数は非公開の
//! ままで、公開されるのは予約名のエントリと、`pub use` で明示選択された
//! 公開名のラッパだけ。
//!
//! 失敗は trap で伝える。状態コードもエラーバッファも署名に足さないので、
//! 成功した呼び出しの結果は Rhodolite の戻り値そのものになる。

use crate::ambient_abi::{InstanceId, ProductionPlan};
use crate::diag::Diag;
use crate::hir;
use crate::wasm::{ABI_VERSION, ENTRY_EXPORT, Scalar, Signature, scalar_of};

/// 公開面の全体。ラッパの生成と埋め込みメタデータはここだけを読む
#[derive(Debug)]
pub struct Signatures {
    entry: (Signature, InstanceId),
    /// 公開名の昇順。別名が同じ実装を指しても名前ごとに1つ並ぶ
    exports: Vec<(Signature, InstanceId)>,
}

impl Signatures {
    /// 生成するラッパを、エントリ・公開名昇順の並びで返す
    pub fn wrappers(&self) -> impl Iterator<Item = (&Signature, InstanceId)> {
        std::iter::once(&self.entry)
            .chain(&self.exports)
            .map(|(signature, instance)| (signature, *instance))
    }

    /// 埋め込む `rhodolite.abi` の中身。
    ///
    /// 圧縮形の正準 JSON。BOM も余白も末尾改行も付けない。鍵の並びは
    /// `version` / `entry` / `exports`、関数は `name` / `params` / `result`
    pub fn metadata(&self) -> String {
        let mut out = format!("{{\"version\":{ABI_VERSION},\"entry\":");
        push_function(&mut out, &self.entry.0);
        out.push_str(",\"exports\":[");
        for (index, (signature, _)) in self.exports.iter().enumerate() {
            if index > 0 {
                out.push(',');
            }
            push_function(&mut out, signature);
        }
        out.push_str("]}");
        out
    }
}

fn push_function(out: &mut String, signature: &Signature) {
    out.push_str("{\"name\":");
    push_string(out, &signature.name);
    out.push_str(",\"params\":[");
    for (index, param) in signature.params.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        push_string(out, param.spelling());
    }
    out.push_str("],\"result\":");
    push_string(out, signature.result.spelling());
    out.push('}');
}

/// JSON の文字列。公開名は識別子なので実際には何も置き換わらないが、
/// 境界で出す以上は綴りに依存しない形にしておく
fn push_string(out: &mut String, value: &str) {
    out.push('"');
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

/// エントリと公開関数の署名を ABI v0 の範囲で確かめる。
///
/// ここで弾くのは**公開面だけ**。同じ型を使う非公開の宣言は、到達しない限り
/// 生産ビルドを止めない
pub fn signatures(
    program: &hir::Program,
    production: &ProductionPlan,
) -> Result<Signatures, Vec<Diag>> {
    let mut diagnostics = Vec::new();
    let plan = &production.plan;

    let entry_callable = callable_of(plan, production.entry);
    let entry = &program.callables[entry_callable];
    if !entry.params.is_empty() {
        diagnostics.push(
            Diag::at(entry.span, "`main` は引数を取れません")
                .label("ここが ABI v0 の入口")
                .help(format!("`{ENTRY_EXPORT}` は引数なしで呼ばれます")),
        );
    }
    let entry_result = match scalar_of(&entry.ret) {
        Some(scalar) => scalar,
        None => {
            diagnostics.push(rejected_result(program, entry.span, &entry.ret));
            Scalar::Unit
        }
    };

    let mut exports = Vec::new();
    for (name, instance) in &production.exports {
        if name == ENTRY_EXPORT {
            diagnostics.push(
                Diag::at(
                    program.callables[callable_of(plan, *instance)].span,
                    format!("`{ENTRY_EXPORT}` は ABI v0 の予約名です"),
                )
                .label("この公開名は使えません")
                .help("`pub use` の別名を変えてください"),
            );
            continue;
        }

        let callable_id = callable_of(plan, *instance);
        let callable = &program.callables[callable_id];
        let mut params = Vec::new();
        for local in &callable.params {
            let decl = callable.body.local(*local);
            // `unit` の引数を消すとソースとホストで引数の個数がずれる
            match decl.ty.as_ref().and_then(scalar_of) {
                Some(Scalar::Bool) => params.push(Scalar::Bool),
                Some(Scalar::Int) => params.push(Scalar::Int),
                _ => {
                    diagnostics.push(
                        Diag::at(
                            decl.span,
                            format!("公開関数 `{name}` の引数を ABI v0 では表せません"),
                        )
                        .label("ここが公開面の引数")
                        .help("公開できる引数は `bool` と `int` だけです"),
                    );
                    params.push(Scalar::Int);
                }
            }
        }
        let result = match scalar_of(&callable.ret) {
            Some(scalar) => scalar,
            None => {
                diagnostics.push(rejected_result(program, callable.span, &callable.ret));
                Scalar::Unit
            }
        };
        exports.push((
            Signature {
                name: name.clone(),
                params,
                result,
            },
            *instance,
        ));
    }

    if !diagnostics.is_empty() {
        return Err(diagnostics);
    }
    // 公開面の並びは公開名の昇順。宣言順にも走査順にも依らせない
    exports.sort_by(|(left, _), (right, _)| left.name.cmp(&right.name));
    Ok(Signatures {
        entry: (
            Signature {
                name: ENTRY_EXPORT.to_string(),
                params: Vec::new(),
                result: entry_result,
            },
            production.entry,
        ),
        exports,
    })
}

fn rejected_result(program: &hir::Program, span: crate::lex::Span, ty: &hir::Type) -> Diag {
    Diag::at(
        span,
        format!(
            "戻り値の型 `{}` を ABI v0 では表せません",
            program.show_type(ty)
        ),
    )
    .label("ここが公開面の戻り値")
    .help("公開できる戻り値は `unit` / `bool` / `int` だけです")
}

fn callable_of(plan: &crate::ambient_abi::Plan, instance: InstanceId) -> hir::CallableId {
    match plan.instance(instance).key.body {
        hir::BodyId::Callable(id) => id,
        hir::BodyId::Test(_) => unreachable!("test は生産の根に入らない"),
    }
}
