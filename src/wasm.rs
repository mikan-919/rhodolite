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

/// 式が stack に残す値。`unit` は実行時表現を持たないので何も残さないし、
/// 発散する式も残さない
fn result_scalar(expr: &hir::Expr) -> Option<Scalar> {
    match &expr.result {
        hir::ExprResult::Value(ty) => scalar_of(ty).filter(|s| s.val_type().is_some()),
        // 発散する式の後ろは到達しない。Wasm の stack は多相なので何も要らない
        hir::ExprResult::Diverges | hir::ExprResult::Poison => None,
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
        .help("v0 の Wasm は unit / bool / int と、束縛・算術・比較・if・while・直接呼び出し・return・assert だけを扱います")
}

// ---------------------------------------------------------------------------
// 対応範囲の検査(到達したところだけ)
// ---------------------------------------------------------------------------

/// 計画に入った instance だけを歩いて、v0 の部分言語から外れた構文を報告する。
///
/// 読み込んだ全コードは通常の静的検査を既に通っている。ここが見るのは
/// 「生産の根から到達したか」だけなので、使われていない richer な宣言は
/// 生産ビルドを止めない(core-wasm-build spec)
pub fn check_support(program: &hir::Program, plan: &Plan) -> Vec<Diag> {
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
        } else if scalar_of(&callable.ret).is_none() {
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
        && scalar_of(ty).is_none()
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
        && scalar_of(ty).is_none()
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

        // ponytail: 借用も移動も v0 では下ろせない。所有付きの Wasm 表現は
        // compile-wasm-owned-data-values の仕事
        hir::ExprKind::Access { .. } => {
            diagnostics.push(unsupported(expr.span, "所有権修飾された場所"))
        }
        hir::ExprKind::Str(_) => diagnostics.push(unsupported(expr.span, "文字列")),
        hir::ExprKind::Nil => diagnostics.push(unsupported(expr.span, "optional の `nil`")),
        hir::ExprKind::UnitStruct(_) | hir::ExprKind::StructLit { .. } => {
            diagnostics.push(unsupported(expr.span, "struct の値"))
        }
        hir::ExprKind::Variant(_) | hir::ExprKind::Call(hir::Call::Ctor { .. }) => {
            diagnostics.push(unsupported(expr.span, "enum の値"))
        }
        hir::ExprKind::Field { .. } | hir::ExprKind::AssignField { .. } => {
            diagnostics.push(unsupported(expr.span, "フィールドの参照"))
        }
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

/// instance 1つ分の、Wasm 関数としての形。
struct Lowered {
    callable: hir::CallableId,
    /// 値を運ぶ引数だけ。`unit` の引数は席を持たない
    params: Vec<ValType>,
    result: Scalar,
    /// HIR のローカル → Wasm のローカル番号。`unit` のローカルは載らない
    slots: BTreeMap<hir::LocalId, u32>,
    /// 引数の後ろに並ぶ宣言ローカルの型
    extra_locals: Vec<ValType>,
}

/// 引数と宣言ローカルへ Wasm のローカル番号を振る。
///
/// 番号は Wasm の規則どおり引数が先。どちらも HIR の宣言順で歩くので、
/// 同じ入力からは同じ割り当てになる
fn lower_signature(program: &hir::Program, callable_id: hir::CallableId) -> Lowered {
    let callable = &program.callables[callable_id];
    let body = &callable.body;
    let mut slots = BTreeMap::new();
    let mut params = Vec::new();

    for local in &callable.params {
        let scalar = body
            .local(*local)
            .ty
            .as_ref()
            .and_then(scalar_of)
            .unwrap_or(Scalar::Unit);
        if let Some(val_type) = scalar.val_type() {
            slots.insert(*local, params.len() as u32);
            params.push(val_type);
        }
    }

    let mut extra_locals = Vec::new();
    for (id, decl) in body.locals() {
        if slots.contains_key(&id) {
            continue;
        }
        let Some(val_type) = decl
            .ty
            .as_ref()
            .and_then(scalar_of)
            .and_then(Scalar::val_type)
        else {
            continue;
        };
        slots.insert(id, (params.len() + extra_locals.len()) as u32);
        extra_locals.push(val_type);
    }

    Lowered {
        callable: callable_id,
        params,
        result: scalar_of(&callable.ret).unwrap_or(Scalar::Unit),
        slots,
        extra_locals,
    }
}

/// 同じ形の関数型を1つに寄せる。並びは最初に要求された順
#[derive(Default)]
struct Types {
    ids: BTreeMap<(Vec<ValType>, Vec<ValType>), u32>,
    section: TypeSection,
}

impl Types {
    fn intern(&mut self, params: &[ValType], result: Scalar) -> u32 {
        let results: Vec<ValType> = result.val_type().into_iter().collect();
        let key = (params.to_vec(), results.clone());
        if let Some(found) = self.ids.get(&key) {
            return *found;
        }
        let id = self.ids.len() as u32;
        self.ids.insert(key, id);
        self.section.ty().function(params.to_vec(), results);
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
    function: Function,
}

impl Emitter<'_> {
    fn push(&mut self, instruction: Instruction<'_>) {
        self.function.instruction(&instruction);
    }

    fn body(&self) -> &hir::Body {
        &self.program.callables[self.lowered.callable].body
    }

    /// 式の列を下ろす。値になるのは最後の式だけで、途中の値は捨てる
    fn sequence(&mut self, exprs: &[hir::ExprId], want: Option<Scalar>) {
        let Some((last, leading)) = exprs.split_last() else {
            return;
        };
        for expr in leading {
            self.expr(*expr, None);
        }
        self.expr(*last, want);
    }

    /// 式を「この結果で終わる」ように下ろす。
    ///
    /// `want` が `None` なら stack には何も残さない。値を産む式なら落とす。
    /// 発散する式は何も残さないので、どちらの `want` でも足すものは無い
    fn expr(&mut self, id: hir::ExprId, want: Option<Scalar>) {
        let produced = result_scalar(self.body().expr(id));
        self.lower(id);
        if produced.is_some() && want.is_none() {
            self.push(Instruction::Drop);
        }
    }

    /// 式そのもの。残す値は `result_scalar` が決めたぶんだけ
    fn lower(&mut self, id: hir::ExprId) {
        let body = self.body();
        let expr = body.expr(id);
        match &expr.kind {
            hir::ExprKind::Int(n) => self.push(Instruction::I64Const(*n)),
            hir::ExprKind::Bool(b) => self.push(Instruction::I32Const(i32::from(*b))),

            hir::ExprKind::Local(local) => {
                if let Some(slot) = self.lowered.slots.get(local).copied() {
                    self.push(Instruction::LocalGet(slot));
                }
            }

            hir::ExprKind::Let { local, value } | hir::ExprKind::AssignLocal { local, value } => {
                let slot = self.lowered.slots.get(local).copied();
                let want = slot.map(|_| local_scalar(body, *local));
                self.expr(*value, want);
                if let Some(slot) = slot {
                    self.push(Instruction::LocalSet(slot));
                }
            }

            // Wasm に単項マイナスは無い。`0 - n` は MIN でも仕様どおり回り込む
            hir::ExprKind::Neg(inner) => {
                let inner = *inner;
                self.push(Instruction::I64Const(0));
                self.expr(inner, Some(Scalar::Int));
                self.push(Instruction::I64Sub);
            }

            hir::ExprKind::Arith { op, lhs, rhs } => {
                let (op, lhs, rhs) = (*op, *lhs, *rhs);
                self.expr(lhs, Some(Scalar::Int));
                self.expr(rhs, Some(Scalar::Int));
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
                let operand = result_scalar(body.expr(lhs))
                    .or_else(|| result_scalar(body.expr(rhs)))
                    .unwrap_or(Scalar::Unit);
                self.expr(lhs, operand.val_type().map(|_| operand));
                self.expr(rhs, operand.val_type().map(|_| operand));
                match operand {
                    Scalar::Int => self.push(Instruction::I64Eq),
                    Scalar::Bool => self.push(Instruction::I32Eq),
                    // unit は値を持たないので常に等しい。両辺の効果だけ走らせた
                    Scalar::Unit => self.push(Instruction::I32Const(1)),
                }
            }

            // Wasm の `return` は関数の結果型ぶんを stack から返す。以降の式は
            // 到達しないので、下ろしても validator は多相な stack で受ける
            hir::ExprKind::Return(value) => {
                let (value, result) = (*value, self.lowered.result);
                match value {
                    Some(value) => self.expr(value, result.val_type().map(|_| result)),
                    None => {}
                }
                self.push(Instruction::Return);
            }

            hir::ExprKind::Assert(cond) => {
                let cond = *cond;
                self.expr(cond, Some(Scalar::Bool));
                self.push(Instruction::I32Eqz);
                self.push(Instruction::If(BlockType::Empty));
                self.push(Instruction::Unreachable);
                self.push(Instruction::End);
            }

            hir::ExprKind::Block(exprs) => {
                let (exprs, want) = (exprs.clone(), result_scalar(expr));
                self.sequence(&exprs, want);
            }

            hir::ExprKind::If { cond, then, orelse } => {
                let (cond, then, orelse) = (*cond, *then, *orelse);
                let want = result_scalar(body.expr(id));
                self.expr(cond, Some(Scalar::Bool));
                self.push(Instruction::If(block_type(want)));
                self.expr(then, want);
                if let Some(orelse) = orelse {
                    self.push(Instruction::Else);
                    self.expr(orelse, want);
                }
                self.push(Instruction::End);
            }

            hir::ExprKind::While { cond, body: inner } => {
                let (cond, inner) = (*cond, *inner);
                self.push(Instruction::Block(BlockType::Empty));
                self.push(Instruction::Loop(BlockType::Empty));
                self.expr(cond, Some(Scalar::Bool));
                self.push(Instruction::I32Eqz);
                self.push(Instruction::BrIf(1));
                self.expr(inner, None);
                self.push(Instruction::Br(0));
                self.push(Instruction::End);
                self.push(Instruction::End);
            }

            // 呼び先は計画が持っている。名前で引き直さない(design.md 決定5)
            hir::ExprKind::Call(hir::Call::Direct { callable, args }) => {
                let (callable, args) = (*callable, args.clone());
                let params: Vec<Option<Scalar>> = self.program.callables[callable]
                    .params
                    .iter()
                    .map(|local| {
                        let scalar = local_scalar(&self.program.callables[callable].body, *local);
                        scalar.val_type().map(|_| scalar)
                    })
                    .collect();
                for (arg, want) in args.iter().zip(params) {
                    self.expr(*arg, want);
                }
                let target = self.instance.calls[&id].target;
                self.push(Instruction::Call(target.index() as u32));
            }

            // 対応範囲の検査が先に止めているので、ここへ来たら検査の抜け
            other => unreachable!("Wasm へ下ろせない式が検査を抜けました: {other:?}"),
        }
    }
}

/// 束縛の scalar。読めない束縛(初期化子が発散した `let`)は `unit` と同じ扱い
fn local_scalar(body: &hir::Body, local: hir::LocalId) -> Scalar {
    body.local(local)
        .ty
        .as_ref()
        .and_then(scalar_of)
        .unwrap_or(Scalar::Unit)
}

fn block_type(want: Option<Scalar>) -> BlockType {
    match want.and_then(Scalar::val_type) {
        Some(val_type) => BlockType::Result(val_type),
        None => BlockType::Empty,
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
pub fn emit(program: &hir::Program, production: &ProductionPlan) -> Result<Vec<u8>, Vec<Diag>> {
    let signatures = crate::wasm_abi::signatures(program, production)?;
    Ok(build(program, production, &signatures))
}

fn build(
    program: &hir::Program,
    production: &ProductionPlan,
    signatures: &crate::wasm_abi::Signatures,
) -> Vec<u8> {
    let plan = &production.plan;
    let lowered: Vec<Lowered> = plan
        .instances()
        .map(|(_, instance)| match instance.key.body {
            hir::BodyId::Callable(id) => lower_signature(program, id),
            hir::BodyId::Test(_) => unreachable!("test は生産の根に入らない"),
        })
        .collect();

    let mut types = Types::default();
    let mut functions = FunctionSection::new();
    let mut code = CodeSection::new();

    for (index, (_, instance)) in plan.instances().enumerate() {
        let lowered = &lowered[index];
        functions.function(types.intern(&lowered.params, lowered.result));

        let locals = lowered
            .extra_locals
            .iter()
            .map(|val_type| (1, *val_type))
            .collect::<Vec<_>>();
        let mut emitter = Emitter {
            program,
            instance,
            lowered,
            function: Function::new(locals),
        };
        let root = program.callables[lowered.callable].body.root.clone();
        let want = lowered.result.val_type().map(|_| lowered.result);
        emitter.sequence(&root, want);
        emitter.push(Instruction::End);
        code.function(&emitter.function);
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
        functions.function(types.intern(&params, signature.result));
        code.function(&wrapper(&params, signature, instance.index() as u32));
        exports.export(&signature.name, ExportKind::Func, next_index);
        next_index += 1;
    }

    let mut module = Module::new();
    module.section(&types.section);
    module.section(&functions);
    module.section(&exports);
    module.section(&code);
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
        let (program, production) = plan_of(src, exports);
        let unsupported = check_support(&program, &production.plan);
        if !unsupported.is_empty() {
            return Err(unsupported);
        }
        emit(&program, &production)
    }

    pub(crate) fn plan_of(src: &str, exports: &[&str]) -> (hir::Program, ProductionPlan) {
        let parsed = crate::parse::parse(&crate::lex::join(crate::lex::lex(src).unwrap()))
            .expect("パースできるはず");
        let program = crate::typecheck::check_and_lower(&parsed).expect("型検査を通るはず");
        let analysis = crate::requirement::analyze(&program);
        let entry = program.free_callable("main").expect("main がない");
        let exports: Vec<(String, hir::CallableId)> = exports
            .iter()
            .map(|spelling| {
                let (public, target) = spelling.split_once('=').unwrap_or((spelling, spelling));
                (
                    public.to_string(),
                    program.free_callable(target).expect("その関数がない"),
                )
            })
            .collect();
        let production = crate::ambient_abi::plan_production(&program, &analysis, entry, &exports)
            .expect("計画できるはず");
        (program, production)
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
        let parsed = crate::parse::parse(&crate::lex::join(crate::lex::lex(src).unwrap())).unwrap();
        let checked = crate::typecheck::check_and_lower(&parsed).unwrap();
        let interpreted = match crate::eval::Interp::new(&checked)
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
            "struct User { name: str }\n\
             fn reached(u: User -> str) { u.name }\n\
             fn main(-> int) { let ignored = reached(User { name = \"a\" })\n 1 }\n",
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
        let (program, production) = plan_of(
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
        let shared = hir::BodyId::Callable(program.free_callable("shared").unwrap());
        assert_eq!(bodies.iter().filter(|body| **body == shared).count(), 1);
    }

    // -----------------------------------------------------------------------
    // scalar の式と制御フロー
    // -----------------------------------------------------------------------

    #[test]
    fn リテラルと束縛と代入() {
        assert_eq!(
            same_as_interpreter("fn main(-> int) {\n let n = 1\n n = n + 2\n n\n}\n"),
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
            same_as_interpreter("fn main(-> int) {\n let n = 1\n if true { n = 5 }\n n\n}\n"),
            5
        );
    }

    #[test]
    fn whileは繰り返して外の束縛を書き換える() {
        assert_eq!(
            same_as_interpreter(
                "fn main(-> int) {\n\
                 \x20 let n = 0\n\
                 \x20 let total = 0\n\
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
                 \x20 let n = 0\n\
                 \x20 let acc = 0\n\
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
                 \x20 let total = 0\n\
                 \x20 let i = 0\n\
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
        let (program, production) = plan_of(src, exports);
        messages(&crate::wasm_abi::signatures(&program, &production).expect_err("止まるはず"))
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

    /// メタデータを知らないエンジンでも検証・実行できる
    #[test]
    fn メタデータを読まなくてもモジュールは動く() {
        let bytes = compile(SURFACE, &["touch"]).expect("生成できるはず");
        validate(&bytes).expect("検証を通るはず");
        assert_eq!(scalars(&invoke(&bytes, ENTRY_EXPORT, &[]).unwrap()), [3]);
    }
}
