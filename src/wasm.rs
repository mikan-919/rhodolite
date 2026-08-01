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

use crate::ambient_abi::{Plan, ProductionPlan};
use crate::diag::Diag;
use crate::hir;
use crate::ownership::CheckedProgram;
use crate::wasm_data::{self, Indices};
use crate::wasm_layout::{self, LayoutId, Layouts, ReprKind};
use crate::wasm_runtime::{self, Body, Runtime};
use std::collections::BTreeMap;
use wasm_encoder::{
    BlockType, CodeSection, CustomSection, ExportKind, ExportSection, Function, FunctionSection,
    Instruction, Module, TypeSection, ValType,
};

/// ホストから呼ぶエントリの予約名。公開名として使うことはできない
pub const ENTRY_EXPORT: &str = "__rhodolite_main";

/// 埋め込むインタフェース記述のセクション名
pub const ABI_SECTION: &str = "rhodolite.abi";

/// 埋め込むインタフェース記述の版
pub const ABI_VERSION: u32 = 0;

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
    fn val_type(self) -> Option<ValType> {
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
        .help("いまの Wasm は unit / bool / int / str / struct と、束縛・代入・フィールド・算術・比較・`clone()`・所有権修飾・if・while・直接呼び出し・return・assert を扱います")
}

/// この backend が下ろせる型か。
///
/// 借用は検査済みの場所を指すアドレス1つなので、指す先が扱えるなら運べる。
/// optional・struct・enum・配列は後続スライス
fn supported(ty: &hir::Type) -> bool {
    if ty.optional {
        return false;
    }
    matches!(
        ty.kind,
        hir::TypeKind::Builtin(
            hir::Builtin::Unit | hir::Builtin::Bool | hir::Builtin::Int | hir::Builtin::Str
        ) | hir::TypeKind::Struct(_)
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

        if instance.layout.is_some() {
            diagnostics.push(unsupported(
                callable.span,
                "実行時に provider を運ぶ ambient 要求",
            ));
        }
        if callable.receiver.is_some() {
            diagnostics.push(unsupported(callable.span, "メソッド"));
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
            let decl = body.local(*local);
            match decl.ty.as_ref().filter(|ty| ty.reference.is_some()) {
                Some(ty) => diagnostics.push(borrowed_signature(
                    program,
                    decl.span,
                    &format!("引数 `{}`", decl.name),
                    ty,
                )),
                None => check_local(program, body, *local, &mut diagnostics),
            }
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
        hir::ExprKind::Call(hir::Call::Direct { args, .. }) => {
            children.extend(args.iter().copied())
        }

        hir::ExprKind::Str(_) => {}
        hir::ExprKind::Access { place, .. } => children.push(*place),
        hir::ExprKind::Clone(inner) => children.push(*inner),
        hir::ExprKind::Nil => diagnostics.push(unsupported(expr.span, "optional の `nil`")),
        hir::ExprKind::UnitStruct(_) => {}
        hir::ExprKind::StructLit { fields, .. } => {
            children.extend(fields.iter().map(|(_, value)| *value))
        }
        hir::ExprKind::Variant(_) | hir::ExprKind::Call(hir::Call::Ctor { .. }) => {
            diagnostics.push(unsupported(expr.span, "enum の値"))
        }
        // `.?` は optional を伝播するので、optional のスライスまで待つ
        hir::ExprKind::Field { optional: true, .. } => {
            diagnostics.push(unsupported(expr.span, "`.?` のフィールド参照"))
        }
        hir::ExprKind::Field { recv, .. } => children.push(*recv),
        hir::ExprKind::AssignField { recv, value, .. } => children.extend([*recv, *value]),
        hir::ExprKind::Array(_) => diagnostics.push(unsupported(expr.span, "配列")),
        hir::ExprKind::Coalesce { .. } => diagnostics.push(unsupported(expr.span, "`??`")),
        hir::ExprKind::For { .. } => diagnostics.push(unsupported(expr.span, "`for`")),
        hir::ExprKind::With { .. } => diagnostics.push(unsupported(expr.span, "`with` の提供")),
        hir::ExprKind::Match { .. } => diagnostics.push(unsupported(expr.span, "`match`")),
        hir::ExprKind::Call(hir::Call::Method { .. }) => {
            diagnostics.push(unsupported(expr.span, "メソッド呼び出し"))
        }
        hir::ExprKind::Call(hir::Call::Associated { .. }) => {
            diagnostics.push(unsupported(expr.span, "関連関数の呼び出し"))
        }
        hir::ExprKind::Call(hir::Call::Slot { slot_span, .. }) => {
            diagnostics.push(unsupported(*slot_span, "スロット経由の呼び出し"))
        }
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
    /// 引数の後ろに並ぶ宣言ローカルの型
    extra_locals: Vec<ValType>,
    /// 所有する非 Copy の束縛 → その並び。掃除の glue をここから引く
    owned: BTreeMap<hir::LocalId, LayoutId>,
    /// 所有する束縛 → 初期化フラグの局所番号(tasks 3.4)
    flags: BTreeMap<hir::LocalId, u32>,
    /// 作業用 `i32` の区画。要る式ごとに別を配るので、入れ子でも踏み合わない
    scratch: BTreeMap<hir::ExprId, u32>,
    /// 掃除が使う共通の作業用区画。先頭が区画の走査用、続いて射影の段ごとの器
    common: u32,
}

/// 引数と宣言ローカルへ Wasm のローカル番号を振る。
///
/// 番号は Wasm の規則どおり引数が先。どちらも HIR の宣言順で歩くので、
/// 同じ入力からは同じ割り当てになる
fn lower_signature(
    layouts: &mut Layouts,
    program: &hir::Program,
    callable_id: hir::CallableId,
) -> Lowered {
    let callable = &program.callables[callable_id];
    let body = &callable.body;
    let mut slots: BTreeMap<hir::LocalId, Vec<u32>> = BTreeMap::new();
    let mut params = Vec::new();

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

    let mut extra_locals = Vec::new();
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
        extra_locals,
        owned,
        flags,
        scratch,
        common,
    }
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
        // フィールドの差し替えは、器のアドレスを持ち回すので常に要る
        hir::ExprKind::AssignField { .. } => true,
        hir::ExprKind::AssignLocal { value, .. } => owned(layouts, *value),
        hir::ExprKind::Eq { lhs, rhs } => owned(layouts, *lhs) || owned(layouts, *rhs),
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
    lowered: &'a Lowered,
    /// 所有権検査が確定した掃除。ここを読むだけで、順を組み直さない
    plan: &'a crate::ownership::BodyPlan,
    layouts: &'a mut Layouts,
    types: &'a mut Types,
    indices: &'a Indices,
    statics: &'a BTreeMap<String, (u32, u32)>,
    out: Body,
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
    fn sequence(&mut self, exprs: &[hir::ExprId], want: &[ValType]) {
        let Some((last, leading)) = exprs.split_last() else {
            return;
        };
        for expr in leading {
            self.expr(*expr, &[]);
        }
        self.expr(*last, want);
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
                let want = self.local_want(local);
                self.expr(value, &want);
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
                let want = self.local_want(local);
                self.expr(value, &want);
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
                self.expr(recv, &[ValType::I32]);
                self.read_slot(&slot);
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
                let (value, results) = (*value, self.lowered.results.clone());
                if let Some(value) = value {
                    self.expr(value, &results);
                }
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
                let want = self.produced(id);
                self.sequence(&exprs, &want);
            }

            hir::ExprKind::If { cond, then, orelse } => {
                let (cond, then, orelse) = (*cond, *then, *orelse);
                let want = self.produced(id);
                self.expr(cond, WANT_BOOL);
                let block = self.block_type(&want);
                self.push(Instruction::If(block));
                self.expr(then, &want);
                self.cleanup(crate::ownership::Exit::Join {
                    branch: id,
                    taken: true,
                });
                if let Some(orelse) = orelse {
                    self.push(Instruction::Else);
                    self.expr(orelse, &want);
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

            // 呼び先は計画が持っている。名前で引き直さない(design.md 決定5)
            hir::ExprKind::Call(hir::Call::Direct { callable, args }) => {
                let (callable, args) = (*callable, args.clone());
                let params: Vec<hir::LocalId> = self.program.callables[callable].params.clone();
                for (arg, param) in args.iter().zip(params) {
                    let program = self.program;
                    let callee = &program.callables[callable].body;
                    let want = local_values(self.layouts, program, callee, param);
                    self.expr(*arg, &want);
                }
                let target = self.instance.calls[&id].target;
                self.push(Instruction::Call(target.index() as u32));
            }

            // 対応範囲の検査が先に止めているので、ここへ来たら検査の抜け
            other => unreachable!("Wasm へ下ろせない式が検査を抜けました: {other:?}"),
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
        let owner = self.program.fields[field].owner;
        let position = self.program.structs[owner]
            .fields
            .iter()
            .position(|declared| *declared == field)
            .expect("宣言に無いフィールド");
        let layout = self
            .kind_of(recv)
            .and_then(|kind| match kind {
                ReprKind::Owned(layout) | ReprKind::Borrowed(layout) => Some(layout),
                ReprKind::Flat => None,
            })
            .expect("フィールドのレシーバは器のアドレス");
        match &self.layouts.get(layout).shape {
            wasm_layout::Shape::Struct { fields, .. } => fields[position],
            other => unreachable!("フィールドを持たない並びです: {other:?}"),
        }
    }

    /// 器のアドレスを、その区画の値か場所へ置き換える。
    ///
    /// Copy な区画は読み出し、直接置かれた所有の子はその場所、`indirect` の
    /// 子は指している根が答えになる
    fn read_slot(&mut self, slot: &wasm_layout::Slot) {
        if slot.indirect {
            self.out.offset(slot.offset).load();
            return;
        }
        if self.layouts.get(slot.layout).copy {
            let (layouts, layout, offset) = (&*self.layouts, slot.layout, slot.offset);
            wasm_data::load_copy(&mut self.out, layouts, layout, offset);
            return;
        }
        self.out.offset(slot.offset);
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
            self.install_slot(&slot, *value, root, temporary);
        }
        self.out.get(root);
    }

    /// 評価した値を区画へ収める。所有の子は一時的な根から中身ごと移す
    fn install_slot(
        &mut self,
        slot: &wasm_layout::Slot,
        value: hir::ExprId,
        container: u32,
        temporary: u32,
    ) {
        if slot.indirect {
            // `indirect` の子は独立した根のまま。アドレスだけを収める
            self.expr(value, &[ValType::I32]);
            self.out.set(temporary);
            self.out
                .get(container)
                .offset(slot.offset)
                .get(temporary)
                .store();
            return;
        }
        if self.layouts.get(slot.layout).copy {
            self.out.get(container);
            let want = self.produced(value);
            self.expr(value, &want);
            let (layouts, layout, offset) = (&*self.layouts, slot.layout, slot.offset);
            wasm_data::store_copy(&mut self.out, layouts, layout, offset);
            return;
        }
        self.expr(value, &[ValType::I32]);
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

        if self.layouts.get(slot.layout).copy && !slot.indirect {
            self.out.get(container);
            let want = self.produced(value);
            self.expr(value, &want);
            let (layouts, layout, offset) = (&*self.layouts, slot.layout, slot.offset);
            wasm_data::store_copy(&mut self.out, layouts, layout, offset);
            return;
        }

        // 新しい値を先に作る。落としてから作ると、途中の trap で二度落ちる
        self.expr(value, &[ValType::I32]);
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
            // 器の側を空にする。残余の破棄がここをもう一度返さないように
            self.out.get(container).offset(slot.offset).num(0).store();
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

    /// 束縛が受ける値の並び
    fn local_want(&mut self, local: hir::LocalId) -> Vec<ValType> {
        let program = self.program;
        let body = &program.callables[self.lowered.callable].body;
        local_values(self.layouts, program, body, local)
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

/// 公開面1つ分の署名。ABI メタデータと入口の検査に使う
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Signature {
    pub name: String,
    pub params: Vec<Scalar>,
    pub result: Scalar,
}

/// 計画された instance を Core Wasm モジュールへ落とす。
///
/// 呼ぶ前に `check_support` と ABI 署名の検査を通しておくこと。ここは
/// 「通った計画を決定的な bytes にする」ことだけを受け持つ
pub fn emit(checked: &CheckedProgram, production: &ProductionPlan) -> Result<Vec<u8>, Vec<Diag>> {
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
        .map(|(_, instance)| match instance.key.body {
            hir::BodyId::Callable(id) => lower_signature(&mut layouts, program, id),
            hir::BodyId::Test(_) => unreachable!("test は生産の根に入らない"),
        })
        .collect();

    // 所有データが1つも到達していないなら、ランタイムも glue も載せない。
    // scalar だけのモジュールのバイト列はこの change の前と変わらない
    let owned_data = layouts.planned().any(|(_, layout)| !layout.copy);
    let statics = collect_literals(program, plan);
    let wrappers = signatures.wrappers().count() as u32;
    let runtime_base = lowered.len() as u32 + wrappers;
    let mut indices = Indices::reserve(&layouts, runtime_base + wasm_runtime::COUNT);
    indices.alloc = runtime_base + wasm_runtime::ALLOC;
    indices.free = runtime_base + wasm_runtime::FREE;

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
            lowered,
            plan: checked.plan.body(hir::BodyId::Callable(lowered.callable)),
            layouts: &mut layouts,
            types: &mut types,
            indices: &indices,
            statics: &statics.at,
            out: Body::with_locals(locals),
        };
        // 引数は呼ばれた時点で所有を得ている。局所の初期値は 0 なので、
        // 持っていることを明示的に立てる(tasks 3.4)
        for local in &program.callables[lowered.callable].params {
            if let Some(flag) = lowered.flags.get(local).copied() {
                wasm_data::mark_initialized(&mut emitter.out, flag);
            }
        }
        let root = program.callables[lowered.callable].body.root.clone();
        let want = lowered.results.clone();
        emitter.sequence(&root, &want);
        emitter.cleanup(crate::ownership::Exit::Fallthrough);
        // `finish` が関数の `end` を付ける
        code.function(&emitter.out.finish());
    }

    // ラッパは実装関数の後ろ。エントリ、続いて公開名の昇順
    let mut exports = ExportSection::new();
    let mut next_index = lowered.len() as u32;
    for (signature, instance) in signatures.wrappers() {
        let params: Vec<ValType> = signature
            .params
            .iter()
            .filter_map(|scalar| scalar.val_type())
            .collect();
        let results: Vec<ValType> = signature.result.val_type().into_iter().collect();
        functions.function(types.intern(&params, &results));
        code.function(&wrapper(&params, signature, instance.index() as u32));
        exports.export(&signature.name, ExportKind::Func, next_index);
        next_index += 1;
    }

    // ランタイムと glue は公開しない実装の後ろ。番号は `indices` の予約と同じ順
    let runtime =
        owned_data.then(|| Runtime::new(&statics.bytes).expect("静的データは 32bit に収まる"));
    if runtime.is_some() {
        for helper in Runtime::helpers(runtime_base)
            .into_iter()
            .chain(wasm_data::glue_functions(&layouts, &indices))
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
        data: signatures.metadata().into_bytes().into(),
    });

    module.finish()
}

/// ホスト境界のラッパ1つ。
///
/// bool 引数は 0/1 だけを受ける。本体へ入る前に trap するので、内部の
/// 関数はどこも「bool は 0 か 1」を前提にできる
fn wrapper(params: &[ValType], signature: &Signature, target: u32) -> Function {
    let mut function = Function::new([]);
    let mut index = 0u32;
    for scalar in &signature.params {
        if *scalar == Scalar::Bool {
            function.instruction(&Instruction::LocalGet(index));
            function.instruction(&Instruction::I32Const(2));
            function.instruction(&Instruction::I32GeU);
            function.instruction(&Instruction::If(BlockType::Empty));
            function.instruction(&Instruction::Unreachable);
            function.instruction(&Instruction::End);
        }
        if scalar.val_type().is_some() {
            index += 1;
        }
    }
    for slot in 0..params.len() as u32 {
        function.instruction(&Instruction::LocalGet(slot));
    }
    function.instruction(&Instruction::Call(target));
    // 内部の bool は構成上 0/1 だが、公開する値は境界でも正規化しておく
    if signature.result == Scalar::Bool {
        function.instruction(&Instruction::I32Eqz);
        function.instruction(&Instruction::I32Eqz);
    }
    function.instruction(&Instruction::End);
    function
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

    #[test]
    fn 最小のプログラムが検証を通る() {
        let bytes = compile("fn main(-> int) { 41 + 1 }\n", &[]).expect("生成できるはず");
        validate(&bytes).expect("検証を通るはず");
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

    /// 到達したら止まる。指すのは到達したその構文
    #[test]
    fn 到達した未対応の構文はビルドを止める() {
        let errors = compile(
            "fn reached(-> int) {\n let xs = [1, 2]\n 1\n}\n\
             fn main(-> int) { reached() }\n",
            &[],
        )
        .expect_err("止まるはず");
        assert!(messages(&errors).contains("Wasm ターゲットでは扱えません"));
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
        assert!(
            found.contains("引数 `u`の借用 `&User` は Wasm の署名には出せません"),
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
            message.contains("引数を ABI v0 では表せません"),
            "{message}"
        );
    }

    /// 公開できない型は公開面でだけ拒否する。同じ型の非公開宣言は縛らない
    #[test]
    fn 公開面の豊かな型は拒否するが非公開は縛らない() {
        let src = "struct User { name: str }\n\
                   fn rich(-> User) { User { name = \"a\" } }\n\
                   fn main(-> int) { 0 }\n";
        assert!(abi_error(src, &["rich"]).contains("戻り値の型 `User` を ABI v0 では表せません"));
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

    /// メタデータを知らないエンジンでも検証・実行できる
    #[test]
    fn メタデータを読まなくてもモジュールは動く() {
        let bytes = compile(SURFACE, &["touch"]).expect("生成できるはず");
        validate(&bytes).expect("検証を通るはず");
        assert_eq!(scalars(&invoke(&bytes, ENTRY_EXPORT, &[]).unwrap()), [3]);
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
}
