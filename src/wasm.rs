//! Core WebAssembly の生成(design.md 決定1・5・6・7)。
//!
//! 入力は3つだけ:
//!
//!   - 型の付いた HIR
//!   - 生産の特殊化計画 (`ambient_abi::ProductionPlan`)
//!   - 公開名 → 根の instance という ABI の対応
//!
//! ここで名前解決・オーバーロード選択・実装探索・要求推論は**やらない**。
//! 関数は宣言ではなく instance から作られ、呼び先は計画が持つ `InstanceId` を
//! 関数番号へ引き直すだけ。だから後で ambient record を足すときも、関数の
//! 同一性と呼び先の選択は作り直さずに済む。
//!
//! v0 が扱うのは scalar の部分言語だけ。到達した範囲にそれ以外があれば
//! 生成の前に診断で止める(core-wasm-build spec)。

use crate::ambient_abi::{Instance, Plan, ProductionPlan};
use crate::diag::Diag;
use crate::hir;
use crate::ownership::CheckedProgram;
use crate::wasm_data::{self, Indices};
use crate::wasm_layout::{self, LayoutId, Layouts, ReprKind};
use crate::wasm_runtime::{self, Body, Runtime};
use crate::wasm_wire;
use std::collections::{BTreeMap, BTreeSet};
use wasm_encoder::{
    BlockType, CodeSection, CustomSection, ExportKind, ExportSection, Function, FunctionSection,
    Instruction, Module, TypeSection, ValType,
};

/// ホストから呼ぶエントリの予約名。公開名として使うことはできない
pub const ENTRY_EXPORT: &str = "__rhodolite_main";

/// 埋め込むインタフェース記述のセクション名
pub const ABI_SECTION: &str = "rhodolite.abi";

/// ABI v0 が境界で運べる型。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Scalar {
    Unit,
    Bool,
    Int,
}

impl Scalar {
    /// ABI メタデータとソース診断で使う綴り
    pub fn spelling(self) -> &'static str {
        match self {
            Scalar::Unit => "unit",
            Scalar::Bool => "bool",
            Scalar::Int => "int",
        }
    }

    /// 値を運ぶ Wasm の型。`unit` は実行時表現を持たない
    pub fn val_type(self) -> Option<ValType> {
        match self {
            Scalar::Unit => None,
            Scalar::Bool => Some(ValType::I32),
            Scalar::Int => Some(ValType::I64),
        }
    }
}

/// scalar として扱える型か。`int?` のような optional は v0 の外
pub fn scalar_of(ty: &hir::Type) -> Option<Scalar> {
    // 参照は v0 の公開 ABI にも局所表現にも無いので scalar から外す。署名に
    // 出た借用は `borrowed_signature` が名指す(tasks 4.6)
    if ty.optional || ty.reference.is_some() {
        return None;
    }
    match &ty.kind {
        hir::TypeKind::Builtin(hir::Builtin::Unit) => Some(Scalar::Unit),
        hir::TypeKind::Builtin(hir::Builtin::Bool) => Some(Scalar::Bool),
        hir::TypeKind::Builtin(hir::Builtin::Int) => Some(Scalar::Int),
        _ => None,
    }
}

/// Wasm の境界に借用が出た(tasks 4.6、core-wasm-build spec)。
///
/// ABI v0 が運べるのは scalar だけで、借用は呼び出し側の記憶を指す。所有権
/// 検査を通っていても、境界を越えた先にその所有者は居ない
fn borrowed_signature(
    program: &hir::Program,
    span: crate::lex::Span,
    what: &str,
    ty: &hir::Type,
) -> Diag {
    Diag::at(
        span,
        format!(
            "{what}の借用 `{}` は Wasm の署名には出せません",
            program.show_type(ty)
        ),
    )
    .label("署名に出た借用")
    .help("所有する値をやり取りするか、その関数を Wasm から到達させないでください")
}

fn unsupported(span: crate::lex::Span, what: &str) -> Diag {
    Diag::at(span, format!("{what} は現在の Wasm ターゲットでは扱えません"))
        .label("ここが未対応")
        .help("いまの Wasm は unit / bool / int / str / struct / enum / optional と、束縛・代入・フィールド・算術・比較・`clone()`・所有権修飾・`??`・`match`・if・while・直接呼び出し・return・assert を扱います")
}

/// この backend が下ろせる型か。
///
/// 借用は検査済みの場所を指すアドレス1つなので、指す先が扱えるなら運べる。
/// optional は完成した所有形に 1 bit 付くだけなので、中身が扱えれば扱える。
/// 配列だけが後続スライス
pub fn supported(ty: &hir::Type) -> bool {
    matches!(
        ty.kind,
        hir::TypeKind::Builtin(
            hir::Builtin::Unit | hir::Builtin::Bool | hir::Builtin::Int | hir::Builtin::Str
        ) | hir::TypeKind::Struct(_)
            | hir::TypeKind::Enum(_)
            | hir::TypeKind::Array(_)
            // callable 値は計画が直接呼び出しへ解決済みで、実行時の表現を
            // 持たない。局所束縛としても引数としても値は流れない
            | hir::TypeKind::Callable { .. }
    )
}

// ---------------------------------------------------------------------------
// 対応範囲の検査(到達したところだけ)
// ---------------------------------------------------------------------------

/// 計画に入った instance だけを歩いて、v0 の部分言語から外れた構文を報告する。
///
/// 読み込んだ全コードは通常の静的検査を既に通っている。ここが見るのは
/// 「生産の根から到達したか」だけなので、使われていない richer な宣言は
/// 生産ビルドを止めない(core-wasm-build spec)
pub fn check_support(checked: &CheckedProgram, plan: &Plan) -> Vec<Diag> {
    check_support_impl(&checked.hir, plan)
}

fn check_support_impl(program: &hir::Program, plan: &Plan) -> Vec<Diag> {
    let mut diagnostics = Vec::new();
    for (_, instance) in plan.instances() {
        let body_id = instance.key.body;
        let hir::BodyId::Callable(callable_id) = body_id else {
            // 生産の根に test は入らないので、ここへ来たら根の作り方の破れ
            diagnostics.push(Diag::msg("test は Wasm の生産ビルドに入りません"));
            continue;
        };
        let callable = &program.callables[callable_id];

        if let Some(receiver) = callable.body.receiver {
            check_local(program, &callable.body, receiver, &mut diagnostics);
        }
        if callable.ret.reference.is_some() {
            diagnostics.push(borrowed_signature(
                program,
                callable.span,
                "戻り値",
                &callable.ret,
            ));
        } else if !supported(&callable.ret) {
            diagnostics.push(unsupported(
                callable.span,
                &format!("戻り値の型 `{}`", program.show_type(&callable.ret)),
            ));
        }

        let body = &callable.body;
        for local in &callable.params {
            // internal calls carry checked borrows as the same root address as
            // owned values. Public ABI validation is intentionally later in
            // wasm_abi, where only exported signatures are rejected.
            check_local(program, body, *local, &mut diagnostics);
        }
        for expr in &body.root {
            check_expr(program, body, *expr, &mut diagnostics);
        }
    }
    diagnostics
}

fn check_local(
    program: &hir::Program,
    body: &hir::Body,
    local: hir::LocalId,
    diagnostics: &mut Vec<Diag>,
) {
    let decl = body.local(local);
    // `ty` が無いのは初期化子が発散した束縛。読めないので実行時表現も要らない
    if let Some(ty) = &decl.ty
        && !supported(ty)
    {
        diagnostics.push(unsupported(
            decl.span,
            &format!("型 `{}` の束縛", program.show_type(ty)),
        ));
    }
}

fn check_expr(
    program: &hir::Program,
    body: &hir::Body,
    id: hir::ExprId,
    diagnostics: &mut Vec<Diag>,
) {
    let expr = body.expr(id);
    if let hir::ExprResult::Value(ty) = &expr.result
        && !supported(ty)
    {
        diagnostics.push(unsupported(
            expr.span,
            &format!("型 `{}` の値", program.show_type(ty)),
        ));
        return;
    }

    let mut children: Vec<hir::ExprId> = Vec::new();
    match &expr.kind {
        hir::ExprKind::Int(_) | hir::ExprKind::Bool(_) => {}
        hir::ExprKind::Local(local) => check_local(program, body, *local, diagnostics),
        hir::ExprKind::Let { local, value } | hir::ExprKind::AssignLocal { local, value } => {
            check_local(program, body, *local, diagnostics);
            children.push(*value);
        }
        hir::ExprKind::Neg(inner) => children.push(*inner),
        hir::ExprKind::Arith { lhs, rhs, .. } | hir::ExprKind::Eq { lhs, rhs } => {
            children.extend([*lhs, *rhs]);
        }
        hir::ExprKind::Return(value) => children.extend(value.iter().copied()),
        hir::ExprKind::Assert(cond) => children.push(*cond),
        hir::ExprKind::Block(exprs) => children.extend(exprs.iter().copied()),
        hir::ExprKind::If { cond, then, orelse } => {
            children.extend([*cond, *then]);
            children.extend(orelse.iter().copied());
        }
        hir::ExprKind::While { cond, body: inner } => children.extend([*cond, *inner]),
        hir::ExprKind::Call(hir::Call::Direct { args, .. })
        | hir::ExprKind::Call(hir::Call::Associated { args, .. }) => {
            children.extend(args.iter().copied())
        }
        // 呼び先は計画が決めているので、実引数だけを見る。callable 値そのものは
        // 実行時の表現を持たない
        hir::ExprKind::Call(hir::Call::Indirect { args, .. }) => {
            children.extend(args.iter().copied())
        }
        hir::ExprKind::Function(_) => {}
        hir::ExprKind::Call(hir::Call::Method { recv, args, .. }) => {
            children.push(*recv);
            children.extend(args.iter().copied());
        }

        hir::ExprKind::Str(_) => {}
        hir::ExprKind::Access { place, .. } => children.push(*place),
        hir::ExprKind::Clone(inner) => children.push(*inner),
        hir::ExprKind::Push { array, value } => children.extend([*array, *value]),
        hir::ExprKind::Nil => {}
        hir::ExprKind::UnitStruct(_) => {}
        hir::ExprKind::StructLit { fields, .. } => {
            children.extend(fields.iter().map(|(_, value)| *value))
        }
        hir::ExprKind::Variant(_) => {}
        hir::ExprKind::Call(hir::Call::Ctor { args, .. }) => children.extend(args.iter().copied()),
        hir::ExprKind::Coalesce { lhs, rhs } => children.extend([*lhs, *rhs]),
        hir::ExprKind::Match { subject, arms } => {
            children.push(*subject);
            for arm in arms {
                children.extend(arm.guard);
                children.push(arm.body);
            }
        }
        // ponytail: `.?` は毎回 optional の帳簿を組み立てる。`indirect` の区画は
        // 子アドレス1つなので、包み直しに1段よけいな確保が要る。使う形が出て
        // きたら `optional_field` に indirect の枝を足す
        hir::ExprKind::Field {
            recv,
            field,
            optional: true,
        } if program.fields[*field].indirect => {
            diagnostics.push(unsupported(
                expr.span,
                &format!(
                    "`indirect` なフィールド `{}` の `.?` 参照",
                    program.fields[*field].name
                ),
            ));
            children.push(*recv);
        }
        hir::ExprKind::Field { recv, .. } => children.push(*recv),
        hir::ExprKind::AssignField { recv, value, .. } => children.extend([*recv, *value]),
        hir::ExprKind::Array(elements) => children.extend(elements.iter().copied()),
        hir::ExprKind::For {
            var,
            iter,
            body: inner,
        } => {
            check_local(program, body, *var, diagnostics);
            children.extend([*iter, *inner]);
        }
        hir::ExprKind::With {
            provisions,
            body: inner,
        } => {
            children.extend(provisions.iter().filter_map(|provision| provision.value));
            children.push(*inner);
        }
        hir::ExprKind::Call(hir::Call::Slot { args, .. }) => children.extend(args.iter().copied()),
        hir::ExprKind::Poison => diagnostics.push(unsupported(expr.span, "この式")),
    }

    for child in children {
        check_expr(program, body, child, diagnostics);
    }
}

// ---------------------------------------------------------------------------
// 関数の割り当て
// ---------------------------------------------------------------------------

/// 値1つを運ぶ Wasm の値の並び(design.md 決定2)。
///
/// 到達した型は対応検査を通っているので、ここで並びが計画できないことは無い。
/// 32bit に収まらない型はその検査が名指して止める
fn values_of(layouts: &mut Layouts, program: &hir::Program, ty: &hir::Type) -> Vec<ValType> {
    layouts
        .repr(program, ty)
        .expect("対応検査を通った型は 32bit に収まる")
        .values
}

/// 式が stack に残す値の並び。発散する式は何も残さない
fn result_values(layouts: &mut Layouts, program: &hir::Program, expr: &hir::Expr) -> Vec<ValType> {
    match &expr.result {
        hir::ExprResult::Value(ty) => values_of(layouts, program, ty),
        // 発散する式の後ろは到達しない。Wasm の stack は多相なので何も要らない
        hir::ExprResult::Diverges | hir::ExprResult::Poison => Vec::new(),
    }
}

/// 束縛が占める値の並び。読めない束縛(初期化子が発散した `let`)は空
fn local_values(
    layouts: &mut Layouts,
    program: &hir::Program,
    body: &hir::Body,
    local: hir::LocalId,
) -> Vec<ValType> {
    match &body.local(local).ty {
        Some(ty) => values_of(layouts, program, ty),
        None => Vec::new(),
    }
}

/// instance 1つ分の、Wasm 関数としての形。
struct Lowered {
    callable: hir::CallableId,
    /// 値を運ぶ引数だけ。`unit` の引数は席を持たない
    params: Vec<ValType>,
    results: Vec<ValType>,
    /// HIR のローカル → Wasm のローカル番号の**並び**。`unit` のローカルは空。
    /// 値を複数持つ表現(Copy な optional など)は席をその数だけ取る
    slots: BTreeMap<hir::LocalId, Vec<u32>>,
    /// trailing hidden ambient fields。body emitter は `SlotId` からここを引き、
    /// source-level local と混ぜない。
    ambient: crate::wasm_ambient::IncomingFields,
    /// `with slot(value)` の provider handle を一度だけ留める局所。
    provisions: crate::wasm_ambient::ProvisionSeats,
    /// provision temporary / moved provider の初期化フラグ。
    provision_flags: BTreeMap<hir::ExprId, u32>,
    /// 引数の後ろに並ぶ宣言ローカルの型
    extra_locals: Vec<ValType>,
    /// 所有する非 Copy の束縛 → その並び。掃除の glue をここから引く
    owned: BTreeMap<hir::LocalId, LayoutId>,
    /// 所有する束縛 → 初期化フラグの局所番号(tasks 3.4)
    flags: BTreeMap<hir::LocalId, u32>,
    /// 作業用 `i32` の区画。要る式ごとに別を配るので、入れ子でも踏み合わない
    scratch: BTreeMap<hir::ExprId, u32>,
    /// 平らな値を一旦受ける、型の合った席(tasks 6.1)。
    ///
    /// Wasm の store は「アドレス、値」の順に積むが、式が産む値は上に載る。
    /// 複数の値を持つ Copy 値(optional)を書くにはどこかで受け直すしかない
    typed: BTreeMap<hir::ExprId, Vec<u32>>,
    /// 掃除が使う共通の作業用区画。先頭が区画の走査用、続いて射影の段ごとの器
    common: u32,
}

/// instance の receiver・引数・ambient fields と宣言ローカルへ Wasm の
/// local 番号を振る。
///
/// 番号は Wasm の規則どおり引数が先。どちらも HIR の宣言順で歩くので、
/// 同じ入力からは同じ割り当てになる
fn lower_instance_signature(
    layouts: &mut Layouts,
    program: &hir::Program,
    plan: &Plan,
    instance: &Instance,
) -> Lowered {
    let hir::BodyId::Callable(callable_id) = instance.key.body else {
        unreachable!("test は生産の instance にならない")
    };
    let callable = &program.callables[callable_id];
    let body = &callable.body;
    let mut slots: BTreeMap<hir::LocalId, Vec<u32>> = BTreeMap::new();
    let mut params = Vec::new();

    // receiver は source-level parameter より前。`Body::receiver` を普通の
    // local と同じ map へ置くので、以後の所有・cleanup は既存経路を再利用する。
    if let Some(receiver) = body.receiver {
        let values = local_values(layouts, program, body, receiver);
        let seats = values
            .iter()
            .map(|value| {
                params.push(*value);
                params.len() as u32 - 1
            })
            .collect();
        slots.insert(receiver, seats);
    }

    for local in &callable.params {
        let values = local_values(layouts, program, body, *local);
        let seats = values
            .iter()
            .map(|value| {
                params.push(*value);
                params.len() as u32 - 1
            })
            .collect();
        slots.insert(*local, seats);
    }

    // planner の layout 順をそのまま trailing `i32` parameters にする。空 layout
    // は何も足さないため、ambient-free instance の過去の bytes を保てる。
    let mut provision_ids = BTreeSet::new();
    for (_, expr) in body.exprs() {
        if let hir::ExprKind::With { provisions, .. } = &expr.kind {
            provision_ids.extend(provisions.iter().filter_map(|provision| provision.value));
        }
    }
    let ambient = crate::wasm_ambient::allocate(
        instance.layout.map(|layout| plan.layout(layout)),
        provision_ids.iter().copied(),
        params.len() as u32,
    );
    params.extend(std::iter::repeat_n(ValType::I32, ambient.incoming.len()));

    let mut extra_locals = Vec::new();
    // provision handle は hidden parameters の直後の ordinary local。planner が
    // `ValueSource::Provision` に記録した expression ID からだけ引く。
    extra_locals.extend(std::iter::repeat_n(ValType::I32, ambient.provisions.len()));
    let mut provision_flags = BTreeMap::new();
    for id in provision_ids {
        provision_flags.insert(id, (params.len() + extra_locals.len()) as u32);
        extra_locals.push(ValType::I32);
    }
    for (id, _) in body.locals() {
        if slots.contains_key(&id) {
            continue;
        }
        let values = local_values(layouts, program, body, id);
        let seats = values
            .iter()
            .map(|value| {
                extra_locals.push(*value);
                (params.len() + extra_locals.len() - 1) as u32
            })
            .collect();
        slots.insert(id, seats);
    }

    // 所有する束縛は、アドレスの席とは別に「持っているか」の 1 bit を要る。
    // 計画は「持っていれば落とす」までしか言えないので(design.md 決定6)
    let mut owned = BTreeMap::new();
    let mut flags = BTreeMap::new();
    for (id, decl) in body.locals() {
        let Some(ty) = &decl.ty else { continue };
        let repr = layouts
            .repr(program, ty)
            .expect("対応検査を通った型は 32bit に収まる");
        if let ReprKind::Owned(layout) = repr.kind {
            owned.insert(id, layout);
            flags.insert(id, (params.len() + extra_locals.len()) as u32);
            extra_locals.push(ValType::I32);
        }
    }

    // 作業用の席は、所有データを触る式にだけ配る。scalar だけの本体は1つも
    // 取らないので、バイト列がこの change の前と変わらない(tasks 1.1)
    let mut scratch = BTreeMap::new();
    for (id, _) in body.exprs() {
        if !needs_scratch(layouts, program, body, id) {
            continue;
        }
        scratch.insert(id, (params.len() + extra_locals.len()) as u32);
        extra_locals.extend(std::iter::repeat_n(
            ValType::I32,
            wasm_data::SCRATCH as usize,
        ));
    }

    // 平らな値を受け直す席。書き込み先が Copy な区画になる式と、`??` の左辺、
    // 値が2つ以上ある等値の両辺だけが要る
    let mut typed: BTreeMap<hir::ExprId, Vec<u32>> = BTreeMap::new();
    for (id, values) in stash_sites(layouts, program, body) {
        if values.is_empty() || typed.contains_key(&id) {
            continue;
        }
        let seats = values
            .iter()
            .map(|value| {
                extra_locals.push(*value);
                (params.len() + extra_locals.len() - 1) as u32
            })
            .collect();
        typed.insert(id, seats);
    }

    // 残余の破棄は射影の段ごとに器のアドレスを1つ持つ。深さはソースから分かる
    let common = (params.len() + extra_locals.len()) as u32;
    if !owned.is_empty() || !scratch.is_empty() {
        let levels = body
            .exprs()
            .map(|(id, _)| projection_depth(body, id))
            .max()
            .unwrap_or(0);
        extra_locals.extend(std::iter::repeat_n(ValType::I32, 2 + levels as usize));
    }

    Lowered {
        callable: callable_id,
        params,
        results: values_of(layouts, program, &callable.ret),
        slots,
        ambient: ambient.incoming,
        provisions: ambient.provisions,
        provision_flags,
        extra_locals,
        owned,
        flags,
        scratch,
        typed,
        common,
    }
}

/// 平らな値を席へ受け直す必要がある式と、その席の型。
///
/// 記憶へ書く値(struct のフィールド・enum の payload)、`??` の左辺、そして
/// 値を2つ以上持つ等値の両辺。scalar だけの本体には1つも出てこない。
///
/// 書き込み先の席は**宣言された型**で数える。`int` を `int?` の区画へ入れる
/// 暗黙の格上げがあるので、式そのものの型では足りない
fn stash_sites(
    layouts: &mut Layouts,
    program: &hir::Program,
    body: &hir::Body,
) -> Vec<(hir::ExprId, Vec<ValType>)> {
    let mut sites = Vec::new();
    for (_, expr) in body.exprs() {
        match &expr.kind {
            hir::ExprKind::StructLit { fields, .. } => {
                for (field, value) in fields {
                    let ty = program.fields[*field].ty.clone();
                    sites.push((*value, values_of(layouts, program, &ty)));
                }
            }
            hir::ExprKind::AssignField { field, value, .. } => {
                let ty = program.fields[*field].ty.clone();
                sites.push((*value, values_of(layouts, program, &ty)));
            }
            hir::ExprKind::Call(hir::Call::Ctor { variant, args }) => {
                for (position, value) in program.variants[*variant].payload.iter().zip(args) {
                    let ty = position.ty.clone();
                    sites.push((*value, values_of(layouts, program, &ty)));
                }
            }
            // 要素は宣言された要素型で受ける。optional 注入で幅が変わる
            hir::ExprKind::Array(elements) => {
                let element = expr.result.ty().and_then(|ty| match &ty.kind {
                    hir::TypeKind::Array(element) => Some((**element).clone()),
                    _ => None,
                });
                if let Some(element) = element {
                    let values = values_of(layouts, program, &element);
                    for value in elements {
                        sites.push((*value, values.clone()));
                    }
                }
            }
            // 押し込む要素も宣言された要素型で受ける(配列リテラルと同じ)
            hir::ExprKind::Push { array, value } => {
                let element = body.expr(*array).result.ty().and_then(|ty| match &ty.kind {
                    hir::TypeKind::Array(element) => Some((**element).clone()),
                    _ => None,
                });
                if let Some(element) = element {
                    sites.push((*value, values_of(layouts, program, &element)));
                }
            }
            hir::ExprKind::Coalesce { lhs, .. } => {
                if let Some(ty) = body.expr(*lhs).result.ty().cloned() {
                    sites.push((*lhs, values_of(layouts, program, &ty)));
                }
            }
            hir::ExprKind::Eq { lhs, rhs } => {
                let wide = [*lhs, *rhs].into_iter().any(|side| {
                    body.expr(side)
                        .result
                        .ty()
                        .is_some_and(|ty| values_of(layouts, program, ty).len() > 1)
                });
                if wide {
                    for side in [*lhs, *rhs] {
                        if let Some(ty) = body.expr(side).result.ty().cloned() {
                            sites.push((side, values_of(layouts, program, &ty)));
                        }
                    }
                }
            }
            _ => {}
        }
    }
    sites
}

/// 作業用の局所が要る式か。
///
/// 所有値を組み立てる・差し替える・持ち出す・借りて比べる式だけ。scalar しか
/// 触らない式には1つも配らないので、この change の前とバイト列が変わらない
fn needs_scratch(
    layouts: &mut Layouts,
    program: &hir::Program,
    body: &hir::Body,
    id: hir::ExprId,
) -> bool {
    let owned = |layouts: &mut Layouts, at: hir::ExprId| {
        matches!(
            body.expr(at)
                .result
                .ty()
                .map(|ty| layouts.repr(program, ty).expect("計画できる").kind),
            Some(ReprKind::Owned(_))
        )
    };
    match &body.expr(id).kind {
        // 器のアドレスを持ち回す式。struct や enum が居ないと出てこない
        hir::ExprKind::AssignField { .. }
        | hir::ExprKind::Field { .. }
        | hir::ExprKind::Match { .. }
        // 対象・buffer・長さ・添字・要素・取り出した根を持ち回す
        | hir::ExprKind::For { .. }
        // 帳簿・次の席・押し込む一時値を持ち回す(MAP-075)
        | hir::ExprKind::Push { .. } => true,
        hir::ExprKind::AssignLocal { value, .. } => owned(layouts, *value),
        hir::ExprKind::Eq { lhs, rhs } => owned(layouts, *lhs) || owned(layouts, *rhs),
        // `??` は左辺の optional を受け直してから枝を選ぶ
        hir::ExprKind::Coalesce { lhs, .. } => owned(layouts, id) || owned(layouts, *lhs),
        _ => owned(layouts, id),
    }
}

/// 場所式が根から何段の射影を通っているか。残余の破棄が潜る深さの上限になる
fn projection_depth(body: &hir::Body, id: hir::ExprId) -> u32 {
    match &body.expr(id).kind {
        hir::ExprKind::Field { recv, .. } => 1 + projection_depth(body, *recv),
        hir::ExprKind::Access { place, .. } => projection_depth(body, *place),
        _ => 0,
    }
}

/// 到達した本体が触る型を、glue の番号を決める前に全部計画しておく。
///
/// 本体を出す途中で新しい並びが増えると、予約済みの番号と食い違う
/// (design.md 決定5)
fn plan_reachable(layouts: &mut Layouts, program: &hir::Program, plan: &Plan) {
    for (_, instance) in plan.instances() {
        let hir::BodyId::Callable(id) = instance.key.body else {
            continue;
        };
        let callable = &program.callables[id];
        let _ = layouts.repr(program, &callable.ret);
        for (_, decl) in callable.body.locals() {
            if let Some(ty) = &decl.ty {
                let _ = layouts.repr(program, ty);
            }
        }
        for (_, expr) in callable.body.exprs() {
            if let hir::ExprResult::Value(ty) = &expr.result {
                let _ = layouts.repr(program, ty);
            }
        }
    }
}

/// `push` が届いた配列の並び(MAP-075)。`reserve` を出す並びを決める。
///
/// 走査は instance の計画順、その中は式の番号順なので、同じ入力からは同じ集合
fn pushed_layouts(
    layouts: &mut Layouts,
    program: &hir::Program,
    plan: &Plan,
) -> BTreeSet<LayoutId> {
    let mut pushed = BTreeSet::new();
    for (_, instance) in plan.instances() {
        let hir::BodyId::Callable(id) = instance.key.body else {
            continue;
        };
        let body = &program.callables[id].body;
        for (_, expr) in body.exprs() {
            let hir::ExprKind::Push { array, .. } = expr.kind else {
                continue;
            };
            let Some(ty) = body.expr(array).result.ty() else {
                continue;
            };
            if let Ok(ReprKind::Owned(layout) | ReprKind::Borrowed(layout)) =
                layouts.repr(program, ty).map(|repr| repr.kind)
            {
                pushed.insert(layout);
            }
        }
    }
    pushed
}

/// 文字列リテラルを静的データへ決定的に並べる(tasks 4.1)。
///
/// 同じ綴りは1つにまとめる。走査は instance の計画順、その中は式の番号順
#[derive(Default)]
struct Statics {
    bytes: Vec<u8>,
    at: BTreeMap<String, (u32, u32)>,
}

fn collect_literals(program: &hir::Program, plan: &Plan) -> Statics {
    let mut statics = Statics::default();
    for (_, instance) in plan.instances() {
        let hir::BodyId::Callable(id) = instance.key.body else {
            continue;
        };
        for (_, expr) in program.callables[id].body.exprs() {
            let hir::ExprKind::Str(text) = &expr.kind else {
                continue;
            };
            if statics.at.contains_key(text) {
                continue;
            }
            let offset = wasm_runtime::PREFIX_END + statics.bytes.len() as u32;
            statics.bytes.extend_from_slice(text.as_bytes());
            statics.at.insert(text.clone(), (offset, text.len() as u32));
        }
    }
    statics
}

/// 同じ形の関数型を1つに寄せる。並びは最初に要求された順
#[derive(Default)]
struct Types {
    ids: BTreeMap<(Vec<ValType>, Vec<ValType>), u32>,
    section: TypeSection,
}

impl Types {
    fn intern(&mut self, params: &[ValType], results: &[ValType]) -> u32 {
        let key = (params.to_vec(), results.to_vec());
        if let Some(found) = self.ids.get(&key) {
            return *found;
        }
        let id = self.ids.len() as u32;
        self.ids.insert(key, id);
        self.section
            .ty()
            .function(params.to_vec(), results.to_vec());
        id
    }
}

// ---------------------------------------------------------------------------
// 式の下ろし
// ---------------------------------------------------------------------------

struct Emitter<'a> {
    program: &'a hir::Program,
    instance: &'a crate::ambient_abi::Instance,
    /// target と ambient projection はこの production plan からだけ読む。
    specializations: &'a Plan,
    lowered: &'a Lowered,
    /// 所有権検査が確定した掃除。ここを読むだけで、順を組み直さない
    plan: &'a crate::ownership::BodyPlan,
    layouts: &'a mut Layouts,
    types: &'a mut Types,
    indices: &'a Indices,
    statics: &'a BTreeMap<String, (u32, u32)>,
    /// いま一時値を握っている構文の入れ子(内側が後ろ)。所有権検査は束縛だけを
    /// 追うので、`for` が抱えた対象のような持ち主のない値はここで覚える
    live: Vec<(LayoutId, u32)>,
    /// `with` が保持する temporary / moved provider。return では lexical end に
    /// 到達しないため、ここから reverse order で guarded drop を出す。
    live_provisions: Vec<(hir::ExprId, LayoutId)>,
    out: Body,
}

#[derive(Clone, Copy)]
enum PlannedReceiver {
    Expr(hir::ExprId),
    Provider,
}

/// instance が受け取った hidden field を、下流の slot receiver へ所有として
/// 渡すか。provider selection を作り直さず、planner が持つ direct edges だけを
/// 辿る。循環は「この経路ではまだ消費を見つけていない」として閉じる。
fn consumes_incoming(
    program: &hir::Program,
    plan: &Plan,
    instance: crate::ambient_abi::InstanceId,
    slot: hir::SlotId,
    visiting: &mut BTreeSet<(usize, hir::SlotId)>,
) -> bool {
    if !visiting.insert((instance.index(), slot)) {
        return false;
    }
    let consumes = plan.instance(instance).calls.values().any(|call| {
        let target = plan.instance(call.target);
        let target_receiver_is_owned = match target.key.body {
            hir::BodyId::Callable(callable) => {
                program.callables[callable].receiver == Some(hir::ReceiverMode::Owned)
            }
            hir::BodyId::Test(_) => false,
        };
        if call.receiver == Some(crate::ambient_abi::ValueSource::Incoming(slot))
            && target_receiver_is_owned
        {
            return true;
        }
        call.projection.iter().any(|(target_slot, source)| {
            *source == crate::ambient_abi::ValueSource::Incoming(slot)
                && consumes_incoming(program, plan, call.target, *target_slot, visiting)
        })
    });
    visiting.remove(&(instance.index(), slot));
    consumes
}

/// `match` の arm 1つ。`pattern` が `None` なら catch-all
struct Arm {
    pattern: Option<(hir::VariantId, Vec<Option<hir::LocalId>>)>,
    guard: Option<hir::ExprId>,
    body: hir::ExprId,
}

/// `int` 1つ、`bool` 1つ。scalar の演算が要求する形
const WANT_INT: &[ValType] = &[ValType::I64];
const WANT_BOOL: &[ValType] = &[ValType::I32];

impl Emitter<'_> {
    fn push(&mut self, instruction: Instruction<'_>) {
        self.out.ins(instruction);
    }

    fn body(&self) -> &hir::Body {
        &self.program.callables[self.lowered.callable].body
    }

    /// 式が残す値の並び
    fn produced(&mut self, id: hir::ExprId) -> Vec<ValType> {
        let program = self.program;
        let expr = &program.callables[self.lowered.callable].body.expr(id);
        result_values(self.layouts, program, expr)
    }

    /// 式の列を下ろす。値になるのは最後の式だけで、途中の値は捨てる
    fn sequence(&mut self, exprs: &[hir::ExprId], want: Option<&hir::Type>) {
        let Some((last, leading)) = exprs.split_last() else {
            return;
        };
        for expr in leading {
            self.expr(*expr, &[]);
        }
        self.value(*last, want);
    }

    /// その位置が求める**型**で値を1つ下ろす(tasks 6.1)。
    ///
    /// 型検査は `int` を `int?` の位置へそのまま通す。実行時表現は違うので、
    /// 格上げが要るならここで tag を足して包む。それ以外は値の並びで下ろす
    fn value(&mut self, id: hir::ExprId, want: Option<&hir::Type>) {
        let Some(want) = want else {
            self.expr(id, &[]);
            return;
        };
        let produced = self.body().expr(id).result.ty().cloned();
        if let Some(produced) = produced
            && want.optional
            && want.reference.is_none()
            && !produced.optional
        {
            self.wrap_optional(id, want);
            return;
        }
        let values = values_of(self.layouts, self.program, want);
        self.expr(id, &values);
    }

    /// 中身の値を optional へ包む。
    ///
    /// Copy な optional は `[tag, ...payload]` を積むだけ。所有する optional は
    /// 根を1つ確保して、tag と中身を置く
    fn wrap_optional(&mut self, id: hir::ExprId, want: &hir::Type) {
        let layout = self
            .layouts
            .plan(self.program, want)
            .expect("対応検査を通った型は 32bit に収まる");
        let payload = match self.layouts.get(layout).shape {
            wasm_layout::Shape::Optional { payload } => payload,
            ref other => unreachable!("optional ではない並びです: {other:?}"),
        };
        if self.layouts.get(layout).copy {
            self.push(Instruction::I32Const(wasm_layout::OPTIONAL_PRESENT as i32));
            let values = self.produced(id);
            self.expr(id, &values);
            return;
        }
        // 中身を先に作る。作業用の席は中身の下ろしが使い終えてから借りる
        self.expr(id, &[ValType::I32]);
        let scratch = self.scratch(id);
        let (root, temporary) = (scratch, scratch + 1);
        self.out.set(temporary);
        wasm_data::alloc_root(&mut self.out, self.indices, self.layouts, layout);
        self.out.set(root);
        self.out
            .get(root)
            .num(wasm_layout::OPTIONAL_PRESENT)
            .ins(Instruction::I32Store8(wasm_data::tag_byte()));
        self.out.get(temporary);
        let (indices, layouts) = (self.indices, &*self.layouts);
        wasm_data::relocate_into(
            &mut self.out,
            indices,
            layouts,
            payload.layout,
            root,
            payload.offset,
            temporary,
        );
        self.out.get(root);
    }

    /// 式を「この結果で終わる」ように下ろす。
    ///
    /// `want` が空なら stack には何も残さない。値を産む式なら、その並びのぶん
    /// だけ落とす。発散する式は何も残さないので足すものは無い
    fn expr(&mut self, id: hir::ExprId, want: &[ValType]) {
        let produced = self.produced(id);
        self.lower(id);
        if !want.is_empty() {
            return;
        }
        // 捨てる値が一時値なら、落とすのはここ。持ち主が他に居ない
        if let Some(layout) = self.temporary(id) {
            let slot = self.lowered.common;
            self.release(layout, slot);
            return;
        }
        for _ in &produced {
            self.push(Instruction::Drop);
        }
    }

    /// 式そのもの。残す値は `result_values` が決めたぶんだけ
    fn lower(&mut self, id: hir::ExprId) {
        let body = self.body();
        let expr = body.expr(id);
        match &expr.kind {
            hir::ExprKind::Int(n) => self.push(Instruction::I64Const(*n)),
            hir::ExprKind::Bool(b) => self.push(Instruction::I32Const(i32::from(*b))),

            hir::ExprKind::Local(local) => {
                let seats = self.lowered.slots.get(local).cloned().unwrap_or_default();
                for seat in seats {
                    self.push(Instruction::LocalGet(seat));
                }
            }

            hir::ExprKind::Let { local, value } => {
                let (local, value) = (*local, *value);
                let seats = self.lowered.slots.get(&local).cloned().unwrap_or_default();
                let want = self.local_type(local);
                self.value(value, want.as_ref());
                // stack の上は並びの最後。奥から埋めるので逆順で受ける
                for seat in seats.iter().rev() {
                    self.push(Instruction::LocalSet(*seat));
                }
                if let Some(flag) = self.lowered.flags.get(&local).copied() {
                    wasm_data::mark_initialized(&mut self.out, flag);
                }
            }

            hir::ExprKind::AssignLocal { local, value } => {
                let (local, value) = (*local, *value);
                let seats = self.lowered.slots.get(&local).cloned().unwrap_or_default();
                let want = self.local_type(local);
                self.value(value, want.as_ref());
                match self.lowered.owned.get(&local).copied() {
                    // 新しい値が出来上がってから古い値を落とす。先に落とすと、
                    // 途中で trap したときに落ちた値をもう一度落としてしまう
                    Some(layout) => {
                        let flag = self.lowered.flags[&local];
                        let scratch = self.scratch(id);
                        let glue = self.indices.of(layout);
                        self.push(Instruction::LocalSet(scratch));
                        wasm_data::drop_guarded(&mut self.out, self.indices, glue, seats[0], flag);
                        self.push(Instruction::LocalGet(scratch));
                        self.push(Instruction::LocalSet(seats[0]));
                        wasm_data::mark_initialized(&mut self.out, flag);
                    }
                    None => {
                        for seat in seats.iter().rev() {
                            self.push(Instruction::LocalSet(*seat));
                        }
                    }
                }
            }

            hir::ExprKind::Str(text) => {
                let (offset, len) = self.statics[text];
                let scratch = self.scratch(id);
                wasm_data::literal_str(&mut self.out, self.indices, scratch, offset, len);
            }

            // フィールドを持たない struct も、他と区別できる根を1つ持つ
            hir::ExprKind::UnitStruct(_) => {
                let layout = self.owned_layout(id).expect("struct の値は所有を産む");
                wasm_data::alloc_root(&mut self.out, self.indices, self.layouts, layout);
            }

            // 値はソースの順に評価し、置き場所は宣言の順(design.md 決定3)
            hir::ExprKind::StructLit { struct_, fields } => {
                let (struct_, fields) = (*struct_, fields.clone());
                self.struct_literal(id, struct_, &fields);
            }

            hir::ExprKind::Field {
                recv,
                field,
                optional: false,
            } => {
                let (recv, field) = (*recv, *field);
                let slot = self.slot_of(recv, field);
                let address = self.scratch(id);
                self.expr(recv, &[ValType::I32]);
                self.out.set(address);
                self.read_slot(&slot, address);
            }

            hir::ExprKind::Field {
                recv,
                field,
                optional: true,
            } => {
                let (recv, field) = (*recv, *field);
                self.optional_field(id, recv, field);
            }

            // payload を持たない enum はタグそのもの、payload を持つ enum は
            // 根を1つ確保して tag を書く(tasks 6.1・6.2)
            hir::ExprKind::Variant(variant) => {
                let variant = *variant;
                self.construct_variant(id, variant, &[]);
            }

            hir::ExprKind::Call(hir::Call::Ctor { variant, args }) => {
                let (variant, args) = (*variant, args.clone());
                self.construct_variant(id, variant, &args);
            }

            hir::ExprKind::Nil => self.nil(id),

            hir::ExprKind::Array(elements) => {
                let elements = elements.clone();
                self.array_literal(id, &elements);
            }

            hir::ExprKind::For {
                var,
                iter,
                body: inner,
            } => {
                let (var, iter, inner) = (*var, *iter, *inner);
                self.for_expr(id, var, iter, inner);
            }

            hir::ExprKind::Coalesce { lhs, rhs } => {
                let (lhs, rhs) = (*lhs, *rhs);
                self.coalesce(id, lhs, rhs);
            }

            hir::ExprKind::Match { subject, arms } => {
                let subject = *subject;
                let arms: Vec<Arm> = arms
                    .iter()
                    .map(|arm| Arm {
                        pattern: match &arm.pattern {
                            hir::Pattern::Variant { variant, bindings } => {
                                Some((*variant, bindings.clone()))
                            }
                            hir::Pattern::CatchAll => None,
                        },
                        guard: arm.guard,
                        body: arm.body,
                    })
                    .collect();
                self.match_expr(id, subject, &arms);
            }

            hir::ExprKind::AssignField { recv, field, value } => {
                let (recv, field, value) = (*recv, *field, *value);
                self.assign_field(id, recv, field, value);
            }

            // 借用は所有を取らずにアドレスを写すだけ。move はそれに加えて
            // 持ち出し元のフラグを落とす(design.md 決定2)
            hir::ExprKind::Access { mode, place } => {
                let (mode, place) = (*mode, *place);
                if mode == hir::AccessMode::Move
                    && let hir::ExprKind::Field {
                        recv,
                        field,
                        optional: false,
                    } = self.body().expr(place).kind
                {
                    self.consume_field(id, recv, field);
                    // 器の残りはここで落ちる。出口まで持ち越さない
                    self.residue(id);
                    self.residue(place);
                    return;
                }
                let want = self.produced(place);
                self.expr(place, &want);
                if mode == hir::AccessMode::Move
                    && let Some(root) = self.moved_root(place)
                    && let Some(flag) = self.lowered.flags.get(&root).copied()
                {
                    wasm_data::mark_moved(&mut self.out, flag);
                }
            }

            // 組み込みの `push`(MAP-075 決定3)
            hir::ExprKind::Push { array, value } => {
                let (array, value) = (*array, *value);
                self.push_element(id, array, value);
            }

            // glue は割り当て済みの器へ写す。根はここで用意する
            hir::ExprKind::Clone(inner) => {
                let inner = *inner;
                let want = self.produced(inner);
                self.expr(inner, &want);
                let layout = self.owned_layout(id).expect("`clone()` は所有を産む");
                let scratch = self.scratch(id);
                let (fresh, source) = (scratch, scratch + 1);
                let temporary = self.temporary(inner);
                if temporary.is_some() {
                    self.push(Instruction::LocalTee(source));
                }
                wasm_data::alloc_root(&mut self.out, self.indices, self.layouts, layout);
                self.push(Instruction::LocalTee(fresh));
                self.push(Instruction::Call(self.indices.of(layout).clone));
                if let Some(layout) = temporary {
                    let (indices, glue) = (self.indices, self.indices.of(layout));
                    wasm_data::drop_root(&mut self.out, indices, glue, source);
                }
                self.push(Instruction::LocalGet(fresh));
            }

            // Wasm に単項マイナスは無い。`0 - n` は MIN でも仕様どおり回り込む
            hir::ExprKind::Neg(inner) => {
                let inner = *inner;
                self.push(Instruction::I64Const(0));
                self.expr(inner, WANT_INT);
                self.push(Instruction::I64Sub);
            }

            hir::ExprKind::Arith { op, lhs, rhs } => {
                let (op, lhs, rhs) = (*op, *lhs, *rhs);
                self.expr(lhs, WANT_INT);
                self.expr(rhs, WANT_INT);
                self.push(match op {
                    hir::ArithOp::Add => Instruction::I64Add,
                    hir::ArithOp::Sub => Instruction::I64Sub,
                    hir::ArithOp::Mul => Instruction::I64Mul,
                    // 0除算と MIN / -1 はどちらも trap する。仕様の失敗2種と同じ
                    hir::ArithOp::Div => Instruction::I64DivS,
                });
            }

            hir::ExprKind::Eq { lhs, rhs } => {
                let (lhs, rhs) = (*lhs, *rhs);
                // 所有の複合値の等値は、並びごとに生成した glue が受ける
                if let Some(layout) = self.compound_operand(lhs, rhs) {
                    let glue = self.indices.of(layout);
                    let scratch = self.scratch(id);
                    self.expr(lhs, &[ValType::I32]);
                    let left = self.temporary(lhs);
                    if left.is_some() {
                        self.push(Instruction::LocalTee(scratch));
                    }
                    self.expr(rhs, &[ValType::I32]);
                    let right = self.temporary(rhs);
                    if right.is_some() {
                        self.push(Instruction::LocalTee(scratch + 1));
                    }
                    self.push(Instruction::Call(glue.eq));
                    // 比べるだけでは所有は動かない。持ち込んだ一時値はここで落とす
                    for (temporary, slot) in [(left, scratch), (right, scratch + 1)] {
                        if let Some(layout) = temporary {
                            let (indices, glue) = (self.indices, self.indices.of(layout));
                            wasm_data::drop_root(&mut self.out, indices, glue, slot);
                        }
                    }
                    return;
                }
                let mut operand = self.produced(lhs);
                if operand.is_empty() {
                    operand = self.produced(rhs);
                }
                // 値を2つ以上持つ Copy 値(optional)は席へ受け直して1つずつ。
                // 空の optional は中身を 0 で埋めてあるので、tag が同じなら
                // 中身の比較も素直に一致する(`nil` の下ろしを参照)
                if operand.len() > 1 {
                    self.expr(lhs, &operand);
                    let left = self.stash(lhs);
                    for seat in left.iter().rev() {
                        self.out.set(*seat);
                    }
                    self.expr(rhs, &operand);
                    let right = self.stash(rhs);
                    for seat in right.iter().rev() {
                        self.out.set(*seat);
                    }
                    for (index, value) in operand.iter().enumerate() {
                        self.out.get(left[index]).get(right[index]);
                        match value {
                            ValType::I64 => self.push(Instruction::I64Eq),
                            _ => self.push(Instruction::I32Eq),
                        }
                        if index > 0 {
                            self.push(Instruction::I32And);
                        }
                    }
                    return;
                }
                self.expr(lhs, &operand);
                self.expr(rhs, &operand);
                match operand.as_slice() {
                    [ValType::I64] => self.push(Instruction::I64Eq),
                    [ValType::I32] => self.push(Instruction::I32Eq),
                    // unit は値を持たないので常に等しい。両辺の効果だけ走らせた
                    [] => self.push(Instruction::I32Const(1)),
                    other => unreachable!("等値を下ろせない表現です: {other:?}"),
                }
            }

            // Wasm の `return` は関数の結果型ぶんを stack から返す。以降の式は
            // 到達しないので、下ろしても validator は多相な stack で受ける
            hir::ExprKind::Return(value) => {
                let value = *value;
                if let Some(value) = value {
                    let want = self.program.callables[self.lowered.callable].ret.clone();
                    self.value(value, Some(&want));
                }
                // 掃除は束縛のぶん。握ったままの一時値は内側から返す
                self.release_live();
                self.cleanup(crate::ownership::Exit::Return(id));
                self.push(Instruction::Return);
            }

            hir::ExprKind::Assert(cond) => {
                let cond = *cond;
                self.expr(cond, WANT_BOOL);
                self.push(Instruction::I32Eqz);
                self.push(Instruction::If(BlockType::Empty));
                self.push(Instruction::Unreachable);
                self.push(Instruction::End);
            }

            hir::ExprKind::Block(exprs) => {
                let exprs = exprs.clone();
                let want = self.body().expr(id).result.ty().cloned();
                self.sequence(&exprs, want.as_ref());
            }

            hir::ExprKind::If { cond, then, orelse } => {
                let (cond, then, orelse) = (*cond, *then, *orelse);
                let want = self.produced(id);
                let want_ty = self.body().expr(id).result.ty().cloned();
                self.expr(cond, WANT_BOOL);
                let block = self.block_type(&want);
                self.push(Instruction::If(block));
                self.value(then, want_ty.as_ref());
                self.cleanup(crate::ownership::Exit::Join {
                    branch: id,
                    taken: true,
                });
                if let Some(orelse) = orelse {
                    self.push(Instruction::Else);
                    self.value(orelse, want_ty.as_ref());
                    self.cleanup(crate::ownership::Exit::Join {
                        branch: id,
                        taken: false,
                    });
                }
                self.push(Instruction::End);
            }

            hir::ExprKind::While { cond, body: inner } => {
                let (cond, inner) = (*cond, *inner);
                self.push(Instruction::Block(BlockType::Empty));
                self.push(Instruction::Loop(BlockType::Empty));
                self.expr(cond, WANT_BOOL);
                self.push(Instruction::I32Eqz);
                self.push(Instruction::BrIf(1));
                self.expr(inner, &[]);
                self.cleanup(crate::ownership::Exit::LoopBack(id));
                self.push(Instruction::Br(0));
                self.push(Instruction::End);
                self.push(Instruction::End);
                self.cleanup(crate::ownership::Exit::LoopExit(id));
            }

            hir::ExprKind::With {
                provisions,
                body: inner,
            } => {
                let values = provisions
                    .iter()
                    .filter_map(|provision| provision.value)
                    .collect::<Vec<_>>();
                self.with(id, &values, *inner);
            }

            // 呼び先は計画が持っている。名前で引き直さない(design.md 決定5)
            hir::ExprKind::Call(hir::Call::Direct { args, .. }) => {
                self.planned_call(id, None, &args.clone());
            }

            hir::ExprKind::Call(hir::Call::Method { recv, args, .. }) => {
                self.planned_call(id, Some(PlannedReceiver::Expr(*recv)), &args.clone());
            }

            hir::ExprKind::Call(hir::Call::Associated { args, .. }) => {
                self.planned_call(id, None, &args.clone());
            }

            // 間接呼び出しも計画が選んだ1つの直接呼び出し。表も funcref も
            // 使わないので、直接呼び出しと同じ物理規約になる
            hir::ExprKind::Call(hir::Call::Indirect { args, .. }) => {
                self.planned_call(id, None, &args.clone());
            }

            // callable 値そのものは実行時の表現を持たない
            hir::ExprKind::Function(_) => {}

            hir::ExprKind::Call(hir::Call::Slot { receiver, args, .. }) => {
                let receiver = matches!(receiver, hir::SlotReceiver::Value)
                    .then_some(PlannedReceiver::Provider);
                self.planned_call(id, receiver, &args.clone());
            }

            // 対応範囲の検査が先に止めているので、ここへ来たら検査の抜け
            other => unreachable!("Wasm へ下ろせない式が検査を抜けました: {other:?}"),
        }
    }

    /// ordinary / inherent / associated calls share the one physical calling
    /// convention: concrete receiver, source-ordered declared arguments, then
    /// the planner-recorded ambient projection.
    fn planned_call(
        &mut self,
        id: hir::ExprId,
        receiver: Option<PlannedReceiver>,
        args: &[hir::ExprId],
    ) {
        let planned = &self.instance.calls[&id];
        let target_instance = self.specializations.instance(planned.target);
        let hir::BodyId::Callable(target_id) = target_instance.key.body else {
            unreachable!("production call target は callable")
        };
        let target = &self.program.callables[target_id];

        match (receiver, target.body.receiver) {
            (Some(PlannedReceiver::Expr(receiver)), Some(local)) => {
                let ty = target.body.local(local).ty.clone();
                self.value(receiver, ty.as_ref());
            }
            (Some(PlannedReceiver::Provider), Some(_)) => {
                let source = planned
                    .receiver
                    .expect("pre-emission validator が provider receiver を検査する");
                self.provider(source);
                if target.receiver == Some(hir::ReceiverMode::Owned)
                    && let crate::ambient_abi::ValueSource::Provision(id) = source
                {
                    self.mark_provision_moved(id);
                }
            }
            (None, None) => {}
            _ => unreachable!("pre-emission validator が receiver shape を検査する"),
        }
        for (arg, parameter) in args.iter().zip(&target.params) {
            let ty = target.body.local(*parameter).ty.clone();
            self.value(*arg, ty.as_ref());
        }
        for (slot, source) in &planned.projection {
            self.provider(*source);
            if let crate::ambient_abi::ValueSource::Provision(id) = source
                && consumes_incoming(
                    self.program,
                    self.specializations,
                    planned.target,
                    *slot,
                    &mut BTreeSet::new(),
                )
            {
                self.mark_provision_moved(*id);
            }
        }
        self.push(Instruction::Call(planned.target.index() as u32));
    }

    /// planner が選んだ provider handle を読み出す。ここで provider selection を
    /// 行わないので、forwarding も slot receiver も単なる local get になる。
    fn provider(&mut self, source: crate::ambient_abi::ValueSource) {
        match source {
            crate::ambient_abi::ValueSource::Incoming(slot) => {
                let local = self
                    .lowered
                    .ambient
                    .get(slot)
                    .expect("pre-emission validator が incoming field を検査する");
                self.out.get(local);
            }
            crate::ambient_abi::ValueSource::Provision(id) => {
                let local = self
                    .lowered
                    .provisions
                    .get(id)
                    .expect("pre-emission validator が provision seat を検査する");
                self.out.get(local);
            }
        }
    }

    fn mark_provision_moved(&mut self, id: hir::ExprId) {
        let flag = self.lowered.provision_flags[&id];
        wasm_data::mark_moved(&mut self.out, flag);
    }

    /// value provisions are evaluated under the outer instance before any
    /// provider is installed. A seat is then reused by every planned call in
    /// the body, and owned temporaries are released at this lexical boundary.
    fn with(&mut self, id: hir::ExprId, provisions: &[hir::ExprId], inner: hir::ExprId) {
        let live_at = self.live_provisions.len();
        for value in provisions {
            let ty = self.body().expr(*value).result.ty().cloned();
            self.value(*value, ty.as_ref());
            let seat = self
                .lowered
                .provisions
                .get(*value)
                .expect("reachable value provision has a reserved seat");
            self.out.set(seat);
            if let Some(layout) = self.temporary(*value) {
                let flag = self.lowered.provision_flags[value];
                wasm_data::mark_initialized(&mut self.out, flag);
                self.live_provisions.push((*value, layout));
            }
        }
        let want = self.body().expr(id).result.ty().cloned();
        self.value(inner, want.as_ref());
        self.release_provisions_from(live_at);
    }

    fn release_provisions_from(&mut self, start: usize) {
        while self.live_provisions.len() > start {
            let (id, layout) = self.live_provisions.pop().expect("length を検査済み");
            let seat = self.lowered.provisions.get(id).expect("seat がある");
            let flag = self.lowered.provision_flags[&id];
            wasm_data::drop_guarded(
                &mut self.out,
                self.indices,
                self.indices.of(layout),
                seat,
                flag,
            );
        }
    }

    /// 式の結果の実行時表現の種別
    fn kind_of(&mut self, id: hir::ExprId) -> Option<ReprKind> {
        let program = self.program;
        let expr = program.callables[self.lowered.callable].body.expr(id);
        let ty = expr.result.ty()?;
        Some(
            self.layouts
                .repr(program, ty)
                .expect("対応検査を通った型は 32bit に収まる")
                .kind,
        )
    }

    /// その式が所有する複合値を産むなら、その並び
    fn owned_layout(&mut self, id: hir::ExprId) -> Option<LayoutId> {
        match self.kind_of(id) {
            Some(ReprKind::Owned(layout)) => Some(layout),
            _ => None,
        }
    }

    /// その式がアドレスで運ぶ並び。所有していても借りていても同じ形を指す
    fn compound_layout(&mut self, id: hir::ExprId) -> Option<LayoutId> {
        match self.kind_of(id) {
            Some(ReprKind::Owned(layout) | ReprKind::Borrowed(layout)) => Some(layout),
            _ => None,
        }
    }

    /// 両辺のどちらかが所有・借用する複合値なら、その並び。
    ///
    /// 表現はどちらも `i32` なので、値の型だけでは `bool` と言い分けられない。
    /// 種別まで見て決める
    fn compound_operand(&mut self, lhs: hir::ExprId, rhs: hir::ExprId) -> Option<LayoutId> {
        for side in [lhs, rhs] {
            let layout = match self.kind_of(side) {
                Some(ReprKind::Owned(layout) | ReprKind::Borrowed(layout)) => layout,
                _ => continue,
            };
            if !self.layouts.get(layout).copy {
                return Some(layout);
            }
        }
        None
    }

    /// 所有を持ち出した場所の根。いまは局所束縛そのものだけ(射影は tasks 5.4)
    fn moved_root(&self, place: hir::ExprId) -> Option<hir::LocalId> {
        match self.body().expr(place).kind {
            hir::ExprKind::Local(local) => Some(local),
            _ => None,
        }
    }

    /// その出口を通るときの掃除を、計画の順に出す(tasks 3.6)。
    ///
    /// glue はアドレスを引数に取るだけなので、stack に載っている値を
    /// またいでも釣り合いは崩れない
    fn cleanup(&mut self, exit: crate::ownership::Exit) {
        let drops: Vec<crate::ownership::Drop> = self.plan.cleanup(exit).to_vec();
        for drop in drops {
            match drop {
                crate::ownership::Drop::Local(local) => {
                    let (Some(flag), Some(layout)) = (
                        self.lowered.flags.get(&local).copied(),
                        self.lowered.owned.get(&local).copied(),
                    ) else {
                        continue;
                    };
                    let address = self.lowered.slots[&local][0];
                    let glue = self.indices.of(layout);
                    wasm_data::drop_guarded(&mut self.out, self.indices, glue, address, flag);
                }
                // 消費した経路だけを飛ばして、器の残り全部を落とす(tasks 3.5)
                crate::ownership::Drop::Remaining { root, consumed } => {
                    self.drop_remaining(root, &consumed);
                }
            }
        }
    }

    /// その式に配った作業用の区画の先頭
    fn scratch(&self, id: hir::ExprId) -> u32 {
        self.lowered.scratch[&id]
    }

    /// その式が、誰も持ち主でない所有値を残すか。
    ///
    /// 場所を読んだだけなら持ち主は元のまま。構築・呼び出し・`clone()`・
    /// `move` は新しい持ち主をこちらへ渡すので、使い終えたら落とす責任が付く
    fn temporary(&mut self, id: hir::ExprId) -> Option<LayoutId> {
        // `.?` の結果は記憶の上に元が無く、読むたびに確保する。持ち主は使い手
        if let hir::ExprKind::Field { optional: true, .. } = self.body().expr(id).kind {
            return self.owned_layout(id);
        }
        let layout = match self.kind_of(id) {
            Some(ReprKind::Owned(layout)) => layout,
            _ => return None,
        };
        match self.plan.access(id).map(|access| access.mode) {
            Some(
                crate::ownership::Mode::Shared
                | crate::ownership::Mode::Mutable
                | crate::ownership::Mode::Read,
            ) => None,
            _ => Some(layout),
        }
    }

    /// stack の一番上にある一時値を、局所へ預けたうえで落とす
    fn release(&mut self, layout: LayoutId, slot: u32) {
        let (indices, glue) = (self.indices, self.indices.of(layout));
        self.out.set(slot);
        wasm_data::drop_root(&mut self.out, indices, glue, slot);
    }

    /// レシーバの並びの中で、そのフィールドが占める区画
    fn slot_of(&mut self, recv: hir::ExprId, field: hir::FieldId) -> wasm_layout::Slot {
        let layout = self
            .compound_layout(recv)
            .expect("フィールドのレシーバは器のアドレス");
        self.struct_slot(layout, field)
    }

    /// struct の並びの中で、そのフィールドが占める区画
    fn struct_slot(&self, layout: LayoutId, field: hir::FieldId) -> wasm_layout::Slot {
        let owner = self.program.fields[field].owner;
        let position = self.program.structs[owner]
            .fields
            .iter()
            .position(|declared| *declared == field)
            .expect("宣言に無いフィールド");
        match &self.layouts.get(layout).shape {
            wasm_layout::Shape::Struct { fields, .. } => fields[position],
            other => unreachable!("フィールドを持たない並びです: {other:?}"),
        }
    }

    /// 器のアドレスが入った局所から、その区画の値か場所を積む。
    ///
    /// Copy な区画は読み出し、直接置かれた所有の子はその場所、`indirect` の
    /// 子は指している根が答えになる
    fn read_slot(&mut self, slot: &wasm_layout::Slot, address: u32) {
        if slot.indirect {
            self.out.get(address).offset(slot.offset).load();
            return;
        }
        if self.layouts.get(slot.layout).copy {
            let (layouts, layout, offset) = (&*self.layouts, slot.layout, slot.offset);
            wasm_data::load_copy(&mut self.out, layouts, layout, address, offset);
            return;
        }
        self.out.get(address).offset(slot.offset);
    }

    /// `.?` — optional なレシーバからフィールドを読む(tasks 5.2)。
    ///
    /// レシーバが `nil` なら結果も `nil`。そうでなければフィールドを optional へ
    /// 包む。宣言型が既に optional なら 1 bit は増えないので(typecheck)、
    /// フィールドの帳簿がそのまま結果になる。
    ///
    /// 記憶の上に「そのフィールドの optional」という帳簿は無い。optional 性は
    /// レシーバの tag が持っているので、読むたびに新しく組み立てる。だから
    /// 結果は必ず持ち主の居ない一時値になる(`temporary` がそう答える)
    fn optional_field(&mut self, id: hir::ExprId, recv: hir::ExprId, field: hir::FieldId) {
        let recv_layout = self
            .compound_layout(recv)
            .expect("`.?` のレシーバは optional の根");
        let recv_payload = match self.layouts.get(recv_layout).shape {
            wasm_layout::Shape::Optional { payload } => payload,
            ref other => unreachable!("optional ではない並びです: {other:?}"),
        };
        let slot = self.struct_slot(recv_payload.layout, field);
        // 宣言型が既に optional なら、区画の帳簿がそのまま結果の形
        let declared_optional = self.program.fields[field].ty.optional;
        let want = self.produced(id);
        let result = self.owned_layout(id);
        let scratch = self.scratch(id);
        let (root, inner, fresh) = (scratch, scratch + 1, scratch + 2);
        let carried = self.temporary(recv);

        self.expr(recv, &[ValType::I32]);
        self.out.set(root);
        self.out.get(root).offset(recv_payload.offset);
        self.out.set(inner);

        match result {
            // 所有する結果は帳簿を1つ確保して、活きているときだけ中身を写す
            Some(result) => {
                let payload = match self.layouts.get(result).shape {
                    wasm_layout::Shape::Optional { payload } => payload,
                    ref other => unreachable!("optional ではない並びです: {other:?}"),
                };
                let (indices, layouts) = (self.indices, &*self.layouts);
                wasm_data::alloc_root(&mut self.out, indices, layouts, result);
                self.out.set(fresh);

                self.out
                    .get(root)
                    .ins(Instruction::I32Load8U(wasm_data::tag_byte()));
                self.out.ins(Instruction::If(BlockType::Empty));
                if declared_optional {
                    // 区画そのものが結果の帳簿。丸ごと深く写す
                    self.out.get(inner).offset(slot.offset);
                    self.out.get(fresh);
                    self.out
                        .ins(Instruction::Call(self.indices.of(result).clone));
                } else {
                    self.out
                        .get(fresh)
                        .num(wasm_layout::OPTIONAL_PRESENT)
                        .ins(Instruction::I32Store8(wasm_data::tag_byte()));
                    self.out.get(inner).offset(slot.offset);
                    self.out.get(fresh).offset(payload.offset);
                    self.out
                        .ins(Instruction::Call(self.indices.of(slot.layout).clone));
                }
                self.out.ins(Instruction::Else);
                self.out
                    .get(fresh)
                    .num(wasm_layout::OPTIONAL_EMPTY)
                    .ins(Instruction::I32Store8(wasm_data::tag_byte()));
                self.out.ins(Instruction::End);
                self.out.get(fresh);
            }
            // Copy な結果は平ら。tag を先頭に、続けて中身を積む
            None => {
                self.out
                    .get(root)
                    .ins(Instruction::I32Load8U(wasm_data::tag_byte()));
                let block = self.block_type(&want);
                self.push(Instruction::If(block));
                if !declared_optional {
                    self.push(Instruction::I32Const(wasm_layout::OPTIONAL_PRESENT as i32));
                }
                self.read_slot(&slot, inner);
                self.push(Instruction::Else);
                self.push(Instruction::I32Const(wasm_layout::OPTIONAL_EMPTY as i32));
                // 空のときの中身は読まれない。`??` と等値がこの 0 埋めを前提にする
                for value in want.iter().skip(1) {
                    match value {
                        ValType::I64 => self.push(Instruction::I64Const(0)),
                        _ => self.push(Instruction::I32Const(0)),
                    }
                }
                self.push(Instruction::End);
            }
        }

        // レシーバを持ち込んでいたなら、写し終えたここで返す
        if let Some(layout) = carried {
            let (indices, glue) = (self.indices, self.indices.of(layout));
            wasm_data::drop_root(&mut self.out, indices, glue, root);
        }
    }

    /// enum の宣言順のタグ
    fn variant_tag(&self, variant: hir::VariantId) -> u32 {
        let owner = self.program.variants[variant].owner;
        self.program.enums[owner]
            .variants
            .iter()
            .position(|declared| *declared == variant)
            .expect("宣言に無い variant") as u32
    }

    /// enum の値を1つ作る(tasks 6.1・6.2)。
    ///
    /// payload を持たない enum はタグそのもの。payload を持つ enum は根を確保し、
    /// tag と**その variant の payload だけ**を初期化する
    fn construct_variant(
        &mut self,
        id: hir::ExprId,
        variant: hir::VariantId,
        args: &[hir::ExprId],
    ) {
        let tag = self.variant_tag(variant);
        let Some(layout) = self.owned_layout(id) else {
            // payload をどの variant も持たない enum。Copy なタグ1つ
            self.push(Instruction::I32Const(tag as i32));
            return;
        };
        let slots = match &self.layouts.get(layout).shape {
            wasm_layout::Shape::Enum { variants, .. } => variants[tag as usize].clone(),
            other => unreachable!("enum ではない並びです: {other:?}"),
        };
        let scratch = self.scratch(id);
        let (root, temporary) = (scratch, scratch + 1);
        wasm_data::alloc_root(&mut self.out, self.indices, self.layouts, layout);
        self.out.set(root);
        self.out
            .get(root)
            .num(tag)
            .ins(Instruction::I32Store(wasm_data::tag_word()));
        let payload_types = self.program.variants[variant]
            .payload
            .iter()
            .map(|position| position.ty.clone())
            .collect::<Vec<_>>();
        for ((slot, value), ty) in slots.iter().zip(args).zip(&payload_types) {
            self.install_slot(slot, *value, ty, root, temporary);
        }
        self.out.get(root);
    }

    /// `nil`。Copy な optional はタグと 0 埋めの中身、所有する optional は
    /// tag だけを書いた根1つ(tasks 6.2)
    fn nil(&mut self, id: hir::ExprId) {
        let Some(layout) = self.owned_layout(id) else {
            // 中身が Copy なら平ら。空を表す 0 と、読まれない 0 埋めの中身。
            // `??` と等値がこの 0 埋めを前提にする
            let values = self.produced(id);
            self.push(Instruction::I32Const(wasm_layout::OPTIONAL_EMPTY as i32));
            for value in values.iter().skip(1) {
                match value {
                    ValType::I64 => self.push(Instruction::I64Const(0)),
                    _ => self.push(Instruction::I32Const(0)),
                }
            }
            return;
        };
        let root = self.scratch(id);
        wasm_data::alloc_root(&mut self.out, self.indices, self.layouts, layout);
        self.out.set(root);
        self.out
            .get(root)
            .num(wasm_layout::OPTIONAL_EMPTY)
            .ins(Instruction::I32Store8(wasm_data::tag_byte()));
        self.out.get(root);
    }

    /// struct の値を1つ組み立てる(tasks 5.1)
    fn struct_literal(
        &mut self,
        id: hir::ExprId,
        struct_: hir::StructId,
        fields: &[(hir::FieldId, hir::ExprId)],
    ) {
        let layout = self.owned_layout(id).expect("struct の値は所有を産む");
        let scratch = self.scratch(id);
        let (root, temporary) = (scratch, scratch + 1);
        let declared = self.program.structs[struct_].fields.clone();

        wasm_data::alloc_root(&mut self.out, self.indices, self.layouts, layout);
        self.out.set(root);
        for (field, value) in fields {
            let position = declared
                .iter()
                .position(|d| d == field)
                .expect("宣言に無いフィールド");
            let slot = match &self.layouts.get(layout).shape {
                wasm_layout::Shape::Struct { fields, .. } => fields[position],
                other => unreachable!("struct ではない並びです: {other:?}"),
            };
            let declared = self.program.fields[*field].ty.clone();
            self.install_slot(&slot, *value, &declared, root, temporary);
        }
        self.out.get(root);
    }

    /// 評価した値を区画へ収める。所有の子は一時的な根から中身ごと移す。
    ///
    /// `declared` はその区画の宣言型。optional への格上げと、`indirect` の
    /// `nil` をここで見分ける
    fn install_slot(
        &mut self,
        slot: &wasm_layout::Slot,
        value: hir::ExprId,
        declared: &hir::Type,
        container: u32,
        temporary: u32,
    ) {
        if slot.indirect {
            // `indirect` の子は独立した根のまま。アドレスだけを収める。
            // 0 は `nil`(design.md 決定3)
            self.child_address(slot, value, temporary);
            self.out.set(temporary);
            self.out
                .get(container)
                .offset(slot.offset)
                .get(temporary)
                .store();
            return;
        }
        if self.layouts.get(slot.layout).copy {
            self.value(value, Some(declared));
            let stash = self.stash(value);
            let (layouts, layout, offset) = (&*self.layouts, slot.layout, slot.offset);
            wasm_data::store_copy(&mut self.out, layouts, layout, container, offset, &stash);
            return;
        }
        self.value(value, Some(declared));
        let (indices, layouts) = (self.indices, &*self.layouts);
        wasm_data::relocate_into(
            &mut self.out,
            indices,
            layouts,
            slot.layout,
            container,
            slot.offset,
            temporary,
        );
    }

    /// 平らな値を受け直す席。無ければ確保していないので、要る側だけが呼ぶ
    fn stash(&self, id: hir::ExprId) -> Vec<u32> {
        self.lowered.typed.get(&id).cloned().unwrap_or_default()
    }

    /// `indirect` な区画へ収める子のアドレスを積む(0 は `nil`)。
    ///
    /// 所有権検査を通っている以上、ここへ来る値は一時値か `nil` しかない。
    /// 場所をそのまま指せる形は無いので、包み直しは要らない
    fn child_address(&mut self, slot: &wasm_layout::Slot, value: hir::ExprId, temporary: u32) {
        let optional = self
            .body()
            .expr(value)
            .result
            .ty()
            .is_some_and(|ty| ty.optional);
        // ponytail: `nil` は毎回 optional の根を作ってすぐ捨てることになる。
        // 再帰する形の底なので、ここだけ近道する
        if matches!(self.body().expr(value).kind, hir::ExprKind::Nil) {
            self.push(Instruction::I32Const(0));
            return;
        }
        if !optional {
            if self.layouts.get(slot.layout).copy {
                // Copy な子も独立した割り当てに置く。glue がそう扱う
                let values = self.produced(value);
                self.expr(value, &values);
                let stash = self.stash(value);
                let (indices, layouts, layout) = (self.indices, &*self.layouts, slot.layout);
                wasm_data::alloc_root(&mut self.out, indices, layouts, layout);
                self.out.set(temporary);
                wasm_data::store_copy(&mut self.out, layouts, layout, temporary, 0, &stash);
                self.out.get(temporary);
                return;
            }
            self.expr(value, &[ValType::I32]);
            return;
        }
        // optional の根から中身を取り出して、空の帳簿だけを返す
        self.expr(value, &[ValType::I32]);
        let optional_layout = self
            .owned_layout(value)
            .expect("所有する optional は根を持つ");
        let payload = match self.layouts.get(optional_layout).shape {
            wasm_layout::Shape::Optional { payload } => payload,
            ref other => unreachable!("optional ではない並びです: {other:?}"),
        };
        self.out.tee(temporary);
        self.out.ins(Instruction::I32Load8U(wasm_data::tag_byte()));
        self.out
            .ins(Instruction::If(BlockType::Result(ValType::I32)));
        let (indices, layouts) = (self.indices, &*self.layouts);
        wasm_data::alloc_root(&mut self.out, indices, layouts, payload.layout);
        self.out.set(self.lowered.common);
        self.out.get(self.lowered.common);
        self.out.get(temporary).offset(payload.offset);
        self.out.num(layouts.extent(payload.layout).size);
        self.out.ins(Instruction::MemoryCopy {
            src_mem: 0,
            dst_mem: 0,
        });
        self.out
            .get(temporary)
            .ins(Instruction::Call(self.indices.free));
        self.out.get(self.lowered.common);
        self.out.ins(Instruction::Else);
        self.out
            .get(temporary)
            .ins(Instruction::Call(self.indices.free));
        self.out.num(0);
        self.out.ins(Instruction::End);
    }

    /// フィールドの差し替え。新しい値が出来てから古い中身を落とす(tasks 5.2)
    fn assign_field(
        &mut self,
        id: hir::ExprId,
        recv: hir::ExprId,
        field: hir::FieldId,
        value: hir::ExprId,
    ) {
        let slot = self.slot_of(recv, field);
        let scratch = self.scratch(id);
        let (container, temporary) = (scratch, scratch + 1);
        self.expr(recv, &[ValType::I32]);
        self.out.set(container);

        let declared = self.program.fields[field].ty.clone();
        if self.layouts.get(slot.layout).copy && !slot.indirect {
            self.value(value, Some(&declared));
            let stash = self.stash(value);
            let (layouts, layout, offset) = (&*self.layouts, slot.layout, slot.offset);
            wasm_data::store_copy(&mut self.out, layouts, layout, container, offset, &stash);
            return;
        }

        // 新しい値を先に作る。落としてから作ると、途中の trap で二度落ちる
        if slot.indirect {
            self.child_address(&slot, value, temporary);
        } else {
            self.value(value, Some(&declared));
        }
        self.out.set(temporary);
        let (indices, layouts) = (self.indices, &*self.layouts);
        wasm_data::drop_slot(
            &mut self.out,
            indices,
            layouts,
            &slot,
            container,
            scratch + 2,
        );
        if slot.indirect {
            self.out
                .get(container)
                .offset(slot.offset)
                .get(temporary)
                .store();
            return;
        }
        self.out.get(temporary);
        wasm_data::relocate_into(
            &mut self.out,
            indices,
            layouts,
            slot.layout,
            container,
            slot.offset,
            temporary,
        );
    }

    /// 直接置かれた/`indirect` なフィールドを消費する(tasks 5.4)。
    ///
    /// 器の残りを落とすのは計画が出す残余の破棄。ここは選ばれた値を独立させ、
    /// 二度落とされない形にするところまで
    fn consume_field(&mut self, id: hir::ExprId, recv: hir::ExprId, field: hir::FieldId) {
        let slot = self.slot_of(recv, field);
        let scratch = self.scratch(id);
        let (container, fresh) = (scratch, scratch + 1);
        self.expr(recv, &[ValType::I32]);

        if slot.indirect {
            self.out.set(container);
            self.out.get(container).offset(slot.offset).load();
            self.out.set(fresh);
            // 器の側を空にする。残余の破棄がここをもう一度返さないように
            self.out.get(container).offset(slot.offset).num(0).store();
            if !slot.nullable {
                // 非 optional の `indirect` は子の根がそのまま値になる
                self.out.get(fresh);
                return;
            }
            let optional = self
                .owned_layout(id)
                .expect("`indirect` な optional は optional の値になる");
            self.wrap_child(optional, fresh, container);
            return;
        }
        let (indices, layouts) = (self.indices, &*self.layouts);
        wasm_data::extract_root(
            &mut self.out,
            indices,
            layouts,
            slot.layout,
            container,
            slot.offset,
            fresh,
        );
    }

    /// `indirect` な optional の区画から取り出した子アドレスを、optional の
    /// 値へ包み直す(`child_address` の逆)。
    ///
    /// 記憶の上では「アドレス、0 なら `nil`」だが、値としての `T?` は
    /// `{tag, 詰め物, payload}` の根1つ(design.md 決定3)。読み出しはその
    /// 食い違いをここで埋める。所有はそのまま移るので、空になった子の根だけ返す
    fn wrap_child(&mut self, optional: LayoutId, child: u32, root: u32) {
        let payload = match self.layouts.get(optional).shape {
            wasm_layout::Shape::Optional { payload } => payload,
            ref other => unreachable!("optional ではない並びです: {other:?}"),
        };
        let size = self.layouts.extent(payload.layout).size;
        let (indices, layouts) = (self.indices, &*self.layouts);
        wasm_data::alloc_root(&mut self.out, indices, layouts, optional);
        self.out.set(root);

        self.out.get(child);
        self.out.ins(Instruction::If(BlockType::Empty));
        self.out
            .get(root)
            .num(wasm_layout::OPTIONAL_PRESENT)
            .ins(Instruction::I32Store8(wasm_data::tag_byte()));
        self.out.get(root).offset(payload.offset);
        self.out.get(child).num(size);
        self.out.ins(Instruction::MemoryCopy {
            src_mem: 0,
            dst_mem: 0,
        });
        self.out.get(child).ins(Instruction::Call(indices.free));
        self.out.ins(Instruction::Else);
        self.out
            .get(root)
            .num(wasm_layout::OPTIONAL_EMPTY)
            .ins(Instruction::I32Store8(wasm_data::tag_byte()));
        self.out.ins(Instruction::End);
        self.out.get(root);
    }

    /// 握ったままの一時値を、内側から順に返す。
    ///
    /// 消費する `for` の途中で `return` すると、渡し終えていない要素と buffer と
    /// 根が宙に浮く。渡した区画は 0 埋めしてあるので、ここで落としても二度には
    /// ならない
    fn release_live(&mut self) {
        for (layout, slot) in self.live.clone().into_iter().rev() {
            let (indices, glue) = (self.indices, self.indices.of(layout));
            wasm_data::drop_root(&mut self.out, indices, glue, slot);
        }
        self.release_provisions_from(0);
    }

    /// 配列リテラル(tasks 7.1)。
    ///
    /// 帳簿と要素の buffer は別の割り当て。値はソースの順に評価して、要素の
    /// 刻み幅で並べる
    fn array_literal(&mut self, id: hir::ExprId, elements: &[hir::ExprId]) {
        let layout = self.owned_layout(id).expect("配列は所有を産む");
        let (element, stride) = self.array_parts(layout);
        let element_ty = self.element_type(id);
        let scratch = self.scratch(id);
        let (root, data, temporary) = (scratch, scratch + 1, scratch + 2);

        // 要素数はソースに書かれた個数。刻みを掛けても 32bit を越えようが無い
        let count = elements.len() as u32;
        let bytes = wasm_layout::checked_mul(count, stride).expect("リテラルの長さは収まる");

        let (indices, layouts) = (self.indices, &*self.layouts);
        wasm_data::alloc_root(&mut self.out, indices, layouts, layout);
        self.out.set(root);
        self.out.num(bytes).ins(Instruction::Call(indices.alloc));
        self.out.set(data);
        self.out
            .get(root)
            .get(data)
            .ins(Instruction::I32Store(wasm_data::buffer_data()));
        for at in [wasm_data::buffer_len(), wasm_data::buffer_capacity()] {
            self.out.get(root).num(count).ins(Instruction::I32Store(at));
        }

        for (position, value) in elements.iter().enumerate() {
            let slot = wasm_layout::Slot {
                offset: position as u32 * stride,
                layout: element,
                indirect: false,
                nullable: false,
            };
            self.install_slot(&slot, *value, &element_ty, data, temporary);
        }
        self.out.get(root);
    }

    /// 組み込みの `push`(MAP-075 決定3)。
    ///
    /// レシーバは検査器が挿した `&mut [T]` なので、評価すると帳簿のアドレスが
    /// 1つ載る。容量は並びごとの `reserve` に任せ、ここは「次の席を求めて要素を
    /// 収め、長さを1つ進める」だけ。席の求め方は `reserve` の後でなければ
    /// ならない — 伸びると `data` が別のアドレスへ移る
    fn push_element(&mut self, id: hir::ExprId, array: hir::ExprId, value: hir::ExprId) {
        let layout = self
            .compound_layout(array)
            .expect("`push` のレシーバは配列を指す");
        let (element, stride) = self.array_parts(layout);
        let element_ty = self.element_type(array);
        let scratch = self.scratch(id);
        let (root, seat, temporary) = (scratch, scratch + 1, scratch + 2);

        let want = self.produced(array);
        self.expr(array, &want);
        self.out.set(root);
        self.push(Instruction::LocalGet(root));
        self.push(Instruction::Call(self.indices.reserve_of(layout)));

        self.out
            .get(root)
            .ins(Instruction::I32Load(wasm_data::buffer_data()));
        self.out
            .get(root)
            .ins(Instruction::I32Load(wasm_data::buffer_len()));
        if stride != 1 {
            self.out.num(stride).ins(Instruction::I32Mul);
        }
        self.out.ins(Instruction::I32Add).set(seat);

        let slot = wasm_layout::Slot {
            offset: 0,
            layout: element,
            indirect: false,
            nullable: false,
        };
        self.install_slot(&slot, value, &element_ty, seat, temporary);

        self.out
            .get(root)
            .get(root)
            .ins(Instruction::I32Load(wasm_data::buffer_len()))
            .num(1)
            .ins(Instruction::I32Add)
            .ins(Instruction::I32Store(wasm_data::buffer_len()));
    }

    /// 配列の並びから、要素の並びと刻み幅
    fn array_parts(&mut self, layout: LayoutId) -> (LayoutId, u32) {
        match self.layouts.get(layout).shape {
            wasm_layout::Shape::Array { element, stride } => (element, stride),
            ref other => unreachable!("配列ではない並びです: {other:?}"),
        }
    }

    /// 宣言された要素型。optional 注入があるので、書いた値の型では足りない
    fn element_type(&self, id: hir::ExprId) -> hir::Type {
        match self.body().expr(id).result.ty().map(|ty| &ty.kind) {
            Some(hir::TypeKind::Array(element)) => (**element).clone(),
            other => unreachable!("配列ではない型です: {other:?}"),
        }
    }

    /// `for`(tasks 7.3〜7.5)。
    ///
    /// 対象は一度だけ評価して局所に置く。借りて回す周回は要素の場所をそのまま
    /// 束ね、消費する周回は要素を独立した根へ移して跡を空にする — 空にした
    /// 区画は配列の drop が何もしないので、途中で抜けても二度落ちない
    fn for_expr(
        &mut self,
        id: hir::ExprId,
        var: hir::LocalId,
        iter: hir::ExprId,
        inner: hir::ExprId,
    ) {
        let scratch = self.scratch(id);
        let (arr, data, len, index, elem, fresh) = (
            scratch,
            scratch + 1,
            scratch + 2,
            scratch + 3,
            scratch + 4,
            scratch + 5,
        );

        self.expr(iter, &[ValType::I32]);
        self.out.set(arr);
        let layout = self.compound_layout(iter).expect("配列はアドレスで運ぶ");
        let (element, stride) = self.array_parts(layout);
        // 一時値なら歩き終えたあとに返す。借りているだけなら持ち主は元のまま
        let holds = self.temporary(iter).is_some();
        // 所有を周回へ渡すのは `move` を書いたときだけ(design.md 決定7)。
        // 一時値の配列でも、素で回せば要素は借りたまま
        let consuming = matches!(
            self.plan.access(iter).map(|access| access.mode),
            Some(crate::ownership::Mode::Move)
        );

        self.out
            .get(arr)
            .ins(Instruction::I32Load(wasm_data::buffer_data()))
            .set(data);
        self.out
            .get(arr)
            .ins(Instruction::I32Load(wasm_data::buffer_len()))
            .set(len);
        self.out.num(0).set(index);

        if holds {
            self.live.push((layout, arr));
        }
        self.push(Instruction::Block(BlockType::Empty));
        self.push(Instruction::Loop(BlockType::Empty));
        self.out.get(index).get(len).ins(Instruction::I32GeU);
        self.push(Instruction::BrIf(1));

        wasm_data::element_address(&mut self.out, data, index, stride);
        self.out.set(elem);
        self.bind_element(var, element, elem, fresh, consuming);

        self.expr(inner, &[]);
        self.cleanup(crate::ownership::Exit::LoopBack(id));
        self.out
            .get(index)
            .num(1)
            .ins(Instruction::I32Add)
            .set(index);
        self.push(Instruction::Br(0));
        self.push(Instruction::End);
        self.push(Instruction::End);
        self.cleanup(crate::ownership::Exit::LoopExit(id));

        if holds {
            self.live.pop();
            let (indices, glue) = (self.indices, self.indices.of(layout));
            wasm_data::drop_root(&mut self.out, indices, glue, arr);
        }
    }

    /// 周回変数へ要素を置く。
    ///
    /// Copy は読み出し、借りて回すなら要素の場所そのもの、消費するなら独立
    /// させた根。消費した区画は 0 で埋めるので、配列の drop がそこを飛ばす
    fn bind_element(
        &mut self,
        var: hir::LocalId,
        element: LayoutId,
        elem: u32,
        fresh: u32,
        consuming: bool,
    ) {
        let seats = self.lowered.slots.get(&var).cloned().unwrap_or_default();
        if self.layouts.get(element).copy {
            if seats.is_empty() {
                return;
            }
            let layouts = &*self.layouts;
            wasm_data::load_copy(&mut self.out, layouts, element, elem, 0);
            for seat in seats.iter().rev() {
                self.out.set(*seat);
            }
            return;
        }
        if seats.is_empty() {
            return;
        }
        if !consuming {
            self.out.get(elem).set(seats[0]);
            return;
        }
        let size = self.layouts.extent(element).size;
        let (indices, layouts) = (self.indices, &*self.layouts);
        wasm_data::alloc_root(&mut self.out, indices, layouts, element);
        self.out.set(fresh);
        self.out.get(fresh).get(elem).num(size);
        self.out.ins(Instruction::MemoryCopy {
            src_mem: 0,
            dst_mem: 0,
        });
        // 渡した跡を空にする。0 埋めの区画は drop が何も解放しない
        self.out.get(elem).num(0).num(size);
        self.out.ins(Instruction::MemoryFill(0));
        self.out.get(fresh).set(seats[0]);
        if let Some(flag) = self.lowered.flags.get(&var).copied() {
            wasm_data::mark_initialized(&mut self.out, flag);
        }
    }

    /// `lhs ?? rhs`(tasks 6.4)。
    ///
    /// 左辺が空のときだけ右辺が走る。結果は必ず**持ち主のこちらにある値**に
    /// する。左辺を借りているだけの `??` は中身を複製するので、片方の経路だけ
    /// 借用・もう片方だけ所有、という揺れが起きない
    fn coalesce(&mut self, id: hir::ExprId, lhs: hir::ExprId, rhs: hir::ExprId) {
        let result_ty = self.body().expr(id).result.ty().cloned();
        let want = self.produced(id);
        // Copy な optional は平ら。tag を席へ受け直してから枝を選ぶ
        if !matches!(self.kind_of(lhs), Some(ReprKind::Owned(_))) {
            let stash = self.stash(lhs);
            let values = self.produced(lhs);
            self.expr(lhs, &values);
            for slot in stash.iter().rev() {
                self.out.set(*slot);
            }
            self.out.get(stash[0]);
            let block = self.block_type(&want);
            self.push(Instruction::If(block));
            for slot in stash.iter().skip(1) {
                self.out.get(*slot);
            }
            self.push(Instruction::Else);
            self.value(rhs, result_ty.as_ref());
            self.push(Instruction::End);
            return;
        }

        let optional = self
            .owned_layout(lhs)
            .expect("所有する optional は根を持つ");
        let payload = match self.layouts.get(optional).shape {
            wasm_layout::Shape::Optional { payload } => payload,
            ref other => unreachable!("optional ではない並びです: {other:?}"),
        };
        let owning = self.temporary(lhs).is_some();
        let scratch = self.scratch(id);
        let (root, fresh) = (scratch, scratch + 1);

        self.expr(lhs, &[ValType::I32]);
        self.out.tee(root);
        self.out.ins(Instruction::I32Load8U(wasm_data::tag_byte()));
        let block = self.block_type(&want);
        self.push(Instruction::If(block));
        {
            let (indices, layouts) = (self.indices, &*self.layouts);
            wasm_data::alloc_root(&mut self.out, indices, layouts, payload.layout);
            self.out.set(fresh);
            if owning {
                // 中身ごと引き取って、空になった帳簿を返す
                self.out.get(fresh);
                self.out.get(root).offset(payload.offset);
                self.out.num(layouts.extent(payload.layout).size);
                self.out.ins(Instruction::MemoryCopy {
                    src_mem: 0,
                    dst_mem: 0,
                });
                self.out.get(root).ins(Instruction::Call(self.indices.free));
            } else {
                // 借りているだけなので写す。所有者は元のまま
                self.out.get(root).offset(payload.offset);
                self.out.get(fresh);
                self.out
                    .ins(Instruction::Call(self.indices.of(payload.layout).clone));
            }
            self.out.get(fresh);
        }
        self.push(Instruction::Else);
        {
            if owning {
                // 空の帳簿。中身は無いので根だけ返す
                self.out.get(root).ins(Instruction::Call(self.indices.free));
            }
            self.value(rhs, result_ty.as_ref());
            // 右辺が場所なら、結果を持ち主のある値へ揃えるために複製する
            if self.temporary(rhs).is_none()
                && let Some(layout) = self.owned_layout(rhs)
            {
                let (indices, layouts) = (self.indices, &*self.layouts);
                wasm_data::alloc_root(&mut self.out, indices, layouts, layout);
                self.out.tee(fresh);
                self.push(Instruction::Call(self.indices.of(layout).clone));
                self.out.get(fresh);
            }
        }
        self.push(Instruction::End);
    }

    /// `match`(tasks 6.5)。
    ///
    /// 対象は一度だけ評価して局所に置く。arm は宣言順に tag を試し、guard が
    /// 偽なら次へ落ちる。消費する対象では、選ばれた arm が payload を独立させ、
    /// 束ねなかった区画を落としてから器の根を返す
    fn match_expr(&mut self, id: hir::ExprId, subject: hir::ExprId, arms: &[Arm]) {
        let want = self.produced(id);
        let result_ty = self.body().expr(id).result.ty().cloned();
        let scratch = self.scratch(id);
        let (subj, fresh, scan) = (scratch, scratch + 1, scratch + 2);

        self.expr(subject, &[ValType::I32]);
        self.out.set(subj);
        // 対象が payload を持たない enum なら、値そのものがタグ
        let flat = matches!(self.kind_of(subject), Some(ReprKind::Flat));
        // 借りた対象も並びは要る。`match &mut l` の payload は器の中に居る
        let subject_layout = self.compound_layout(subject);
        // 所有を持ち込んだか、payload まで渡すのかは検査済みのアクセスが決める
        let holds = self.temporary(subject).is_some();
        let hands_over = matches!(
            self.plan.access(subject).map(|access| access.mode),
            Some(crate::ownership::Mode::Move)
        );

        let block = self.block_type(&want);
        self.push(Instruction::Block(block));
        for (index, arm) in arms.iter().enumerate() {
            self.push(Instruction::Block(BlockType::Empty));
            if let Some((variant, bindings)) = &arm.pattern {
                let tag = self.variant_tag(*variant);
                if flat {
                    self.out.get(subj);
                } else {
                    self.out
                        .get(subj)
                        .ins(Instruction::I32Load(wasm_data::tag_word()));
                }
                self.out.num(tag).ins(Instruction::I32Ne);
                self.push(Instruction::BrIf(0));
                let slots = self.payload_slots(subject_layout, tag);
                // guard は束縛を読めるので、まず借りた形で置く
                for (position, bound) in bindings.iter().enumerate() {
                    let Some(local) = bound else { continue };
                    self.bind_payload(*local, &slots[position], subj);
                }
                if let Some(guard) = arm.guard {
                    self.expr(guard, WANT_BOOL);
                    self.push(Instruction::I32Eqz);
                    self.push(Instruction::If(BlockType::Empty));
                    self.cleanup(crate::ownership::Exit::MatchGuard { at: id, arm: index });
                    self.push(Instruction::Br(1));
                    self.push(Instruction::End);
                }
                if hands_over {
                    self.hand_over_payloads(bindings, &slots, subj, fresh, scan);
                }
            } else if hands_over || holds {
                // catch-all は中身を受け取らない。器はまだ全部持っている
                if let Some(layout) = subject_layout
                    && hands_over
                {
                    let (indices, glue) = (self.indices, self.indices.of(layout));
                    wasm_data::drop_root(&mut self.out, indices, glue, subj);
                }
            }
            self.value(arm.body, result_ty.as_ref());
            self.cleanup(crate::ownership::Exit::MatchArm { at: id, arm: index });
            self.push(Instruction::Br(1));
            self.push(Instruction::End);
        }
        // どの arm も取らなかった。網羅していれば到達しないし、guard で全部
        // 外れたときはインタプリタも実行時に失敗する
        self.push(Instruction::Unreachable);
        self.push(Instruction::End);

        // 借りた形で回した一時値は、arm を抜けてから返す
        if holds
            && !hands_over
            && let Some(layout) = subject_layout
        {
            let (indices, glue) = (self.indices, self.indices.of(layout));
            wasm_data::drop_root(&mut self.out, indices, glue, subj);
        }
    }

    /// その variant の payload 区画
    fn payload_slots(&mut self, subject: Option<LayoutId>, tag: u32) -> Vec<wasm_layout::Slot> {
        let Some(layout) = subject else {
            return Vec::new();
        };
        match &self.layouts.get(layout).shape {
            wasm_layout::Shape::Enum { variants, .. } => variants[tag as usize].clone(),
            other => unreachable!("enum ではない並びです: {other:?}"),
        }
    }

    /// payload を借りた形で束縛へ置く。所有はまだ器のまま
    fn bind_payload(&mut self, local: hir::LocalId, slot: &wasm_layout::Slot, subj: u32) {
        let seats = self.lowered.slots.get(&local).cloned().unwrap_or_default();
        if seats.is_empty() {
            return;
        }
        self.read_slot(slot, subj);
        for seat in seats.iter().rev() {
            self.out.set(*seat);
        }
    }

    /// 消費する `match` で、選ばれた arm へ payload の所有を渡す(tasks 6.5)。
    ///
    /// 束ねた区画は独立した根へ移し、束ねなかった区画は落とす。最後に器の根
    /// だけを返す — 中身はもう器のものではないので glue は通さない
    fn hand_over_payloads(
        &mut self,
        bindings: &[Option<hir::LocalId>],
        slots: &[wasm_layout::Slot],
        subj: u32,
        fresh: u32,
        scan: u32,
    ) {
        for (position, slot) in slots.iter().enumerate() {
            let bound = bindings.get(position).copied().flatten();
            let Some(local) = bound else {
                // `_` で受けた区画は誰のものにもならないので落とす
                let (indices, layouts) = (self.indices, &*self.layouts);
                wasm_data::drop_slot(&mut self.out, indices, layouts, slot, subj, scan);
                continue;
            };
            if self.layouts.get(slot.layout).copy && !slot.indirect {
                // Copy は借りた形のまま値を持っている。移すものが無い
                continue;
            }
            let seats = self.lowered.slots.get(&local).cloned().unwrap_or_default();
            if slot.indirect {
                // 既に子の根を指している。器の側を空にして所有を渡す
                self.out.get(subj).offset(slot.offset).num(0).store();
            } else {
                self.out.get(subj);
                let (indices, layouts) = (self.indices, &*self.layouts);
                wasm_data::extract_root(
                    &mut self.out,
                    indices,
                    layouts,
                    slot.layout,
                    subj,
                    slot.offset,
                    fresh,
                );
                self.out.set(seats[0]);
            }
            if let Some(flag) = self.lowered.flags.get(&local).copied() {
                wasm_data::mark_initialized(&mut self.out, flag);
            }
        }
        self.out.get(subj).ins(Instruction::Call(self.indices.free));
    }

    /// 射影を消費した直後の残余を出す。取り出した値は stack に載ったままだが、
    /// 落とすのは器のほうなので釣り合いは崩れない
    fn residue(&mut self, at: hir::ExprId) {
        let drops: Vec<crate::ownership::Drop> = self.plan.residue(at).to_vec();
        for drop in drops {
            if let crate::ownership::Drop::Remaining { root, consumed } = drop {
                self.drop_remaining(root, &consumed);
            }
        }
    }

    /// 射影を消費した器の、残り全部を落とす(tasks 3.5)。
    ///
    /// 消費した**経路**を飛ばすので、`move u.inner.name` でも持ち出された
    /// `name` を二度落とさない。最後に器の根そのものを返す
    fn drop_remaining(&mut self, root: hir::LocalId, consumed: &[crate::ownership::Projection]) {
        let (Some(flag), Some(layout)) = (
            self.lowered.flags.get(&root).copied(),
            self.lowered.owned.get(&root).copied(),
        ) else {
            return;
        };
        let address = self.lowered.slots[&root][0];
        self.out.get(flag);
        self.out.ins(Instruction::If(BlockType::Empty));
        self.drop_around(layout, address, consumed, 0);
        self.out
            .get(address)
            .ins(Instruction::Call(self.indices.free));
        wasm_data::mark_moved(&mut self.out, flag);
        self.out.ins(Instruction::End);
    }

    /// `container` が指す並びの、`consumed` の先頭が指す区画**以外**を落とす。
    /// 経路が続くならその区画へ潜る
    fn drop_around(
        &mut self,
        layout: LayoutId,
        container: u32,
        consumed: &[crate::ownership::Projection],
        level: u32,
    ) {
        let slots = match &self.layouts.get(layout).shape {
            wasm_layout::Shape::Struct { fields, .. } => fields.clone(),
            // 消費できる射影を持つのは、いまは struct だけ
            other => unreachable!("射影を消費できない並びです: {other:?}"),
        };
        let skipped = match consumed.first() {
            Some(crate::ownership::Projection::Field(field)) => {
                let owner = self.program.fields[*field].owner;
                self.program.structs[owner]
                    .fields
                    .iter()
                    .position(|declared| declared == field)
            }
            // 経路が尽きたか、struct 以外の射影。残り全部を落とす
            _ => None,
        };

        let scan = self.lowered.common;
        for (position, slot) in slots.iter().enumerate() {
            if Some(position) != skipped {
                let (indices, layouts) = (self.indices, &*self.layouts);
                wasm_data::drop_slot(&mut self.out, indices, layouts, slot, container, scan);
                continue;
            }
            let rest = &consumed[1..];
            if rest.is_empty() {
                // ここが持ち出された区画。触らない
                continue;
            }
            let inner = self.lowered.common + 1 + level;
            if slot.indirect {
                self.out
                    .get(container)
                    .offset(slot.offset)
                    .load()
                    .set(inner);
                self.drop_around(slot.layout, inner, rest, level + 1);
                self.out
                    .get(inner)
                    .ins(Instruction::Call(self.indices.free));
            } else {
                self.out.get(container).offset(slot.offset).set(inner);
                self.drop_around(slot.layout, inner, rest, level + 1);
            }
        }
    }

    /// 束縛の宣言型。読めない束縛(初期化子が発散した `let`)は `None`
    fn local_type(&self, local: hir::LocalId) -> Option<hir::Type> {
        self.body().local(local).ty.clone()
    }

    /// 値を1つも産まない・1つだけ産むブロックは即値で書ける。複数の値を
    /// 産むブロックだけが関数型の番号を要る(multi-value)
    fn block_type(&mut self, want: &[ValType]) -> BlockType {
        match want {
            [] => BlockType::Empty,
            [one] => BlockType::Result(*one),
            many => BlockType::FunctionType(self.types.intern(&[], many)),
        }
    }
}

// ---------------------------------------------------------------------------
// モジュールの組み立て
// ---------------------------------------------------------------------------

/// 公開面が境界で運ぶ値1つ。
///
/// scalar は ABI v0 の表現のまま。所有型は直列化した bytes を指す
/// `(i32 ptr, i32 len)` になる(design.md 決定8)
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Port {
    Scalar(Scalar),
    Rich(hir::Type),
}

impl Port {
    /// 境界で占める Core Wasm の値
    pub fn val_types(&self) -> Vec<ValType> {
        match self {
            Port::Scalar(scalar) => scalar.val_type().into_iter().collect(),
            Port::Rich(_) => vec![ValType::I32, ValType::I32],
        }
    }

    pub fn rich(&self) -> Option<&hir::Type> {
        match self {
            Port::Rich(ty) => Some(ty),
            Port::Scalar(_) => None,
        }
    }
}

/// 公開面1つ分の署名。ABI メタデータと入口の検査に使う
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Signature {
    pub name: String,
    pub params: Vec<Port>,
    pub result: Port,
}

/// 計画された instance を Core Wasm モジュールへ落とす。
///
/// 呼ぶ前に `check_support` と ABI 署名の検査を通しておくこと。ここは
/// 「通った計画を決定的な bytes にする」ことだけを受け持つ
pub fn emit(checked: &CheckedProgram, production: &ProductionPlan) -> Result<Vec<u8>, Vec<Diag>> {
    let plan_errors = crate::wasm_ambient::validate_plan(&checked.hir, &production.plan);
    if !plan_errors.is_empty() {
        return Err(plan_errors);
    }
    let signatures = crate::wasm_abi::signatures(checked, production)?;
    Ok(build(checked, production, &signatures))
}

fn build(
    checked: &CheckedProgram,
    production: &ProductionPlan,
    signatures: &crate::wasm_abi::Signatures,
) -> Vec<u8> {
    let program = &checked.hir;
    let plan = &production.plan;
    // 到達した型の並びはここで1つの表に溜める。番号は要求順なので決定的
    let mut layouts = Layouts::default();
    plan_reachable(&mut layouts, program, plan);
    let lowered: Vec<Lowered> = plan
        .instances()
        .map(|(_, instance)| lower_instance_signature(&mut layouts, program, plan, instance))
        .collect();

    // 所有データが1つも到達していないなら、ランタイムも glue も載せない。
    // scalar だけのモジュールのバイト列はこの change の前と変わらない
    let owned_data = layouts.planned().any(|(_, layout)| !layout.copy);
    let statics = collect_literals(program, plan);
    let wrappers = signatures.wrappers().count() as u32;
    let runtime_base = lowered.len() as u32 + wrappers;
    let glue_base = runtime_base + wasm_runtime::COUNT;
    let mut indices = Indices::reserve(&layouts, glue_base);
    indices.alloc = runtime_base + wasm_runtime::ALLOC;
    indices.free = runtime_base + wasm_runtime::FREE;
    // 配列の `reserve` は glue の後ろ。`push` が届いた並びにだけ出す
    // (MAP-075 決定3)
    let reserve_base = glue_base + indices.glue_count();
    let pushed = pushed_layouts(&mut layouts, program, plan);
    indices.reserve_arrays(&layouts, reserve_base, &pushed);
    let helper_end = reserve_base + indices.reserve_count();

    // 公開署名に出た型だけが直列化関数を持つ。内部の型は境界に出ない
    let version = signatures.version();
    let roots: Vec<LayoutId> = signatures
        .rich_types()
        .iter()
        .map(|ty| layouts.plan(program, ty).expect("公開型は 32bit に収まる"))
        .collect();
    let wires = wasm_wire::Wires::reserve(&layouts, &roots, helper_end);
    let reserve_index = runtime_base + wasm_runtime::RESERVE;

    let mut types = Types::default();
    let mut functions = FunctionSection::new();
    let mut code = CodeSection::new();

    for (index, (_, instance)) in plan.instances().enumerate() {
        let lowered = &lowered[index];
        functions.function(types.intern(&lowered.params, &lowered.results));

        let locals = lowered
            .extra_locals
            .iter()
            .map(|val_type| (1, *val_type))
            .collect::<Vec<_>>();
        let mut emitter = Emitter {
            program,
            instance,
            specializations: plan,
            lowered,
            plan: checked.plan.body(hir::BodyId::Callable(lowered.callable)),
            layouts: &mut layouts,
            types: &mut types,
            indices: &indices,
            statics: &statics.at,
            live: Vec::new(),
            live_provisions: Vec::new(),
            out: Body::with_locals(locals),
        };
        // 引数は呼ばれた時点で所有を得ている。局所の初期値は 0 なので、
        // 持っていることを明示的に立てる(tasks 3.4)
        for local in program.callables[lowered.callable]
            .body
            .receiver
            .into_iter()
            .chain(program.callables[lowered.callable].params.iter().copied())
        {
            if let Some(flag) = lowered.flags.get(&local).copied() {
                wasm_data::mark_initialized(&mut emitter.out, flag);
            }
        }
        let root = program.callables[lowered.callable].body.root.clone();
        let want = program.callables[lowered.callable].ret.clone();
        emitter.sequence(&root, Some(&want));
        emitter.cleanup(crate::ownership::Exit::Fallthrough);
        // `finish` が関数の `end` を付ける
        code.function(&emitter.out.finish());
    }

    // ラッパは実装関数の後ろ。エントリ、続いて公開名の昇順
    let mut exports = ExportSection::new();
    for (next_index, (signature, instance)) in (lowered.len() as u32..).zip(signatures.wrappers()) {
        let params: Vec<ValType> = signature.params.iter().flat_map(Port::val_types).collect();
        let results: Vec<ValType> = signature.result.val_types();
        functions.function(types.intern(&params, &results));
        code.function(&wrapper(
            Boundary {
                program,
                layouts: &mut layouts,
                wires: &wires,
                indices: &indices,
                reserve: reserve_index,
            },
            signature,
            instance.index() as u32,
        ));
        exports.export(&signature.name, ExportKind::Func, next_index);
    }
    if version >= 1 {
        exports.export(crate::wasm_abi::MEMORY_EXPORT, ExportKind::Memory, 0);
        exports.export(
            crate::wasm_abi::RESERVE_EXPORT,
            ExportKind::Func,
            reserve_index,
        );
    }

    // ランタイムと glue は公開しない実装の後ろ。番号は `indices` の予約と同じ順
    let runtime =
        owned_data.then(|| Runtime::new(&statics.bytes).expect("静的データは 32bit に収まる"));
    if runtime.is_some() {
        for helper in Runtime::helpers(runtime_base)
            .into_iter()
            .chain(wasm_data::glue_functions(&layouts, &indices))
            .chain(wasm_data::reserve_functions(&layouts, &indices))
            .chain(wasm_wire::wire_functions(
                program, &layouts, &wires, &indices,
            ))
        {
            functions.function(types.intern(&helper.params, &helper.results));
            code.function(&helper.body);
        }
    }

    let mut module = Module::new();
    module.section(&types.section);
    module.section(&functions);
    if let Some(runtime) = &runtime {
        module.section(&runtime.memory_section());
    }
    module.section(&exports);
    module.section(&code);
    if let Some(runtime) = &runtime {
        module.section(&runtime.data_section());
    }
    module.section(&CustomSection {
        name: ABI_SECTION.into(),
        data: signatures.metadata(program).into_bytes().into(),
    });

    module.finish()
}

/// ラッパを出すのに要る、モジュール側の道具立て
struct Boundary<'a> {
    program: &'a hir::Program,
    layouts: &'a mut Layouts,
    wires: &'a wasm_wire::Wires,
    indices: &'a Indices,
    /// `__rhodolite_abi_reserve` の関数番号
    reserve: u32,
}

/// ホスト境界のラッパ1つ。
///
/// scalar はそのまま渡す。bool 引数は 0/1 だけを受け、本体へ入る前に trap
/// するので、内部の関数はどこも「bool は 0 か 1」を前提にできる。
///
/// 豊かな値は `(ptr, len)` で受け取り、**本体が走る前に**受け渡し領域の中に
/// あることを確かめてから復号する。復号は新しい所有を作るので、その後にホストが
/// 同じ bytes を書き換えても中身は動かない。豊かな戻り値は領域を取り直して
/// 符号化し、内部の持ち主はそこで落とす(rhodolite-wasm-abi spec)
fn wrapper(at: Boundary<'_>, signature: &Signature, target: u32) -> Function {
    let rich: Vec<&hir::Type> = signature.params.iter().filter_map(Port::rich).collect();
    let flat: Vec<ValType> = signature.params.iter().flat_map(Port::val_types).collect();
    let base = flat.len() as u32;
    // 復号した引数の根、続いて結果の根・大きさ・領域・作業用。scalar だけの
    // 署名は1つも取らないので、v0 のモジュールはバイト列が変わらない
    let extra = if signature.ports().any(|port| port.rich().is_some()) {
        rich.len() as u32 + 4
    } else {
        0
    };
    let mut b = if extra == 0 {
        Body::with_locals(Vec::new())
    } else {
        Body::with_locals(vec![(extra, ValType::I32)])
    };
    let (result_root, size, area, scratch) = (
        base + rich.len() as u32,
        base + rich.len() as u32 + 1,
        base + rich.len() as u32 + 2,
        base + rich.len() as u32 + 3,
    );

    // bool は 0/1 以外を受けない
    let mut slot = 0u32;
    for port in &signature.params {
        if let Port::Scalar(Scalar::Bool) = port {
            b.get(slot).num(2).ins(Instruction::I32GeU);
            b.trap_if();
        }
        slot += port.val_types().len() as u32;
    }

    // 豊かな引数は、渡された slice が受け渡し領域に収まっているかを先に見る
    let mut pointer = 0u32;
    let mut which = 0usize;
    for port in &signature.params {
        let width = port.val_types().len() as u32;
        if let Port::Rich(ty) = port {
            let layout = at.layouts.plan(at.program, ty).expect("公開型は収まる");
            let root = base + which as u32;
            wasm_wire::check_slice(
                &mut b,
                wasm_runtime::EXCHANGE,
                pointer,
                pointer + 1,
                scratch,
            );
            // 復号先は新しい割り当て。ホストの bytes とは記憶を共有しない
            wasm_data::alloc_root(&mut b, at.indices, at.layouts, layout);
            b.set(root);
            b.get(pointer);
            b.get(pointer).get(pointer + 1).ins(Instruction::I32Add);
            b.get(root);
            b.ins(Instruction::Call(at.wires.of(layout).decode));
            // 余りが残る並びは正準ではない
            b.get(pointer)
                .get(pointer + 1)
                .ins(Instruction::I32Add)
                .ins(Instruction::I32Ne);
            b.trap_if();
            which += 1;
        }
        pointer += width;
    }

    // 本体へ渡す形へ組み直す
    let mut slot = 0u32;
    let mut which = 0usize;
    for port in &signature.params {
        let width = port.val_types().len() as u32;
        match port {
            Port::Scalar(_) => {
                for offset in 0..width {
                    b.get(slot + offset);
                }
            }
            Port::Rich(ty) => {
                let layout = at.layouts.plan(at.program, ty).expect("公開型は収まる");
                let root = base + which as u32;
                match at
                    .layouts
                    .repr(at.program, ty)
                    .expect("公開型は収まる")
                    .kind
                {
                    // 所有する値は根のアドレスがそのまま内部の表現
                    ReprKind::Owned(_) => b.get(root),
                    // Copy な値は平ら。読み出したら帳簿は要らない
                    _ => {
                        let layouts = &*at.layouts;
                        wasm_data::load_copy(&mut b, layouts, layout, root, 0);
                        b.get(root).ins(Instruction::Call(at.indices.free))
                    }
                };
                which += 1;
            }
        }
        slot += width;
    }
    b.ins(Instruction::Call(target));

    match &signature.result {
        Port::Scalar(Scalar::Bool) => {
            // 内部の bool は構成上 0/1 だが、公開する値は境界でも正規化する
            b.ins(Instruction::I32Eqz).ins(Instruction::I32Eqz);
        }
        Port::Scalar(_) => {}
        Port::Rich(ty) => {
            let ty = ty.clone();
            let layout = at.layouts.plan(at.program, &ty).expect("公開型は収まる");
            let owned = matches!(
                at.layouts
                    .repr(at.program, &ty)
                    .expect("公開型は収まる")
                    .kind,
                ReprKind::Owned(_)
            );
            if owned {
                b.set(result_root);
            } else {
                // 平らな値はいったん帳簿へ置く。符号化は記憶の上で行う
                let stash: Vec<u32> = Vec::new();
                wasm_data::alloc_root(&mut b, at.indices, at.layouts, layout);
                b.set(result_root);
                let layouts = &*at.layouts;
                wasm_data::store_copy(&mut b, layouts, layout, result_root, 0, &stash);
            }
            b.get(result_root)
                .ins(Instruction::Call(at.wires.of(layout).size))
                .set(size);
            // 領域を取り直す。前の領域(引数を置いた場所)はここで返る
            b.get(size).ins(Instruction::Call(at.reserve)).set(area);
            b.get(result_root)
                .get(area)
                .ins(Instruction::Call(at.wires.of(layout).encode))
                .ins(Instruction::Drop);
            // 符号化し終えたので内部の持ち主は落とす
            let (indices, glue) = (at.indices, at.indices.of(layout));
            wasm_data::drop_root(&mut b, indices, glue, result_root);
            b.get(area).get(size);
        }
    }
    b.finish()
}

/// 生成した bytes を独立に検証する。生成器と同じ知識を使わないための一手間
pub fn validate(bytes: &[u8]) -> Result<(), String> {
    wasmparser::Validator::new()
        .validate_all(bytes)
        .map(|_| ())
        .map_err(|error| error.to_string())
}

// ---------------------------------------------------------------------------
// テスト
// ---------------------------------------------------------------------------

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// ソース1本を生産ビルドと同じ順で通す。`exports` は `公開名=関数名`
    pub(crate) fn compile(src: &str, exports: &[&str]) -> Result<Vec<u8>, Vec<Diag>> {
        let (checked, production) = plan_of(src, exports);
        let unsupported = check_support(&checked, &production.plan);
        if !unsupported.is_empty() {
            return Err(unsupported);
        }
        emit(&checked, &production)
    }

    /// 所有権検査まで通した本物の入力。掃除の計画が要るので HIR だけでは足りない
    pub(crate) fn plan_of(src: &str, exports: &[&str]) -> (CheckedProgram, ProductionPlan) {
        let parsed = crate::parse::parse(&crate::lex::join(crate::lex::lex(src).unwrap()))
            .expect("パースできるはず");
        let program = crate::typecheck::check_and_lower(&parsed).expect("型検査を通るはず");
        let checked = crate::ownership::check(program).expect("所有権検査を通るはず");
        let analysis = crate::requirement::analyze(&checked);
        let entry = checked.hir.free_callable("main").expect("main がない");
        let exports: Vec<(String, hir::CallableId)> = exports
            .iter()
            .map(|spelling| {
                let (public, target) = spelling.split_once('=').unwrap_or((spelling, spelling));
                (
                    public.to_string(),
                    checked.hir.free_callable(target).expect("その関数がない"),
                )
            })
            .collect();
        let production = crate::ambient_abi::plan_production(&checked, &analysis, entry, &exports)
            .expect("計画できるはず");
        (checked, production)
    }

    /// 生成した bytes を独立した Core Wasm エンジンで走らせる。
    ///
    /// 生成器の知識を一切共有しないので、通ることが移植性の証明になる
    /// (design.md 決定9)。エンジンは開発依存で、生成物にも CLI にも入らない
    pub(crate) fn invoke(
        bytes: &[u8],
        name: &str,
        args: &[wasmi::Val],
    ) -> Result<Vec<wasmi::Val>, String> {
        let engine = wasmi::Engine::default();
        let module = wasmi::Module::new(&engine, bytes).map_err(|e| e.to_string())?;
        let mut store = wasmi::Store::new(&engine, ());
        let instance = wasmi::Linker::new(&engine)
            .instantiate_and_start(&mut store, &module)
            .map_err(|e| e.to_string())?;
        let func = instance
            .get_func(&store, name)
            .ok_or_else(|| format!("`{name}` という export がない"))?;
        let mut results = vec![wasmi::Val::I32(0); func.ty(&store).results().len()];
        func.call(&mut store, args, &mut results)
            .map_err(|e| e.to_string())?;
        Ok(results)
    }

    /// 返った値を i64 の並びへ均す。`wasmi::Val` は等値を持たないので
    /// 比較はこの形で行う
    pub(crate) fn scalars(values: &[wasmi::Val]) -> Vec<i64> {
        values
            .iter()
            .map(|value| match value {
                wasmi::Val::I32(n) => i64::from(*n),
                wasmi::Val::I64(n) => *n,
                other => panic!("scalar ではない: {other:?}"),
            })
            .collect()
    }

    /// エントリを走らせて int の結果を取る
    pub(crate) fn run_int(src: &str) -> i64 {
        let bytes = compile(src, &[]).expect("生成できるはず");
        validate(&bytes).expect("検証を通るはず");
        match invoke(&bytes, ENTRY_EXPORT, &[])
            .expect("走るはず")
            .as_slice()
        {
            [wasmi::Val::I64(n)] => *n,
            other => panic!("int ではない: {other:?}"),
        }
    }

    /// インタプリタと Wasm の結果を突き合わせる。両方が同じ値を出して初めて
    /// 「同じ意味」と言える
    pub(crate) fn same_as_interpreter(src: &str) -> i64 {
        let (checked, _) = plan_of(src, &[]);
        let interpreted = match crate::eval::Interp::new_checked(&checked)
            .run("main")
            .expect("インタプリタでも走るはず")
        {
            crate::eval::Value::Int(n) => n,
            other => panic!("int ではない: {other:?}"),
        };
        let compiled = run_int(src);
        assert_eq!(compiled, interpreted, "インタプリタと Wasm が食い違う");
        compiled
    }

    fn messages(diagnostics: &[Diag]) -> String {
        diagnostics
            .iter()
            .map(|d| d.msg.clone())
            .collect::<Vec<_>>()
            .join(" / ")
    }

    // ---- 名前付き関数の値(tasks 4.3) ----

    const CALLBACK_SRC: &str = "fn double(value: int -> int) { value * 2 }\n\
         fn negate(value: int -> int) { 0 - value }\n\
         fn apply(f: fn(int -> int), value: int -> int) { f(value) }\n";

    #[test]
    fn callbackの戻り値はインタプリタと一致する() {
        assert_eq!(
            same_as_interpreter(&format!(
                "{CALLBACK_SRC}fn main(-> int) {{ let f = double\n\
                 \x20 apply(f, 21) + apply(negate, 5) }}\n"
            )),
            37
        );
    }

    /// slot を要る callback と要らない callback で、隠れた ambient record の
    /// 有無が分かれても両方が正しく走る
    #[test]
    fn 隠れたambient記録はcallbackごとに分かれる() {
        let src = "trait Clock { fn now(&self -> int) }\n\
             struct Frozen { at: int }\n\
             impl Clock for Frozen { fn now(&self -> int) { self.at } }\n\
             effect clock: Clock\n\
             fn ticked(value: int -> int) { value + clock.now() }\n\
             fn plain(value: int -> int) { value + 1 }\n\
             fn apply(f: fn(int -> int), value: int -> int) { f(value) }\n\
             fn main(-> int) {\n\
             \x20 let quiet = apply(plain, 1)\n\
             \x20 with clock(Frozen { at = 1000 }) { apply(ticked, quiet) }\n\
             }\n";
        assert_eq!(same_as_interpreter(src), 1002);
    }

    /// 表も funcref も使わない。間接呼び出しは直接呼び出しへ落ちる
    #[test]
    fn 間接呼び出しはtableもfuncrefも使わない() {
        let src = format!("{CALLBACK_SRC}fn main(-> int) {{ apply(double, 21) }}\n");
        let bytes = compile(&src, &[]).expect("生成できるはず");
        validate(&bytes).expect("検証を通るはず");
        let parsed = wasmparser::Parser::new(0).parse_all(&bytes);
        for payload in parsed {
            assert!(
                !matches!(
                    payload.expect("解析できるはず"),
                    wasmparser::Payload::TableSection(_) | wasmparser::Payload::ElementSection(_)
                ),
                "table / element section が出ている"
            );
        }
    }

    #[test]
    fn callbackを含むbuildは決定的() {
        let src =
            format!("{CALLBACK_SRC}fn main(-> int) {{ apply(double, 1) + apply(negate, 2) }}\n");
        let bytes = compile(&src, &[]).expect("生成できるはず");
        assert_eq!(
            bytes,
            compile(&src, &[]).expect("同じ bytes を生成できるはず"),
            "callback を含む build は決定的"
        );
    }

    #[test]
    fn 最小のプログラムが検証を通る() {
        let bytes = compile("fn main(-> int) { 41 + 1 }\n", &[]).expect("生成できるはず");
        validate(&bytes).expect("検証を通るはず");
    }

    #[test]
    fn canonical_fixtureは独立engineで検証実行できる() {
        let src = std::fs::read_to_string("examples/canonical.rd").expect("fixture を読めるはず");
        let bytes = compile(&src, &[]).expect("生成できるはず");
        validate(&bytes).expect("検証を通るはず");
        assert_eq!(
            scalars(&invoke(&bytes, ENTRY_EXPORT, &[]).expect("走るはず")),
            [0]
        );
        assert_eq!(
            bytes,
            compile(&src, &[]).expect("同じ bytes を生成できるはず"),
            "canonical build は決定的"
        );
    }

    /// 生成物は host import も start section も持たない。instantiate だけでは
    /// main は走らない
    #[test]
    fn 生成物はimportもstartも持たない() {
        let bytes = compile("fn main(-> int) { 1 }\n", &[]).expect("生成できるはず");
        for payload in wasmparser::Parser::new(0).parse_all(&bytes) {
            match payload.expect("読めるはず") {
                wasmparser::Payload::ImportSection(_) => panic!("import がある"),
                wasmparser::Payload::StartSection { .. } => panic!("start section がある"),
                _ => {}
            }
        }
    }

    /// 同じ入力からは同じ bytes。hash の走査順も pointer も出力に出ない
    #[test]
    fn 同じ入力からは同じbytesが出る() {
        let src = "fn helper(n: int -> int) { n * 2 }\n\
                   fn pub_one(flag: bool -> bool) { flag }\n\
                   fn main(-> int) { helper(21) }\n";
        let first = compile(src, &["one=pub_one"]).expect("生成できるはず");
        let second = compile(src, &["one=pub_one"]).expect("生成できるはず");
        assert_eq!(first, second);
    }

    /// 到達しない宣言は scalar の外でも生産ビルドを止めない
    #[test]
    fn 到達しない未対応の宣言はビルドを止めない() {
        let bytes = compile(
            "struct User { name: str }\n\
             fn unused(u: User -> str) { u.name }\n\
             fn main(-> int) { 1 }\n",
            &[],
        )
        .expect("生成できるはず");
        validate(&bytes).expect("検証を通るはず");
    }

    /// 具体的な provider は planner が選んだ implementation instance に直結する。
    #[test]
    fn 到達したambient_slot_callは実行できる() {
        assert_eq!(
            same_as_interpreter(
                "trait Clock { fn now(self -> int) }\n\
             struct Frozen {}\n\
             impl Clock for Frozen { fn now(self -> int) { 1 } }\n\
             effect clock: Clock\n\
             fn reached(-> int) { with clock(Frozen {}) { clock.now() } }\n\
             fn main(-> int) { reached() }\n",
            ),
            1
        );
    }

    #[test]
    fn trait_receiverのshared_mutable_ownedと再帰はインタプリタと一致する() {
        assert_eq!(
            same_as_interpreter(
                "trait Counter { fn read(&self -> int)\n\
                 fn bump(&mut self, by: int)\n\
                 fn descend(&self, depth: int -> int)\n\
                 fn take(self, next: str -> str) }\n\
                 struct Cell { label: str, value: int }\n\
                 impl Counter for Cell {\n\
                 \x20 fn read(&self -> int) { self.value }\n\
                 \x20 fn bump(&mut self, by: int) { self.value = self.value + by }\n\
                 \x20 fn descend(&self, depth: int -> int) { if depth == 0: self.value else: self.descend(depth - 1) }\n\
                 \x20 fn take(self, next: str -> str) { next }\n\
                 }\n\
                 effect counter: Counter\n\
                 fn main(-> int) {\n\
                 \x20 let mut cell = Cell { label = \"before\", value = 2 }\n\
                 \x20 let shared = with counter(&cell) { counter.read() + counter.descend(2) }\n\
                 \x20 let changed = with counter(&mut cell) { counter.bump(3)\n counter.read() }\n\
                 \x20 let result = with counter(move cell) { counter.take(\"after\") }\n\
                 \x20 if result == \"after\": shared + changed else: 0\n\
                 }\n"
            ),
            9
        );
    }

    #[test]
    fn ambient_projectionはvalueとtypeのslotをforwardする() {
        assert_eq!(
            same_as_interpreter(
                "trait Clock { fn now(&self -> int) }\n\
                 trait Answer { fn get(-> int) }\n\
                 struct Frozen { at: int }\n\
                 struct Zero {}\n\
                 struct FortyTwo {}\n\
                 impl Clock for Frozen { fn now(&self -> int) { self.at } }\n\
                 impl Clock for Zero { fn now(&self -> int) { 0 } }\n\
                 impl Answer for FortyTwo { fn get(-> int) { 42 } }\n\
                 effect clock: Clock\n\
                 effect answer: Answer\n\
                 fn relay(-> int) { clock.now() + answer::get() }\n\
                 fn main(-> int) {\n\
                 \x20 let frozen = with clock(Frozen { at = 7 }), answer<FortyTwo> { relay() }\n\
                 \x20 let zero = with clock(Zero {}), answer<FortyTwo> { relay() }\n\
                 \x20 frozen + zero\n\
                 }\n"
            ),
            91
        );
    }

    #[test]
    fn temporary_providerはloopのたびに一度だけ破棄される() {
        let src = "trait Probe { fn size(&self -> int) }\n\
                   struct Item { label: str }\n\
                   impl Probe for Item { fn size(&self -> int) { if self.label == \"x\": 1 else: 0 } }\n\
                   effect probe: Probe\n\
                   fn main(-> int) {\n\
                   \x20 let mut n = 0\n\
                   \x20 while (n == 3000) == false {\n\
                   \x20   let size = with probe(Item { label = \"x\" }) { probe.size() }\n\
                   \x20   assert size == 1\n\
                   \x20   n = n + 1\n\
                   \x20 }\n\
                   \x20 n\n\
                   }\n";
        let bytes = compile(src, &[]).expect("生成できるはず");
        validate(&bytes).expect("検証を通るはず");
        assert_eq!(invoke_capped(&bytes, ENTRY_EXPORT, 1), Ok(vec![3000]));
    }

    #[test]
    fn forwarded_owned_providerはcalleeへ所有を移す() {
        let src = "trait Consume { fn take(self -> int) }\n\
                   struct Item { label: str }\n\
                   impl Consume for Item { fn take(self -> int) { if self.label == \"x\": 1 else: 0 } }\n\
                   effect item: Consume\n\
                   fn relay(-> int) { item.take() }\n\
                   fn main(-> int) {\n\
                   \x20 let mut n = 0\n\
                   \x20 while (n == 3000) == false {\n\
                   \x20   let result = with item(Item { label = \"x\" }) { relay() }\n\
                   \x20   assert result == 1\n\
                   \x20   n = n + 1\n\
                   \x20 }\n\
                   \x20 n\n\
                   }\n";
        assert_eq!(same_as_interpreter(src), 3000);
        let bytes = compile(src, &[]).expect("生成できるはず");
        assert_eq!(invoke_capped(&bytes, ENTRY_EXPORT, 1), Ok(vec![3000]));
    }

    #[test]
    fn nested_withはouter_contextで値を評価してshadowを復元する() {
        assert_eq!(
            same_as_interpreter(
                "trait Clock { fn now(&self -> int) }\n\
                 struct ClockValue { at: int }\n\
                 impl Clock for ClockValue { fn now(&self -> int) { self.at } }\n\
                 effect clock: Clock\n\
                 fn relay(-> int) { clock.now() }\n\
                 fn main(-> int) {\n\
                 \x20 with clock(ClockValue { at = 3 }) {\n\
                 \x20   let derived = with clock(ClockValue { at = clock.now() + 1 }) { relay() }\n\
                 \x20   let shadowed = with clock(ClockValue { at = 0 }) { relay() }\n\
                 \x20   derived + shadowed + relay()\n\
                 \x20 }\n\
                 }\n"
            ),
            7
        );
    }

    #[test]
    fn multi_provision_headの値はすべてouter_contextを見る() {
        assert_eq!(
            same_as_interpreter(
                "trait Left { fn now(&self -> int) }\n\
                 trait Right { fn now(&self -> int) }\n\
                 struct LeftValue { at: int }\n\
                 struct RightValue { at: int }\n\
                 impl Left for LeftValue { fn now(&self -> int) { self.at } }\n\
                 impl Right for RightValue { fn now(&self -> int) { self.at } }\n\
                 effect left: Left\n\
                 effect right: Right\n\
                 fn main(-> int) {\n\
                 \x20 with left(LeftValue { at = 2 }), right(RightValue { at = 3 }) {\n\
                 \x20   with left(LeftValue { at = left.now() + 1 }), right(RightValue { at = right.now() + 1 }) {\n\
                 \x20     left.now() + right.now()\n\
                 \x20   }\n\
                 \x20 }\n\
                 }\n"
            ),
            7
        );
    }

    /// 借用は境界を越えられない。署名に出たら名指して止める(tasks 4.6)
    #[test]
    fn 署名に出た借用はビルドを止める() {
        let errors = compile(
            "struct User { id: int }\n\
             fn peek(u: &User -> &User) { u }\n\
             fn main(-> int) { let one = User { id = 1 }\n peek(&one).id }\n",
            &[],
        )
        .expect_err("止まるはず");
        let found = messages(&errors);
        assert!(
            found.contains("戻り値の借用 `&User` は Wasm の署名には出せません"),
            "{found}"
        );
    }

    /// 共有された instance の実装は1つ。呼び出しは全部そこを指す
    #[test]
    fn 共有された実装は1つだけ出る() {
        let (checked, production) = plan_of(
            "fn shared(-> int) { 1 }\n\
             fn other(-> int) { shared() }\n\
             fn main(-> int) { shared() + other() }\n",
            &["other"],
        );
        let bodies: Vec<hir::BodyId> = production
            .plan
            .instances()
            .map(|(_, instance)| instance.key.body)
            .collect();
        let shared = hir::BodyId::Callable(checked.hir.free_callable("shared").unwrap());
        assert_eq!(bodies.iter().filter(|body| **body == shared).count(), 1);
    }

    // -----------------------------------------------------------------------
    // scalar の式と制御フロー
    // -----------------------------------------------------------------------

    #[test]
    fn リテラルと束縛と代入() {
        assert_eq!(
            same_as_interpreter("fn main(-> int) {\n let mut n = 1\n n = n + 2\n n\n}\n"),
            3
        );
        // unit の束縛は Wasm のローカルを持たないが、初期化子の効果は走る
        assert_eq!(
            same_as_interpreter(
                "fn bump(n: int -> int) { n }\n\
                 fn nothing(n: int) { let ignored = bump(n) }\n\
                 fn main(-> int) {\n let u = nothing(1)\n 7\n}\n"
            ),
            7
        );
    }

    /// ブロックの値は最後の式。途中の値は落とす(stack が釣り合う)
    #[test]
    fn ブロックの値は最後の式() {
        assert_eq!(same_as_interpreter("fn main(-> int) {\n 1\n 2\n 3\n}\n"), 3);
    }

    #[test]
    fn 算術は境界でインタプリタと一致する() {
        for (src, expected) in [
            ("1 + 2 * 3", 7),
            ("-7 / 2", -3),
            ("9223372036854775807 + 1", i64::MIN),
            ("9223372036854775807 * 2", -2),
            ("-(-9223372036854775807 - 1)", i64::MIN),
        ] {
            assert_eq!(
                same_as_interpreter(&format!("fn main(-> int) {{ {src} }}\n")),
                expected,
                "{src}"
            );
        }
    }

    /// 割り算の失敗2種は trap。インタプリタは診断、Wasm は trap で伝える
    #[test]
    fn 割り算の失敗はtrapになる() {
        for src in [
            "fn main(-> int) { 1 / 0 }\n",
            "fn main(-> int) { (-9223372036854775807 - 1) / -1 }\n",
        ] {
            let bytes = compile(src, &[]).expect("生成できるはず");
            assert!(invoke(&bytes, ENTRY_EXPORT, &[]).is_err(), "{src}");
        }
    }

    #[test]
    fn 等値はintでもboolでも比較できる() {
        assert_eq!(
            same_as_interpreter("fn main(-> int) { if 1 == 1: 1 else: 0 }\n"),
            1
        );
        assert_eq!(
            same_as_interpreter("fn main(-> int) { if (1 == 2) == false: 1 else: 0 }\n"),
            1
        );
    }

    #[test]
    fn ifは値でもunitでも下ろせる() {
        assert_eq!(
            same_as_interpreter("fn main(-> int) { if true: 1 else: 2 }\n"),
            1
        );
        // else の無い `if` は unit。分岐の中の代入は外から見える
        assert_eq!(
            same_as_interpreter("fn main(-> int) {\n let mut n = 1\n if true { n = 5 }\n n\n}\n"),
            5
        );
    }

    #[test]
    fn whileは繰り返して外の束縛を書き換える() {
        assert_eq!(
            same_as_interpreter(
                "fn main(-> int) {\n\
                 \x20 let mut n = 0\n\
                 \x20 let mut total = 0\n\
                 \x20 while n == n + 0 {\n\
                 \x20   if n == 5 { return total }\n\
                 \x20   total = total + n\n\
                 \x20   n = n + 1\n\
                 \x20 }\n\
                 \x20 total\n\
                 }\n"
            ),
            10
        );
    }

    /// 入れ子の制御フローと、条件が偽になって抜けるループ
    #[test]
    fn 入れ子の制御フローが走る() {
        assert_eq!(
            same_as_interpreter(
                "fn done(n: int -> bool) { n == 3 }\n\
                 fn main(-> int) {\n\
                 \x20 let mut n = 0\n\
                 \x20 let mut acc = 0\n\
                 \x20 while done(n) == false {\n\
                 \x20   if n == 1 { acc = acc + 10 } else { acc = acc + 1 }\n\
                 \x20   n = n + 1\n\
                 \x20 }\n\
                 \x20 acc\n\
                 }\n"
            ),
            12
        );
    }

    /// `return` はそこで関数を抜ける。後続の式は評価されない
    #[test]
    fn returnは後続の式を評価しない() {
        assert_eq!(
            same_as_interpreter("fn main(-> int) {\n return 1\n 1 / 0\n}\n"),
            1
        );
    }

    #[test]
    fn 再帰と相互再帰が走る() {
        assert_eq!(
            same_as_interpreter(
                "fn fact(n: int -> int) { if n == 0: 1 else: n * fact(n - 1) }\n\
                 fn main(-> int) { fact(5) }\n"
            ),
            120
        );
        assert_eq!(
            same_as_interpreter(
                "fn even(n: int -> bool) { if n == 0: true else: odd(n - 1) }\n\
                 fn odd(n: int -> bool) { if n == 0: false else: even(n - 1) }\n\
                 fn main(-> int) { if even(10): 1 else: 0 }\n"
            ),
            1
        );
    }

    /// 偽の assert は trap する。真なら何も起きない
    #[test]
    fn 偽のassertはtrapする() {
        let bytes =
            compile("fn main(-> int) {\n assert false\n 1\n}\n", &[]).expect("生成できるはず");
        assert!(invoke(&bytes, ENTRY_EXPORT, &[]).is_err());
        assert_eq!(
            same_as_interpreter("fn main(-> int) {\n assert true\n 1\n}\n"),
            1
        );
    }

    /// unit を返す呼び出しは stack に何も残さない
    #[test]
    fn unitを返す呼び出しは値を残さない() {
        let bytes = compile("fn side() { assert true }\nfn main() { side() }\n", &[])
            .expect("生成できるはず");
        validate(&bytes).expect("検証を通るはず");
        assert_eq!(scalars(&invoke(&bytes, ENTRY_EXPORT, &[]).unwrap()), []);
    }

    /// 局所・ループ・分岐・呼び出し・再帰・return・assert を1本に混ぜる
    #[test]
    fn 全部混ぜた本体がインタプリタと一致する() {
        assert_eq!(
            same_as_interpreter(
                "fn triple(n: int -> int) { n * 3 }\n\
                 fn sum_to(n: int -> int) { if n == 0: 0 else: n + sum_to(n - 1) }\n\
                 fn main(-> int) {\n\
                 \x20 let mut total = 0\n\
                 \x20 let mut i = 0\n\
                 \x20 while (i == 4) == false {\n\
                 \x20   if i == 2 { total = total + triple(i) } else { total = total + i }\n\
                 \x20   i = i + 1\n\
                 \x20 }\n\
                 \x20 assert total == 10\n\
                 \x20 total + sum_to(4)\n\
                 }\n"
            ),
            20
        );
    }

    /// エントリの戻り値は3種類とも通る
    #[test]
    fn エントリの戻り値は三種類とも通る() {
        for (src, expected) in [
            ("fn main() { assert true }\n", vec![]),
            ("fn main(-> bool) { true }\n", vec![1]),
            ("fn main(-> int) { 9 }\n", vec![9]),
        ] {
            let bytes = compile(src, &[]).expect("生成できるはず");
            validate(&bytes).expect("検証を通るはず");
            assert_eq!(
                scalars(&invoke(&bytes, ENTRY_EXPORT, &[]).unwrap()),
                expected,
                "{src}"
            );
        }
    }

    // -----------------------------------------------------------------------
    // Rhodolite Wasm ABI v0
    // -----------------------------------------------------------------------

    fn abi_error(src: &str, exports: &[&str]) -> String {
        let (checked, production) = plan_of(src, exports);
        messages(&crate::wasm_abi::signatures(&checked, &production).expect_err("止まるはず"))
    }

    fn export_names(bytes: &[u8]) -> Vec<String> {
        let mut names = Vec::new();
        for payload in wasmparser::Parser::new(0).parse_all(bytes) {
            if let wasmparser::Payload::ExportSection(section) = payload.unwrap() {
                for export in section {
                    names.push(export.unwrap().name.to_string());
                }
            }
        }
        names
    }

    fn abi_metadata(bytes: &[u8]) -> String {
        let mut found = Vec::new();
        for payload in wasmparser::Parser::new(0).parse_all(bytes) {
            if let wasmparser::Payload::CustomSection(section) = payload.unwrap()
                && section.name() == ABI_SECTION
            {
                found.push(String::from_utf8(section.data().to_vec()).expect("UTF-8"));
            }
        }
        assert_eq!(found.len(), 1, "`{ABI_SECTION}` はちょうど1つ");
        found.remove(0)
    }

    const SURFACE: &str = "fn find(id: int, live: bool -> bool) { if live: id == 1 else: false }\n\
                           fn touch(n: int) { assert n == n }\n\
                           fn main(-> int) { 3 }\n";

    /// 出るのは予約名のエントリと公開名だけ。内部の実装名は export されない
    #[test]
    fn 公開されるのはエントリと公開名だけ() {
        let bytes = compile(SURFACE, &["find_user=find", "touch"]).expect("生成できるはず");
        validate(&bytes).expect("検証を通るはず");
        assert_eq!(export_names(&bytes), [ENTRY_EXPORT, "find_user", "touch"]);
        // 別名を付けた元の綴りも、正準名も出ない
        assert!(!export_names(&bytes).iter().any(|name| name == "find"));
    }

    /// 別名が同じ関数を指しても、実装は共有してラッパだけが増える
    #[test]
    fn 別名は名前ごとにラッパを持ち実装を共有する() {
        let bytes = compile(SURFACE, &["a=find", "z=find"]).expect("生成できるはず");
        validate(&bytes).expect("検証を通るはず");
        assert_eq!(export_names(&bytes), [ENTRY_EXPORT, "a", "z"]);
        for name in ["a", "z"] {
            assert_eq!(
                scalars(&invoke(&bytes, name, &[wasmi::Val::I64(1), wasmi::Val::I32(1)]).unwrap()),
                [1]
            );
        }
    }

    /// int は i64 のまま、bool は 0/1 の i32、unit は結果を持たない
    #[test]
    fn 対応する署名がそのまま境界を渡る() {
        let bytes = compile(SURFACE, &["find_user=find", "touch"]).expect("生成できるはず");
        assert_eq!(
            scalars(
                &invoke(
                    &bytes,
                    "find_user",
                    &[wasmi::Val::I64(1), wasmi::Val::I32(1)]
                )
                .unwrap()
            ),
            [1]
        );
        assert_eq!(
            scalars(
                &invoke(
                    &bytes,
                    "find_user",
                    &[wasmi::Val::I64(2), wasmi::Val::I32(1)]
                )
                .unwrap()
            ),
            [0]
        );
        assert_eq!(
            scalars(&invoke(&bytes, "touch", &[wasmi::Val::I64(7)]).unwrap()),
            []
        );
        // i64 の bit pattern はそのまま往復する
        assert_eq!(
            scalars(
                &invoke(
                    &bytes,
                    "find_user",
                    &[wasmi::Val::I64(i64::MIN), wasmi::Val::I32(0)]
                )
                .unwrap()
            ),
            [0]
        );
    }

    /// bool 引数は 0/1 以外を受けない。本体へ入る前に trap する
    #[test]
    fn 不正なbool引数は本体の前でtrapする() {
        let bytes = compile(SURFACE, &["find_user=find"]).expect("生成できるはず");
        for raw in [2, -1, i32::MIN] {
            assert!(
                invoke(
                    &bytes,
                    "find_user",
                    &[wasmi::Val::I64(1), wasmi::Val::I32(raw)]
                )
                .is_err(),
                "{raw}"
            );
        }
    }

    /// 実行時の失敗は trap で届く。状態コードも戻り値も増えない
    #[test]
    fn 公開関数の実行時失敗はtrapで届く() {
        let bytes = compile(
            "fn divide(a: int, b: int -> int) { a / b }\n\
             fn main(-> int) { 0 }\n",
            &["divide"],
        )
        .expect("生成できるはず");
        assert_eq!(
            scalars(&invoke(&bytes, "divide", &[wasmi::Val::I64(7), wasmi::Val::I64(2)]).unwrap()),
            [3]
        );
        assert!(
            invoke(&bytes, "divide", &[wasmi::Val::I64(7), wasmi::Val::I64(0)]).is_err(),
            "0除算は trap"
        );
    }

    #[test]
    fn 予約名は公開名に使えない() {
        assert!(
            abi_error(SURFACE, &["__rhodolite_main=find"]).contains("予約名"),
            "{}",
            abi_error(SURFACE, &["__rhodolite_main=find"])
        );
    }

    #[test]
    fn mainが引数を取ると入口の署名を拒否する() {
        assert!(
            abi_error("fn main(n: int -> int) { n }\n", &[]).contains("`main` は引数を取れません")
        );
    }

    #[test]
    fn unit引数の公開関数を拒否する() {
        let message = abi_error(
            "fn nothing(u: unit -> int) { 1 }\n\
             fn main(-> int) { 0 }\n",
            &["nothing"],
        );
        assert!(
            message.contains("`unit` 引数は境界に出せません"),
            "{message}"
        );
    }

    /// 公開できない型は公開面でだけ拒否する。同じ型の非公開宣言は縛らない
    #[test]
    fn 公開面の借用は拒否するが所有は境界に出せる() {
        let src = "struct User { name: str }\n\
                   fn rich(-> User) { User { name = \"a\" } }\n\
                   fn peek(u: &User -> int) { 1 }\n\
                   fn main(-> int) { 0 }\n";
        // 所有型は ABI v1 として通る
        compile(src, &["rich"]).expect("生成できるはず");
        // 借用は境界を越えられない
        assert!(
            abi_error(src, &["peek"]).contains("公開面の借用 `&User` は Wasm の署名には出せません"),
            "{}",
            abi_error(src, &["peek"])
        );
        // 同じ宣言があっても、公開せず到達もしなければビルドは通る
        compile(src, &[]).expect("生成できるはず");
    }

    /// 埋め込みメタデータは正準形。鍵の並びも公開名の並びも固定
    #[test]
    fn 埋め込みメタデータは正準形になる() {
        let bytes = compile(SURFACE, &["zebra=touch", "alpha=find"]).expect("生成できるはず");
        assert_eq!(
            abi_metadata(&bytes),
            "{\"version\":0,\
             \"entry\":{\"name\":\"__rhodolite_main\",\"params\":[],\"result\":\"int\"},\
             \"exports\":[\
             {\"name\":\"alpha\",\"params\":[\"int\",\"bool\"],\"result\":\"bool\"},\
             {\"name\":\"zebra\",\"params\":[\"int\"],\"result\":\"unit\"}]}"
        );
    }

    // -----------------------------------------------------------------------
    // 所有する文字列(tasks 4.1〜4.4)
    // -----------------------------------------------------------------------

    /// 線形メモリの上限を決めて走らせる。
    ///
    /// 掃除が漏れていれば有界なループでもメモリを伸ばし続けるので、上限に
    /// ぶつかって trap する。「解放した記憶を使い回している」の直接の証拠
    fn invoke_capped(bytes: &[u8], name: &str, max_pages: u32) -> Result<Vec<i64>, String> {
        struct Cap(u32);
        impl wasmi::ResourceLimiter for Cap {
            fn memory_growing(
                &mut self,
                _current: usize,
                desired: usize,
                _maximum: Option<usize>,
            ) -> Result<bool, wasmi_core::LimiterError> {
                Ok(desired <= self.0 as usize * 65536)
            }

            fn table_growing(
                &mut self,
                _current: usize,
                _desired: usize,
                _maximum: Option<usize>,
            ) -> Result<bool, wasmi_core::LimiterError> {
                Ok(true)
            }

            fn instances(&self) -> usize {
                1
            }

            fn tables(&self) -> usize {
                0
            }

            fn memories(&self) -> usize {
                1
            }
        }

        let engine = wasmi::Engine::default();
        let module = wasmi::Module::new(&engine, bytes).map_err(|e| e.to_string())?;
        let mut store = wasmi::Store::new(&engine, Cap(max_pages));
        store.limiter(|cap| cap);
        let instance = wasmi::Linker::new(&engine)
            .instantiate_and_start(&mut store, &module)
            .map_err(|e| e.to_string())?;
        let func = instance
            .get_func(&store, name)
            .ok_or_else(|| format!("`{name}` という export がない"))?;
        let mut results = vec![wasmi::Val::I32(0); func.ty(&store).results().len()];
        func.call(&mut store, &[], &mut results)
            .map_err(|e| e.to_string())?;
        Ok(scalars(&results))
    }

    /// 所有データを載せたモジュールも、import も start section も持たない
    #[test]
    fn 文字列を載せてもimportもstartも増えない() {
        let bytes =
            compile("fn main(-> int) {\n let s = \"hi\"\n 1\n}\n", &[]).expect("生成できるはず");
        validate(&bytes).expect("検証を通るはず");
        for payload in wasmparser::Parser::new(0).parse_all(&bytes) {
            match payload.expect("読めるはず") {
                wasmparser::Payload::ImportSection(_) => panic!("import がある"),
                wasmparser::Payload::StartSection { .. } => panic!("start section がある"),
                _ => {}
            }
        }
    }

    /// リテラルから作った文字列は、綴りが同じなら等しい
    #[test]
    fn 文字列の等値はインタプリタと一致する() {
        for (src, expected) in [
            ("let a = \"hi\"\n let b = \"hi\"\n if a == b: 1 else: 0", 1),
            ("let a = \"hi\"\n let b = \"ho\"\n if a == b: 1 else: 0", 0),
            // 長さが違えばそこで打ち切る
            ("let a = \"hi\"\n let b = \"hit\"\n if a == b: 1 else: 0", 0),
            ("let a = \"\"\n let b = \"\"\n if a == b: 1 else: 0", 1),
            // 多バイト文字も byte 列として比べる
            (
                "let a = \"あい\"\n let b = \"あい\"\n if a == b: 1 else: 0",
                1,
            ),
            (
                "let a = \"あい\"\n let b = \"あう\"\n if a == b: 1 else: 0",
                0,
            ),
        ] {
            assert_eq!(
                same_as_interpreter(&format!("fn main(-> int) {{\n {src}\n}}\n")),
                expected,
                "{src}"
            );
        }
    }

    /// `clone()` は中身まで写す。元と複製は同じ記憶を共有しない
    #[test]
    fn cloneは独立した文字列を作る() {
        assert_eq!(
            same_as_interpreter(
                "fn main(-> int) {\n\
                 \x20 let a = \"hi\"\n\
                 \x20 let b = a.clone()\n\
                 \x20 if a == b: 1 else: 0\n\
                 }\n"
            ),
            1
        );
    }

    /// 所有は関数の境界を越えて渡り、渡した先で落ちる
    #[test]
    fn 文字列は引数と戻り値で渡せる() {
        assert_eq!(
            same_as_interpreter(
                "fn matches(left: str, right: str -> bool) { left == right }\n\
                 fn main(-> int) {\n\
                 \x20 let a = \"hi\"\n\
                 \x20 let b = \"hi\"\n\
                 \x20 if matches(move a, move b): 1 else: 0\n\
                 }\n"
            ),
            1
        );
        assert_eq!(
            same_as_interpreter(
                "fn greeting(-> str) { \"hi\" }\n\
                 fn main(-> int) {\n\
                 \x20 let s = greeting()\n\
                 \x20 if s == \"hi\": 1 else: 0\n\
                 }\n"
            ),
            1
        );
    }

    /// 代入は新しい値が出来てから古い値を落とす
    #[test]
    fn 代入は古い文字列を落として差し替える() {
        assert_eq!(
            same_as_interpreter(
                "fn main(-> int) {\n\
                 \x20 let mut s = \"one\"\n\
                 \x20 s = \"two\"\n\
                 \x20 if s == \"two\": 1 else: 0\n\
                 }\n"
            ),
            1
        );
    }

    /// 経路によって持っているかが変わる束縛でも、二度落とさない
    #[test]
    fn 分岐をまたいだ所有でも掃除は一度だけ() {
        assert_eq!(
            same_as_interpreter(
                "fn take(s: str -> int) { 1 }\n\
                 fn main(-> int) {\n\
                 \x20 let s = \"hi\"\n\
                 \x20 if true { return take(move s) }\n\
                 \x20 0\n\
                 }\n"
            ),
            1
        );
        // 分岐の中だけで作った文字列は、合流の手前で落ちる
        assert_eq!(
            same_as_interpreter(
                "fn main(-> int) {\n\
                 \x20 if true {\n\
                 \x20   let inner = \"hi\"\n\
                 \x20   1\n\
                 \x20 } else {\n\
                 \x20   let other = \"ho\"\n\
                 \x20   2\n\
                 \x20 }\n\
                 }\n"
            ),
            1
        );
    }

    /// 有界なループは、解放した記憶を使い回すのでメモリを伸ばさない。
    ///
    /// 1ページに縛って走らせる。掃除が漏れていれば伸ばそうとして trap する
    #[test]
    fn ループで作った文字列は使い回される() {
        let src = "fn main(-> int) {\n\
                   \x20 let mut n = 0\n\
                   \x20 while (n == 5000) == false {\n\
                   \x20   let each = \"repeated\"\n\
                   \x20   n = n + 1\n\
                   \x20 }\n\
                   \x20 n\n\
                   }\n";
        let bytes = compile(src, &[]).expect("生成できるはず");
        validate(&bytes).expect("検証を通るはず");
        assert_eq!(invoke_capped(&bytes, ENTRY_EXPORT, 1), Ok(vec![5000]));
    }

    /// 誰にも束縛されない一時値も落ちる。
    ///
    /// `s == "x"` の右辺は持ち主の居ない所有値なので、比べ終えたところで
    /// 落とさないとループのたびに溜まる
    #[test]
    fn 束縛されない一時値も落ちる() {
        let src = "fn main(-> int) {\n\
                   \x20 let s = \"compared\"\n\
                   \x20 let mut n = 0\n\
                   \x20 while (n == 3000) == false {\n\
                   \x20   if (s == \"compared\") == false { return 0 }\n\
                   \x20   n = n + 1\n\
                   \x20 }\n\
                   \x20 n\n\
                   }\n";
        let bytes = compile(src, &[]).expect("生成できるはず");
        validate(&bytes).expect("検証を通るはず");
        assert_eq!(invoke_capped(&bytes, ENTRY_EXPORT, 1), Ok(vec![3000]));
    }

    /// 同時に生きる所有値が上限を越えれば、伸ばせずに trap する。
    ///
    /// 上限を広げれば同じプログラムが通ることも見る。stack を使い切ったのでは
    /// なく、記憶が足りなかったのだと言い分けるため
    #[test]
    fn 記憶が足りなければtrapする() {
        // 1回の呼び出しで 200 byte 強を握る。500 段で1ページを越える
        let wide = "x".repeat(200);
        let src = format!(
            "fn deep(n: int -> int) {{\n\
             \x20 let held = \"{wide}\"\n\
             \x20 if n == 0: 0 else: deep(n - 1)\n\
             }}\n\
             fn main(-> int) {{ deep(500) }}\n"
        );
        let bytes = compile(&src, &[]).expect("生成できるはず");
        validate(&bytes).expect("検証を通るはず");
        assert!(
            invoke_capped(&bytes, ENTRY_EXPORT, 1).is_err(),
            "1ページには収まらない"
        );
        assert_eq!(invoke_capped(&bytes, ENTRY_EXPORT, 8), Ok(vec![0]));
    }

    // -----------------------------------------------------------------------
    // 所有する struct(tasks 5.1〜5.5)
    // -----------------------------------------------------------------------

    /// フィールドは宣言順に置かれ、Copy も所有も読み書きできる
    #[test]
    fn structのフィールドを読み書きできる() {
        assert_eq!(
            same_as_interpreter(
                "struct User { id: int, admin: bool, name: str }\n\
                 fn main(-> int) {\n\
                 \x20 let mut u = User { id = 7, admin = true, name = \"a\" }\n\
                 \x20 u.id = u.id + 1\n\
                 \x20 u.name = \"b\"\n\
                 \x20 if u.admin == false { return 0 }\n\
                 \x20 if u.name == \"b\": u.id else: 0\n\
                 }\n"
            ),
            8
        );
    }

    /// 値はソースの順に評価する。置き場所が宣言順でも順序は入れ替わらない
    #[test]
    fn 構築はソースの順に評価する() {
        assert_eq!(
            same_as_interpreter(
                "struct Pair { first: int, second: int }\n\
                 fn bump(n: &mut int -> int) { 0 }\n\
                 fn main(-> int) {\n\
                 \x20 let mut seen = 0\n\
                 \x20 let p = Pair { second = { seen = 1\n 2 }, first = seen }\n\
                 \x20 p.first * 10 + p.second\n\
                 }\n"
            ),
            12
        );
    }

    /// フィールドを持たない struct も値として扱える
    #[test]
    fn フィールドのないstructも値になる() {
        assert_eq!(
            same_as_interpreter(
                "struct Marker {}\n\
                 fn take(m: Marker -> int) { 1 }\n\
                 fn main(-> int) {\n let m = Marker {}\n take(move m)\n}\n"
            ),
            1
        );
    }

    /// 深い複製は元と記憶を共有しない。片方を書き換えても他方は動かない
    #[test]
    fn structのcloneは中身まで独立する() {
        assert_eq!(
            same_as_interpreter(
                "struct User { name: str }\n\
                 fn main(-> int) {\n\
                 \x20 let a = User { name = \"same\" }\n\
                 \x20 let mut b = a.clone()\n\
                 \x20 if (a == b) == false { return 0 }\n\
                 \x20 b.name = \"other\"\n\
                 \x20 if a.name == \"same\": 1 else: 0\n\
                 }\n"
            ),
            1
        );
    }

    /// 等値は宣言順に潜って、違いを見つけたところで打ち切る
    #[test]
    fn structの等値は構造で決まる() {
        for (fields, expected) in [
            ("id = 1, name = \"a\"", 1),
            ("id = 2, name = \"a\"", 0),
            ("id = 1, name = \"b\"", 0),
        ] {
            assert_eq!(
                same_as_interpreter(&format!(
                    "struct User {{ id: int, name: str }}\n\
                     fn main(-> int) {{\n\
                     \x20 let a = User {{ id = 1, name = \"a\" }}\n\
                     \x20 let b = User {{ {fields} }}\n\
                     \x20 if a == b: 1 else: 0\n\
                     }}\n"
                )),
                expected,
                "{fields}"
            );
        }
    }

    /// `indirect` の子は別の割り当てに置かれる。読み・複製・等値・掃除が
    /// その1段を越えて働く。
    ///
    /// 再帰する鎖そのものは `Node?` を組み立てられるようになってから
    /// (optional のスライス)
    #[test]
    fn indirectな子も辿れて複製できる() {
        assert_eq!(
            same_as_interpreter(
                "struct Inner { name: str }\n\
                 struct Boxed { tag: int, indirect inner: Inner }\n\
                 fn main(-> int) {\n\
                 \x20 let a = Boxed { tag = 1, inner = Inner { name = \"x\" } }\n\
                 \x20 let mut b = a.clone()\n\
                 \x20 if (a == b) == false { return 0 }\n\
                 \x20 b.inner = Inner { name = \"y\" }\n\
                 \x20 if a == b { return 0 }\n\
                 \x20 if a.inner.name == \"x\": a.tag else: 0\n\
                 }\n"
            ),
            1
        );
    }

    /// 器ごと動かしても、掃除は最後の持ち主のところで一度だけ
    #[test]
    fn structの丸ごとの移動は一度だけ落とす() {
        assert_eq!(
            same_as_interpreter(
                "struct User { name: str }\n\
                 fn consume(u: User -> int) { if u.name == \"a\": 1 else: 0 }\n\
                 fn main(-> int) {\n\
                 \x20 let u = User { name = \"a\" }\n\
                 \x20 consume(move u)\n\
                 }\n"
            ),
            1
        );
    }

    /// フィールドを消費したら、器の残りだけが落ちる(tasks 3.5・5.4)
    #[test]
    fn フィールドを消費すると器の残りが落ちる() {
        assert_eq!(
            same_as_interpreter(
                "struct User { id: int, name: str, note: str }\n\
                 fn take(s: str -> int) { if s == \"n\": 1 else: 0 }\n\
                 fn main(-> int) {\n\
                 \x20 let u = User { id = 1, name = \"n\", note = \"x\" }\n\
                 \x20 take(move u.name)\n\
                 }\n"
            ),
            1
        );
    }

    /// 射影を消費する形でも記憶は漏れない。
    ///
    /// 取り出した値・器の残り・器の根の3つが全部返らないと、1ページでは
    /// 回りきらない
    #[test]
    fn 射影を消費するループでも記憶は漏れない() {
        let src = "struct User { name: str, note: str }\n\
                   fn take(s: str -> int) { if s == \"a\": 1 else: 0 }\n\
                   fn main(-> int) {\n\
                   \x20 let mut n = 0\n\
                   \x20 while (n == 2000) == false {\n\
                   \x20   let u = User { name = \"a\", note = \"b\" }\n\
                   \x20   n = n + take(move u.name)\n\
                   \x20 }\n\
                   \x20 n\n\
                   }\n";
        let bytes = compile(src, &[]).expect("生成できるはず");
        validate(&bytes).expect("検証を通るはず");
        assert_eq!(invoke_capped(&bytes, ENTRY_EXPORT, 1), Ok(vec![2000]));
    }

    /// 有界なループなら、struct を作って捨てても記憶は使い回される
    #[test]
    fn ループで作ったstructは使い回される() {
        let src = "struct User { id: int, name: str }\n\
                   fn main(-> int) {\n\
                   \x20 let mut n = 0\n\
                   \x20 while (n == 3000) == false {\n\
                   \x20   let each = User { id = n, name = \"looped\" }\n\
                   \x20   n = n + 1\n\
                   \x20 }\n\
                   \x20 n\n\
                   }\n";
        let bytes = compile(src, &[]).expect("生成できるはず");
        validate(&bytes).expect("検証を通るはず");
        assert_eq!(invoke_capped(&bytes, ENTRY_EXPORT, 1), Ok(vec![3000]));
    }

    /// 所有データを使わないモジュールには、メモリも allocator も載らない
    #[test]
    fn scalarだけのモジュールにはランタイムが載らない() {
        let bytes = compile("fn main(-> int) { 1 }\n", &[]).expect("生成できるはず");
        for payload in wasmparser::Parser::new(0).parse_all(&bytes) {
            match payload.expect("読めるはず") {
                wasmparser::Payload::MemorySection(_) => panic!("メモリがある"),
                wasmparser::Payload::DataSection(_) => panic!("データがある"),
                _ => {}
            }
        }
    }

    // -----------------------------------------------------------------------
    // enum・optional・`??`・`match`(tasks 6.1〜6.6)
    // -----------------------------------------------------------------------

    /// payload を持たない enum はタグだけ。heap を使わない
    #[test]
    fn payloadのないenumはタグだけで回る() {
        assert_eq!(
            same_as_interpreter(
                "enum Color { Red\n Green\n Blue }\n\
                 fn rank(c: Color -> int) {\n\
                 \x20 match c {\n\
                 \x20   Color::Red: 1\n\
                 \x20   Color::Green: 2\n\
                 \x20   Color::Blue: 3\n\
                 \x20 }\n\
                 }\n\
                 fn main(-> int) { rank(Color::Green) + rank(Color::Blue) }\n"
            ),
            5
        );
        // タグだけの enum を使うプログラムには allocator が載らない
        let bytes = compile(
            "enum Color { Red\n Green }\n\
             fn main(-> int) {\n let c = Color::Red\n if c == Color::Red: 1 else: 0\n}\n",
            &[],
        )
        .expect("生成できるはず");
        validate(&bytes).expect("検証を通るはず");
        for payload in wasmparser::Parser::new(0).parse_all(&bytes) {
            if let wasmparser::Payload::MemorySection(_) = payload.expect("読めるはず") {
                panic!("タグだけの enum にメモリが載っている");
            }
        }
    }

    /// Copy な optional は平らな値。`??` も等値も heap を使わない
    #[test]
    fn copyなoptionalは平らなまま扱える() {
        for (init, expected) in [("5", 5), ("nil", 7)] {
            assert_eq!(
                same_as_interpreter(&format!(
                    "fn main(-> int) {{\n let x: int? = {init}\n x ?? 7\n}}\n"
                )),
                expected,
                "{init}"
            );
        }
        // 等値は tag と中身の両方を見る
        for (left, right, expected) in [
            ("5", "5", 1),
            ("5", "6", 0),
            ("nil", "nil", 1),
            ("5", "nil", 0),
            ("nil", "5", 0),
        ] {
            assert_eq!(
                same_as_interpreter(&format!(
                    "fn main(-> int) {{\n\
                     \x20 let a: int? = {left}\n\
                     \x20 let b: int? = {right}\n\
                     \x20 if a == b: 1 else: 0\n\
                     }}\n"
                )),
                expected,
                "{left} == {right}"
            );
        }
    }

    /// 所有する optional は `{tag, 中身}` の根1つ。借りた `??` は元を残す
    #[test]
    fn 所有するoptionalは借りても消費しても取り出せる() {
        for (init, expected) in [("\"a\"", 1), ("nil", 0)] {
            // 借りた `??`。元の束縛はそのまま生きている
            assert_eq!(
                same_as_interpreter(&format!(
                    "fn main(-> int) {{\n\
                     \x20 let s: str? = {init}\n\
                     \x20 let first = if (s ?? \"d\") == \"a\": 1 else: 0\n\
                     \x20 let second = if (s ?? \"d\") == \"a\": 1 else: 0\n\
                     \x20 first * second\n\
                     }}\n"
                )),
                expected,
                "借用 {init}"
            );
            // 消費する `??`。中身の所有がそのまま渡る
            assert_eq!(
                same_as_interpreter(&format!(
                    "fn take(s: str -> int) {{ if s == \"a\": 1 else: 0 }}\n\
                     fn main(-> int) {{\n\
                     \x20 let s: str? = {init}\n\
                     \x20 take(move s ?? \"d\")\n\
                     }}\n"
                )),
                expected,
                "消費 {init}"
            );
        }
    }

    /// 活きていない payload には触らない。
    ///
    /// `Nil` の側には中身が無いので、そこを読んだり落としたりすれば壊れる
    #[test]
    fn 活きていないpayloadは読まれも落とされもしない() {
        assert_eq!(
            same_as_interpreter(
                "enum L { N\n C(str) }\n\
                 fn label(v: L -> int) {\n\
                 \x20 match v {\n\
                 \x20   L::C(t): if t == \"a\": 1 else: 2\n\
                 \x20   L::N: 0\n\
                 \x20 }\n\
                 }\n\
                 fn main(-> int) {\n\
                 \x20 let empty = L::N\n\
                 \x20 let full = L::C(\"a\")\n\
                 \x20 label(move empty) * 10 + label(move full)\n\
                 }\n"
            ),
            1
        );
    }

    /// guard は宣言順に試され、外れたら次の arm へ落ちる
    #[test]
    fn guardは宣言順に試される() {
        for (value, expected) in [(1, 10), (2, 99), (9, 99)] {
            assert_eq!(
                same_as_interpreter(&format!(
                    "enum L {{ N\n C(int) }}\n\
                     fn main(-> int) {{\n\
                     \x20 let v = L::C({value})\n\
                     \x20 match v {{\n\
                     \x20   L::C(n) if n == 1: 10\n\
                     \x20   L::N: 30\n\
                     \x20   _: 99\n\
                     \x20 }}\n\
                     }}\n"
                )),
                expected,
                "{value}"
            );
        }
    }

    /// guard が外れた arm の束縛も、そこで落ちる
    #[test]
    fn guardが外れたarmの束縛も落ちる() {
        let src = "enum L { N\n C(str) }\n\
                   fn main(-> int) {\n\
                   \x20 let mut n = 0\n\
                   \x20 while (n == 2000) == false {\n\
                   \x20   let v = L::C(\"guarded\")\n\
                   \x20   n = n + match move v {\n\
                   \x20     L::C(t) if t == \"other\": 0\n\
                   \x20     _: 1\n\
                   \x20   }\n\
                   \x20 }\n\
                   \x20 n\n\
                   }\n";
        let bytes = compile(src, &[]).expect("生成できるはず");
        validate(&bytes).expect("検証を通るはず");
        assert_eq!(invoke_capped(&bytes, ENTRY_EXPORT, 1), Ok(vec![2000]));
    }

    /// 借りた payload は元の値を消費しない。何度でも見られる
    #[test]
    fn 借りたpayloadは対象を残す() {
        assert_eq!(
            same_as_interpreter(
                "enum L { N\n C(str) }\n\
                 fn main(-> int) {\n\
                 \x20 let v = L::C(\"a\")\n\
                 \x20 let first = match v {\n\
                 \x20   L::C(t): if t == \"a\": 1 else: 0\n\
                 \x20   L::N: 0\n\
                 \x20 }\n\
                 \x20 let second = match v {\n\
                 \x20   L::C(t): if t == \"a\": 1 else: 0\n\
                 \x20   L::N: 0\n\
                 \x20 }\n\
                 \x20 first + second\n\
                 }\n"
            ),
            2
        );
    }

    /// 一時値を借りた形で `match` しても、arm を抜けたところで落ちる
    #[test]
    fn 一時値を対象にしたmatchも掃除される() {
        let src = "enum L { N\n C(str) }\n\
                   fn make(-> L) { L::C(\"made\") }\n\
                   fn scored(b: bool -> int) { if b: 1 else: 0 }\n\
                   fn main(-> int) {\n\
                   \x20 let mut n = 0\n\
                   \x20 while (n == 2000) == false {\n\
                   \x20   n = n + match make() {\n\
                   \x20     L::C(t): scored(t == \"made\")\n\
                   \x20     L::N: 0\n\
                   \x20   }\n\
                   \x20 }\n\
                   \x20 n\n\
                   }\n";
        let bytes = compile(src, &[]).expect("生成できるはず");
        validate(&bytes).expect("検証を通るはず");
        assert_eq!(invoke_capped(&bytes, ENTRY_EXPORT, 1), Ok(vec![2000]));
    }

    /// Copy な optional は struct の区画にも直に置ける
    #[test]
    fn copyなoptionalはstructの区画にも置ける() {
        for (init, expected) in [("5", 5), ("nil", 7)] {
            assert_eq!(
                same_as_interpreter(&format!(
                    "struct Row {{ id: int, mark: int? }}\n\
                     fn main(-> int) {{\n\
                     \x20 let mut r = Row {{ id = 1, mark = {init} }}\n\
                     \x20 let first = r.mark ?? 7\n\
                     \x20 r.mark = 9\n\
                     \x20 first * 100 + (r.mark ?? 0)\n\
                     }}\n"
                )),
                expected * 100 + 9,
                "{init}"
            );
        }
        // 区画に置いた optional も、等値は tag と中身の両方を見る
        for (left, right, expected) in [("5", "5", 1), ("5", "nil", 0), ("nil", "nil", 1)] {
            assert_eq!(
                same_as_interpreter(&format!(
                    "struct Row {{ mark: int? }}\n\
                     fn main(-> int) {{\n\
                     \x20 let a = Row {{ mark = {left} }}\n\
                     \x20 let b = Row {{ mark = {right} }}\n\
                     \x20 if a == b: 1 else: 0\n\
                     }}\n"
                )),
                expected,
                "{left} == {right}"
            );
        }
    }

    /// 消費する `match` は payload の所有を arm へ渡し、束ねなかった区画を落とす
    #[test]
    fn 消費するmatchはpayloadの所有を渡す() {
        assert_eq!(
            same_as_interpreter(
                "enum L { N\n C(str, str) }\n\
                 fn take(s: str -> int) { if s == \"kept\": 1 else: 0 }\n\
                 fn main(-> int) {\n\
                 \x20 let v = L::C(\"kept\", \"dropped\")\n\
                 \x20 match move v {\n\
                 \x20   L::C(kept, _): take(move kept)\n\
                 \x20   L::N: 0\n\
                 \x20 }\n\
                 }\n"
            ),
            1
        );
    }

    /// enum も optional も、深い複製は元と記憶を共有しない
    #[test]
    fn enumとoptionalのcloneは独立する() {
        assert_eq!(
            same_as_interpreter(
                "enum L { N\n C(str) }\n\
                 fn main(-> int) {\n\
                 \x20 let a = L::C(\"same\")\n\
                 \x20 let b = a.clone()\n\
                 \x20 if a == b: 1 else: 0\n\
                 }\n"
            ),
            1
        );
        assert_eq!(
            same_as_interpreter(
                "fn main(-> int) {\n\
                 \x20 let a: str? = \"same\"\n\
                 \x20 let b = a.clone()\n\
                 \x20 if a == b: 1 else: 0\n\
                 }\n"
            ),
            1
        );
    }

    /// 等値は tag を先に見る。同じ tag のときだけ中身へ潜る
    #[test]
    fn enumとoptionalの等値はタグから決まる() {
        for (left, right, expected) in [
            ("L::C(\"a\")", "L::C(\"a\")", 1),
            ("L::C(\"a\")", "L::C(\"b\")", 0),
            ("L::N", "L::N", 1),
            ("L::N", "L::C(\"a\")", 0),
        ] {
            assert_eq!(
                same_as_interpreter(&format!(
                    "enum L {{ N\n C(str) }}\n\
                     fn main(-> int) {{\n\
                     \x20 let a = {left}\n\
                     \x20 let b = {right}\n\
                     \x20 if a == b: 1 else: 0\n\
                     }}\n"
                )),
                expected,
                "{left} == {right}"
            );
        }
        for (left, right, expected) in [
            ("\"a\"", "\"a\"", 1),
            ("\"a\"", "\"b\"", 0),
            ("nil", "nil", 1),
            ("nil", "\"a\"", 0),
        ] {
            assert_eq!(
                same_as_interpreter(&format!(
                    "fn main(-> int) {{\n\
                     \x20 let a: str? = {left}\n\
                     \x20 let b: str? = {right}\n\
                     \x20 if a == b: 1 else: 0\n\
                     }}\n"
                )),
                expected,
                "optional {left} == {right}"
            );
        }
    }

    /// `indirect` な payload は再帰する形を作る。鎖を辿っても根の大きさは一定
    #[test]
    fn 再帰するenumを辿れる() {
        assert_eq!(
            same_as_interpreter(
                "enum List { Nil\n Cons(int, indirect List) }\n\
                 fn total(l: List -> int) {\n\
                 \x20 match move l {\n\
                 \x20   List::Cons(head, rest): head + total(move rest)\n\
                 \x20   List::Nil: 0\n\
                 \x20 }\n\
                 }\n\
                 fn main(-> int) {\n\
                 \x20 let l = List::Cons(1, List::Cons(2, List::Cons(3, List::Nil)))\n\
                 \x20 total(move l)\n\
                 }\n"
            ),
            6
        );
        // 深く複製しても、元と複製は別々に生き死にする
        assert_eq!(
            same_as_interpreter(
                "enum List { Nil\n Cons(int, indirect List) }\n\
                 fn main(-> int) {\n\
                 \x20 let l = List::Cons(1, List::Cons(2, List::Nil))\n\
                 \x20 let copy = l.clone()\n\
                 \x20 if l == copy: 1 else: 0\n\
                 }\n"
            ),
            1
        );
    }

    /// `indirect next: Node?` の鎖も組める。`nil` は割り当てを作らない
    #[test]
    fn indirectなoptionalの鎖を組める() {
        assert_eq!(
            same_as_interpreter(
                "struct Node { value: int, indirect next: Node? }\n\
                 fn main(-> int) {\n\
                 \x20 let tail = Node { value = 3, next = nil }\n\
                 \x20 let mid = Node { value = 2, next = move tail }\n\
                 \x20 let head = Node { value = 1, next = move mid }\n\
                 \x20 let same = head.clone()\n\
                 \x20 if head == same: head.value else: 0\n\
                 }\n"
            ),
            1
        );
    }

    /// enum も optional も、有界なループなら記憶を使い回す
    #[test]
    fn enumとoptionalのループは記憶を使い回す() {
        for src in [
            // 消費する match
            "enum L { N\n C(str) }\n\
             fn take(s: str -> int) { if s == \"looped\": 1 else: 0 }\n\
             fn main(-> int) {\n\
             \x20 let mut n = 0\n\
             \x20 while (n == 2000) == false {\n\
             \x20   let v = L::C(\"looped\")\n\
             \x20   n = n + match move v {\n\
             \x20     L::C(t): take(move t)\n\
             \x20     L::N: 0\n\
             \x20   }\n\
             \x20 }\n\
             \x20 n\n\
             }\n",
            // 借りた match。対象は毎周回で落ちる
            "enum L { N\n C(str) }\n\
             fn scored(b: bool -> int) { if b: 1 else: 0 }\n\
             fn main(-> int) {\n\
             \x20 let mut n = 0\n\
             \x20 while (n == 2000) == false {\n\
             \x20   let v = L::C(\"looped\")\n\
             \x20   n = n + match v {\n\
             \x20     L::C(t): scored(t == \"looped\")\n\
             \x20     L::N: 0\n\
             \x20   }\n\
             \x20 }\n\
             \x20 n\n\
             }\n",
            // 借りた `??` と消費する `??`
            "fn scored(b: bool -> int) { if b: 1 else: 0 }\n\
             fn main(-> int) {\n\
             \x20 let mut n = 0\n\
             \x20 while (n == 2000) == false {\n\
             \x20   let s: str? = \"looped\"\n\
             \x20   n = n + scored((s ?? \"d\") == \"looped\")\n\
             \x20 }\n\
             \x20 n\n\
             }\n",
            "fn take(s: str -> int) { if s == \"looped\": 1 else: 0 }\n\
             fn main(-> int) {\n\
             \x20 let mut n = 0\n\
             \x20 while (n == 2000) == false {\n\
             \x20   let s: str? = \"looped\"\n\
             \x20   n = n + take(move s ?? \"d\")\n\
             \x20 }\n\
             \x20 n\n\
             }\n",
            // 再帰する enum を作って捨てる
            "enum List { Nil\n Cons(str, indirect List) }\n\
             fn scored(b: bool -> int) { if b: 1 else: 0 }\n\
             fn main(-> int) {\n\
             \x20 let mut n = 0\n\
             \x20 while (n == 500) == false {\n\
             \x20   let l = List::Cons(\"a\", List::Cons(\"b\", List::Nil))\n\
             \x20   let copy = l.clone()\n\
             \x20   n = n + scored(l == copy)\n\
             \x20 }\n\
             \x20 n\n\
             }\n",
        ] {
            let bytes = compile(src, &[]).expect("生成できるはず");
            validate(&bytes).expect("検証を通るはず");
            let expected = if src.contains("500") { 500 } else { 2000 };
            assert_eq!(
                invoke_capped(&bytes, ENTRY_EXPORT, 1),
                Ok(vec![expected]),
                "{src}"
            );
        }
    }

    /// 排他で束ねた payload は、対象の中身をその場で書き換えられる。
    ///
    /// 借用は署名に出せないので、書き換えは arm の中で直に行う
    #[test]
    fn 排他のmatchはpayloadを書き換える() {
        assert_eq!(
            same_as_interpreter(
                "struct User { rank: int }\n\
                 enum Lookup { Missing\n Found(User) }\n\
                 fn main(-> int) {\n\
                 \x20 let mut l = Lookup::Found(User { rank = 1 })\n\
                 \x20 let first = match &mut l {\n\
                 \x20   Lookup::Found(u) { u.rank = u.rank + 1\n u.rank }\n\
                 \x20   Lookup::Missing: 0\n\
                 \x20 }\n\
                 \x20 let second = match &mut l {\n\
                 \x20   Lookup::Found(u) { u.rank = u.rank + 1\n u.rank }\n\
                 \x20   Lookup::Missing: 0\n\
                 \x20 }\n\
                 \x20 first * 10 + second\n\
                 }\n"
            ),
            23
        );
    }

    /// catch-all の arm は、名指ししなかった variant を全部受ける
    #[test]
    fn catch_allのarmが残りを受ける() {
        for (value, expected) in [("A", 1), ("B", 9), ("C", 9)] {
            assert_eq!(
                same_as_interpreter(&format!(
                    "enum Kind {{ A\n B\n C }}\n\
                     fn main(-> int) {{\n\
                     \x20 let k = Kind::{value}\n\
                     \x20 match k {{\n\
                     \x20   Kind::A: 1\n\
                     \x20   _: 9\n\
                     \x20 }}\n\
                     }}\n"
                )),
                expected,
                "{value}"
            );
        }
    }

    /// `indirect` な optional の鎖を、消費しながら辿れる(tasks 6.4)。
    ///
    /// 記憶の上では「アドレス、0 なら `nil`」だが、値としての `Node?` は
    /// 帳簿1つ。読み出しがその食い違いを埋めているので、連結リストが
    /// 組めるだけでなく歩ける
    #[test]
    fn indirectなoptionalの鎖を辿れる() {
        assert_eq!(
            same_as_interpreter(
                "struct Node { value: int, indirect next: Node? }\n\
                 fn total(n: Node -> int) {\n\
                 \x20 let v = n.value\n\
                 \x20 let tail = move n.next ?? return v\n\
                 \x20 v + total(move tail)\n\
                 }\n\
                 fn main(-> int) {\n\
                 \x20 let tail = Node { value = 3, next = nil }\n\
                 \x20 let mid = Node { value = 2, next = move tail }\n\
                 \x20 let head = Node { value = 1, next = move mid }\n\
                 \x20 total(move head)\n\
                 }\n"
            ),
            6
        );
    }

    /// 取り出した `nil` も、値としての空の帳簿になる
    #[test]
    fn indirectなoptionalの空も取り出せる() {
        assert_eq!(
            same_as_interpreter(
                "struct Node { value: int, indirect next: Node? }\n\
                 fn main(-> int) {\n\
                 \x20 let head = Node { value = 1, next = nil }\n\
                 \x20 let peeked = head.next\n\
                 \x20 let fallback = move peeked ?? Node { value = 9, next = nil }\n\
                 \x20 fallback.value\n\
                 }\n"
            ),
            9
        );
    }

    /// 鎖を組んでは辿るループでも記憶は漏れない。
    ///
    /// 包み直した帳簿・移した中身・空になった子の根が全部返らないと、
    /// 1ページでは回りきらない
    #[test]
    fn 鎖を辿るループでも記憶は漏れない() {
        let src = "struct Node { value: int, indirect next: Node? }\n\
                   fn total(n: Node -> int) {\n\
                   \x20 let v = n.value\n\
                   \x20 let tail = move n.next ?? return v\n\
                   \x20 v + total(move tail)\n\
                   }\n\
                   fn main(-> int) {\n\
                   \x20 let mut n = 0\n\
                   \x20 while (n == 1000) == false {\n\
                   \x20   let tail = Node { value = 1, next = nil }\n\
                   \x20   let head = Node { value = 1, next = move tail }\n\
                   \x20   n = n + total(move head) - 1\n\
                   \x20 }\n\
                   \x20 n\n\
                   }\n";
        let bytes = compile(src, &[]).expect("生成できるはず");
        validate(&bytes).expect("検証を通るはず");
        assert_eq!(invoke_capped(&bytes, ENTRY_EXPORT, 1), Ok(vec![1000]));
    }

    // -----------------------------------------------------------------------
    // 配列と `for`(tasks 7.1〜7.6)
    // -----------------------------------------------------------------------

    /// 共有 `for` は配列を消費しない。同じ配列を二度歩ける
    #[test]
    fn 共有forは配列を残す() {
        assert_eq!(
            same_as_interpreter(
                "fn main(-> int) {\n\
                 \x20 let xs = [1, 2, 3]\n\
                 \x20 let mut total = 0\n\
                 \x20 for x in xs { total = total + x }\n\
                 \x20 for x in xs { total = total + x }\n\
                 \x20 total\n\
                 }\n"
            ),
            12
        );
    }

    /// 一時値の配列を素で回しても、要素は借りたまま。配列だけが後で返る
    #[test]
    fn 一時値の配列を素で回せる() {
        assert_eq!(
            same_as_interpreter(
                "fn main(-> int) {\n\
                 \x20 let mut hits = 0\n\
                 \x20 for s in [\"a\", \"b\", \"a\"] { if s == \"a\" { hits = hits + 1 } }\n\
                 \x20 hits\n\
                 }\n"
            ),
            2
        );
    }

    /// 空の配列は一周も回らない
    #[test]
    fn 空の配列は一周も回らない() {
        assert_eq!(
            same_as_interpreter(
                "fn main(-> int) {\n\
                 \x20 let xs: [int] = []\n\
                 \x20 let mut total = 7\n\
                 \x20 for x in xs { total = total + x }\n\
                 \x20 total\n\
                 }\n"
            ),
            7
        );
    }

    /// 借りた非 Copy 要素は場所のまま。配列も要素もそのまま残る
    #[test]
    fn 借りた要素は配列を残す() {
        assert_eq!(
            same_as_interpreter(
                "fn main(-> int) {\n\
                 \x20 let xs = [\"a\", \"b\", \"a\"]\n\
                 \x20 let mut hits = 0\n\
                 \x20 for s in xs { if s == \"a\" { hits = hits + 1 } }\n\
                 \x20 if xs == [\"a\", \"b\", \"a\"]: hits else: 0\n\
                 }\n"
            ),
            2
        );
    }

    /// 排他 `for` は要素をその場で書き換える。構造は変わらない
    #[test]
    fn 排他forは要素を書き換える() {
        assert_eq!(
            same_as_interpreter(
                "struct Cell { n: int }\n\
                 fn main(-> int) {\n\
                 \x20 let mut xs = [Cell { n = 1 }, Cell { n = 2 }]\n\
                 \x20 let mut k = 10\n\
                 \x20 for c in &mut xs { c.n = k\n k = k + 1 }\n\
                 \x20 let mut total = 0\n\
                 \x20 for c in xs { total = total + c.n }\n\
                 \x20 total\n\
                 }\n"
            ),
            21
        );
    }

    /// 消費 `for` は要素の所有を周回へ渡す。使い切った配列は一度だけ返る
    #[test]
    fn 消費forは要素の所有を渡す() {
        assert_eq!(
            same_as_interpreter(
                "fn take(s: str -> int) { if s == \"a\": 1 else: 0 }\n\
                 fn main(-> int) {\n\
                 \x20 let xs = [\"a\", \"b\", \"a\"]\n\
                 \x20 let mut hits = 0\n\
                 \x20 for s in move xs { hits = hits + take(move s) }\n\
                 \x20 hits\n\
                 }\n"
            ),
            2
        );
    }

    /// 途中で抜けても、渡し終えた要素は二度落ちない
    #[test]
    fn 消費forを途中で抜けても二度落ちない() {
        assert_eq!(
            same_as_interpreter(
                "fn take(s: str -> int) { if s == \"stop\": 1 else: 0 }\n\
                 fn main(-> int) {\n\
                 \x20 let xs = [\"go\", \"stop\", \"go\"]\n\
                 \x20 for s in move xs {\n\
                 \x20   if take(move s) == 1 { return 5 }\n\
                 \x20 }\n\
                 \x20 0\n\
                 }\n"
            ),
            5
        );
    }

    /// 入れ子の配列と optional の要素も、深く複製・比較できる
    #[test]
    fn 入れ子とoptionalの要素を扱える() {
        assert_eq!(
            same_as_interpreter(
                "fn main(-> int) {\n\
                 \x20 let xs = [[\"a\"], [\"b\", \"c\"]]\n\
                 \x20 let ys = xs.clone()\n\
                 \x20 if (xs == ys) == false { return 0 }\n\
                 \x20 let os: [str?] = [\"a\", nil]\n\
                 \x20 let ps = os.clone()\n\
                 \x20 if os == ps: 1 else: 0\n\
                 }\n"
            ),
            1
        );
    }

    /// 配列の等値は長さから決まり、違う要素を見つけたところで打ち切る
    #[test]
    fn 配列の等値は長さと要素で決まる() {
        for (other, expected) in [("[1, 2, 3]", 1), ("[1, 2]", 0), ("[1, 2, 4]", 0), ("[]", 0)] {
            assert_eq!(
                same_as_interpreter(&format!(
                    "fn main(-> int) {{\n\
                     \x20 let xs = [1, 2, 3]\n\
                     \x20 let ys: [int] = {other}\n\
                     \x20 if xs == ys: 1 else: 0\n\
                     }}\n"
                )),
                expected,
                "{other}"
            );
        }
    }

    /// 深い複製は元と記憶を共有しない
    #[test]
    fn 配列のcloneは中身まで独立する() {
        assert_eq!(
            same_as_interpreter(
                "struct Cell { n: int }\n\
                 fn main(-> int) {\n\
                 \x20 let xs = [Cell { n = 1 }]\n\
                 \x20 let mut ys = xs.clone()\n\
                 \x20 for c in &mut ys { c.n = 9 }\n\
                 \x20 let mut total = 0\n\
                 \x20 for c in xs { total = total + c.n }\n\
                 \x20 for c in ys { total = total + c.n }\n\
                 \x20 total\n\
                 }\n"
            ),
            10
        );
    }

    /// 有界なループなら、3つの反復モードのどれでも記憶を使い回す
    #[test]
    fn 配列のループは記憶を使い回す() {
        for src in [
            // 共有
            "fn main(-> int) {\n\
             \x20 let mut n = 0\n\
             \x20 while (n == 1000) == false {\n\
             \x20   let xs = [\"a\", \"b\"]\n\
             \x20   for s in xs { n = n + 0 }\n\
             \x20   n = n + 1\n\
             \x20 }\n\
             \x20 n\n\
             }\n",
            // 消費。要素も配列も毎周回で返る
            "fn take(s: str -> int) { if s == \"a\": 1 else: 0 }\n\
             fn main(-> int) {\n\
             \x20 let mut n = 0\n\
             \x20 while (n == 1000) == false {\n\
             \x20   let xs = [\"a\", \"b\"]\n\
             \x20   for s in move xs { n = n + take(move s) }\n\
             \x20 }\n\
             \x20 n\n\
             }\n",
            // 途中で抜ける消費。残りの要素も配列も返る
            "fn take(s: str -> int) { if s == \"a\": 1 else: 0 }\n\
             fn stopped(-> int) {\n\
             \x20 let xs = [\"a\", \"b\", \"c\"]\n\
             \x20 for s in move xs { return take(move s) }\n\
             \x20 0\n\
             }\n\
             fn main(-> int) {\n\
             \x20 let mut n = 0\n\
             \x20 while (n == 1000) == false { n = n + stopped() }\n\
             \x20 n\n\
             }\n",
        ] {
            let bytes = compile(src, &[]).expect("生成できるはず");
            validate(&bytes).expect("検証を通るはず");
            assert_eq!(
                invoke_capped(&bytes, ENTRY_EXPORT, 1),
                Ok(vec![1000]),
                "{src}"
            );
        }
    }

    // -----------------------------------------------------------------------
    // 組み込みの `push`(MAP-075)
    // -----------------------------------------------------------------------

    /// 容量の境界(1→2→4)を越えても、要素の並びと長さは interpreter と一致する
    #[test]
    fn pushは容量の境界を越えても同じ配列を作る() {
        assert_eq!(
            same_as_interpreter(
                "fn main(-> int) {\n\
                 \x20 let mut xs = [1]\n\
                 \x20 xs.push(2)\n\
                 \x20 xs.push(3)\n\
                 \x20 xs.push(4)\n\
                 \x20 xs.push(5)\n\
                 \x20 let mut total = 0\n\
                 \x20 let mut count = 0\n\
                 \x20 for x in &xs { total = total + x }\n\
                 \x20 for x in &xs { count = count + 1 }\n\
                 \x20 total * 10 + count\n\
                 }\n"
            ),
            155
        );
    }

    /// 容量 0 の配列への初回 push も通る
    #[test]
    fn 空の配列へのpushも通る() {
        assert_eq!(
            same_as_interpreter(
                "fn main(-> int) {\n\
                 \x20 let mut xs: [int] = []\n\
                 \x20 xs.push(7)\n\
                 \x20 let mut total = 0\n\
                 \x20 for x in xs { total = total + x }\n\
                 \x20 total\n\
                 }\n"
            ),
            7
        );
    }

    /// 所有する要素を push しても、伸ばした先で中身は生きたまま残る
    #[test]
    fn pushした所有要素は伸ばしても残る() {
        assert_eq!(
            same_as_interpreter(
                "fn main(-> int) {\n\
                 \x20 let mut xs = [\"a\"]\n\
                 \x20 xs.push(\"b\")\n\
                 \x20 xs.push(\"c\")\n\
                 \x20 let mut hits = 0\n\
                 \x20 for s in &xs { if s == \"a\" { hits = hits + 1 } }\n\
                 \x20 for s in &xs { if s == \"c\" { hits = hits + 10 } }\n\
                 \x20 hits\n\
                 }\n"
            ),
            11
        );
    }

    /// 有界なループの中で作っては捨てる配列は、伸ばして解放した buffer を
    /// 使い回す。1ページに縛って走らせるので、漏れていれば trap する。
    ///
    /// 伸ばす側を関数に切り出してあるのは、同じ本体で `&mut` レシーバを二度
    /// 借りるループが所有権検査に通らないため — `&mut c.bump(..)` を二度書く
    /// ループと同じ既存の制限で、`push` に固有のものではない
    #[test]
    fn pushで伸ばした記憶は使い回される() {
        let src = "fn built(-> int) {\n\
                   \x20 let mut xs = [1]\n\
                   \x20 xs.push(2)\n\
                   \x20 xs.push(3)\n\
                   \x20 xs.push(4)\n\
                   \x20 xs.push(5)\n\
                   \x20 let mut total = 0\n\
                   \x20 for x in xs { total = total + x }\n\
                   \x20 total\n\
                   }\n\
                   fn main(-> int) {\n\
                   \x20 let mut n = 0\n\
                   \x20 while (n == 2000) == false {\n\
                   \x20   if built() == 15 { n = n + 1 } else { return 0 }\n\
                   \x20 }\n\
                   \x20 n\n\
                   }\n";
        let bytes = compile(src, &[]).expect("生成できるはず");
        validate(&bytes).expect("検証を通るはず");
        assert_eq!(invoke_capped(&bytes, ENTRY_EXPORT, 1), Ok(vec![2000]));
    }

    /// 伸ばすための割り当てが取れなければ、既存の allocator 規約どおり
    /// `unreachable` で落ちる(ADR-0011 §3)。push 固有の失敗値は無いので、
    /// 呼び出し側からは trap としてしか観測できない。上限を広げれば同じ
    /// プログラムが通ることも見る
    #[test]
    fn pushの割り当てが取れなければtrapする() {
        let src = "fn main(-> int) {\n\
                   \x20 let mut xs = [0]\n\
                   \x20 let mut n = 0\n\
                   \x20 while (n == 20000) == false {\n\
                   \x20   xs.push(n)\n\
                   \x20   n = n + 1\n\
                   \x20 }\n\
                   \x20 n\n\
                   }\n";
        let bytes = compile(src, &[]).expect("生成できるはず");
        validate(&bytes).expect("検証を通るはず");
        assert!(
            invoke_capped(&bytes, ENTRY_EXPORT, 1).is_err(),
            "1ページには収まらない"
        );
        assert_eq!(invoke_capped(&bytes, ENTRY_EXPORT, 16), Ok(vec![20000]));
    }

    // -----------------------------------------------------------------------
    // `.?` の伝播(tasks 5.2)
    // -----------------------------------------------------------------------

    /// `nil` のレシーバはインタプリタが受け付けない(「compound value の
    /// location が必要です」)ので、そちらは Wasm 単体で見る。インタプリタ側の
    /// 欠陥はこの change の範囲外
    #[test]
    fn optionalフィールドは伝播する() {
        assert_eq!(
            same_as_interpreter(
                "struct User { rank: int }\n\
                 fn main(-> int) {\n\
                 \x20 let u: User? = User { rank = 7 }\n\
                 \x20 u.?rank ?? 0\n\
                 }\n"
            ),
            7
        );
        assert_eq!(
            run_int(
                "struct User { rank: int }\n\
                 fn main(-> int) {\n\
                 \x20 let u: User? = nil\n\
                 \x20 u.?rank ?? 0\n\
                 }\n"
            ),
            0
        );
    }

    /// 宣言型が既に optional なら 1 bit は増えない。鎖のどこが切れても伝わる
    #[test]
    fn optionalフィールドの鎖はどこで切れても伝わる() {
        assert_eq!(
            same_as_interpreter(
                "struct Inner { n: int }\n\
                 struct Outer { inner: Inner? }\n\
                 fn main(-> int) {\n\
                 \x20 let o: Outer? = Outer { inner = Inner { n = 5 } }\n\
                 \x20 o.?inner.?n ?? 0\n\
                 }\n"
            ),
            5
        );
        for outer in ["Outer { inner = nil }", "nil"] {
            assert_eq!(
                run_int(&format!(
                    "struct Inner {{ n: int }}\n\
                     struct Outer {{ inner: Inner? }}\n\
                     fn main(-> int) {{\n\
                     \x20 let o: Outer? = {outer}\n\
                     \x20 o.?inner.?n ?? 0\n\
                     }}\n"
                )),
                0,
                "{outer}"
            );
        }
    }

    /// 所有するフィールドも読める。読むたびに帳簿を組み立てるので持ち主が付く
    #[test]
    fn 所有するoptionalフィールドも読める() {
        assert_eq!(
            same_as_interpreter(
                "struct User { name: str }\n\
                 fn take(s: str -> int) { if s == \"a\": 1 else: 0 }\n\
                 fn main(-> int) {\n\
                 \x20 let u: User? = User { name = \"a\" }\n\
                 \x20 take(u.?name.clone() ?? \"b\")\n\
                 }\n"
            ),
            1
        );
        assert_eq!(
            run_int(
                "struct User { name: str }\n\
                 fn take(s: str -> int) { if s == \"a\": 1 else: 0 }\n\
                 fn main(-> int) {\n\
                 \x20 let u: User? = nil\n\
                 \x20 take(u.?name.clone() ?? \"b\")\n\
                 }\n"
            ),
            0
        );
    }

    /// `.?` は毎回帳簿を確保する。有界ループで記憶が増えないこと
    #[test]
    fn optionalフィールドのループは記憶を使い回す() {
        let src = "struct User { name: str }\n\
                   fn take(s: str -> int) { if s == \"a\": 1 else: 0 }\n\
                   fn main(-> int) {\n\
                   \x20 let mut n = 0\n\
                   \x20 while (n == 1000) == false {\n\
                   \x20   let u: User? = User { name = \"a\" }\n\
                   \x20   n = n + take(u.?name.clone() ?? \"b\")\n\
                   \x20 }\n\
                   \x20 n\n\
                   }\n";
        let bytes = compile(src, &[]).expect("生成できるはず");
        validate(&bytes).expect("検証を通るはず");
        assert_eq!(invoke_capped(&bytes, ENTRY_EXPORT, 1), Ok(vec![1000]));
    }

    // -----------------------------------------------------------------------
    // 維持する owned-data 総合 fixture(task 9.1)
    // -----------------------------------------------------------------------

    /// 小さな単機能テストとは別に、owned-data の対応面を実ファイルで維持する。
    /// 戻り値は各操作と変更後の状態から組み立てるため、参照インタプリタとの比較が
    /// 最終 mutation state の差も検出する。
    #[test]
    fn owned_data総合fixtureはインタプリタと一致する() {
        for (path, expected) in [
            (
                "tests/fixtures/wasm-owned-data/all-constructs.rd",
                2_333_116,
            ),
            ("tests/fixtures/wasm-owned-data/final-mutation-state.rd", 23),
        ] {
            let src = std::fs::read_to_string(path).expect("fixture を読めるはず");
            assert_eq!(same_as_interpreter(&src), expected, "{path}");
        }
    }

    /// `indirect` の区画への `.?` は、黙って壊れず止まる
    #[test]
    fn indirectへのoptionalフィールド参照はビルドを止める() {
        let errors = compile(
            "struct Node { value: int, indirect next: Node? }\n\
             fn main(-> int) {\n\
             \x20 let n: Node? = Node { value = 1, next = nil }\n\
             \x20 let tail = n.?next.clone() ?? Node { value = 2, next = nil }\n\
             \x20 tail.value\n\
             }\n",
            &[],
        )
        .expect_err("止まるはず");
        assert!(
            messages(&errors).contains("`indirect` なフィールド `next` の `.?` 参照"),
            "{}",
            messages(&errors)
        );
    }

    /// メタデータを知らないエンジンでも検証・実行できる
    #[test]
    fn メタデータを読まなくてもモジュールは動く() {
        let bytes = compile(SURFACE, &["touch"]).expect("生成できるはず");
        validate(&bytes).expect("検証を通るはず");
        assert_eq!(scalars(&invoke(&bytes, ENTRY_EXPORT, &[]).unwrap()), [3]);
    }

    // -----------------------------------------------------------------------
    // Rhodolite Wasm ABI v1(tasks 8.1〜8.8)
    // -----------------------------------------------------------------------

    /// ホストの役。`memory` を直に読み書きして、正準 bytes を受け渡し領域へ置く
    struct Host {
        store: wasmi::Store<()>,
        instance: wasmi::Instance,
        memory: wasmi::Memory,
    }

    impl Host {
        fn new(bytes: &[u8]) -> Host {
            validate(bytes).expect("検証を通るはず");
            let engine = wasmi::Engine::default();
            let module = wasmi::Module::new(&engine, bytes).expect("読めるはず");
            let mut store = wasmi::Store::new(&engine, ());
            let instance = wasmi::Linker::new(&engine)
                .instantiate_and_start(&mut store, &module)
                .expect("立ち上がるはず");
            let memory = instance
                .get_memory(&store, "memory")
                .expect("ABI v1 は memory を出す");
            Host {
                store,
                instance,
                memory,
            }
        }

        fn call(
            &mut self,
            name: &str,
            args: &[wasmi::Val],
            results: usize,
        ) -> Result<Vec<i64>, String> {
            let func = self
                .instance
                .get_func(&self.store, name)
                .ok_or_else(|| format!("`{name}` が無い"))?;
            let mut out = vec![wasmi::Val::I32(0); results];
            func.call(&mut self.store, args, &mut out)
                .map_err(|e| e.to_string())?;
            Ok(scalars(&out))
        }

        /// 受け渡し領域を取り直して、そこへ bytes を書く
        fn stage(&mut self, bytes: &[u8]) -> i32 {
            let area = self
                .call(
                    "__rhodolite_abi_reserve",
                    &[wasmi::Val::I32(bytes.len() as i32)],
                    1,
                )
                .expect("予約できるはず")[0] as i32;
            self.memory
                .write(&mut self.store, area as usize, bytes)
                .expect("書けるはず");
            area
        }

        fn read(&self, at: i32, len: i32) -> Vec<u8> {
            let mut buf = vec![0u8; len as usize];
            self.memory
                .read(&self.store, at as usize, &mut buf)
                .expect("読めるはず");
            buf
        }
    }

    /// wire 形式の組み立て。ホスト側は仕様だけを見て書ける
    fn wire_str(text: &str) -> Vec<u8> {
        let mut out = (text.len() as u32).to_le_bytes().to_vec();
        out.extend_from_slice(text.as_bytes());
        out
    }

    /// 所有する値が境界を往復する。復号は新しい所有を作る
    #[test]
    fn 豊かな値は正準bytesで往復する() {
        let bytes = compile(
            "fn echo(s: str -> str) { s }\n\
             fn main(-> int) { 0 }\n",
            &["echo"],
        )
        .expect("生成できるはず");
        let mut host = Host::new(&bytes);
        let area = host.stage(&wire_str("こんにちは"));
        let out = host
            .call(
                "echo",
                &[
                    wasmi::Val::I32(area),
                    wasmi::Val::I32(wire_str("こんにちは").len() as i32),
                ],
                2,
            )
            .expect("走るはず");
        let (at, len) = (out[0] as i32, out[1] as i32);
        assert_eq!(host.read(at, len), wire_str("こんにちは"));
    }

    /// scalar と豊かな値が混ざった署名も、順番どおりに渡る
    #[test]
    fn 混ざった署名も渡る() {
        let bytes = compile(
            "struct User { id: int, name: str }\n\
             fn pick(flag: bool, u: User, bump: int -> int) {\n\
             \x20 if flag: u.id + bump else: 0\n\
             }\n\
             fn main(-> int) { 0 }\n",
            &["pick"],
        )
        .expect("生成できるはず");
        let mut host = Host::new(&bytes);
        let mut encoded = 7i64.to_le_bytes().to_vec();
        encoded.extend(wire_str("u"));
        let area = host.stage(&encoded);
        assert_eq!(
            host.call(
                "pick",
                &[
                    wasmi::Val::I32(1),
                    wasmi::Val::I32(area),
                    wasmi::Val::I32(encoded.len() as i32),
                    wasmi::Val::I64(5),
                ],
                1,
            )
            .expect("走るはず"),
            [12]
        );
    }

    /// 入れ子と再帰も、宣言の型どおりに往復する
    #[test]
    fn 入れ子と再帰も往復する() {
        let bytes = compile(
            "enum List { Nil\n Cons(int, indirect List) }\n\
             fn total(l: List -> int) {\n\
             \x20 match move l {\n\
             \x20   List::Cons(head, rest): head + total(move rest)\n\
             \x20   List::Nil: 0\n\
             \x20 }\n\
             }\n\
             fn main(-> int) { 0 }\n",
            &["total"],
        )
        .expect("生成できるはず");
        let mut host = Host::new(&bytes);
        // Cons(1, Cons(2, Nil)) — tag は宣言順で Nil=0, Cons=1
        let mut encoded = Vec::new();
        for value in [1i64, 2] {
            encoded.extend(1u32.to_le_bytes());
            encoded.extend(value.to_le_bytes());
        }
        encoded.extend(0u32.to_le_bytes());
        let area = host.stage(&encoded);
        assert_eq!(
            host.call(
                "total",
                &[wasmi::Val::I32(area), wasmi::Val::I32(encoded.len() as i32)],
                1,
            )
            .expect("走るはず"),
            [3]
        );
    }

    /// 壊れた入力は本体が走る前に trap する
    #[test]
    fn 壊れた入力は本体の前でtrapする() {
        let bytes = compile(
            "fn echo(s: str -> str) { s }\n\
             fn main(-> int) { 0 }\n",
            &["echo"],
        )
        .expect("生成できるはず");

        for (what, encoded) in [
            // 長さが slice からはみ出す
            ("はみ出す長さ", vec![9, 0, 0, 0, b'a']),
            // 途中で切れている
            ("途中切れ", vec![4, 0, 0]),
            // UTF-8 として不正
            ("不正な UTF-8", vec![1, 0, 0, 0, 0xFF]),
            // 余分な bytes が付いている
            ("余りがある", vec![1, 0, 0, 0, b'a', b'x']),
            // overlong な符号化
            ("overlong", vec![2, 0, 0, 0, 0xC0, 0x80]),
            // surrogate
            ("surrogate", vec![3, 0, 0, 0, 0xED, 0xA0, 0x80]),
        ] {
            let mut host = Host::new(&bytes);
            let area = host.stage(&encoded);
            assert!(
                host.call(
                    "echo",
                    &[wasmi::Val::I32(area), wasmi::Val::I32(encoded.len() as i32)],
                    2,
                )
                .is_err(),
                "{what}"
            );
        }
    }

    /// 受け渡し領域の外を指す slice は受け取らない
    #[test]
    fn 領域の外を指すsliceは拒否される() {
        let bytes = compile(
            "fn echo(s: str -> str) { s }\n\
             fn main(-> int) { 0 }\n",
            &["echo"],
        )
        .expect("生成できるはず");
        let mut host = Host::new(&bytes);
        let area = host.stage(&wire_str("a"));
        for (what, at, len) in [
            ("領域の手前", area - 8, 5),
            ("領域の後ろへ伸びる", area, 4096),
            ("そもそも別の場所", 0, 5),
        ] {
            assert!(
                host.call("echo", &[wasmi::Val::I32(at), wasmi::Val::I32(len)], 2)
                    .is_err(),
                "{what}"
            );
        }
    }

    /// 予約する前に呼ばれたら、正しい slice はあり得ない
    #[test]
    fn 予約前の呼び出しは拒否される() {
        let bytes = compile(
            "fn echo(s: str -> str) { s }\n\
             fn main(-> int) { 0 }\n",
            &["echo"],
        )
        .expect("生成できるはず");
        let mut host = Host::new(&bytes);
        assert!(
            host.call("echo", &[wasmi::Val::I32(64), wasmi::Val::I32(1)], 2)
                .is_err()
        );
    }

    /// 結果の bytes は次の予約まで。取り直せば前の中身は保証されない
    #[test]
    fn 結果は次の予約で無効になる() {
        let bytes = compile(
            "fn echo(s: str -> str) { s }\n\
             fn main(-> int) { 0 }\n",
            &["echo"],
        )
        .expect("生成できるはず");
        let mut host = Host::new(&bytes);
        let encoded = wire_str("first");
        let area = host.stage(&encoded);
        let out = host
            .call(
                "echo",
                &[wasmi::Val::I32(area), wasmi::Val::I32(encoded.len() as i32)],
                2,
            )
            .expect("走るはず");
        assert_eq!(host.read(out[0] as i32, out[1] as i32), wire_str("first"));
        // 取り直すと同じ場所が別の用途で使われる
        host.stage(&wire_str("second"));
        assert_ne!(host.read(out[0] as i32, out[1] as i32), wire_str("first"));
    }

    /// scalar だけの公開面は今も ABI v0。メモリも予約入口も出さない
    #[test]
    fn scalarだけの公開面はabi_v0のまま() {
        let bytes = compile(SURFACE, &["find_user=find", "touch"]).expect("生成できるはず");
        assert!(abi_metadata(&bytes).starts_with("{\"version\":0,"));
        assert_eq!(export_names(&bytes), [ENTRY_EXPORT, "find_user", "touch"]);
    }

    /// 豊かな公開面は v1 を選び、メモリと予約入口を出す
    #[test]
    fn 豊かな公開面はabi_v1を選ぶ() {
        let bytes = compile(
            "fn echo(s: str -> str) { s }\n\
             fn main(-> int) { 0 }\n",
            &["echo"],
        )
        .expect("生成できるはず");
        assert!(abi_metadata(&bytes).starts_with("{\"version\":1,"));
        assert_eq!(
            export_names(&bytes),
            [ENTRY_EXPORT, "echo", "memory", "__rhodolite_abi_reserve"]
        );
    }

    /// メタデータは公開署名から辿れる型グラフを載せる。再帰も閉じる
    #[test]
    fn abi_v1のメタデータは型グラフを載せる() {
        let bytes = compile(
            "struct Profile { handle: str }\n\
             struct User { id: int, profile: Profile?, tags: [str] }\n\
             fn lookup(u: User -> User) { u }\n\
             fn main(-> int) { 0 }\n",
            &["lookup"],
        )
        .expect("生成できるはず");
        assert_eq!(
            abi_metadata(&bytes),
            // ID は子を訪ねる前に押さえるので、宣言を深さ優先で辿った順になる
            "{\"version\":1,\
             \"entry\":{\"name\":\"__rhodolite_main\",\"params\":[],\"result\":\"int\"},\
             \"exports\":[{\"name\":\"lookup\",\"params\":[\"t0\"],\"result\":\"t0\"}],\
             \"types\":[\
             {\"id\":\"t0\",\"kind\":\"struct\",\"name\":\"User\",\"fields\":[\
             {\"name\":\"id\",\"type\":\"int\"},\
             {\"name\":\"profile\",\"type\":\"t1\"},\
             {\"name\":\"tags\",\"type\":\"t4\"}]},\
             {\"id\":\"t1\",\"kind\":\"optional\",\"payload\":\"t2\"},\
             {\"id\":\"t2\",\"kind\":\"struct\",\"name\":\"Profile\",\"fields\":[\
             {\"name\":\"handle\",\"type\":\"t3\"}]},\
             {\"id\":\"t3\",\"kind\":\"str\"},\
             {\"id\":\"t4\",\"kind\":\"array\",\"element\":\"t3\"}]}"
        );
    }

    /// 再帰する型は ID を先に押さえるので、辿り切って閉じる
    #[test]
    fn 再帰する公開型もメタデータで閉じる() {
        let bytes = compile(
            "struct Node { value: int, indirect next: Node? }\n\
             fn head(n: Node -> int) { n.value }\n\
             fn main(-> int) { 0 }\n",
            &["head"],
        )
        .expect("生成できるはず");
        assert_eq!(
            abi_metadata(&bytes),
            "{\"version\":1,\
             \"entry\":{\"name\":\"__rhodolite_main\",\"params\":[],\"result\":\"int\"},\
             \"exports\":[{\"name\":\"head\",\"params\":[\"t0\"],\"result\":\"int\"}],\
             \"types\":[\
             {\"id\":\"t0\",\"kind\":\"struct\",\"name\":\"Node\",\"fields\":[\
             {\"name\":\"value\",\"type\":\"int\"},\
             {\"name\":\"next\",\"type\":\"t1\"}]},\
             {\"id\":\"t1\",\"kind\":\"optional\",\"payload\":\"t0\"}]}"
        );
    }

    /// 公開署名から辿れない型は、内部で使っていても表に出ない
    #[test]
    fn 非公開の型はメタデータに出ない() {
        let bytes = compile(
            "struct Hidden { note: str }\n\
             struct Shown { id: int }\n\
             fn helper(-> int) {\n let h = Hidden { note = \"x\" }\n 1\n}\n\
             fn shown(s: Shown -> int) { s.id + helper() }\n\
             fn main(-> int) { 0 }\n",
            &["shown"],
        )
        .expect("生成できるはず");
        let metadata = abi_metadata(&bytes);
        assert!(metadata.contains("\"name\":\"Shown\""), "{metadata}");
        assert!(!metadata.contains("Hidden"), "{metadata}");
    }

    /// 同じ公開面からは同じメタデータ。走査の仕方に依らない
    #[test]
    fn abi_v1のメタデータは決定的() {
        let src = "struct User { id: int, name: str }\n\
                   fn one(u: User -> int) { u.id }\n\
                   fn two(u: User -> User) { u }\n\
                   fn main(-> int) { 0 }\n";
        let first = compile(src, &["two", "one"]).expect("生成できるはず");
        let second = compile(src, &["one", "two"]).expect("生成できるはず");
        // 公開名も公開型も同じなら、書いた順が違ってもメタデータは動かない。
        // モジュールの bytes は生産の根の並びに従うので、そこまでは揃わない
        assert_eq!(abi_metadata(&first), abi_metadata(&second));
        assert_eq!(export_names(&first), export_names(&second));
    }

    /// 予約名は公開名に使えない
    #[test]
    fn abiの予約名は公開名に使えない() {
        for name in ["memory", "__rhodolite_abi_reserve"] {
            let message = abi_error(SURFACE, &[&format!("{name}=touch")]);
            assert!(message.contains("予約名"), "{name}: {message}");
        }
    }

    // -----------------------------------------------------------------------
    // ABI v0 の出力固定(tasks 1.1)
    // -----------------------------------------------------------------------

    /// バイト列の指紋。所有データ対応で emitter を作り替えるあいだ、scalar だけの
    /// プログラムの出力が動いていないことを見張る。
    ///
    /// FNV-1a を自前で持つのは、`DefaultHasher` が Rust の版をまたいで同じ値を
    /// 約束しないから。ここで欲しいのは「今日と明日で同じ」ではなく
    /// 「このバイト列なら常にこの値」
    fn fingerprint(bytes: &[u8]) -> String {
        let digest = bytes.iter().fold(0xcbf2_9ce4_8422_2325u64, |hash, byte| {
            (hash ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01b3)
        });
        format!("{}:{digest:016x}", bytes.len())
    }

    /// scalar だけのプログラムの bytes は、この変更の前後で1バイトも動かない。
    ///
    /// 指紋が変わったら、それは ABI v0 の出力が変わったということ。意図した
    /// 変更なら期待値を更新し、そうでないなら退行
    #[test]
    fn scalarのみのモジュールはバイト列が変わらない() {
        for (src, exports, expected) in [
            (
                "fn main(-> int) { 41 + 1 }\n",
                &[][..],
                "163:a3ccb6c701f0c33f",
            ),
            (
                "fn main() { assert true }\n",
                &[][..],
                "165:b2df3b8ed5689f47",
            ),
            (
                SURFACE,
                &["find_user=find", "touch"][..],
                "361:f7afc53c69601b08",
            ),
        ] {
            let bytes = compile(src, exports).expect("生成できるはず");
            validate(&bytes).expect("検証を通るはず");
            assert_eq!(fingerprint(&bytes), expected, "{src}");
        }
    }

    fn instance_signature_snapshot(
        checked: &CheckedProgram,
        production: &ProductionPlan,
        instance_id: crate::ambient_abi::InstanceId,
    ) -> String {
        let program = &checked.hir;
        let mut layouts = Layouts::default();
        plan_reachable(&mut layouts, program, &production.plan);
        let instance = production.plan.instance(instance_id);
        let lowered = lower_instance_signature(&mut layouts, program, &production.plan, instance);
        let ambient = instance
            .layout
            .into_iter()
            .flat_map(|layout| production.plan.layout(layout).fields.iter())
            .map(|(slot, _)| {
                format!(
                    "{}={}",
                    program.slots[*slot].name,
                    lowered.ambient.get(*slot).expect("hidden field がある")
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        format!(
            "{} params={:?} ambient=[{ambient}]",
            program.show_body(instance.key.body),
            lowered.params
        )
    }

    /// internal signature は callable 名ではなく specialization instance から作る。
    /// 空/value/type-only/multiple-field record と同一 callable の複数 instance を
    /// 一つの固定した snapshot にする。
    #[test]
    fn instance署名とambient_localの並びは固定される() {
        let src = "trait Clock { fn now(self -> int)\n fn zero(-> int) }\n\
                   trait Database { fn value(self -> int) }\n\
                   struct FirstClock {}\n\
                   struct SecondClock {}\n\
                   struct FirstDb {}\n\
                   impl Clock for FirstClock { fn now(self -> int) { 1 }\n fn zero(-> int) { 0 } }\n\
                   impl Clock for SecondClock { fn now(self -> int) { 2 }\n fn zero(-> int) { 0 } }\n\
                   impl Database for FirstDb { fn value(self -> int) { 3 } }\n\
                   effect clock: Clock\n\
                   effect db: Database\n\
                   fn needs(-> int) { clock.now() }\n\
                   fn zeroed(-> int) { clock::zero() }\n\
                   fn both(-> int) { clock.now() + db.value() }\n\
                   fn main(-> int) {\n\
                   \x20 let first = with clock(FirstClock {}), db(FirstDb {}) { needs() + zeroed() + both() }\n\
                   \x20 let second = with clock(SecondClock {}), db(FirstDb {}) { needs() }\n\
                   \x20 first + second\n\
                   }\n";
        let (checked, production) = plan_of(src, &[]);
        let program = &checked.hir;
        let mut snapshots = production
            .plan
            .instances()
            .filter(|(_, instance)| {
                matches!(
                    program.show_body(instance.key.body).as_str(),
                    "main" | "needs" | "zeroed" | "both"
                )
            })
            .map(|(id, _)| instance_signature_snapshot(&checked, &production, id))
            .collect::<Vec<_>>();
        snapshots.sort();
        assert_eq!(
            snapshots,
            [
                "both params=[I32, I32] ambient=[clock=0,db=1]",
                "main params=[] ambient=[]",
                "needs params=[I32] ambient=[clock=0]",
                "needs params=[I32] ambient=[clock=0]",
                "zeroed params=[] ambient=[]",
            ]
        );
    }

    #[test]
    fn instance署名はreceiverを宣言引数より先に置く() {
        let src = "struct Counter { value: int }\n\
                   impl Counter { fn add(self, delta: int -> int) { self.value + delta } }\n\
                   fn main(-> int) { Counter { value = 4 }.add(5) }\n";
        let (checked, production) = plan_of(src, &[]);
        let program = &checked.hir;
        let (id, instance) = production
            .plan
            .instances()
            .find(|(_, instance)| program.show_body(instance.key.body) == "impl Counter::add")
            .expect("method instance がある");
        let mut layouts = Layouts::default();
        plan_reachable(&mut layouts, program, &production.plan);
        let lowered = lower_instance_signature(&mut layouts, program, &production.plan, instance);
        let receiver = program.callables[lowered.callable]
            .body
            .receiver
            .expect("receiver がある");
        let parameter = program.callables[lowered.callable].params[0];
        assert_eq!(id.index(), 1, "main の次に method instance が確保される");
        assert_eq!(lowered.params, [ValType::I32, ValType::I64]);
        assert_eq!(lowered.slots[&receiver], [0]);
        assert_eq!(lowered.slots[&parameter], [1]);
    }

    /// 契約メソッドの本体も通常の specialized callable instance として下ろす。
    /// `with` / slot の呼び出し自体の data lowering は後続 task で接続するが、
    /// 到達した trait implementation の署名はここで既に ordinary method と同じ。
    #[test]
    fn trait_impl_instanceはreceiverを宣言引数より先に置く() {
        let src = "trait Score { fn add(self, delta: int -> int) }\n\
                   struct Counter { value: int }\n\
                   impl Score for Counter { fn add(self, delta: int -> int) { self.value + delta } }\n\
                   effect score: Score\n\
                   fn main(-> int) { with score(Counter { value = 4 }) { score.add(5) } }\n";
        let (checked, production) = plan_of(src, &[]);
        let program = &checked.hir;
        let (id, instance) = production
            .plan
            .instances()
            .find(|(_, instance)| {
                let hir::BodyId::Callable(callable) = instance.key.body else {
                    return false;
                };
                matches!(
                    program.callables[callable].owner,
                    hir::CallableOwner::TraitImpl(_)
                )
            })
            .expect("trait implementation instance がある");
        let mut layouts = Layouts::default();
        plan_reachable(&mut layouts, program, &production.plan);
        let lowered = lower_instance_signature(&mut layouts, program, &production.plan, instance);
        let receiver = program.callables[lowered.callable]
            .body
            .receiver
            .expect("receiver がある");
        let parameter = program.callables[lowered.callable].params[0];
        assert_eq!(
            id.index(),
            1,
            "main の次に trait implementation instance が確保される"
        );
        assert_eq!(lowered.params, [ValType::I32, ValType::I64]);
        assert_eq!(lowered.slots[&receiver], [0]);
        assert_eq!(lowered.slots[&parameter], [1]);
    }

    /// trait call は emitter が名前や trait ID を引き直さず、計画の instance
    /// 番号をそのまま function index として使う。実装の選択が違う instance は
    /// 分け、同じ選択に対する繰り返し呼び出しは1つに寄せる。
    #[test]
    fn trait_targetとfunction_indexは計画どおり固定される() {
        let src = "trait Clock { fn now(self -> int) }\n\
                   struct Frozen { at: int }\n\
                   struct Zero {}\n\
                   impl Clock for Frozen { fn now(self -> int) { self.at } }\n\
                   impl Clock for Zero { fn now(self -> int) { 0 } }\n\
                   effect clock: Clock\n\
                   fn ticks(-> int) { clock.now() }\n\
                   fn main(-> int) {\n\
                   \x20 let frozen = with clock(Frozen { at = 7 }) { ticks() + ticks() }\n\
                   \x20 let zero = with clock(Zero {}) { ticks() }\n\
                   \x20 frozen + zero\n\
                   }\n";
        let (checked, production) = plan_of(src, &[]);
        let program = &checked.hir;
        let snapshot = production
            .plan
            .instances()
            .map(|(id, instance)| {
                let providers = instance
                    .key
                    .providers
                    .iter()
                    .map(|(slot, implementation)| {
                        format!(
                            "{}={}",
                            program.slots[*slot].name,
                            program.structs[program.trait_impls[*implementation].type_].name
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(",");
                let targets = instance
                    .calls
                    .values()
                    .map(|call| call.target.index().to_string())
                    .collect::<Vec<_>>()
                    .join(",");
                format!(
                    "function#{} {} [{providers}] -> [{targets}]",
                    id.index(),
                    program.show_body(instance.key.body)
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(
            snapshot,
            [
                "function#0 main [] -> [1,1,2]",
                "function#1 ticks [clock=Frozen] -> [3]",
                "function#2 ticks [clock=Zero] -> [4]",
                "function#3 impl Frozen::now [] -> []",
                "function#4 impl Zero::now [] -> []",
            ]
        );
    }

    #[test]
    fn inherent_methodとassociated_functionは計画したtargetへ下りる() {
        assert_eq!(
            same_as_interpreter(
                "struct Counter { value: int }\n\
                 impl Counter {\n\
                 \x20 fn make(value: int -> Counter) { Counter { value = value } }\n\
                 \x20 fn add(self, delta: int -> int) { self.value + delta }\n\
                 \x20 fn read(&self -> int) { self.value }\n\
                 }\n\
                 fn main(-> int) {\n\
                 \x20 let counter = Counter::make(4)\n\
                 \x20 move counter.add(5)\n\
                 }\n"
            ),
            9
        );
        assert_eq!(
            same_as_interpreter(
                "struct Counter { value: int }\n\
                 impl Counter {\n\
                 \x20 fn make(value: int -> Counter) { Counter { value = value } }\n\
                 \x20 fn read(&self -> int) { self.value }\n\
                 }\n\
                 fn main(-> int) { Counter::make(7).read() }\n"
            ),
            7
        );
    }

    #[test]
    fn inherent_methodのreceiverとowned_dataはインタプリタと一致する() {
        let src = "struct Counter { label: str, value: int }\n\
                   impl Counter {\n\
                   \x20 fn new(label: str -> Counter) { Counter { label = label, value = 0 } }\n\
                   \x20 fn inspect(&self -> int) { self.value }\n\
                   \x20 fn bump(&mut self, by: int) { self.value = self.value + by }\n\
                   \x20 fn descend(&self, depth: int -> int) { if depth == 0: self.value else: self.descend(depth - 1) }\n\
                   \x20 fn early(&self, stop: bool -> int) { if stop { return self.value }\n 0 }\n\
                   \x20 fn replace(self, next: str -> str) { next }\n\
                   }\n\
                   fn main(-> int) {\n\
                   \x20 let mut counter = Counter::new(\"before\")\n\
                   \x20 &mut counter.bump(3)\n\
                   \x20 let score = counter.inspect() + counter.descend(2) + counter.early(true)\n\
                   \x20 let replacement = move counter.replace(\"after\")\n\
                   \x20 if replacement == \"after\": score else: 0\n\
                   }\n";
        assert_eq!(same_as_interpreter(src), 9);

        let looping = "struct Item { label: str }\n\
                       impl Item {\n\
                       \x20 fn new(label: str -> Item) { Item { label = label } }\n\
                       \x20 fn take(self, replacement: str -> str) { replacement }\n\
                       }\n\
                       fn main(-> int) {\n\
                       \x20 let mut n = 0\n\
                       \x20 while (n == 3000) == false {\n\
                       \x20   let item = Item::new(\"old\")\n\
                       \x20   let replacement = move item.take(\"new\")\n\
                       \x20   assert replacement == \"new\"\n\
                       \x20   n = n + 1\n\
                       \x20 }\n\
                       \x20 n\n\
                       }\n";
        let bytes = compile(looping, &[]).expect("生成できるはず");
        validate(&bytes).expect("検証を通るはず");
        assert_eq!(invoke_capped(&bytes, ENTRY_EXPORT, 1), Ok(vec![3000]));
    }

    /// 所有データを内部で使う ABI v0 モジュールも、scalar の公開面を保ったまま
    /// 既存の allocator・glue・関数番号を出す。この fixture は struct、enum、
    /// optional、array、clone、move、field mutation を一緒に通す。
    #[test]
    fn owned_data総合fixtureのバイト列は変わらない() {
        let src = std::fs::read_to_string("tests/fixtures/wasm-owned-data/all-constructs.rd")
            .expect("fixture を読めるはず");
        let bytes = compile(&src, &[]).expect("生成できるはず");
        validate(&bytes).expect("検証を通るはず");
        assert_eq!(fingerprint(&bytes), "6522:8ab1ea22a8f5b6b9");
        assert_eq!(
            scalars(&invoke(&bytes, ENTRY_EXPORT, &[]).expect("独立エンジンで走るはず")),
            [2_333_116]
        );
    }

    /// rich な公開値は ABI v1 の wrapper、wire codec、memory export を追加する。
    /// internal calling convention を広げても、この既存境界の bytes を動かさない。
    #[test]
    fn owned_data_abi_v1のバイト列は変わらない() {
        let src = "struct User { id: int, name: str }\n\
                   fn echo(user: User -> User) { user }\n\
                   fn main(-> int) { 0 }\n";
        let bytes = compile(src, &["echo"]).expect("生成できるはず");
        validate(&bytes).expect("検証を通るはず");
        assert_eq!(fingerprint(&bytes), "2177:9b7ff1cc2bcbe545");

        let mut host = Host::new(&bytes);
        let mut encoded = 7i64.to_le_bytes().to_vec();
        encoded.extend(wire_str("snapshot"));
        let area = host.stage(&encoded);
        let out = host
            .call(
                "echo",
                &[wasmi::Val::I32(area), wasmi::Val::I32(encoded.len() as i32)],
                2,
            )
            .expect("独立エンジンで走るはず");
        assert_eq!(host.read(out[0] as i32, out[1] as i32), encoded);
    }

    /// 固定した bytes は独立エンジンでも同じ観測を返す。指紋だけでは
    /// 「壊れたまま固定した」を捕まえられない
    #[test]
    fn 固定したモジュールは独立エンジンで同じ結果を返す() {
        let bytes = compile(SURFACE, &["find_user=find", "touch"]).expect("生成できるはず");
        assert_eq!(scalars(&invoke(&bytes, ENTRY_EXPORT, &[]).unwrap()), [3]);
        assert_eq!(
            scalars(
                &invoke(
                    &bytes,
                    "find_user",
                    &[wasmi::Val::I64(1), wasmi::Val::I32(1)]
                )
                .unwrap()
            ),
            [1]
        );
        assert_eq!(
            scalars(&invoke(&bytes, "touch", &[wasmi::Val::I64(7)]).unwrap()),
            []
        );
    }

    // -----------------------------------------------------------------------
    // 汎用具体化の生成(MAP-070)
    // -----------------------------------------------------------------------

    /// 定義順に、各関数が直に呼ぶ相手を集める。表越しの呼び出しに出会ったら
    /// その場で落とす。汎用具体化も直呼びだけで届くことの根拠
    fn direct_call_targets(bytes: &[u8]) -> Vec<Vec<u32>> {
        let mut bodies = Vec::new();
        for payload in wasmparser::Parser::new(0).parse_all(bytes) {
            match payload.unwrap() {
                wasmparser::Payload::ImportSection(section) => {
                    assert_eq!(section.count(), 0, "host import は増やさない");
                }
                wasmparser::Payload::TableSection(section) => {
                    assert_eq!(section.count(), 0, "function table は持たない");
                }
                wasmparser::Payload::CodeSectionEntry(body) => {
                    let mut targets = Vec::new();
                    let mut reader = body.get_operators_reader().expect("命令を読めるはず");
                    while !reader.eof() {
                        match reader.read().expect("命令を読めるはず") {
                            wasmparser::Operator::Call { function_index } => {
                                targets.push(function_index);
                            }
                            wasmparser::Operator::CallIndirect { .. } => {
                                panic!("表越しの呼び出しがある");
                            }
                            _ => {}
                        }
                    }
                    bodies.push(targets);
                }
                _ => {}
            }
        }
        bodies
    }

    /// 本体の名前で instance 番号を引く。番号はそのまま function index
    fn instance_indices(
        checked: &CheckedProgram,
        production: &ProductionPlan,
        name: &str,
    ) -> Vec<u32> {
        production
            .plan
            .instances()
            .filter(|(_, instance)| checked.hir.show_body(instance.key.body) == name)
            .map(|(id, _)| id.index() as u32)
            .collect()
    }

    /// 同じ汎用宣言でも、型引数・callback・provider が違えば別々の instance に
    /// なり、署名と隠し欄の並びが固定される。ambient を要らない具体化は兄弟の
    /// 隠し欄を持たない
    #[test]
    fn 汎用具体化のinstance署名と隠し欄は固定される() {
        let src = "trait Clock { fn now(&self -> int) }\n\
                   struct Frozen { at: int }\n\
                   impl Clock for Frozen { fn now(&self -> int) { self.at } }\n\
                   effect clock: Clock\n\
                   fn ticked(value: int -> int) { value + clock.now() }\n\
                   fn flagged(value: bool -> int) { if value: clock.now() else: 0 }\n\
                   fn plain(value: int -> int) { value + 1 }\n\
                   fn apply<T, U>(f: fn(T -> U), x: T -> U) { f(x) }\n\
                   fn main(-> int) {\n\
                   \x20 let quiet = apply(plain, 1)\n\
                   \x20 with clock(Frozen { at = 7 }) { apply(ticked, quiet) + apply(flagged, true) }\n\
                   }\n";
        let (checked, production) = plan_of(src, &[]);
        let program = &checked.hir;
        let mut snapshots = production
            .plan
            .instances()
            .filter(|(_, instance)| program.show_body(instance.key.body) == "apply")
            .map(|(id, _)| instance_signature_snapshot(&checked, &production, id))
            .collect::<Vec<_>>();
        snapshots.sort();
        assert_eq!(
            snapshots,
            [
                // bool 引数 + clock の隠し欄
                "apply params=[I32, I32] ambient=[clock=1]",
                // int 引数 + clock の隠し欄
                "apply params=[I64, I32] ambient=[clock=1]",
                // ambient を要らない兄弟は隠し欄を持たない
                "apply params=[I64] ambient=[]",
            ]
        );
    }

    /// callback の束縛が違えば、呼び出し口はそれぞれ自分の具体化を直に呼ぶ。
    /// 共有の分岐点は無く、表も funcref も import も増えない
    #[test]
    fn 汎用具体化の呼び出しは束縛ごとの直呼びになる() {
        let src = "fn double(value: int -> int) { value * 2 }\n\
                   fn negate(value: int -> int) { 0 - value }\n\
                   fn apply<T, U>(f: fn(T -> U), x: T -> U) { f(x) }\n\
                   fn main(-> int) { apply(double, 21) + apply(double, 1) + apply(negate, 5) }\n";
        let (checked, production) = plan_of(src, &[]);
        let applies = instance_indices(&checked, &production, "apply");
        let double = instance_indices(&checked, &production, "double");
        let negate = instance_indices(&checked, &production, "negate");
        assert_eq!(
            applies.len(),
            2,
            "束縛ごとに具体化が分かれ、同じ束縛は畳まれる"
        );
        assert_eq!(double.len(), 1);
        assert_eq!(negate.len(), 1);

        let bytes = compile(src, &[]).expect("生成できるはず");
        validate(&bytes).expect("検証を通るはず");
        let bodies = direct_call_targets(&bytes);
        let mut bound: Vec<Vec<u32>> = applies
            .iter()
            .map(|index| bodies[*index as usize].clone())
            .collect();
        bound.sort();
        let mut expected = vec![double.clone(), negate.clone()];
        expected.sort();
        assert_eq!(
            bound, expected,
            "具体化はそれぞれ自分の callback を直に呼ぶ"
        );
        let mut called = bodies[0].clone();
        called.sort();
        called.dedup();
        assert_eq!(
            called, applies,
            "呼び出し口は計画した instance 番号をそのまま使う"
        );
        assert_eq!(scalars(&invoke(&bytes, ENTRY_EXPORT, &[]).unwrap()), [39]);
    }

    /// 到達しない汎用宣言は関数を1つも生まない。宣言を消したモジュールと
    /// バイト単位で同じになることで見る
    #[test]
    fn 到達しない汎用宣言は関数を生まない() {
        let reached = "fn identity<T>(x: T -> T) { x }\n\
                       fn main(-> int) { identity(3) }\n";
        let with_unreached = "fn identity<T>(x: T -> T) { x }\n\
                              fn unreached<T>(x: T, n: int -> T) { if n == 0: x else: unreached(x, n - 1) }\n\
                              fn main(-> int) { identity(3) }\n";
        let (checked, production) = plan_of(with_unreached, &[]);
        assert!(
            instance_indices(&checked, &production, "unreached").is_empty(),
            "到達しない宣言は instance にならない"
        );
        assert_eq!(
            compile(with_unreached, &[]).expect("生成できるはず"),
            compile(reached, &[]).expect("生成できるはず"),
            "到達しない宣言はバイト列を変えない"
        );
    }

    /// 所有値を汎用の消費 callback へ `move` する呼び出しを有界なループで
    /// 繰り返しても、解放した記憶を使い回すのでメモリは伸びない
    #[test]
    fn 汎用の消費callbackを繰り返してもメモリは伸びない() {
        let src = "struct Tag { name: str }\n\
                   fn weigh(t: Tag -> int) { if t.name == \"kept\": 1 else: 0 }\n\
                   fn apply<T, U>(f: fn(T -> U), x: T -> U) { f(move x) }\n\
                   fn main(-> int) {\n\
                   \x20 let mut n = 0\n\
                   \x20 while (n == 3000) == false {\n\
                   \x20   n = n + apply(weigh, Tag { name = \"kept\" })\n\
                   \x20 }\n\
                   \x20 n\n\
                   }\n";
        let bytes = compile(src, &[]).expect("生成できるはず");
        validate(&bytes).expect("検証を通るはず");
        assert_eq!(invoke_capped(&bytes, ENTRY_EXPORT, 1), Ok(vec![3000]));
    }

    /// 汎用 trait メソッドの具体化も、型引数ごとに1つの関数へ下りて走り切る
    #[test]
    fn 汎用trait_methodの具体化は普通のmethodと同じに走る() {
        let src = "trait Box<T> { fn wrap<U>(&self, value: T, f: fn(T -> U) -> U) }\n\
                   struct Container { tag: int }\n\
                   impl<T> Box<T> for Container { fn wrap<U>(&self, value: T, f: fn(T -> U) -> U) { f(value) } }\n\
                   fn double(value: int -> int) { value * 2 }\n\
                   fn flip(value: bool -> bool) { value == false }\n\
                   fn main(-> int) {\n\
                   \x20 let c = Container { tag = 1 }\n\
                   \x20 if c.wrap(false, flip): c.wrap(20, double) else: 0\n\
                   }\n";
        assert_eq!(same_as_interpreter(src), 40);
        let bytes = compile(src, &[]).expect("生成できるはず");
        let (checked, production) = plan_of(src, &[]);
        assert_eq!(
            instance_indices(&checked, &production, "impl Container::wrap").len(),
            2,
            "型引数ごとに具体化が1つずつ"
        );
        direct_call_targets(&bytes);
    }
}
