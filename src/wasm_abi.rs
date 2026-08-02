//! Rhodolite Wasm ABI(rhodolite-wasm-abi spec)。
//!
//! ホストと Core Wasm の境界の取り決めだけを持つ。生成した実装関数は非公開の
//! ままで、公開されるのは予約名のエントリと、`pub use` で明示選択された
//! 公開名のラッパだけ。
//!
//! 公開面が scalar だけなら ABI v0 のまま、所有型が1つでも出たらモジュール
//! 全体が v1 になる(design.md 決定8)。v1 は豊かな値を直列化した bytes として
//! 運ぶので、内部の記憶の並びはホストの契約に入らない。
//!
//! 失敗は trap で伝える。状態コードもエラーバッファも署名に足さないので、
//! 成功した呼び出しの結果は Rhodolite の戻り値そのものになる。

use crate::ambient_abi::{InstanceId, ProductionPlan};
use crate::diag::Diag;
use crate::hir;
use crate::ownership::CheckedProgram;
use crate::wasm::{ENTRY_EXPORT, Port, Scalar, Signature, scalar_of, supported};

/// ホストが受け渡し領域を予約する関数の名前。公開名には使えない
pub const RESERVE_EXPORT: &str = "__rhodolite_abi_reserve";

/// 線形メモリの export 名。公開名には使えない
pub const MEMORY_EXPORT: &str = "memory";

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

    /// この公開面が要る ABI の版。所有型が1つでも出たら v1
    pub fn version(&self) -> u32 {
        let rich = self
            .wrappers()
            .any(|(signature, _)| signature.ports().any(|port| port.rich().is_some()));
        u32::from(rich)
    }

    /// 公開署名に出た所有型を、entry・公開名昇順・署名内の順で辿る。
    ///
    /// メタデータの型 ID も直列化関数の番号もこの順に振るので、走査の仕方が
    /// 変わっても公開名と公開型が同じなら同じ並びになる(design.md 決定9)
    pub fn rich_types(&self) -> Vec<hir::Type> {
        let mut found: Vec<hir::Type> = Vec::new();
        for (signature, _) in self.wrappers() {
            for ty in signature.ports().filter_map(Port::rich) {
                if !found.contains(ty) {
                    found.push(ty.clone());
                }
            }
        }
        found
    }
}

impl Signature {
    /// 署名が運ぶ値を、引数・戻り値の順に
    pub fn ports(&self) -> impl Iterator<Item = &Port> {
        self.params.iter().chain(std::iter::once(&self.result))
    }
}

/// エントリと公開関数の署名を確かめる。
///
/// ここで弾くのは**公開面だけ**。同じ型を使う非公開の宣言は、到達しない限り
/// 生産ビルドを止めない
pub fn signatures(
    checked: &CheckedProgram,
    production: &ProductionPlan,
) -> Result<Signatures, Vec<Diag>> {
    signatures_impl(&checked.hir, production)
}

fn signatures_impl(
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
                .label("ここが ABI の入口")
                .help(format!("`{ENTRY_EXPORT}` は引数なしで呼ばれます")),
        );
    }
    let entry_result = result_port(program, entry.span, &entry.ret, &mut diagnostics);

    let mut exports = Vec::new();
    for (name, instance) in &production.exports {
        let callable_id = callable_of(plan, *instance);
        let callable = &program.callables[callable_id];
        if let Some(reason) = reserved(name) {
            diagnostics.push(
                Diag::at(callable.span, format!("`{name}` は {reason}"))
                    .label("この公開名は使えません")
                    .help("`pub use` の別名を変えてください"),
            );
            continue;
        }

        let mut params = Vec::new();
        for local in &callable.params {
            let decl = callable.body.local(*local);
            params.push(param_port(program, name, decl, &mut diagnostics));
        }
        let result = result_port(program, callable.span, &callable.ret, &mut diagnostics);
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

/// 予約している export 名か。理由まで返して診断に載せる
fn reserved(name: &str) -> Option<&'static str> {
    match name {
        ENTRY_EXPORT => Some("入口の予約名です"),
        RESERVE_EXPORT => Some("受け渡し領域の予約名です"),
        MEMORY_EXPORT => Some("線形メモリの予約名です"),
        _ => None,
    }
}

/// 引数1つ。`unit` は消すとソースとホストで個数がずれるので拒否する
fn param_port(
    program: &hir::Program,
    name: &str,
    decl: &hir::LocalDecl,
    diagnostics: &mut Vec<Diag>,
) -> Port {
    let Some(ty) = &decl.ty else {
        return Port::Scalar(Scalar::Int);
    };
    if ty.reference.is_some() {
        diagnostics.push(borrowed(program, decl.span, ty));
        return Port::Scalar(Scalar::Int);
    }
    match scalar_of(ty) {
        Some(Scalar::Bool) => Port::Scalar(Scalar::Bool),
        Some(Scalar::Int) => Port::Scalar(Scalar::Int),
        Some(Scalar::Unit) => {
            diagnostics.push(
                Diag::at(
                    decl.span,
                    format!("公開関数 `{name}` の `unit` 引数は境界に出せません"),
                )
                .label("ここが公開面の引数")
                .help("引数から外すか、値を持つ型にしてください"),
            );
            Port::Scalar(Scalar::Int)
        }
        None if supported(ty) => Port::Rich(ty.clone()),
        None => {
            diagnostics.push(
                Diag::at(
                    decl.span,
                    format!(
                        "公開関数 `{name}` の引数の型 `{}` は境界に出せません",
                        program.show_type(ty)
                    ),
                )
                .label("ここが公開面の引数"),
            );
            Port::Scalar(Scalar::Int)
        }
    }
}

fn result_port(
    program: &hir::Program,
    span: crate::lex::Span,
    ty: &hir::Type,
    diagnostics: &mut Vec<Diag>,
) -> Port {
    if ty.reference.is_some() {
        diagnostics.push(borrowed(program, span, ty));
        return Port::Scalar(Scalar::Unit);
    }
    match scalar_of(ty) {
        Some(scalar) => Port::Scalar(scalar),
        None if supported(ty) => Port::Rich(ty.clone()),
        None => {
            diagnostics.push(
                Diag::at(
                    span,
                    format!("戻り値の型 `{}` は境界に出せません", program.show_type(ty)),
                )
                .label("ここが公開面の戻り値"),
            );
            Port::Scalar(Scalar::Unit)
        }
    }
}

/// 借用は呼び出し側の記憶を指す。境界を越えた先にその所有者は居ない
fn borrowed(program: &hir::Program, span: crate::lex::Span, ty: &hir::Type) -> Diag {
    Diag::at(
        span,
        format!(
            "公開面の借用 `{}` は Wasm の署名には出せません",
            program.show_type(ty)
        ),
    )
    .label("署名に出た借用")
    .help("所有する値をやり取りしてください")
}

fn callable_of(plan: &crate::ambient_abi::Plan, instance: InstanceId) -> hir::CallableId {
    match plan.instance(instance).key.body {
        hir::BodyId::Callable(id) => id,
        hir::BodyId::Test(_) => unreachable!("test は生産の根に入らない"),
    }
}

// ---------------------------------------------------------------------------
// 埋め込みメタデータ
// ---------------------------------------------------------------------------

/// 公開署名から辿れる型のグラフ(design.md 決定9)。
///
/// **宣言された型**を辿る。内部の並びが `indirect` をどう畳んでいるかは
/// ホストの契約ではないので、offset も容量も allocator のヘッダも出さない。
/// 子を訪ねる前に ID を予約するので、再帰する型も閉じる
#[derive(Default)]
struct Schema {
    order: Vec<hir::Type>,
}

impl Schema {
    /// scalar は綴りそのまま、それ以外は `t0`, `t1`, ... の参照
    fn reference(&mut self, program: &hir::Program, ty: &hir::Type) -> String {
        if let Some(scalar) = scalar_of(ty) {
            return scalar.spelling().to_string();
        }
        format!("t{}", self.intern(program, ty))
    }

    fn intern(&mut self, program: &hir::Program, ty: &hir::Type) -> usize {
        if let Some(found) = self.order.iter().position(|seen| seen == ty) {
            return found;
        }
        let id = self.order.len();
        self.order.push(ty.clone());
        // 子は ID を押さえた後で辿る。再帰しても同じ ID へ戻るだけ
        self.children(program, ty);
        id
    }

    fn children(&mut self, program: &hir::Program, ty: &hir::Type) {
        if ty.optional {
            let mut inner = ty.clone();
            inner.optional = false;
            self.reference(program, &inner);
            return;
        }
        match &ty.kind {
            hir::TypeKind::Struct(id) => {
                for field in program.structs[*id].fields.clone() {
                    let field_ty = program.fields[field].ty.clone();
                    self.reference(program, &field_ty);
                }
            }
            hir::TypeKind::Enum(id) => {
                for variant in program.enums[*id].variants.clone() {
                    for position in 0..program.variants[variant].payload.len() {
                        let payload = program.variants[variant].payload[position].ty.clone();
                        self.reference(program, &payload);
                    }
                }
            }
            hir::TypeKind::Array(element) => {
                let element = (**element).clone();
                self.reference(program, &element);
            }
            hir::TypeKind::Builtin(_) | hir::TypeKind::Poison => {}
        }
    }

    /// 型1つ分の記述。wire 形式を読み書きするのに要るものだけを出す
    fn describe(&mut self, program: &hir::Program, ty: &hir::Type, out: &mut String) {
        if ty.optional {
            let mut inner = ty.clone();
            inner.optional = false;
            let payload = self.reference(program, &inner);
            out.push_str("\"kind\":\"optional\",\"payload\":");
            push_string(out, &payload);
            return;
        }
        match &ty.kind {
            hir::TypeKind::Builtin(hir::Builtin::Str) => out.push_str("\"kind\":\"str\""),
            hir::TypeKind::Array(element) => {
                let element = (**element).clone();
                let reference = self.reference(program, &element);
                out.push_str("\"kind\":\"array\",\"element\":");
                push_string(out, &reference);
            }
            hir::TypeKind::Struct(id) => {
                out.push_str("\"kind\":\"struct\",\"name\":");
                push_string(out, &program.structs[*id].name);
                out.push_str(",\"fields\":[");
                for (index, field) in program.structs[*id].fields.clone().iter().enumerate() {
                    if index > 0 {
                        out.push(',');
                    }
                    let declared = program.fields[*field].ty.clone();
                    let reference = self.reference(program, &declared);
                    out.push_str("{\"name\":");
                    push_string(out, &program.fields[*field].name);
                    out.push_str(",\"type\":");
                    push_string(out, &reference);
                    out.push('}');
                }
                out.push(']');
            }
            hir::TypeKind::Enum(id) => {
                out.push_str("\"kind\":\"enum\",\"name\":");
                push_string(out, &program.enums[*id].name);
                out.push_str(",\"variants\":[");
                for (index, variant) in program.enums[*id].variants.clone().iter().enumerate() {
                    if index > 0 {
                        out.push(',');
                    }
                    out.push_str("{\"name\":");
                    push_string(out, &program.variants[*variant].name);
                    out.push_str(",\"payload\":[");
                    for position in 0..program.variants[*variant].payload.len() {
                        if position > 0 {
                            out.push(',');
                        }
                        let payload = program.variants[*variant].payload[position].ty.clone();
                        let reference = self.reference(program, &payload);
                        push_string(out, &reference);
                    }
                    out.push_str("]}");
                }
                out.push(']');
            }
            // scalar は綴りで書くので表に載らない
            other => unreachable!("表に載らない型です: {other:?}"),
        }
    }
}

impl Signatures {
    /// 埋め込む `rhodolite.abi` の中身。
    ///
    /// 圧縮形の正準 JSON。BOM も余白も末尾改行も付けない。鍵の並びは
    /// `version` / `entry` / `exports`、v1 ではその後ろに `types`。
    /// 関数は `name` / `params` / `result`
    pub fn metadata(&self, program: &hir::Program) -> String {
        let version = self.version();
        let mut schema = Schema::default();
        // 型 ID は entry・公開名昇順・署名内の順に予約する
        let mut entry = String::new();
        push_function(&mut schema, program, &mut entry, &self.entry.0);
        let mut exports = String::new();
        for (index, (signature, _)) in self.exports.iter().enumerate() {
            if index > 0 {
                exports.push(',');
            }
            push_function(&mut schema, program, &mut exports, signature);
        }

        let mut out =
            format!("{{\"version\":{version},\"entry\":{entry},\"exports\":[{exports}]}}");
        if version == 0 {
            return out;
        }
        out.pop();
        out.push_str(",\"types\":[");
        // `describe` が子を辿って表を伸ばすので、添字で歩く
        let mut index = 0;
        while index < schema.order.len() {
            if index > 0 {
                out.push(',');
            }
            let ty = schema.order[index].clone();
            out.push_str(&format!("{{\"id\":\"t{index}\","));
            schema.describe(program, &ty, &mut out);
            out.push('}');
            index += 1;
        }
        out.push_str("]}");
        out
    }
}

fn push_function(
    schema: &mut Schema,
    program: &hir::Program,
    out: &mut String,
    signature: &Signature,
) {
    out.push_str("{\"name\":");
    push_string(out, &signature.name);
    out.push_str(",\"params\":[");
    for (index, param) in signature.params.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        let reference = port_reference(schema, program, param);
        push_string(out, &reference);
    }
    out.push_str("],\"result\":");
    let reference = port_reference(schema, program, &signature.result);
    push_string(out, &reference);
    out.push('}');
}

fn port_reference(schema: &mut Schema, program: &hir::Program, port: &Port) -> String {
    match port {
        Port::Scalar(scalar) => scalar.spelling().to_string(),
        Port::Rich(ty) => schema.reference(program, ty),
    }
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
