//! 評価。下ろした HIR を走らせる。
//!
//! 入力は型検査を通った `hir::Program` なので、実行時に名前で候補を探すことは
//! しない。局所束縛は `LocalId`、struct と enum の同一性は `StructId` /
//! `VariantId`、具体的な呼び出しは `CallableId` への直行、スロット経由は
//! その場の提供が持つ `TraitImplId` から契約メソッドを1手で引く
//! (design.md 決定9)。
//!
//! `Env` は呼び出しごとに作り直し、`Ambient` はそのまま渡す。
//! **この差1行が言語の全部**(CONTEXT.md「ambient」)。

use crate::diag::Diag;
use crate::hir;
use crate::ownership;
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};

// ---------------------------------------------------------------------------
// 値
// ---------------------------------------------------------------------------

/// 実行時の値。
///
/// ハンドラ専用の変種は**作らない**。`Postgres::new(url)` が返すのは
/// `StructId` を持つただの `Struct` で、`db.save(u)` の解決は提供された
/// 実装から引く。「ハンドラはただの impl」を値の側でも守る形。
///
/// 表示名は値が持たない。名前は `Program` の arena にあり、描画のときだけ
/// そこから引く(`Interp::show`)。
#[derive(Debug, Clone)]
pub enum Value {
    Int(i64),
    Str(String),
    Bool(bool),
    Unit,
    /// `User?` の無い方
    Nil,
    Struct(Obj),
    /// `Gold` / `Lookup.Found(user)`。variant の宣言と宣言順の payload を持つ
    /// **不変**な値。fieldless は payload が空。
    ///
    /// struct を流用しないのは、フィールドアクセス・メソッド解決・表示が
    /// enum と struct を取り違えない不変条件を作るため(design.md 決定3)
    Enum {
        variant: hir::VariantId,
        payload: Vec<Value>,
    },
    Array(Vec<Value>),
    /// 公開境界を越えた callable(`callable-handle-lifecycle`)。指す関数の
    /// 同一性は runtime の表にだけあり、値は不透明な ID しか持たない
    Callable(CallableHandle),
}

/// 公開境界を越えた callable の使い捨て handle。
///
/// runtime が採番した不透明な ID だけを持つ。`hir::CallableId` などの内部の
/// 位置は載せないし、そこから導きもしない(ADR-0011、design.md 決定2)。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct CallableHandle(u64);

/// handle と行き先の対応表。`mint` で作り、`take` で1回だけ使い切る。
/// 解放の操作はこれ以外に無い(CAB-Q2、design.md 決定1〜3)。
#[derive(Debug, Default)]
struct HandleTable {
    minted: u64,
    live: HashMap<u64, hir::CallableId>,
}

impl HandleTable {
    fn mint(&mut self, target: hir::CallableId) -> CallableHandle {
        self.minted += 1;
        self.live.insert(self.minted, target);
        CallableHandle(self.minted)
    }

    /// 生きている handle を取り出し、同時に無効にする。二度目は `None`。
    fn take(&mut self, handle: CallableHandle) -> Option<hir::CallableId> {
        self.live.remove(&handle.0)
    }
}

/// struct の実体。
///
#[derive(Debug, Clone)]
pub struct Obj {
    pub type_: hir::StructId,
}

fn new_obj(type_: hir::StructId) -> Value {
    Value::Struct(Obj { type_ })
}

// ---------------------------------------------------------------------------
// 脱出
// ---------------------------------------------------------------------------

/// 式の評価を途中で終わらせるもの。`Result` の `Err` 側に載せて `?` で運ぶ。
#[derive(Debug)]
pub enum Flow {
    /// `return` — 関数の境界で受け止める。エラーではないので診断にしない
    Return,
    /// 実行時エラー。実行前の段と同じ `Diag` に載せる。span は式の境界
    /// (`Interp::eval`)で内側から1回だけ埋まる
    Error(Diag),
}

impl std::fmt::Display for Flow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Flow::Error(d) => write!(f, "{d}"),
            Flow::Return => write!(f, "`return` が関数の外に出ました"),
        }
    }
}

pub type Eval = Result<Value, Flow>;

/// 位置なしで失敗する。span は評価中の式が `eval` の境界で付ける。
fn fail<T>(msg: impl Into<String>) -> Result<T, Flow> {
    Err(Flow::Error(Diag::msg(msg)))
}

// ---------------------------------------------------------------------------
// 所有権検査済み評価器の値基盤
// ---------------------------------------------------------------------------

/// Checked evaluator 内だけで使う store のアドレス。値としては観測できない。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct LocationId(usize);

#[derive(Clone, PartialEq, Eq, Debug)]
enum OwnedValue {
    Int(i64),
    Str(String),
    Bool(bool),
    Unit,
    Nil,
    /// 名前付き関数の値。同一性そのもので、捕捉も record も持たない
    Function(hir::CallableId),
    Location(LocationId),
}

#[derive(Debug)]
enum StoredValue {
    Struct {
        type_: hir::StructId,
        fields: BTreeMap<hir::FieldId, OwnedValue>,
    },
    Enum {
        variant: hir::VariantId,
        payload: Vec<OwnedValue>,
    },
    Array(Vec<OwnedValue>),
}

/// 環境のスロットは所有値または静的に検査済みの場所である。reference を
/// compound value に埋め込む変種は意図的に持たない。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct RuntimeSlotId(usize);

#[derive(Clone, PartialEq, Eq, Debug)]
struct RuntimePlace {
    root: RuntimeSlotId,
    path: Vec<ownership::Projection>,
}

#[derive(Debug)]
enum Slot {
    Empty,
    Owned(OwnedValue),
    Reference(RuntimePlace),
}

/// 実行1回だけに属する location store。`Vec<Option<_>>` は stable address の
/// ためだけの実装詳細で、参照カウント、GC、実行時借用カウンタを持たない。
#[derive(Debug, Default)]
struct Store {
    locations: Vec<Option<StoredValue>>,
}

impl Store {
    fn alloc(&mut self, value: StoredValue) -> OwnedValue {
        let id = LocationId(self.locations.len());
        self.locations.push(Some(value));
        OwnedValue::Location(id)
    }

    fn get(&self, id: LocationId) -> &StoredValue {
        self.locations[id.0]
            .as_ref()
            .expect("所有権計画が drop 済みの location を読んだ")
    }

    fn get_mut(&mut self, id: LocationId) -> &mut StoredValue {
        self.locations[id.0]
            .as_mut()
            .expect("所有権計画が drop 済みの location を変更した")
    }

    /// 所有グラフを再帰的に手放す。子を先に落とすので、親を除いた後に
    /// 到達不能な値が残らない。accepted program は unique ownership なので
    /// location を二度通ることはコンパイラ不変条件違反である。
    fn drop_value(&mut self, value: OwnedValue) {
        let OwnedValue::Location(id) = value else {
            return;
        };
        let stored = self.locations[id.0]
            .take()
            .expect("所有権計画が location を二度 drop した");
        match stored {
            StoredValue::Struct { fields, .. } => {
                for (_, value) in fields.into_iter().rev() {
                    self.drop_value(value);
                }
            }
            StoredValue::Enum { payload, .. } | StoredValue::Array(payload) => {
                for value in payload.into_iter().rev() {
                    self.drop_value(value);
                }
            }
        }
    }

    fn clone_value(&mut self, value: &OwnedValue) -> OwnedValue {
        match value {
            OwnedValue::Int(n) => OwnedValue::Int(*n),
            OwnedValue::Str(text) => OwnedValue::Str(text.clone()),
            OwnedValue::Bool(value) => OwnedValue::Bool(*value),
            OwnedValue::Unit => OwnedValue::Unit,
            OwnedValue::Nil => OwnedValue::Nil,
            OwnedValue::Function(callable) => OwnedValue::Function(*callable),
            OwnedValue::Location(id) => match self.get(*id) {
                StoredValue::Struct { type_, fields } => {
                    let type_ = *type_;
                    let fields: Vec<_> = fields
                        .iter()
                        .map(|(field, value)| (*field, value.clone()))
                        .collect();
                    let mut cloned = BTreeMap::new();
                    for (field, value) in fields {
                        cloned.insert(field, self.clone_value(&value));
                    }
                    self.alloc(StoredValue::Struct {
                        type_,
                        fields: cloned,
                    })
                }
                StoredValue::Enum { variant, payload } => {
                    let variant = *variant;
                    let payload = payload.clone();
                    let payload = payload
                        .iter()
                        .map(|value| self.clone_value(value))
                        .collect();
                    self.alloc(StoredValue::Enum { variant, payload })
                }
                StoredValue::Array(values) => {
                    let values = values.clone();
                    let values = values.iter().map(|value| self.clone_value(value)).collect();
                    self.alloc(StoredValue::Array(values))
                }
            },
        }
    }
}

// ---------------------------------------------------------------------------
// ownership-checked evaluator
// ---------------------------------------------------------------------------

/// `new_checked` の実行状態。compound value は全て `Store` の location で、
/// 環境には値または place しか入らない。
#[derive(Debug)]
enum CheckedValue {
    Owned(OwnedValue),
    Reference(RuntimePlace),
}

type CheckedEnv = HashMap<hir::LocalId, RuntimeSlotId>;

#[derive(Clone)]
struct CheckedAmbientBinding {
    implementation: hir::TraitImplId,
    value: Option<RuntimeSlotId>,
}

type CheckedAmbient = BTreeMap<hir::SlotId, CheckedAmbientBinding>;

struct CheckedInterp<'p> {
    program: &'p hir::Program,
    plan: &'p ownership::Plan,
    /// 呼び出しの外側にある handle 表。この実行が観測できる値を作るときだけ触る
    handles: &'p RefCell<HandleTable>,
    store: Store,
    slots: Vec<Slot>,
    returned: Option<CheckedValue>,
    ambient: Vec<CheckedAmbient>,
}

impl<'p> CheckedInterp<'p> {
    fn new(
        program: &'p hir::Program,
        plan: &'p ownership::Plan,
        handles: &'p RefCell<HandleTable>,
    ) -> Self {
        Self {
            program,
            plan,
            handles,
            store: Store::default(),
            slots: Vec::new(),
            returned: None,
            ambient: vec![CheckedAmbient::new()],
        }
    }

    fn run(mut self, entry: &str) -> Eval {
        let Some(callable) = self.program.free_callable(entry) else {
            return fail(format!("関数 `{entry}` がありません"));
        };
        let value = match self.call(callable, None, Vec::new()) {
            Ok(value) => value,
            Err(error) => {
                self.dispose();
                return Err(error);
            }
        };
        let shown = self.checked_public_value(&value)?;
        self.drop_checked(value);
        self.dispose();
        Ok(shown)
    }

    /// handle から取り出した関数を引数付きで走らせる。`run` と同じ機構
    /// (呼び出しごとの新しい `Store`、空の束縛と ambient)を通す。
    fn invoke(mut self, callable: hir::CallableId, args: Vec<Value>) -> Eval {
        let arity = self.program.callables[callable].params.len();
        if arity != args.len() {
            return fail(format!(
                "callable handle の呼び出しには引数 {arity} 個が要ります({} 個渡されました)",
                args.len()
            ));
        }
        let args = args
            .iter()
            .map(internal_value)
            .collect::<Result<Vec<_>, Flow>>()?
            .into_iter()
            .map(CheckedValue::Owned)
            .collect();
        let value = match self.call(callable, None, args) {
            Ok(value) => value,
            Err(error) => {
                self.dispose();
                return Err(error);
            }
        };
        let shown = self.checked_public_value(&value)?;
        self.drop_checked(value);
        self.dispose();
        Ok(shown)
    }

    fn run_test(mut self, id: hir::TestId) -> Eval {
        let body = &self.program.tests[id].body;
        let mut env = CheckedEnv::new();
        let frame = self.slots.len();
        let value = match self.body(body, &mut env) {
            Err(Flow::Return) => self
                .returned
                .take()
                .unwrap_or(CheckedValue::Owned(OwnedValue::Unit)),
            Ok(value) => value,
            Err(Flow::Error(diag)) => {
                self.cleanup_frame(frame);
                self.dispose();
                return Err(Flow::Error(diag));
            }
        };
        self.cleanup_frame(frame);
        let shown = self.checked_public_value(&value)?;
        self.drop_checked(value);
        self.dispose();
        Ok(shown)
    }

    fn call(
        &mut self,
        callable: hir::CallableId,
        recv: Option<CheckedValue>,
        args: Vec<CheckedValue>,
    ) -> Result<CheckedValue, Flow> {
        let declared = &self.program.callables[callable];
        let frame = self.slots.len();
        let mut env = CheckedEnv::new();
        if let (Some(local), Some(value)) = (declared.body.receiver, recv) {
            self.bind(&mut env, local, value)?;
        }
        for (local, value) in declared.params.iter().zip(args) {
            self.bind(&mut env, *local, value)?;
        }
        let result = match self.body(&declared.body, &mut env) {
            Err(Flow::Return) => Ok(self
                .returned
                .take()
                .unwrap_or(CheckedValue::Owned(OwnedValue::Unit))),
            Ok(value) => Ok(value),
            Err(Flow::Error(diag)) => Err(Flow::Error(diag)),
        };
        self.cleanup_frame(frame);
        result
    }

    fn bind(
        &mut self,
        env: &mut CheckedEnv,
        local: hir::LocalId,
        value: CheckedValue,
    ) -> Result<(), Flow> {
        let slot = RuntimeSlotId(self.slots.len());
        self.slots.push(match value {
            CheckedValue::Owned(value) => Slot::Owned(value),
            CheckedValue::Reference(place) => Slot::Reference(place),
        });
        env.insert(local, slot);
        Ok(())
    }

    fn body(&mut self, body: &hir::Body, env: &mut CheckedEnv) -> Result<CheckedValue, Flow> {
        let mut last = CheckedValue::Owned(OwnedValue::Unit);
        for id in &body.root {
            last = self.eval(body, *id, env)?;
        }
        Ok(last)
    }

    fn body_plan(&self, body: &hir::Body) -> &ownership::BodyPlan {
        let id = self
            .program
            .bodies
            .iter()
            .copied()
            .find(|id| match id {
                hir::BodyId::Callable(callable) => {
                    std::ptr::eq(body, &self.program.callables[*callable].body)
                }
                hir::BodyId::Test(test) => std::ptr::eq(body, &self.program.tests[*test].body),
            })
            .expect("評価する本体はプログラムに属する");
        self.plan.body(id)
    }

    fn eval(
        &mut self,
        body: &hir::Body,
        id: hir::ExprId,
        env: &mut CheckedEnv,
    ) -> Result<CheckedValue, Flow> {
        let expr = body.expr(id);
        if matches!(
            expr.kind,
            hir::ExprKind::Local(_) | hir::ExprKind::Field { .. } | hir::ExprKind::Access { .. }
        ) && let Some(access) = self.body_plan(body).access(id).cloned()
        {
            return self.access(env, access);
        }
        (|| match &expr.kind {
            hir::ExprKind::Int(n) => Ok(CheckedValue::Owned(OwnedValue::Int(*n))),
            hir::ExprKind::Str(s) => Ok(CheckedValue::Owned(OwnedValue::Str(s.clone()))),
            hir::ExprKind::Bool(b) => Ok(CheckedValue::Owned(OwnedValue::Bool(*b))),
            hir::ExprKind::Nil => Ok(CheckedValue::Owned(OwnedValue::Nil)),
            hir::ExprKind::UnitStruct(struct_) => {
                Ok(CheckedValue::Owned(self.store.alloc(StoredValue::Struct {
                    type_: *struct_,
                    fields: BTreeMap::new(),
                })))
            }
            hir::ExprKind::Function(callable) => {
                Ok(CheckedValue::Owned(OwnedValue::Function(*callable)))
            }
            hir::ExprKind::Variant(variant) => {
                Ok(CheckedValue::Owned(self.store.alloc(StoredValue::Enum {
                    variant: *variant,
                    payload: Vec::new(),
                })))
            }
            hir::ExprKind::Local(local) => self.read_local(env, *local),
            hir::ExprKind::Access { .. } => fail("所有権 access の計画がありません"),
            hir::ExprKind::Field {
                recv,
                field,
                optional,
            } => {
                let evaluated = self.eval(body, *recv, env)?;
                let recv_value = self.owned_ref(&evaluated)?;
                if *optional && recv_value == OwnedValue::Nil {
                    self.drop_checked(evaluated);
                    return Ok(CheckedValue::Owned(OwnedValue::Nil));
                }
                if expr.result.ty().is_some_and(|ty| self.program.is_copy(ty)) {
                    let field = self.field(recv_value, *field)?;
                    let copied = self.store.clone_value(&field);
                    self.drop_checked(evaluated);
                    Ok(CheckedValue::Owned(copied))
                } else if expr.result.ty().is_some_and(|ty| ty.reference.is_none())
                    && matches!(evaluated, CheckedValue::Owned(_))
                {
                    // Fresh compound projection is itself an owned temporary. There is
                    // no source place to borrow, so move the selected child and drop the
                    // residue exactly like a consuming bound projection.
                    let CheckedValue::Owned(value) = evaluated else {
                        unreachable!()
                    };
                    let mut path = Vec::new();
                    if *optional {
                        path.push(ownership::Projection::OptionalPayload);
                    }
                    path.push(ownership::Projection::Field(*field));
                    Ok(CheckedValue::Owned(self.extract(value, &path)?))
                } else {
                    let mut place = match evaluated {
                        CheckedValue::Reference(place) => place,
                        CheckedValue::Owned(value) => RuntimePlace {
                            root: self.alloc_slot(Slot::Owned(value)),
                            path: Vec::new(),
                        },
                    };
                    if *optional {
                        place.path.push(ownership::Projection::OptionalPayload);
                    }
                    place.path.push(ownership::Projection::Field(*field));
                    Ok(CheckedValue::Reference(place))
                }
            }
            hir::ExprKind::StructLit { struct_, fields } => {
                let mut values = BTreeMap::new();
                for (field, value) in fields {
                    let evaluated = self.eval(body, *value, env)?;
                    values.insert(*field, self.owned(evaluated)?);
                }
                Ok(CheckedValue::Owned(self.store.alloc(StoredValue::Struct {
                    type_: *struct_,
                    fields: values,
                })))
            }
            hir::ExprKind::Array(items) => {
                let mut values = Vec::with_capacity(items.len());
                for item in items {
                    let evaluated = self.eval(body, *item, env)?;
                    values.push(self.owned(evaluated)?);
                }
                Ok(CheckedValue::Owned(
                    self.store.alloc(StoredValue::Array(values)),
                ))
            }
            hir::ExprKind::Let { local, value } => {
                let value = self.eval(body, *value, env)?;
                self.bind(env, *local, value)?;
                Ok(CheckedValue::Owned(OwnedValue::Unit))
            }
            hir::ExprKind::AssignLocal { local, value } => {
                let value = self.eval(body, *value, env)?;
                let slot = *env.get(local).expect("検査済み代入先がある");
                self.drop_slot(slot);
                self.slots[slot.0] = self.slot_from(value);
                Ok(CheckedValue::Owned(OwnedValue::Unit))
            }
            hir::ExprKind::AssignField { recv, field, value } => {
                // The ownership plan records receiver access before RHS evaluation.
                // Resolve the canonical runtime place first to preserve that order.
                let place = self.place_from_expr(body, *recv)?;
                let place = self.runtime_place(env, &place)?;
                let evaluated = self.eval(body, *value, env)?;
                let value = self.owned(evaluated)?;
                self.replace_runtime_field(&place, *field, value)?;
                Ok(CheckedValue::Owned(OwnedValue::Unit))
            }
            // 組み込みの `push`(MAP-075 決定4)。`StoredValue::Array` は既に
            // `Vec` なので、容量の帳簿は Rust に任せて要素を1つ足すだけ。
            // 場所は右辺より先に解決する — 所有権計画がレシーバの排他アクセスを
            // 値の評価より前に記録しているため
            hir::ExprKind::Push { array, value } => {
                let place = self.place_from_expr(body, *array)?;
                let place = self.runtime_place(env, &place)?;
                let evaluated = self.eval(body, *value, env)?;
                let element = self.owned(evaluated)?;
                let target = self.read_place(&place)?;
                let id = self.location(target)?;
                let StoredValue::Array(values) = self.store.get_mut(id) else {
                    return fail("push の対象が配列ではありません");
                };
                values.push(element);
                Ok(CheckedValue::Owned(OwnedValue::Unit))
            }
            hir::ExprKind::Clone(inner) => {
                let evaluated = self.eval(body, *inner, env)?;
                let value = self.owned_ref(&evaluated)?;
                let cloned = self.store.clone_value(&value);
                self.drop_checked(evaluated);
                Ok(CheckedValue::Owned(cloned))
            }
            hir::ExprKind::Eq { lhs, rhs } => {
                let evaluated_lhs = self.eval(body, *lhs, env)?;
                let lhs = self.owned_ref(&evaluated_lhs)?;
                let evaluated_rhs = self.eval(body, *rhs, env)?;
                let rhs = self.owned_ref(&evaluated_rhs)?;
                let equal = self.equal(&lhs, &rhs);
                self.drop_checked(evaluated_rhs);
                self.drop_checked(evaluated_lhs);
                Ok(CheckedValue::Owned(OwnedValue::Bool(equal)))
            }
            hir::ExprKind::Neg(inner) => {
                let evaluated = self.eval(body, *inner, env)?;
                match self.owned(evaluated)? {
                    OwnedValue::Int(n) => {
                        Ok(CheckedValue::Owned(OwnedValue::Int(n.wrapping_neg())))
                    }
                    _ => fail("`-` は整数だけです"),
                }
            }
            hir::ExprKind::Arith { op, lhs, rhs } => {
                let evaluated_lhs = self.eval(body, *lhs, env)?;
                let lhs = self.owned(evaluated_lhs)?;
                let evaluated_rhs = self.eval(body, *rhs, env)?;
                let rhs = self.owned(evaluated_rhs)?;
                let (OwnedValue::Int(a), OwnedValue::Int(b)) = (lhs, rhs) else {
                    return fail("算術演算には int が必要です");
                };
                let value = match op {
                    hir::ArithOp::Add => a.wrapping_add(b),
                    hir::ArithOp::Sub => a.wrapping_sub(b),
                    hir::ArithOp::Mul => a.wrapping_mul(b),
                    hir::ArithOp::Div => match a.checked_div(b) {
                        Some(value) => value,
                        None if b == 0 => return fail("0 で割れません"),
                        None => return fail("この割り算は int の範囲を超えます"),
                    },
                };
                Ok(CheckedValue::Owned(OwnedValue::Int(value)))
            }
            hir::ExprKind::Return(value) => {
                let value = match value {
                    Some(value) => self.eval(body, *value, env)?,
                    None => CheckedValue::Owned(OwnedValue::Unit),
                };
                self.returned = Some(value);
                Err(Flow::Return)
            }
            hir::ExprKind::Assert(inner) => {
                let evaluated = self.eval(body, *inner, env)?;
                match self.owned(evaluated)? {
                    OwnedValue::Bool(true) => Ok(CheckedValue::Owned(OwnedValue::Unit)),
                    OwnedValue::Bool(false) => fail("assert が偽になりました"),
                    _ => fail("assert には bool が必要です"),
                }
            }
            hir::ExprKind::Block(ids) => {
                let mut last = CheckedValue::Owned(OwnedValue::Unit);
                for id in ids {
                    last = self.eval(body, *id, env)?;
                }
                Ok(last)
            }
            hir::ExprKind::If { cond, then, orelse } => {
                let evaluated = self.eval(body, *cond, env)?;
                let OwnedValue::Bool(cond) = self.owned(evaluated)? else {
                    return fail("条件には bool が必要です");
                };
                if cond {
                    self.eval(body, *then, env)
                } else if let Some(otherwise) = orelse {
                    self.eval(body, *otherwise, env)
                } else {
                    Ok(CheckedValue::Owned(OwnedValue::Unit))
                }
            }
            hir::ExprKind::While { cond, body: inner } => {
                loop {
                    let evaluated = self.eval(body, *cond, env)?;
                    let OwnedValue::Bool(keep_going) = self.owned(evaluated)? else {
                        return fail("条件には bool が必要です");
                    };
                    if !keep_going {
                        break;
                    }
                    self.eval(body, *inner, env)?;
                }
                Ok(CheckedValue::Owned(OwnedValue::Unit))
            }
            hir::ExprKind::Coalesce { lhs, rhs } => {
                let left = self.eval(body, *lhs, env)?;
                if self.owned_ref(&left)? == OwnedValue::Nil {
                    self.drop_checked(left);
                    self.eval(body, *rhs, env)
                } else {
                    Ok(left)
                }
            }
            hir::ExprKind::For {
                var,
                iter,
                body: inner,
            } => self.eval_for(body, *var, *iter, *inner, env),
            hir::ExprKind::Match { subject, arms } => self.eval_match(body, *subject, arms, env),
            hir::ExprKind::With {
                provisions,
                body: inner,
            } => self.eval_with(body, provisions, *inner, env),
            hir::ExprKind::Call(call) => self.call_expr(body, call, env),
            _ => fail("この構文の ownership-aware 評価はまだ未実装です"),
        })()
        .map_err(|flow| match flow {
            Flow::Error(mut diagnostic) if diagnostic.span.is_none() => {
                diagnostic.span = Some(expr.span);
                diagnostic.label = Some("ここで失敗しました".to_string());
                Flow::Error(diagnostic)
            }
            other => other,
        })
    }

    fn access(
        &mut self,
        env: &mut CheckedEnv,
        access: ownership::Access,
    ) -> Result<CheckedValue, Flow> {
        use ownership::Mode;
        let place = self.runtime_place(env, &access.place)?;
        match access.mode {
            Mode::Read => {
                let value = self.read_place(&place)?;
                Ok(CheckedValue::Owned(self.store.clone_value(&value)))
            }
            Mode::Shared | Mode::Mutable => Ok(CheckedValue::Reference(place)),
            Mode::Move => Ok(CheckedValue::Owned(self.take_place(&place)?)),
        }
    }

    fn explicit_move(&self, body: &hir::Body, id: hir::ExprId) -> bool {
        matches!(
            body.expr(id).kind,
            hir::ExprKind::Access {
                mode: hir::AccessMode::Move,
                ..
            }
        )
    }

    fn eval_for(
        &mut self,
        body: &hir::Body,
        var: hir::LocalId,
        iter: hir::ExprId,
        inner: hir::ExprId,
        env: &mut CheckedEnv,
    ) -> Result<CheckedValue, Flow> {
        let evaluated = self.eval(body, iter, env)?;
        let mut temporary = None;
        let subject = match evaluated {
            CheckedValue::Reference(place) => CheckedValue::Reference(place),
            CheckedValue::Owned(value) if self.explicit_move(body, iter) => {
                CheckedValue::Owned(value)
            }
            CheckedValue::Owned(value) => {
                let slot = self.alloc_slot(Slot::Owned(value));
                temporary = Some(slot);
                CheckedValue::Reference(RuntimePlace {
                    root: slot,
                    path: Vec::new(),
                })
            }
        };
        let result = match subject {
            CheckedValue::Reference(place) => {
                let array = self.read_place(&place)?;
                let id = self.location(array)?;
                let len = match self.store.get(id) {
                    StoredValue::Array(values) => values.len(),
                    _ => return fail("for で回せるのは配列だけです"),
                };
                let mut result = Ok(CheckedValue::Owned(OwnedValue::Unit));
                for index in 0..len {
                    let frame = self.slots.len();
                    let mut element = place.clone();
                    element
                        .path
                        .push(ownership::Projection::ArrayElement(Some(index as i64)));
                    self.bind(env, var, CheckedValue::Reference(element))?;
                    result = self.eval(body, inner, env);
                    env.remove(&var);
                    self.cleanup_frame(frame);
                    if result.is_err() {
                        break;
                    }
                }
                result
            }
            CheckedValue::Owned(value) => {
                let id = self.location(value)?;
                let stored = self.store.locations[id.0]
                    .take()
                    .expect("consuming for buffer は生存している");
                let StoredValue::Array(values) = stored else {
                    return fail("for で回せるのは配列だけです");
                };
                let mut values = values.into_iter();
                let mut result = Ok(CheckedValue::Owned(OwnedValue::Unit));
                while let Some(value) = values.next() {
                    let frame = self.slots.len();
                    self.bind(env, var, CheckedValue::Owned(value))?;
                    result = self.eval(body, inner, env);
                    env.remove(&var);
                    self.cleanup_frame(frame);
                    if result.is_err() {
                        for remaining in values {
                            self.store.drop_value(remaining);
                        }
                        break;
                    }
                }
                result
            }
        };
        if let Some(slot) = temporary {
            self.drop_slot(slot);
        }
        result
    }

    fn eval_match(
        &mut self,
        body: &hir::Body,
        subject_id: hir::ExprId,
        arms: &[hir::MatchArm],
        env: &mut CheckedEnv,
    ) -> Result<CheckedValue, Flow> {
        let evaluated = self.eval(body, subject_id, env)?;
        let mut temporary = None;
        let subject = match evaluated {
            CheckedValue::Reference(place) => CheckedValue::Reference(place),
            CheckedValue::Owned(value) if self.explicit_move(body, subject_id) => {
                CheckedValue::Owned(value)
            }
            CheckedValue::Owned(value) => {
                let slot = self.alloc_slot(Slot::Owned(value));
                temporary = Some(slot);
                CheckedValue::Reference(RuntimePlace {
                    root: slot,
                    path: Vec::new(),
                })
            }
        };
        let snapshot = self.owned_ref(&subject)?;
        let id = self.location(snapshot)?;
        let variant = match self.store.get(id) {
            StoredValue::Enum { variant, .. } => *variant,
            _ => return fail("`match` の対象は enum だけです"),
        };
        let exact = arms.iter().find(|arm| {
            matches!(arm.pattern, hir::Pattern::Variant { variant: candidate, .. } if candidate == variant)
        });
        let mut result = None;
        if let Some(arm) = exact {
            let frame = self.slots.len();
            if let hir::Pattern::Variant { bindings, .. } = &arm.pattern {
                match &subject {
                    CheckedValue::Reference(place) => {
                        for (index, binding) in bindings.iter().enumerate() {
                            if let Some(local) = binding {
                                let mut payload = place.clone();
                                payload
                                    .path
                                    .push(ownership::Projection::EnumPayload(variant, index));
                                self.bind(env, *local, CheckedValue::Reference(payload))?;
                            }
                        }
                    }
                    CheckedValue::Owned(value) => {
                        let id = self.location(value.clone())?;
                        let StoredValue::Enum { payload, .. } = self.store.locations[id.0]
                            .take()
                            .expect("consuming match subject は生存している")
                        else {
                            unreachable!()
                        };
                        for (binding, value) in bindings.iter().zip(payload) {
                            if let Some(local) = binding {
                                self.bind(env, *local, CheckedValue::Owned(value))?;
                            } else {
                                self.store.drop_value(value);
                            }
                        }
                    }
                }
            }
            let selected = match arm.guard {
                Some(guard) => {
                    let guard = self.eval(body, guard, env)?;
                    matches!(self.owned(guard)?, OwnedValue::Bool(true))
                }
                None => true,
            };
            if selected {
                result = Some(self.eval(body, arm.body, env));
            }
            if let hir::Pattern::Variant { bindings, .. } = &arm.pattern {
                for local in bindings.iter().flatten() {
                    env.remove(local);
                }
            }
            self.cleanup_frame(frame);
        }
        if result.is_none() {
            result = arms
                .iter()
                .find(|arm| matches!(arm.pattern, hir::Pattern::CatchAll))
                .map(|arm| self.eval(body, arm.body, env));
        }
        let result = result.unwrap_or_else(|| fail("一致する arm がありません"));
        if let Some(slot) = temporary {
            self.drop_slot(slot);
        }
        if let CheckedValue::Owned(OwnedValue::Location(id)) = subject
            && self.store.locations.get(id.0).is_some_and(Option::is_some)
        {
            self.store.drop_value(OwnedValue::Location(id));
        }
        result
    }

    fn eval_with(
        &mut self,
        body: &hir::Body,
        provisions: &[hir::Provision],
        inner: hir::ExprId,
        env: &mut CheckedEnv,
    ) -> Result<CheckedValue, Flow> {
        let mut evaluated = Vec::with_capacity(provisions.len());
        for provision in provisions {
            evaluated.push(match provision.value {
                Some(value) => Some(self.eval(body, value, env)?),
                None => None,
            });
        }
        let mut next = self.ambient.last().cloned().unwrap_or_default();
        let mut owned_slots = Vec::new();
        for (provision, value) in provisions.iter().zip(evaluated) {
            let value = value.map(|value| {
                let slot = self.alloc_slot(self.slot_from(value));
                owned_slots.push(slot);
                slot
            });
            next.insert(
                provision.slot,
                CheckedAmbientBinding {
                    implementation: provision.implementation,
                    value,
                },
            );
        }
        self.ambient.push(next);
        let result = self.eval(body, inner, env);
        self.ambient.pop();
        for slot in owned_slots.into_iter().rev() {
            self.drop_slot(slot);
        }
        result
    }

    fn call_expr(
        &mut self,
        body: &hir::Body,
        call: &hir::Call,
        env: &mut CheckedEnv,
    ) -> Result<CheckedValue, Flow> {
        match call {
            hir::Call::Direct { callable, args } | hir::Call::Associated { callable, args } => {
                let args = self.args(body, args, env)?;
                self.call(*callable, None, args)
            }
            hir::Call::Method {
                callable,
                recv,
                args,
            } => {
                let recv = self.eval(body, *recv, env)?;
                let args = self.args(body, args, env)?;
                self.call(*callable, Some(recv), args)
            }
            hir::Call::Ctor { variant, args } => {
                let args = self.args(body, args, env)?;
                let mut payload = Vec::new();
                for value in args {
                    payload.push(self.owned(value)?);
                }
                Ok(CheckedValue::Owned(self.store.alloc(StoredValue::Enum {
                    variant: *variant,
                    payload,
                })))
            }
            // 呼び先は callable 値そのものが持っている。別名で写しても
            // 同じ名前付き関数を指す(design.md 決定4)
            hir::Call::Indirect { callee, args } => {
                let callee = self.eval(body, *callee, env)?;
                let callable = match callee {
                    CheckedValue::Owned(OwnedValue::Function(callable)) => callable,
                    _ => return fail("間接呼び出しの呼び先が callable 値ではありません"),
                };
                let args = self.args(body, args, env)?;
                self.call(callable, None, args)
            }
            hir::Call::Slot {
                slot,
                method,
                receiver,
                args,
                ..
            } => {
                let binding = self
                    .ambient
                    .last()
                    .and_then(|ambient| ambient.get(slot))
                    .cloned()
                    .ok_or_else(|| Flow::Error(Diag::msg("必要な値が提供されていません")))?;
                let callable = self
                    .program
                    .implementation_of(binding.implementation, *method)
                    .ok_or_else(|| {
                        Flow::Error(Diag::msg("提供された実装にメソッドがありません"))
                    })?;
                let recv = match receiver {
                    hir::SlotReceiver::Type => None,
                    hir::SlotReceiver::Value => {
                        let slot = binding.value.ok_or_else(|| {
                            Flow::Error(Diag::msg("型だけの提供には実体がありません"))
                        })?;
                        let place = RuntimePlace {
                            root: slot,
                            path: Vec::new(),
                        };
                        Some(match self.program.callables[callable].receiver {
                            Some(hir::ReceiverMode::Owned) => {
                                CheckedValue::Owned(self.take_place(&place)?)
                            }
                            Some(hir::ReceiverMode::Shared | hir::ReceiverMode::Mutable) => {
                                CheckedValue::Reference(place)
                            }
                            None => return fail("実体スロット呼び出しに receiver がありません"),
                        })
                    }
                };
                let args = self.args(body, args, env)?;
                self.call(callable, recv, args)
            }
        }
    }

    fn args(
        &mut self,
        body: &hir::Body,
        args: &[hir::ExprId],
        env: &mut CheckedEnv,
    ) -> Result<Vec<CheckedValue>, Flow> {
        args.iter().map(|arg| self.eval(body, *arg, env)).collect()
    }

    fn alloc_slot(&mut self, slot: Slot) -> RuntimeSlotId {
        let id = RuntimeSlotId(self.slots.len());
        self.slots.push(slot);
        id
    }

    fn slot_from(&self, value: CheckedValue) -> Slot {
        match value {
            CheckedValue::Owned(value) => Slot::Owned(value),
            CheckedValue::Reference(place) => Slot::Reference(place),
        }
    }

    fn runtime_place(
        &self,
        env: &CheckedEnv,
        place: &ownership::Place,
    ) -> Result<RuntimePlace, Flow> {
        let root = *env
            .get(&place.root)
            .ok_or_else(|| Flow::Error(Diag::msg("place の runtime slot がありません")))?;
        match &self.slots[root.0] {
            Slot::Reference(base) => {
                let mut resolved = base.clone();
                resolved.path.extend(place.path.iter().cloned());
                Ok(resolved)
            }
            _ => Ok(RuntimePlace {
                root,
                path: place.path.clone(),
            }),
        }
    }

    fn drop_slot(&mut self, slot: RuntimeSlotId) {
        if let Slot::Owned(value) = std::mem::replace(&mut self.slots[slot.0], Slot::Empty) {
            self.store.drop_value(value);
        }
    }

    fn drop_checked(&mut self, value: CheckedValue) {
        if let CheckedValue::Owned(value) = value {
            self.store.drop_value(value);
        }
    }

    fn cleanup_frame(&mut self, start: usize) {
        for index in (start..self.slots.len()).rev() {
            self.drop_slot(RuntimeSlotId(index));
        }
    }

    fn dispose(&mut self) {
        for index in (0..self.slots.len()).rev() {
            self.drop_slot(RuntimeSlotId(index));
        }
        // `f(make(), 1 / 0)` のように、後続の部分式が失敗した時点では前の
        // 一時所有値がまだ slot に入っていない。実行文脈そのものを捨てる最後の
        // sweep は、そうした root も allocation の逆順に再帰 drop する。
        for index in (0..self.store.locations.len()).rev() {
            if self.store.locations[index].is_some() {
                self.store
                    .drop_value(OwnedValue::Location(LocationId(index)));
            }
        }
        debug_assert!(self.store.locations.iter().all(Option::is_none));
    }

    fn extract(
        &mut self,
        value: OwnedValue,
        path: &[ownership::Projection],
    ) -> Result<OwnedValue, Flow> {
        let Some((head, tail)) = path.split_first() else {
            return Ok(value);
        };
        if matches!(head, ownership::Projection::OptionalPayload) {
            return self.extract(value, tail);
        }
        let id = self.location(value)?;
        let stored = self.store.locations[id.0]
            .take()
            .expect("consuming projection の root は生存している");
        let selected = match (stored, head) {
            (StoredValue::Struct { fields, .. }, ownership::Projection::Field(field)) => {
                let mut selected = None;
                for (candidate, child) in fields.into_iter().rev() {
                    if candidate == *field {
                        selected = Some(child);
                    } else {
                        self.store.drop_value(child);
                    }
                }
                selected.ok_or_else(|| Flow::Error(Diag::msg("フィールドがありません")))?
            }
            (StoredValue::Enum { payload, .. }, ownership::Projection::EnumPayload(_, index)) => {
                let mut selected = None;
                for (candidate, child) in payload.into_iter().enumerate().rev() {
                    if candidate == *index {
                        selected = Some(child);
                    } else {
                        self.store.drop_value(child);
                    }
                }
                selected.ok_or_else(|| Flow::Error(Diag::msg("payload がありません")))?
            }
            (StoredValue::Array(values), ownership::Projection::ArrayElement(Some(index))) => {
                let mut selected = None;
                for (candidate, child) in values.into_iter().enumerate().rev() {
                    if i64::try_from(candidate) == Ok(*index) {
                        selected = Some(child);
                    } else {
                        self.store.drop_value(child);
                    }
                }
                selected.ok_or_else(|| Flow::Error(Diag::msg("配列要素がありません")))?
            }
            (stored, _) => {
                for child in match stored {
                    StoredValue::Struct { fields, .. } => fields.into_values().collect(),
                    StoredValue::Enum { payload, .. } | StoredValue::Array(payload) => payload,
                } {
                    self.store.drop_value(child);
                }
                return fail("consuming projection の形が値と一致しません");
            }
        };
        self.extract(selected, tail)
    }

    fn read_local(&mut self, env: &CheckedEnv, local: hir::LocalId) -> Result<CheckedValue, Flow> {
        match env.get(&local).map(|slot| &self.slots[slot.0]) {
            Some(Slot::Owned(value)) => Ok(CheckedValue::Owned(value.clone())),
            Some(Slot::Reference(place)) => Ok(CheckedValue::Reference(place.clone())),
            _ => fail("未初期化または move 済みの local を読みました"),
        }
    }

    fn owned(&self, value: CheckedValue) -> Result<OwnedValue, Flow> {
        match value {
            CheckedValue::Owned(value) => Ok(value),
            CheckedValue::Reference(place) => self.read_place(&place),
        }
    }

    fn owned_ref(&self, value: &CheckedValue) -> Result<OwnedValue, Flow> {
        match value {
            CheckedValue::Owned(value) => Ok(value.clone()),
            CheckedValue::Reference(place) => self.read_place(place),
        }
    }

    fn read_place(&self, place: &RuntimePlace) -> Result<OwnedValue, Flow> {
        let root = match &self.slots[place.root.0] {
            Slot::Owned(value) => value.clone(),
            Slot::Reference(other) => {
                let mut joined = other.clone();
                joined.path.extend(place.path.iter().cloned());
                return self.read_place(&joined);
            }
            _ => return fail("move 済みの place を読みました"),
        };
        self.follow(root, &place.path)
    }

    fn follow(
        &self,
        mut value: OwnedValue,
        path: &[ownership::Projection],
    ) -> Result<OwnedValue, Flow> {
        for projection in path {
            value = match projection {
                ownership::Projection::Field(field) => {
                    match self.store.get(self.location(value)?) {
                        StoredValue::Struct { fields, .. } => fields
                            .get(field)
                            .cloned()
                            .ok_or_else(|| Flow::Error(Diag::msg("フィールドがありません")))?,
                        _ => return fail("field projection の対象は struct ではありません"),
                    }
                }
                ownership::Projection::OptionalPayload => value,
                ownership::Projection::EnumPayload(_, index) => {
                    match self.store.get(self.location(value)?) {
                        StoredValue::Enum { payload, .. } => payload
                            .get(*index)
                            .cloned()
                            .ok_or_else(|| Flow::Error(Diag::msg("payload がありません")))?,
                        _ => return fail("payload projection の対象は enum ではありません"),
                    }
                }
                ownership::Projection::ArrayElement(Some(index)) => {
                    match self.store.get(self.location(value)?) {
                        StoredValue::Array(values) => values
                            .get(
                                usize::try_from(*index)
                                    .map_err(|_| Flow::Error(Diag::msg("配列添字が範囲外です")))?,
                            )
                            .cloned()
                            .ok_or_else(|| Flow::Error(Diag::msg("配列要素がありません")))?,
                        _ => return fail("要素 projection の対象は配列ではありません"),
                    }
                }
                ownership::Projection::ArrayElement(None) => {
                    return fail("実行時の配列 place には具体的な添字が必要です");
                }
            };
        }
        Ok(value)
    }

    fn take_place(&mut self, place: &RuntimePlace) -> Result<OwnedValue, Flow> {
        let root = std::mem::replace(&mut self.slots[place.root.0], Slot::Empty);
        match root {
            Slot::Owned(value) if place.path.is_empty() => Ok(value),
            Slot::Owned(value) => self.extract(value, &place.path),
            Slot::Reference(_) => fail("borrowed place を move しました"),
            Slot::Empty => fail("move 済みの place を move しました"),
        }
    }

    fn place_from_expr(&self, body: &hir::Body, id: hir::ExprId) -> Result<ownership::Place, Flow> {
        match &body.expr(id).kind {
            hir::ExprKind::Access { place, .. } => self.place_from_expr(body, *place),
            hir::ExprKind::Local(root) => Ok(ownership::Place {
                root: *root,
                path: Vec::new(),
            }),
            hir::ExprKind::Field {
                recv,
                field,
                optional: false,
            } => {
                let mut place = self.place_from_expr(body, *recv)?;
                place.path.push(ownership::Projection::Field(*field));
                Ok(place)
            }
            _ => fail("代入先が place ではありません"),
        }
    }

    fn replace_runtime_field(
        &mut self,
        place: &RuntimePlace,
        field: hir::FieldId,
        value: OwnedValue,
    ) -> Result<(), Flow> {
        let parent = self.read_place(place)?;
        let id = self.location(parent)?;
        let old = {
            let StoredValue::Struct { fields, .. } = self.store.get_mut(id) else {
                return fail("フィールドを持たない値には代入できません");
            };
            fields.insert(field, value)
        };
        if let Some(old) = old {
            self.store.drop_value(old);
        }
        Ok(())
    }

    fn field(&self, value: OwnedValue, field: hir::FieldId) -> Result<OwnedValue, Flow> {
        self.follow(value, &[ownership::Projection::Field(field)])
    }
    fn location(&self, value: OwnedValue) -> Result<LocationId, Flow> {
        match value {
            OwnedValue::Location(id) => Ok(id),
            _ => fail("compound value の location が必要です"),
        }
    }

    fn equal(&self, left: &OwnedValue, right: &OwnedValue) -> bool {
        match (left, right) {
            (OwnedValue::Int(a), OwnedValue::Int(b)) => a == b,
            (OwnedValue::Str(a), OwnedValue::Str(b)) => a == b,
            (OwnedValue::Bool(a), OwnedValue::Bool(b)) => a == b,
            (OwnedValue::Unit, OwnedValue::Unit) | (OwnedValue::Nil, OwnedValue::Nil) => true,
            (OwnedValue::Location(a), OwnedValue::Location(b)) => {
                match (self.store.get(*a), self.store.get(*b)) {
                    (
                        StoredValue::Struct {
                            type_: ta,
                            fields: fa,
                        },
                        StoredValue::Struct {
                            type_: tb,
                            fields: fb,
                        },
                    ) => {
                        ta == tb
                            && fa.len() == fb.len()
                            && fa
                                .iter()
                                .zip(fb)
                                .all(|((ka, va), (kb, vb))| ka == kb && self.equal(va, vb))
                    }
                    (
                        StoredValue::Enum {
                            variant: va,
                            payload: pa,
                        },
                        StoredValue::Enum {
                            variant: vb,
                            payload: pb,
                        },
                    ) => {
                        va == vb
                            && pa.len() == pb.len()
                            && pa.iter().zip(pb).all(|(a, b)| self.equal(a, b))
                    }
                    (StoredValue::Array(a), StoredValue::Array(b)) => {
                        a.len() == b.len() && a.iter().zip(b).all(|(a, b)| self.equal(a, b))
                    }
                    _ => false,
                }
            }
            _ => false,
        }
    }

    fn checked_public_value(&self, value: &CheckedValue) -> Eval {
        let value = self.owned_ref(value)?;
        self.public_value(&value)
    }
    fn public_value(&self, value: &OwnedValue) -> Eval {
        Ok(match value {
            OwnedValue::Int(n) => Value::Int(*n),
            OwnedValue::Str(s) => Value::Str(s.clone()),
            OwnedValue::Bool(b) => Value::Bool(*b),
            OwnedValue::Unit => Value::Unit,
            OwnedValue::Nil => Value::Nil,
            // 公開境界を越えてよいかは型検査(`callable-public-boundary`)が
            // 既に見ている。ここは越えた callable に使い捨ての姿を与えるだけ
            OwnedValue::Function(callable) => {
                Value::Callable(self.handles.borrow_mut().mint(*callable))
            }
            OwnedValue::Location(id) => match self.store.get(*id) {
                StoredValue::Struct { type_, .. } => new_obj(*type_),
                StoredValue::Enum { variant, payload } => Value::Enum {
                    variant: *variant,
                    payload: payload
                        .iter()
                        .map(|value| self.public_value(value))
                        .collect::<Result<_, _>>()?,
                },
                StoredValue::Array(values) => Value::Array(
                    values
                        .iter()
                        .map(|value| self.public_value(value))
                        .collect::<Result<_, _>>()?,
                ),
            },
        })
    }
}

/// 観測できる値を内部表現へ戻す。scalar と unit だけを運ぶ。
///
/// struct / enum / array の marshalling は host 境界(Wasm ABI)が形を決める
/// CAB-030 以降の仕事なので、ここでは panic せず診断で断る(design.md 決定6)。
fn internal_value(value: &Value) -> Result<OwnedValue, Flow> {
    Ok(match value {
        Value::Int(n) => OwnedValue::Int(*n),
        Value::Str(text) => OwnedValue::Str(text.clone()),
        Value::Bool(b) => OwnedValue::Bool(*b),
        Value::Unit => OwnedValue::Unit,
        Value::Nil => OwnedValue::Nil,
        Value::Struct(_) | Value::Enum { .. } | Value::Array(_) | Value::Callable(_) => {
            return fail(format!(
                "callable handle の呼び出しが運べる引数は今のところ scalar と unit だけです({value:?})"
            ));
        }
    })
}

// ---------------------------------------------------------------------------
// インタプリタ
// ---------------------------------------------------------------------------

pub struct Interp<'p> {
    program: &'p hir::Program,
    plan: &'p ownership::Plan,
    /// 公開境界を越えた callable の使い捨て handle 表。
    ///
    /// 実行1回で捨てる `CheckedInterp` より長生きしなければならないので
    /// `Interp` が持つ。`run`/`run_test`/`show` の `&self` を変えずに採番する
    /// ため内部可変性で包む(design.md 決定1)
    handles: RefCell<HandleTable>,
}

impl<'p> Interp<'p> {
    /// 所有権検査済み HIR を評価する入口。
    pub fn new_checked(checked: &'p ownership::CheckedProgram) -> Self {
        Interp {
            program: &checked.hir,
            plan: &checked.plan,
            handles: RefCell::default(),
        }
    }

    /// 値の表示。値が名前を持たないので、描画はプログラムを知っている側の仕事
    /// (design.md 決定9)。CLI の綴りは従来どおり。
    pub fn show(&self, value: &Value) -> String {
        match value {
            Value::Int(n) => n.to_string(),
            Value::Str(s) => format!("{s:?}"),
            Value::Bool(b) => b.to_string(),
            Value::Unit => "unit".to_string(),
            Value::Nil => "nil".to_string(),
            Value::Struct(o) => self.program.structs[o.type_].name.clone(),
            Value::Enum { variant, payload } => {
                let declared = &self.program.variants[*variant];
                let owner = &self.program.enums[declared.owner].name;
                if payload.is_empty() {
                    format!("{owner}.{}", declared.name)
                } else {
                    let shown: Vec<String> = payload.iter().map(|v| self.show(v)).collect();
                    format!("{owner}.{}({})", declared.name, shown.join(", "))
                }
            }
            Value::Array(xs) => format!("[{} 要素]", xs.len()),
            // 指す関数の名前は出さない。出せるのは採番した不透明な ID だけ
            Value::Callable(handle) => format!("callable#{}", handle.0),
        }
    }

    /// エントリ(`main`)を呼ぶ。
    ///
    /// **ambient は空から始まる。**提供されていないものは何も届かない、が出発点。
    pub fn run(&self, entry: &str) -> Eval {
        CheckedInterp::new(self.program, self.plan, &self.handles).run(entry)
    }

    /// `test` の本体を走らせる。関数と同じ扱いで、`Env` も `Ambient` も空から。
    pub fn run_test(&self, id: hir::TestId) -> Eval {
        CheckedInterp::new(self.program, self.plan, &self.handles).run_test(id)
    }

    /// handle を1つ使い切って、その行き先を引数付きで呼ぶ。
    ///
    /// 表からの取り出しは行き先を走らせる**前**なので、行き先自身が失敗しても
    /// handle は消費される(design.md 決定3)。解放の操作は別に無い。
    /// 既に使った handle と、そもそも採番していない handle は同じ形の実行時
    /// 診断で断る(design.md 決定4)。
    ///
    /// いまの呼び出し元はこのモジュールの単体テストだけ。CLI と Wasm 側の
    /// invoke export は CAB-030 の仕事なので、それまで未使用の警告を抑える
    #[allow(dead_code)]
    pub fn invoke(&self, handle: CallableHandle, args: Vec<Value>) -> Eval {
        let Some(callable) = self.handles.borrow_mut().take(handle) else {
            return fail("この callable handle は既に使い切られたか、存在しません");
        };
        CheckedInterp::new(self.program, self.plan, &self.handles).invoke(callable, args)
    }
}

// ---------------------------------------------------------------------------
// テスト
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lex::{join, lex};
    use crate::parse;

    /// ソースを型検査して下ろし、指定した関数を引数なしで呼ぶ。
    /// 失敗は診断のまま返すので、文言も span も見られる(`Display` は文言だけ)。
    ///
    /// 評価器の入力は型検査を通った HIR だけなので、テストの題材も型が
    /// 合っていなければならない(design.md 決定7)。
    fn run(src: &str, entry: &str) -> Result<Value, Diag> {
        let program = parse::parse(&join(lex(src).unwrap())).expect("パースできるはず");
        let hir = crate::typecheck::check_and_lower(&program, &[])
            .unwrap_or_else(|d| panic!("型検査を通るはず: {d:?}"));
        let checked =
            crate::ownership::check(hir).unwrap_or_else(|d| panic!("所有権検査を通るはず: {d:?}"));
        Interp::new_checked(&checked)
            .run(entry)
            .map_err(|f| match f {
                Flow::Error(d) => d,
                Flow::Return => panic!("return が関数境界を越えた"),
            })
    }

    fn checked_run(src: &str, entry: &str) -> Result<Value, Diag> {
        let program = parse::parse(&join(lex(src).unwrap())).expect("パースできるはず");
        let hir = crate::typecheck::check_and_lower(&program, &[])
            .unwrap_or_else(|d| panic!("型検査を通るはず: {d:?}"));
        let checked =
            crate::ownership::check(hir).unwrap_or_else(|d| panic!("所有権検査を通るはず: {d:?}"));
        Interp::new_checked(&checked)
            .run(entry)
            .map_err(|f| match f {
                Flow::Error(d) => d,
                Flow::Return => panic!("return が関数境界を越えた"),
            })
    }

    // ---- 名前付き関数の値(tasks 4.1) ----

    const CALLBACK_SRC: &str = "fn double(value: int -> int) { value * 2 }
fn negate(value: int -> int) { 0 - value }
fn apply(f: fn(int -> int), value: int -> int) { f(value) }
";

    #[test]
    fn callback引数を通した呼び出しが走る() {
        let src = format!("{CALLBACK_SRC}fn main(-> int) {{ apply(double, 21) }}\n");
        assert!(matches!(run(&src, "main"), Ok(Value::Int(42))));
    }

    /// 別名で写しても同じ名前付き関数を指す(Copy)
    #[test]
    fn callable値の別名は同じ関数を指す() {
        let src = format!(
            "{CALLBACK_SRC}fn main(-> int) {{ let f = double
 let g = f
 apply(f, 1) + apply(g, 2) + g(3) }}
"
        );
        assert!(matches!(run(&src, "main"), Ok(Value::Int(12))));
    }

    /// callback は helper を通して何段でも渡せる
    #[test]
    fn callbackはhelperを跨いで渡せる() {
        let src = format!(
            "{CALLBACK_SRC}fn twice(f: fn(int -> int), value: int -> int) {{ apply(f, apply(f, value)) }}
fn main(-> int) {{ twice(double, 3) + twice(negate, 4) }}
"
        );
        assert!(matches!(run(&src, "main"), Ok(Value::Int(16))));
    }

    // ---- 公開境界を越えた callable の handle(callable-handle-lifecycle) ----

    /// 公開エクスポートは entry module の `pub use` からしか生まれないので、
    /// 2ファイル構成で読み込む。`run`/`checked_run` が渡している空の
    /// `public_exports` では CAB-010 の callable 戻り値の緩和が効かない
    fn checked_public(lib: &str) -> ownership::CheckedProgram {
        let loaded = crate::module::load_files(&[
            (
                "main.rd",
                "pub use lib::{pick, risky}\nfn main(-> int) { 1 }\n",
            ),
            ("lib.rd", lib),
        ])
        .expect("ロードできるはず");
        let hir = crate::typecheck::check_and_lower(&loaded.program, &loaded.public_exports)
            .unwrap_or_else(|d| panic!("型検査を通るはず: {d:?}"));
        crate::ownership::check(hir).unwrap_or_else(|d| panic!("所有権検査を通るはず: {d:?}"))
    }

    /// `pick` は要求ゼロの `double` をそのまま返す。`risky` が返す `guard` は
    /// 引数しだいで実行時に落ちるので、失敗しても handle が消えることを見られる
    const HANDLE_LIB: &str = "fn double(value: int -> int) { value * 2 }
fn guard(value: int -> int) { assert value == 200
 value }
fn pick(-> fn(int -> int)) { double }
fn risky(-> fn(int -> int)) { guard }
";

    fn handle_of(interp: &Interp<'_>, entry: &str) -> CallableHandle {
        match interp.run(entry) {
            Ok(Value::Callable(handle)) => handle,
            other => panic!("handle が出るはず: {other:?}"),
        }
    }

    fn diag_of(result: Eval) -> Diag {
        match result {
            Err(Flow::Error(diag)) => diag,
            other => panic!("実行時診断で断るはず: {other:?}"),
        }
    }

    #[test]
    fn 公開エクスポートのcallable戻り値はhandleとして観測できる() {
        let checked = checked_public(HANDLE_LIB);
        let interp = Interp::new_checked(&checked);
        let handle = handle_of(&interp, "lib::pick");
        // 表示にも行き先の名前は出ない
        assert_eq!(interp.show(&Value::Callable(handle)), "callable#1");
    }

    #[test]
    fn handleは1回だけ呼べる() {
        let checked = checked_public(HANDLE_LIB);
        let interp = Interp::new_checked(&checked);
        let handle = handle_of(&interp, "lib::pick");
        assert!(matches!(
            interp.invoke(handle, vec![Value::Int(21)]),
            Ok(Value::Int(42))
        ));
    }

    /// 同じ関数をもう一度返せば別の handle が出る。回復の道は残っている
    #[test]
    fn 同じhandleの二度目の呼び出しは断る() {
        let checked = checked_public(HANDLE_LIB);
        let interp = Interp::new_checked(&checked);
        let handle = handle_of(&interp, "lib::pick");
        assert!(matches!(
            interp.invoke(handle, vec![Value::Int(1)]),
            Ok(Value::Int(2))
        ));
        assert!(
            diag_of(interp.invoke(handle, vec![Value::Int(1)]))
                .msg
                .contains("既に使い切られた")
        );

        let fresh = handle_of(&interp, "lib::pick");
        assert_ne!(fresh, handle, "採番し直した handle は別物");
        assert!(matches!(
            interp.invoke(fresh, vec![Value::Int(3)]),
            Ok(Value::Int(6))
        ));
    }

    /// 解放は呼び出しの中で起きる。行き先の成否には依らない(CAB-Q2)
    #[test]
    fn 行き先が失敗してもhandleは消費される() {
        let checked = checked_public(HANDLE_LIB);
        let interp = Interp::new_checked(&checked);
        let handle = handle_of(&interp, "lib::risky");
        assert!(
            diag_of(interp.invoke(handle, vec![Value::Int(1)]))
                .msg
                .contains("assert"),
            "行き先自身が実行時に落ちる"
        );
        assert!(
            diag_of(interp.invoke(handle, vec![Value::Int(200)]))
                .msg
                .contains("既に使い切られた")
        );
    }

    #[test]
    fn 採番していないhandleの呼び出しは断る() {
        let checked = checked_public(HANDLE_LIB);
        let interp = Interp::new_checked(&checked);
        assert!(
            diag_of(interp.invoke(CallableHandle(9999), vec![Value::Int(1)]))
                .msg
                .contains("既に使い切られた")
        );
    }

    #[test]
    fn handleの呼び出しは引数の個数を見る() {
        let checked = checked_public(HANDLE_LIB);
        let interp = Interp::new_checked(&checked);
        let handle = handle_of(&interp, "lib::pick");
        assert!(
            diag_of(interp.invoke(handle, Vec::new()))
                .msg
                .contains("引数 1 個が要ります")
        );
    }

    #[test]
    fn ownership検査済み入口は計画に対応する式を評価する() {
        let src = "fn main(-> int) { let n = 40\n n + 2 }\n";
        let parsed = parse::parse(&join(lex(src).unwrap())).expect("パースできる");
        let hir = crate::typecheck::check_and_lower(&parsed, &[]).expect("型検査を通る");
        let checked = crate::ownership::check(hir).expect("所有権検査を通る");
        let interp = Interp::new_checked(&checked);
        assert!(matches!(interp.run("main"), Ok(Value::Int(42))));
    }

    #[test]
    fn ownership検査済み評価器はcloneとfield更新をstoreで実行する() {
        let src = "struct User { score: int }\n\
fn main(-> int) {\n\
  let mut original = User { score = 1 }\n\
  let copied = original.clone()\n\
  original.score = 9\n\
  copied.score\n\
}\n";
        let parsed = parse::parse(&join(lex(src).unwrap())).expect("パースできる");
        let hir = crate::typecheck::check_and_lower(&parsed, &[]).expect("型検査を通る");
        let checked = crate::ownership::check(hir).expect("所有権検査を通る");
        assert!(matches!(
            Interp::new_checked(&checked).run("main"),
            Ok(Value::Int(1))
        ));
    }

    #[test]
    fn checked評価器の排他借用は呼び出し境界を越えて元を更新する() {
        let src = "struct User { score: int }\n\
fn edit(user: &mut User) { user.score = 7 }\n\
fn main(-> int) { let mut user = User { score = 1 }\n\
 edit(&mut user)\n user.score }\n";
        let value = checked_run(src, "main").unwrap_or_else(|d| panic!("{d:?}"));
        assert!(matches!(value, Value::Int(7)), "{value:?}");
    }

    #[test]
    fn checked評価器は消費matchとforを所有値で実行する() {
        let src = "struct User { score: int }\n\
enum Box { Full(User) Empty }\n\
fn take(user: User -> int) { user.score }\n\
fn main(-> int) {\n\
 let boxed = Box::Full(User { score = 2 })\n\
 let from_box = match move boxed { Box::Full(user): take(move user)\n\
   Box::Empty: 0 }\n\
 let users = [User { score = 3 }, User { score = 4 }]\n\
 let mut total = from_box\n\
 for user in move users { total = total + take(move user) }\n\
 total\n}\n";
        assert!(matches!(checked_run(src, "main"), Ok(Value::Int(9))));
    }

    #[test]
    fn checked評価器はcoalesceとproviderを所有モードどおり実行する() {
        let src = "struct User { score: int }\n\
trait Reader { fn read(&self -> int) }\n\
impl Reader for User { fn read(&self -> int) { self.score } }\n\
effect reader: Reader\n\
fn from_reader(-> int) { reader.read() }\n\
fn take(user: User -> int) { user.score }\n\
fn main(-> int) {\n\
 let maybe: User? = User { score = 5 }\n\
 let chosen = move maybe ?? User { score = 0 }\n\
 let provider = User { score = 6 }\n\
 let provided = with reader(move provider) { from_reader() }\n\
 take(move chosen) + provided\n}\n";
        assert!(matches!(checked_run(src, "main"), Ok(Value::Int(11))));
    }

    #[test]
    fn checked評価器は返された排他参照の出自を呼び出し元へ戻す() {
        let src = "struct User { score: int }\n\
fn identity(user: &mut User -> &mut User) { user }\n\
fn main(-> int) {\n\
 let mut user = User { score = 1 }\n\
 let edit = identity(&mut user)\n\
 edit.score = 8\n\
 user.score\n}\n";
        assert!(matches!(checked_run(src, "main"), Ok(Value::Int(8))));
    }

    #[test]
    fn checked評価器は借用matchでpayloadを更新する() {
        let src = "struct User { score: int }\n\
enum Box { Full(User) Empty }\n\
fn main(-> int) {\n\
 let mut boxed = Box::Full(User { score = 1 })\n\
 match &mut boxed { Box::Full(user): user.score = 9\n\
   Box::Empty: assert true }\n\
 match boxed { Box::Full(user): user.score\n\
   Box::Empty: 0 }\n}\n";
        assert!(matches!(checked_run(src, "main"), Ok(Value::Int(9))));
    }

    #[test]
    fn checked評価器はearly_returnと失敗でも実行文脈を破棄する() {
        let success = "struct User { name: str }\n\
fn choose(-> int) { let user = User { name = \"a\" }\n return 3 }\n\
fn main(-> int) { choose() }\n";
        assert!(matches!(checked_run(success, "main"), Ok(Value::Int(3))));

        let failure = "struct User { name: str }\n\
fn main(-> int) { let user = User { name = \"a\" }\n assert false\n 0 }\n";
        assert!(checked_run(failure, "main").is_err());

        // The first argument is an unrooted owned temporary when evaluation of
        // the second fails. Context disposal must sweep it as well as slots.
        let partial_argument = "struct User { score: int }\n\
fn consume(user: User, n: int) {}\n\
fn main(-> int) { consume(User { score = 1 }, 1 / 0)\n 0 }\n";
        assert!(checked_run(partial_argument, "main").is_err());
    }

    #[test]
    fn checked評価器はindirect所有グラフをcloneして再帰dropする() {
        let src = "struct Node { value: int, indirect next: Node? }\n\
fn main(-> int) {\n\
 let leaf = Node { value = 2, next = nil }\n\
 let root = Node { value = 1, next = leaf }\n\
 let copied = root.clone()\n\
 copied.value + root.value\n}\n";
        assert!(matches!(checked_run(src, "main"), Ok(Value::Int(2))));
    }

    #[test]
    fn checked評価器はfieldless_enumをcopyして所有を重ねない() {
        let src = "enum Rank { Gold Silver }\n\
fn main(-> bool) {\n\
 let rank = Gold\n\
 let copied = rank\n\
 copied == rank\n}\n";
        assert!(matches!(checked_run(src, "main"), Ok(Value::Bool(true))));
    }

    #[test]
    fn checked評価器は一時structの非copy_fieldを所有として取り出す() {
        let src = "struct Inner { score: int }\n\
struct Outer { inner: Inner, other: Inner }\n\
fn take(inner: Inner -> int) { inner.score }\n\
fn main(-> int) {\n\
 let selected = Outer { inner = Inner { score = 7 }, other = Inner { score = 9 } }.inner\n\
 take(move selected)\n}\n";
        assert!(matches!(checked_run(src, "main"), Ok(Value::Int(7))));
    }

    #[test]
    fn owned_storeのcloneは配列を独立locationへ複製する() {
        let mut store = Store::default();
        let original = store.alloc(StoredValue::Array(vec![OwnedValue::Int(1)]));
        let cloned = store.clone_value(&original);
        let (OwnedValue::Location(original), OwnedValue::Location(cloned)) = (original, cloned)
        else {
            panic!("compound values are locations");
        };
        assert_ne!(original, cloned);
        let StoredValue::Array(values) = store.get_mut(original) else {
            panic!("array expected");
        };
        values[0] = OwnedValue::Int(9);
        let StoredValue::Array(values) = store.get(cloned) else {
            panic!("array expected");
        };
        assert_eq!(values, &[OwnedValue::Int(1)]);
    }

    #[test]
    fn owned_storeのdropは子locationも解放する() {
        let mut store = Store::default();
        let child = store.alloc(StoredValue::Array(vec![OwnedValue::Int(1)]));
        let parent = store.alloc(StoredValue::Array(vec![child]));
        store.drop_value(parent);
        assert!(store.locations.iter().all(Option::is_none));
    }

    /// 走らせた結果の綴り。値が名前を持たなくなったので、表示は
    /// プログラムを知っている側から引く
    fn shown(src: &str, entry: &str) -> String {
        let program = parse::parse(&join(lex(src).unwrap())).expect("パースできるはず");
        let hir = crate::typecheck::check_and_lower(&program, &[])
            .unwrap_or_else(|d| panic!("型検査を通るはず: {d:?}"));
        let checked = crate::ownership::check(hir).expect("所有権検査を通るはず");
        let interp = Interp::new_checked(&checked);
        let value = interp.run(entry).expect("走るはず");
        interp.show(&value)
    }

    /// `clone()` は独立した実体を作る。片方を変えても他方は動かない
    /// (design.md 決定4、tasks 6.5)
    #[test]
    fn cloneした値は元と独立している() {
        let src = "struct Inner { n: int }
struct Outer { inner: Inner, xs: [Inner] }
fn main(-> int) {
  let mut a = Outer { inner = Inner { n = 1 }, xs = [Inner { n = 2 }] }
  let b = a.clone()
  a.inner.n = 9
  b.inner.n
}
";
        assert!(matches!(run(src, "main"), Ok(Value::Int(1))));
    }

    /// 深い複製は構造的に等しい値を作る
    #[test]
    fn cloneした値は元と等しい() {
        let src = "struct Inner { n: int }
struct Outer { inner: Inner, xs: [Inner] }
fn main(-> bool) {
  let a = Outer { inner = Inner { n = 1 }, xs = [Inner { n = 2 }] }
  a.clone() == a
}
";
        assert!(matches!(run(src, "main"), Ok(Value::Bool(true))));
    }

    /// enum の payload も辿る
    #[test]
    fn cloneはenumのpayloadも辿る() {
        let src = "struct Inner { n: int }
enum Box { Full(Inner) Empty }
fn main(-> int) {
  let mut a = Box::Full(Inner { n = 1 })
  let b = a.clone()
  match &mut a { Box::Full(i): i.n = 9
    Box::Empty: assert true }
  match move b { Box::Full(i): i.n
    Box::Empty: 0 }
}
";
        assert!(matches!(run(src, "main"), Ok(Value::Int(1))));
    }

    /// 実行時に失敗していた形が、いまは実行前に止まること。
    ///
    /// 評価器へ届くのは型検査を通った HIR だけなので、こういう題材は
    /// もう走らない(design.md 決定7)。文言は型検査側の綴り
    fn rejected(src: &str) -> String {
        let program = parse::parse(&join(lex(src).unwrap())).expect("パースできるはず");
        let errors =
            crate::typecheck::check_and_lower(&program, &[]).expect_err("実行前に止まるはず");
        errors
            .into_iter()
            .map(|d| d.msg)
            .collect::<Vec<_>>()
            .join(" / ")
    }

    /// 失敗した式のソース上の位置。
    fn failing_source(src: &str, entry: &str) -> String {
        let span = run(src, entry).unwrap_err().span.expect("span を持つはず");
        src[span.start as usize..span.end as usize].to_string()
    }

    fn int(src: &str) -> i64 {
        match run(src, "main").expect("評価が通るはず") {
            Value::Int(n) => n,
            other => panic!("整数ではない: {other:?}"),
        }
    }

    #[test]
    fn ブロックの値は最後の式() {
        assert_eq!(int("fn main(-> int) {\n 1\n 2\n 3\n}\n"), 3);
    }

    #[test]
    fn 算術と優先順位() {
        assert_eq!(int("fn main(-> int) {\n 1 + 2 * 3\n}\n"), 7);
        assert_eq!(int("fn main(-> int) {\n -4 / 2\n}\n"), -2);
    }

    #[test]
    fn ゼロ除算はエラーになる() {
        assert!(run("fn main(-> int) {\n 1 / 0\n}\n", "main").is_err());
    }

    /// int は符号付き64bit。加減乗と単項マイナスは 2^64 で回り込む。
    /// ホスト側のオーバーフロー検査の有無で振る舞いが変わってはいけない
    #[test]
    fn 加減乗と符号反転は境界で回り込む() {
        assert_eq!(
            int("fn main(-> int) {\n 9223372036854775807 + 1\n}\n"),
            i64::MIN
        );
        assert_eq!(
            int("fn main(-> int) {\n (-9223372036854775807 - 1) - 1\n}\n"),
            i64::MAX
        );
        assert_eq!(int("fn main(-> int) {\n 9223372036854775807 * 2\n}\n"), -2);
        assert_eq!(
            int("fn main(-> int) {\n -(-9223372036854775807 - 1)\n}\n"),
            i64::MIN
        );
    }

    /// 割り算は 0 方向へ切り捨てる。`-7 / 2` は -4 ではなく -3
    #[test]
    fn 割り算はゼロ方向へ切り捨てる() {
        assert_eq!(int("fn main(-> int) {\n -7 / 2\n}\n"), -3);
        assert_eq!(int("fn main(-> int) {\n 7 / -2\n}\n"), -3);
    }

    /// 最小値 / -1 は int に収まらない。ラップさせずに実行時失敗にする
    #[test]
    fn 最小値をマイナス1で割ると実行時失敗() {
        assert!(
            run(
                "fn main(-> int) {\n (-9223372036854775807 - 1) / -1\n}\n",
                "main"
            )
            .is_err()
        );
    }

    /// 直接失敗した式そのものを指す。
    #[test]
    fn 実行時エラーは失敗した式を指す() {
        assert_eq!(
            failing_source("fn main() {\n assert false\n}\n", "main"),
            "assert false"
        );
    }

    /// 内側が先に位置を埋めるので、囲む式(`+` や `if`)には上書きされない。
    #[test]
    fn 入れ子の失敗は外側ではなく内側の式を指す() {
        assert_eq!(
            failing_source("fn main(-> int) {\n 1 + (2 / 0)\n}\n", "main"),
            "2 / 0"
        );
    }

    /// 呼び出しは境界を越えても位置を保つ。指すのは呼び出し元ではなく callee の式。
    #[test]
    fn 呼び出し先の失敗は呼び出し元ではなくcalleeの式を指す() {
        let src = "fn boom() {\n assert false\n}\n\
                   fn main() {\n boom()\n}\n";
        assert_eq!(failing_source(src, "main"), "assert false");
    }

    /// 式を1つも評価する前の失敗は位置を持てない。文言だけは残る。
    #[test]
    fn 式の前で失敗するエントリ名の診断は位置を持たない() {
        let diag = run("fn main(-> int) { 1 }\n", "nope").expect_err("未知のエントリは失敗する");
        assert_eq!(diag.span, None);
        assert!(diag.msg.contains("関数 `nope` がありません"), "{diag}");
    }

    #[test]
    fn letと参照() {
        assert_eq!(int("fn main(-> int) {\n let x = 40\n x + 2\n}\n"), 42);
    }

    #[test]
    fn 束縛されていない名前はエラー() {
        assert!(rejected("fn main() {\n nope\n}\n").contains("`nope` は値として読めません"));
    }

    #[test]
    fn 呼び出しでenvは切れる() {
        // callee は呼び出し元の `x` を見られない。名前が解決できないので
        // 下ろす前に止まる
        let src = "fn callee(-> int) {\n x\n}\n\
                   fn main(-> int) {\n let x = 1\n callee()\n}\n";
        assert!(rejected(src).contains("`x` は値として読めません"));
    }

    #[test]
    fn returnは関数境界で止まる() {
        let src = "fn f(-> int) {\n return 7\n 999\n}\n\
                   fn main(-> int) {\n f() + 1\n}\n";
        assert_eq!(int(src), 8);
    }

    #[test]
    fn structのフィールドを読み書きする() {
        let src = "struct User { rank: int }\n\
                   fn main(-> int) {\n\
                   \x20 let mut u = User { rank = 1 }\n\
                   \x20 u.rank = 2\n\
                   \x20 u.rank\n\
                   }\n";
        assert_eq!(int(src), 2);
    }

    #[test]
    fn optional_field_accessは値を読みnilを伝播する() {
        let present = "struct User { id: int }\n\
                       fn find(-> User?) { User { id = 7 } }\n\
                       fn main(-> int) { find().?id ?? 0 }\n";
        assert_eq!(int(present), 7);

        let absent = "struct User { id: int }\n\
                      fn find(-> User?) { nil }\n\
                      fn main(-> int?) { find().?id }\n";
        assert!(matches!(run(absent, "main"), Ok(Value::Nil)));
    }

    #[test]
    fn optional_field_accessの連鎖は途中のnilを伝播する() {
        let present = "struct Inner { n: int }\n\
                       struct Outer { inner: Inner? }\n\
                       fn find(-> Outer?) { Outer { inner = Inner { n = 9 } } }\n\
                       fn main(-> int) { find().?inner.?n ?? 0 }\n";
        assert_eq!(int(present), 9);

        let absent = "struct Inner { n: int }\n\
                      struct Outer { inner: Inner? }\n\
                      fn find(-> Outer?) { Outer { inner = nil } }\n\
                      fn main(-> int?) { find().?inner.?n }\n";
        assert!(matches!(run(absent, "main"), Ok(Value::Nil)));
    }

    #[test]
    fn optional_field_accessはレシーバを一度だけ評価する() {
        let src = "struct Counter { n: int }\n\
                   struct Box { n: int }\n\
                   fn next(c: &mut Counter -> Box?) {\n\
                   \x20 c.n = c.n + 1\n\
                   \x20 Box { n = c.n }\n\
                   }\n\
                   fn main(-> int) {\n\
                   \x20 let mut c = Counter { n = 0 }\n\
                   \x20 let n = next(&mut c).?n\n\
                   \x20 c.n * 10 + (n ?? 0)\n\
                   }\n";
        assert_eq!(int(src), 11);
    }

    /// **差し替えが成立する条件。**呼び出し先での変更が呼び出し元から見えること
    #[test]
    fn 呼び出し先でのフィールド変更が呼び出し元に見える() {
        let src = "struct User { rank: int }\n\
                   fn stamp(u: &mut User) {\n\
                   \x20 u.rank = 99\n\
                   }\n\
                   fn main(-> int) {\n\
                   \x20 let mut u = User { rank = 1 }\n\
                   \x20 stamp(&mut u)\n\
                   \x20 u.rank\n\
                   }\n";
        assert_eq!(int(src), 99);
    }

    #[test]
    fn フィールド0個のstructは名前だけで値になる() {
        // enum が入っても、フィールド0個の struct はそのまま名前で値になる
        let src = "struct Gold {}\n\
                   fn main(-> bool) {\n\
                   \x20 assert Gold == Gold\n\
                   \x20 Gold == Gold\n\
                   }\n";
        assert!(matches!(run(src, "main"), Ok(Value::Bool(true))));

        // 別の struct との比較は同じ型ではないので実行前に止まる
        let e = rejected(
            "struct Gold {}\n\
             struct Silver {}\n\
             fn main(-> bool) { Gold == Silver }\n",
        );
        assert!(e.contains("`==` の両辺は同じ型"), "{e}");
    }

    // ---- enum ----

    const RANKS: &str = "enum Rank { Bronze Gold }\n\
                         enum Grade { Low High }\n";

    #[test]
    fn variantは裸の名前で値になる() {
        let src = format!("{RANKS}fn main(-> Rank) {{\n Gold\n}}\n");
        assert_eq!(shown(&src, "main"), "Rank.Gold");
    }

    #[test]
    fn 同じvariantは等しく別のvariantは等しくない() {
        let src = format!(
            "{RANKS}fn main(-> bool) {{\n\
             \x20 assert Gold == Gold\n\
             \x20 Gold == Bronze\n\
             }}\n"
        );
        assert!(matches!(run(&src, "main"), Ok(Value::Bool(false))));
    }

    /// variant 名が同じでも所属 enum が違えば別の値。`VariantId` は所属 enum
    /// ごと一意なので、値の比較規則としてここで固定する
    #[test]
    fn 別のenumの同名variantとは等しくない() {
        let a = Value::Enum {
            variant: hir::Id::from_index(0),
            payload: Vec::new(),
        };
        let b = Value::Enum {
            variant: hir::Id::from_index(1),
            payload: Vec::new(),
        };
        assert!(matches!(
            (&a, &b),
            (Value::Enum { variant: av, .. }, Value::Enum { variant: bv, .. }) if av != bv
        ));
        assert!(
            matches!(a.clone(), Value::Enum { variant, .. } if variant == hir::Id::from_index(0))
        );
    }

    #[test]
    fn ローカルはvariantを隠す() {
        let src = format!("{RANKS}fn main(-> int) {{\n let Gold = 7\n Gold\n}}\n");
        assert_eq!(int(&src), 7);
    }

    #[test]
    fn 限定したvariantは裸の参照と同じ値になる() {
        let src = format!(
            "{RANKS}fn main(-> Rank) {{\n\
             \x20 assert Rank::Gold == Gold\n\
             \x20 assert Rank::Gold == Rank::Gold\n\
             \x20 assert (Rank::Gold == Rank::Bronze) == false\n\
             \x20 Rank::Gold\n\
             }}\n"
        );
        assert_eq!(shown(&src, "main"), "Rank.Gold");
    }

    #[test]
    fn 限定したvariantはローカルに隠されない() {
        let src = format!(
            "{RANKS}fn main(-> bool) {{\n let Gold = Rank::Bronze\n Rank::Gold == Gold\n}}\n"
        );
        assert!(matches!(run(&src, "main"), Ok(Value::Bool(false))));
    }

    #[test]
    fn 宣言に無い限定variantと非enumの修飾は値にならない() {
        for path in ["Rank::Silver", "Grade::Gold", "Missing::Gold"] {
            let e = rejected(&format!("{RANKS}fn main() {{\n {path}\n}}\n"));
            assert!(
                e.contains("variant ではありません") || e.contains("値として読めません"),
                "{path}: {e}"
            );
        }
    }

    // ---- match ----

    #[test]
    fn matchは一致したarmの値を産む() {
        let src = format!(
            "{RANKS}fn label(r: Rank -> str) {{\n\
             \x20 match r {{\n\
             \x20   Rank::Bronze: \"bronze\"\n\
             \x20   Rank::Gold {{ \"gold\" }}\n\
             \x20 }}\n\
             }}\n\
             fn main(-> str) {{\n label(Gold)\n}}\n"
        );
        assert!(matches!(run(&src, "main"), Ok(Value::Str(s)) if s == "gold"));
    }

    #[test]
    fn matchは対象を一度だけ評価する() {
        let src = format!(
            "{RANKS}struct Counter {{ n: int }}\n\
             fn next(c: &mut Counter -> Rank) {{\n\
             \x20 c.n = c.n + 1\n\
             \x20 Gold\n\
             }}\n\
             fn main(-> int) {{\n\
             \x20 let mut c = Counter {{ n = 0 }}\n\
             \x20 let picked = match next(&mut c) {{ Rank::Bronze: 0\nRank::Gold: 1 }}\n\
             \x20 c.n * 10 + picked\n\
             }}\n"
        );
        assert_eq!(int(&src), 11);
    }

    #[test]
    fn 選ばれなかったarmは走らない() {
        let src = format!(
            "{RANKS}fn boom(-> int) {{ 1 / 0 }}\n\
             fn main(-> int) {{\n match Bronze {{ Rank::Bronze: 7\nRank::Gold: boom() }}\n}}\n"
        );
        assert_eq!(int(&src), 7);
    }

    #[test]
    fn armのreturnは関数を抜ける() {
        let src = format!(
            "{RANKS}fn pick(r: Rank -> int) {{\n\
             \x20 match r {{ Rank::Bronze: return 1\nRank::Gold: return 2 }}\n\
             \x20 999\n\
             }}\n\
             fn main(-> int) {{\n pick(Gold)\n}}\n"
        );
        assert_eq!(int(&src), 2);
    }

    #[test]
    fn 限定armが無いvariantはcatch_allへ落ちる() {
        let src = format!(
            "{RANKS}fn label(r: Rank -> str) {{\n\
             \x20 match r {{\n\
             \x20   Rank::Gold: \"gold\"\n\
             \x20   _ {{ \"other\" }}\n\
             \x20 }}\n\
             }}\n\
             fn main(-> str) {{\n label(Bronze)\n}}\n"
        );
        assert!(matches!(run(&src, "main"), Ok(Value::Str(s)) if s == "other"));
    }

    /// 限定 arm が先。`_` があっても一致する arm の本体だけが走る
    #[test]
    fn 限定armはcatch_allより優先する() {
        let src = format!(
            "{RANKS}fn boom(-> int) {{ 1 / 0 }}\n\
             fn main(-> int) {{\n match Gold {{ Rank::Gold: 7\n_: boom() }}\n}}\n"
        );
        assert_eq!(int(&src), 7);
    }

    #[test]
    fn catch_allはpayloadを束縛しない() {
        let src = format!(
            "{LOOKUP}fn main(-> int) {{\n\
             \x20 let reason = 1\n\
             \x20 match Lookup::Missing(\"gone\") {{\n\
             \x20   Lookup::Skipped: 0\n\
             \x20   _: reason\n\
             \x20 }}\n\
             }}\n"
        );
        // payload の名前は増えないので、外側の `reason` がそのまま見える
        assert_eq!(int(&src), 1);
    }

    // ---- arm の guard ----

    #[test]
    fn 真のguardは限定armの本体を選ぶ() {
        let src = format!(
            "{RANKS}fn main(-> str) {{\n\
             \x20 match Gold {{\n\
             \x20   Rank::Gold if 1 == 1: \"gold\"\n\
             \x20   _: \"other\"\n\
             \x20 }}\n\
             }}\n"
        );
        assert!(matches!(run(&src, "main"), Ok(Value::Str(s)) if s == "gold"));
    }

    /// 偽の guard は本体を走らせず `_` へ落ちる(design.md 決定6)
    #[test]
    fn 偽のguardはcatch_allへ落ちて本体を走らせない() {
        let src = format!(
            "{RANKS}fn boom(-> int) {{ 1 / 0 }}\n\
             fn main(-> int) {{\n\
             \x20 match Gold {{\n\
             \x20   Rank::Gold if 1 == 2: boom()\n\
             \x20   _: 7\n\
             \x20 }}\n\
             }}\n"
        );
        assert_eq!(int(&src), 7);
    }

    #[test]
    fn guardはpayloadを束縛した後に走る() {
        let src = format!(
            "{LOOKUP}fn main(-> int) {{\n\
             \x20 match Lookup::Found(User {{ id = 5 }}, 5) {{\n\
             \x20   Lookup::Found(u, n) if u.id == n: n\n\
             \x20   _: 0\n\
             \x20 }}\n\
             }}\n"
        );
        assert_eq!(int(&src), 5);
    }

    /// 対象は一度、guard は variant が一致したときだけ一度、本体はその後だけ
    #[test]
    fn guardは一致したarmで一度だけ走る() {
        let src = format!(
            "{RANKS}struct Counter {{ n: int }}\n\
             fn bump(c: &mut Counter -> bool) {{\n\
             \x20 c.n = c.n + 1\n\
             \x20 true\n\
             }}\n\
             fn main(-> int) {{\n\
             \x20 let mut c = Counter {{ n = 0 }}\n\
             \x20 let picked = match Gold {{\n\
             \x20   Rank::Bronze if bump(&mut c): 1\n\
             \x20   Rank::Gold if bump(&mut c): 2\n\
             \x20   _: 3\n\
             \x20 }}\n\
             \x20 c.n * 10 + picked\n\
             }}\n"
        );
        // 一致しない `Bronze` の guard は走らないので、増分は1回だけ
        assert_eq!(int(&src), 12);
    }

    /// 推論境界の外を通った guard の実行時の網。位置は arm 全体でなく guard 式
    #[test]
    fn 非boolのguardは実行時に失敗して原因の位置を指す() {
        let e = rejected(&format!(
            "{RANKS}fn unknown(-> int) {{ 1 }}\n\
             fn main(-> int) {{\n\
             \x20 match Gold {{\n\
             \x20   Rank::Gold if unknown(): 1\n\
             \x20   _: 0\n\
             \x20 }}\n\
             }}\n"
        ));
        assert!(e.contains("arm の guard"), "{e}");
    }

    /// 静的検査は偽の guard に `_` を強制するが、評価器は網を残す
    #[test]
    fn 偽のguardでcatch_allが無ければ実行時エラー() {
        // guard は偽になりうるので、guard 付きの arm だけでは網羅にならない。
        // 落ちる先の無い `match` は実行前に止まる
        let e = rejected(&format!(
            "{RANKS}fn main(-> int) {{\n match Gold {{ Rank::Bronze: 0\nRank::Gold if 1 == 2: 1 }}\n}}\n"
        ));
        assert!(e.contains("variant `Gold` を扱っていません"), "{e}");
    }

    /// 静的検査を通さず評価器を直接使う経路の防御。型検査を通れば起きない
    #[test]
    fn 対象がenumでないかarmが無ければ実行時エラー() {
        // 対象が enum でないことも、arm が網羅していないことも実行前に分かる
        let e = rejected(&format!(
            "{RANKS}fn main(-> int) {{\n match 1 {{ Rank::Gold: 1 }}\n}}\n"
        ));
        assert!(
            e.contains("非 optional な enum である必要があります"),
            "{e}"
        );

        let e = rejected(&format!(
            "{RANKS}fn main(-> int) {{\n match Bronze {{ Rank::Gold: 1 }}\n}}\n"
        ));
        assert!(e.contains("variant `Bronze` を扱っていません"), "{e}");
    }

    // ---- payload を持つ variant ----

    const LOOKUP: &str = "struct User { id: int }\n\
                          enum Lookup { Found(User, int) Missing(str) Skipped }\n";

    #[test]
    fn payloadは限定pathの呼び出しで構築され表示に出る() {
        let src = format!(
            "{LOOKUP}fn main(-> Lookup) {{\n\
             \x20 let u = User {{ id = 1 }}\n\
             \x20 Lookup::Found(u, 2)\n\
             }}\n"
        );
        assert_eq!(shown(&src, "main"), "Lookup.Found(User, 2)");
        // fieldless の表示は従来のまま
        let src = format!("{LOOKUP}fn main(-> Lookup) {{\n Skipped\n}}\n");
        assert_eq!(shown(&src, "main"), "Lookup.Skipped");
    }

    #[test]
    fn payloadの引数は左から一度ずつ評価される() {
        let src = "enum Pair { Two(int, int) }\n\
                   struct Counter { n: int }\n\
                   fn bump(c: &mut Counter -> int) {\n c.n = c.n + 1\n c.n\n}\n\
                   fn main(-> int) {\n\
                   \x20 let mut c = Counter { n = 0 }\n\
                   \x20 let first = bump(&mut c)\n\
                   \x20 let second = bump(&mut c)\n\
                   \x20 let p = Pair::Two(first, second)\n\
                   \x20 match p { Pair::Two(a, b): a * 100 + b * 10 + c.n }\n\
                   }\n";
        // 左が先に1回、右が次に1回。呼び出しは合計2回
        assert_eq!(int(src), 122);
    }

    #[test]
    fn payloadの等値は対応ごとの構造的同値() {
        let src = format!(
            "{LOOKUP}fn main(-> bool) {{\n\
             \x20 assert Lookup::Missing(\"a\") == Lookup::Missing(\"a\")\n\
             \x20 assert (Lookup::Missing(\"a\") == Lookup::Missing(\"b\")) == false\n\
             \x20 assert (Lookup::Missing(\"a\") == Lookup::Skipped) == false\n\
             \x20 let u = User {{ id = 1 }}\n\
             \x20 let mut v = User {{ id = 1 }}\n\
             \x20 assert Lookup::Found(u.clone(), 2) == Lookup::Found(v.clone(), 2)\n\
             \x20 assert (Lookup::Found(u.clone(), 2) == Lookup::Found(v.clone(), 3)) == false\n\
             \x20 v.id = 9\n\
             \x20 Lookup::Found(u, 2) == Lookup::Found(v, 2)\n\
             }}\n"
        );
        assert!(matches!(run(&src, "main"), Ok(Value::Bool(false))));
    }

    /// payload への値渡しは clone で明示し、元の変更は payload へ漏れない。
    #[test]
    fn cloneした複合payloadは元の変更を共有しない() {
        let src = format!(
            "{LOOKUP}fn main(-> int) {{\n\
             \x20 let mut u = User {{ id = 1 }}\n\
             \x20 let l = Lookup::Found(u.clone(), 0)\n\
             \x20 u.id = 7\n\
             \x20 match move l {{\n\
             \x20   Lookup::Found(found, _): found.id\n\
             \x20   Lookup::Missing(_): 0\n\
             \x20   Lookup::Skipped: 0\n\
             \x20 }}\n\
             }}\n"
        );
        assert_eq!(int(&src), 1);
    }

    #[test]
    fn armは宣言順のpayloadを名前へ束縛する() {
        let src = format!(
            "{LOOKUP}fn main(-> int) {{\n\
             \x20 let u = User {{ id = 3 }}\n\
             \x20 match Lookup::Found(u, 5) {{\n\
             \x20   Lookup::Found(found, n): found.id * 10 + n\n\
             \x20   Lookup::Missing(_): 0\n\
             \x20   Lookup::Skipped: 0\n\
             \x20 }}\n\
             }}\n"
        );
        assert_eq!(int(&src), 35);
    }

    #[test]
    fn discardした位置は名前にならない() {
        let src = format!(
            "{LOOKUP}fn main(-> int) {{\n\
             \x20 let _ = 9\n\
             \x20 match Lookup::Missing(\"gone\") {{\n\
             \x20   Lookup::Found(_, _): 0\n\
             \x20   Lookup::Missing(_): _\n\
             \x20   Lookup::Skipped: 0\n\
             \x20 }}\n\
             }}\n"
        );
        // `_` は arm が束縛しないので、外側の `let _` がそのまま見える
        assert_eq!(int(&src), 9);
    }

    #[test]
    fn payload束縛はarmの外へ漏れない() {
        let e = rejected(&format!(
            "{LOOKUP}fn main(-> str) {{\n\
             \x20 let ignored = match Lookup::Missing(\"gone\") {{\n\
             \x20   Lookup::Found(_, _): 0\n\
             \x20   Lookup::Missing(reason): 0\n\
             \x20   Lookup::Skipped: 0\n\
             \x20 }}\n\
             \x20 reason\n\
             }}\n"
        ));
        assert!(e.contains("`reason` は値として読めません"), "{e}");
    }

    #[test]
    fn payload束縛はarmの間だけ外側を隠す() {
        let src = format!(
            "{LOOKUP}fn main(-> int) {{\n\
             \x20 let reason = 1\n\
             \x20 let inner = match Lookup::Missing(\"gone\") {{\n\
             \x20   Lookup::Found(_, _): \"f\"\n\
             \x20   Lookup::Missing(reason): reason.clone()\n\
             \x20   Lookup::Skipped: \"s\"\n\
             \x20 }}\n\
             \x20 assert inner == \"gone\"\n\
             \x20 reason\n\
             }}\n"
        );
        assert_eq!(int(&src), 1);
    }

    /// 静的検査を通さず評価器を直接使う経路の防御。型検査を通れば起きない
    #[test]
    fn payloadの個数が合わなければ実行時エラー() {
        let e = rejected(&format!(
            "{LOOKUP}fn main(-> Lookup) {{\n let u = User {{ id = 1 }}\n Lookup::Found(u)\n}}\n"
        ));
        assert!(e.contains("引数を 2 個取りますが、1 個渡しています"), "{e}");

        let e = rejected(&format!(
            "{LOOKUP}fn main(-> int) {{\n\
             \x20 match Lookup::Missing(\"x\") {{ Lookup::Missing(a, b): 1 }}\n\
             }}\n"
        ));
        assert!(e.contains("payload を 2 個束縛します"), "{e}");
    }

    #[test]
    fn payload_variantは構築しないと値にならない() {
        for expr in ["Lookup::Found", "Found"] {
            let e = rejected(&format!("{LOOKUP}fn main() {{\n {expr}\n}}\n"));
            assert!(e.contains("payload"), "{expr}: {e}");
        }
    }

    #[test]
    fn enumはstructではないのでフィールドを読めない() {
        let e = rejected(&format!("{RANKS}fn main() {{\n Gold.name\n}}\n"));
        assert!(e.contains("struct ではないので `name` を読めません"), "{e}");
    }

    #[test]
    fn assertが偽ならエラー() {
        assert!(run("fn main() {\n assert 1 == 2\n}\n", "main").is_err());
    }

    #[test]
    fn ifは値を産む() {
        let src = "fn main(-> int) {\n\
                   \x20 if 1 == 1: 10\n\
                   \x20 else: 20\n\
                   }\n";
        assert_eq!(int(src), 10);
    }

    #[test]
    fn elseの側も選ばれる() {
        let src = "fn main(-> int) {\n\
                   \x20 if 1 == 2: 10\n\
                   \x20 else: 20\n\
                   }\n";
        assert_eq!(int(src), 20);
    }

    #[test]
    fn whileが回る() {
        let src = "fn main(-> int) {\n\
                   \x20 let mut n = 0\n\
                   \x20 while n == 0: n = 1\n\
                   \x20 n\n\
                   }\n";
        assert_eq!(int(src), 1);
    }

    /// `??` の右辺は左辺が nil のときだけ走る。
    /// 右辺に `return` が置ける(`db.find(id) ?? return false`)ので短絡は必須
    #[test]
    fn 合体演算子は短絡する() {
        let src = "fn main(-> int) {\n\
                   \x20 let n: int? = 1\n\
                   \x20 n ?? return 999\n\
                   }\n";
        assert_eq!(int(src), 1);
    }

    // ---- 段2: impl と配列 ----

    #[test]
    fn パス呼び出しでimplの関数を呼ぶ() {
        let src = "struct Frozen { t: int }\n\
                   impl Frozen {\n\
                   \x20 fn at(t: int -> Frozen) {\n\
                   \x20   Frozen { t = t }\n\
                   \x20 }\n\
                   }\n\
                   fn main(-> int) {\n\
                   \x20 Frozen::at(1000).t\n\
                   }\n";
        assert_eq!(int(src), 1000);
    }

    #[test]
    fn cloneした配列は元と独立している() {
        // `clone()` は配列とその要素を独立した store location に複製する。
        let src = "struct Cell { n: int }\n\
                   fn main(-> bool) {\n\
                   \x20 let mut a = [Cell { n = 1 }, Cell { n = 2 }]\n\
                   \x20 let b = a.clone()\n\
                   \x20 for item in &mut a: item.n = 9\n\
                   \x20 a == b\n\
                   }\n";
        assert!(matches!(run(src, "main"), Ok(Value::Bool(false))));
    }

    #[test]
    fn forで配列を回す() {
        let src = "fn main(-> int) {\n\
                   \x20 let mut total = 0\n\
                   \x20 for x in [1, 2, 3]: total = total + x\n\
                   \x20 total\n\
                   }\n";
        assert_eq!(int(src), 6);
    }

    #[test]
    fn 配列の等値は中身で決まる() {
        let src = "fn main(-> bool) {\n\
                   \x20 [1, 2] == [1, 2]\n\
                   }\n";
        assert!(matches!(run(src, "main"), Ok(Value::Bool(true))));
    }

    /// trait を候補側に持つ効果。ただの変数では絞れないので曖昧エラーになる
    #[test]
    fn 同名メソッドが二つのtraitにあると曖昧() {
        let src = "trait A { fn get(-> int) }\n\
                   trait B { fn get(-> int) }\n\
                   struct X {}\n\
                   impl A for X {\n\
                   \x20 fn get(-> int) {\n\
                   \x20   1\n\
                   \x20 }\n\
                   }\n\
                   impl B for X {\n\
                   \x20 fn get(-> int) {\n\
                   \x20   2\n\
                   \x20 }\n\
                   }\n\
                   fn main() {\n\
                   \x20 X::get()\n\
                   }\n";
        let e = rejected(src);
        assert!(e.contains("どの trait"), "{e}");
    }

    #[test]
    fn 無い型のメソッドはエラー() {
        assert!(rejected("fn main() {\n Nope::go()\n}\n").contains("呼び出し先が決まりません"));
    }

    // ---- 段2b: self を取るメソッド ----

    /// ハンドラの本体が自分の保存先に手が届くこと。これが無いと差し替えが書けない
    #[test]
    fn メソッドはselfでレシーバに触れる() {
        let src = "trait Clock { fn now(&self -> int) }\n\
                   struct Frozen { t: int }\n\
                   impl Clock for Frozen {\n\
                   \x20 fn now(&self -> int) {\n\
                   \x20   self.t\n\
                   \x20 }\n\
                   }\n\
                   fn main(-> int) {\n\
                   \x20 let c = Frozen { t = 1000 }\n\
                   \x20 c.now()\n\
                   }\n";
        assert_eq!(int(src), 1000);
    }

    /// `InMemoryDb` の骨。配列を持って足せること
    #[test]
    fn メソッドがselfの配列を変更できる() {
        let src = "trait Db {\n\
                   \x20 fn save(&mut self, x: int -> unit)\n\
                   \x20 fn count(&self -> int)\n\
                   }\n\
                   struct Store { xs: [int] }\n\
                   impl Store {\n\
                   \x20 fn new(-> Store) {\n\
                   \x20   let empty: [int] = []\n\
                   \x20   Store { xs = empty }\n\
                   \x20 }\n\
                   }\n\
                   impl Db for Store {\n\
                   \x20 fn save(&mut self, x: int -> unit) {\n\
                   \x20   self.xs = [x]\n\
                   \x20 }\n\
                   \x20 fn count(&self -> int) {\n\
                   \x20   let mut n = 0\n\
                   \x20   for y in self.xs: n = n + 1\n\
                   \x20   n\n\
                   \x20 }\n\
                   }\n\
                   fn main(-> int) {\n\
                   \x20 let mut s = Store::new()\n\
                   \x20 &mut s.save(7)\n\
                   \x20 s.count()\n\
                   }\n";
        assert_eq!(int(src), 1);
    }

    #[test]
    fn selfを取らないメソッドはドットで呼べない() {
        let src = "struct S {}\n\
                   impl S {\n\
                   \x20 fn go(-> int) {\n\
                   \x20   1\n\
                   \x20 }\n\
                   }\n\
                   fn main() {\n\
                   \x20 let s = S {}\n\
                   \x20 s.go()\n\
                   }\n";
        let e = rejected(src);
        assert!(e.contains("self を取りません"), "{e}");
    }

    #[test]
    fn selfを取るメソッドはパスで呼べない() {
        let src = "struct S { n: int }\n\
                   impl S {\n\
                   \x20 fn get(self -> int) {\n\
                   \x20   self.n\n\
                   \x20 }\n\
                   }\n\
                   fn main() {\n\
                   \x20 S::get()\n\
                   }\n";
        let e = rejected(src);
        assert!(e.contains("レシーバ"), "{e}");
    }

    /// `self` は ambient と違って普通の束縛。呼び出しで切れる
    #[test]
    fn selfは呼び出し先に届かない() {
        let src = "struct S { n: int }\n\
                   fn helper(-> int) {\n\
                   \x20 self.n\n\
                   }\n\
                   impl S {\n\
                   \x20 fn get(self -> int) {\n\
                   \x20   helper()\n\
                   \x20 }\n\
                   }\n\
                   fn main(-> int) {\n\
                   \x20 let s = S { n = 1 }\n\
                   \x20 s.get()\n\
                   }\n";
        assert!(rejected(src).contains("`self` は値として読めません"));
    }

    // ---- 段4: 正典の完走 ----

    /// **v1 の到達目標。**`examples/canonical.rd` の test が緑になること。
    ///
    /// 中で起きていること: `handle` → `promote` → `stamp` と3段潜って
    /// `clock.now()` と `db.save(u)` に届く。間の2つは1文字も書いていない。
    /// 提供を `Frozen`/`InMemoryDb` に差し替えても呼ばれる側は無変更。
    #[test]
    fn 正典のテストが通る() {
        let src = std::fs::read_to_string("examples/canonical.rd").unwrap();
        let program = parse::parse(&join(lex(&src).unwrap())).expect("パースできるはず");
        let hir = crate::typecheck::check_and_lower(&program, &[]).expect("型検査を通る");
        let checked = crate::ownership::check(hir).expect("所有権検査を通る");
        let interp = Interp::new_checked(&checked);

        assert_eq!(checked.hir.tests.len(), 1, "正典の test は1つ");
        for (id, declared) in checked.hir.tests.iter() {
            if let Err(e) = interp.run_test(id) {
                panic!("test {:?} が失敗: {e}", declared.name);
            }
        }
    }

    /// 本番側の経路も走ること。Postgres は空の DB なので false が返る
    #[test]
    fn 正典のmainが走る() {
        let src = std::fs::read_to_string("examples/canonical.rd").unwrap();
        assert!(matches!(run(&src, "main"), Ok(Value::Bool(false))));
    }

    #[test]
    fn nilは合体演算子の左辺で短絡する() {
        let src = "fn main(-> bool) {\n\
                   \x20 let u = nil ?? return false\n\
                   \x20 true\n\
                   }\n";
        assert!(matches!(run(src, "main"), Ok(Value::Bool(false))));
    }

    // ---- 段3: ambient ----

    /// 土台。`Frozen` を `clock` に提供して `clock.now()` が届く
    const CLOCK: &str = "trait Clock { fn now(self -> int) }\n\
                         effect clock: Clock\n\
                         struct Frozen { t: int }\n\
                         impl Frozen {\n\
                         \x20 fn at(t: int -> Frozen) {\n\
                         \x20   Frozen { t = t }\n\
                         \x20 }\n\
                         }\n\
                         impl Clock for Frozen {\n\
                         \x20 fn now(self -> int) {\n\
                         \x20   self.t\n\
                         \x20 }\n\
                         }\n";

    #[test]
    fn 提供したハンドラがスロット経由で呼ばれる() {
        let src = format!(
            "{CLOCK}\
             fn main(-> int) {{\n\
             \x20 with clock(Frozen::at(1000)) {{\n\
             \x20   clock.now()\n\
             \x20 }}\n\
             }}\n"
        );
        assert_eq!(int(&src), 1000);
    }

    /// **この言語の一点突破。**
    /// `promote` は1文字も書いていないのに `stamp` の要求が `main` から届く
    #[test]
    fn ambientは関数呼び出しで切れない() {
        let src = format!(
            "{CLOCK}\
             fn stamp(-> int) {{\n\
             \x20 clock.now()\n\
             }}\n\
             fn promote(-> int) {{\n\
             \x20 stamp()\n\
             }}\n\
             fn handle(-> int) {{\n\
             \x20 promote()\n\
             }}\n\
             fn main(-> int) {{\n\
             \x20 with clock(Frozen::at(1000)) {{\n\
             \x20   handle()\n\
             \x20 }}\n\
             }}\n"
        );
        assert_eq!(int(&src), 1000);
    }

    /// 対になる確認。`let` はどれだけ近くても呼び出し先に届かない
    #[test]
    fn 通常の束縛は呼び出しで切れる() {
        let src = "fn callee(-> int) {\n\
                   \x20 c\n\
                   }\n\
                   fn main(-> int) {\n\
                   \x20 let c = 1\n\
                   \x20 callee()\n\
                   }\n";
        assert!(rejected(src).contains("`c` は値として読めません"));
    }

    /// 差し替え。**呼ばれる側を一切変更しない**
    #[test]
    fn 同じ関数が提供を変えると別の答えを返す() {
        let src = format!(
            "{CLOCK}\
             fn stamp(-> int) {{\n\
             \x20 clock.now()\n\
             }}\n\
             fn main(-> int) {{\n\
             \x20 let a = with clock(Frozen::at(1)) {{ stamp() }}\n\
             \x20 let b = with clock(Frozen::at(2)) {{ stamp() }}\n\
             \x20 b - a\n\
             }}\n"
        );
        assert_eq!(int(&src), 1);
    }

    #[test]
    fn 提供はブロックの外へ出ない() {
        let src = format!(
            "{CLOCK}\
             fn main(-> int) {{\n\
             \x20 with clock(Frozen::at(1000)) {{ clock.now() }}\n\
             \x20 clock.now()\n\
             }}\n"
        );
        let e = run(&src, "main").expect_err("外では提供されていない");
        assert!(e.msg.contains("提供されていません"), "{e}");
    }

    #[test]
    fn 入れ子は内側が勝つ() {
        let src = format!(
            "{CLOCK}\
             fn main(-> int) {{\n\
             \x20 with clock(Frozen::at(1)) {{\n\
             \x20   with clock(Frozen::at(2)) {{\n\
             \x20     clock.now()\n\
             \x20   }}\n\
             \x20 }}\n\
             }}\n"
        );
        assert_eq!(int(&src), 2);
    }

    /// 提供する値は提供の**外**で評価される(requirement.rs 手順4と同じ規則)
    #[test]
    fn 提供する値は提供の外で評価される() {
        let src = format!(
            "{CLOCK}\
             fn main(-> int) {{\n\
             \x20 with clock(Frozen::at(clock.now())) {{ 1 }}\n\
             }}\n"
        );
        let e = run(&src, "main").expect_err("clock はまだ立っていない");
        assert!(e.msg.contains("提供されていません"), "{e}");
    }

    /// トレイトを実装していない値はスロットに入らない
    #[test]
    fn 実装していない値は提供できない() {
        let src = "trait Clock { fn now(self -> int) }\n\
                   effect clock: Clock\n\
                   struct Nope {}\n\
                   fn main(-> int) {\n\
                   \x20 with clock(Nope {}) { 1 }\n\
                   }\n";
        assert!(rejected(src).contains("実装していない"));
    }

    /// 同じ型が2つの trait に同名メソッドを持っていても、スロット経由なら決まる
    #[test]
    fn スロット経由なら同名メソッドでも曖昧にならない() {
        let src = "trait Clock { fn now(self -> int) }\n\
                   trait Other { fn now(self -> int) }\n\
                   effect clock: Clock\n\
                   struct Both {}\n\
                   impl Clock for Both {\n\
                   \x20 fn now(self -> int) {\n\
                   \x20   1\n\
                   \x20 }\n\
                   }\n\
                   impl Other for Both {\n\
                   \x20 fn now(self -> int) {\n\
                   \x20   2\n\
                   \x20 }\n\
                   }\n\
                   fn main(-> int) {\n\
                   \x20 with clock(Both {}) { clock.now() }\n\
                   }\n";
        // ただの変数なら「どの trait か決まらない」になる場面。
        // スロットには trait 名が付いているので Clock 側が選ばれる
        assert_eq!(int(src), 1);
    }

    #[test]
    fn 型提供から内側で実体を初期化できる() {
        let src = "trait Database {\n\
                   \x20 fn new(-> Postgres)\n\
                   \x20 fn value(&self -> int)\n\
                   }\n\
                   effect db: Database\n\
                   struct Postgres {}\n\
                   impl Database for Postgres {\n\
                   \x20 fn new(-> Postgres) { Postgres {} }\n\
                   \x20 fn value(&self -> int) { 7 }\n\
                   }\n\
                   fn main(-> int) {\n\
                   \x20 with db<Postgres> {\n\
                   \x20   let db = db::new()\n\
                   \x20   with db(db) { db.value() }\n\
                   \x20 }\n\
                   }\n";
        assert_eq!(shown(src, "main"), "7");
    }

    #[test]
    fn 型だけの提供では値射影を使えない() {
        let src = "trait Database { fn value(self -> int) }\n\
                   effect db: Database\n\
                   struct Postgres {}\n\
                   impl Database for Postgres {\n\
                   \x20 fn value(self -> int) { 7 }\n\
                   }\n\
                   fn main(-> int) {\n\
                   \x20 with db<Postgres> { db.value() }\n\
                   }\n";
        let error = run(src, "main").unwrap_err();
        assert!(error.msg.contains("型だけ"), "{error}");
    }

    #[test]
    fn 型射影もスロットのtraitで関連関数を絞る() {
        let src = "trait Database { fn new(-> Both) }\n\
                   trait Other { fn new(-> Both) }\n\
                   effect db: Database\n\
                   struct Both { n: int }\n\
                   impl Database for Both {\n\
                   \x20 fn new(-> Both) { Both { n = 1 } }\n\
                   }\n\
                   impl Other for Both {\n\
                   \x20 fn new(-> Both) { Both { n = 2 } }\n\
                   }\n\
                   fn main(-> int) {\n\
                   \x20 with db<Both> { db::new().n }\n\
                   }\n";
        assert_eq!(shown(src, "main"), "1");
    }

    #[test]
    fn letなしの代入ではスロットを隠せない() {
        let src = "trait Database { fn save(self -> int) }\n\
                   effect db: Database\n\
                   struct Store {}\n\
                   impl Database for Store { fn save(self -> int) { 1 } }\n\
                   fn main() { db = Store }\n";
        let e = rejected(src);
        assert!(e.contains("`db` はスロットなので代入できません"), "{e}");
    }

    /// `u.x = u.clone()` は有限な深い複製を格納する。構造比較が安全に終わること。
    #[test]
    fn cloneを格納したstructを比較しても落ちない() {
        let src = "struct Node { indirect x: Node? }\n\
                   fn main(-> bool) {\n\
                   \x20 let mut u = Node { x = nil }\n\
                   \x20 u.x = u.clone()\n\
                   \x20 u == u\n\
                   }\n";
        // 自己比較なので、有限な構造比較の結果は真。
        assert!(matches!(run(src, "main"), Ok(Value::Bool(true))));
    }

    /// 相互に clone を格納しても、それぞれ有限な値になり比較が安全に終わる。
    #[test]
    fn cloneを格納した二つのstructの比較は安全に終わる() {
        let src = "struct Node { indirect x: Node? }\n\
                   fn main(-> bool) {\n\
                   \x20 let mut a = Node { x = nil }\n\
                   \x20 let mut b = Node { x = nil }\n\
                   \x20 a.x = b.clone()\n\
                   \x20 b.x = a.clone()\n\
                   \x20 a == b\n\
                   }\n";
        assert!(matches!(run(src, "main"), Ok(Value::Bool(_))));
    }

    #[test]
    fn 同名の引数はスロットを一貫して隠す() {
        let src = "trait Database { fn save(&mut self, u: int -> unit) }\n\
                   effect db: Database\n\
                   struct Slot { n: int }\n\
                   impl Database for Slot {\n\
                   \x20 fn save(&mut self, u: int -> unit) { self.n = 1 }\n\
                   }\n\
                   struct Local { n: int }\n\
                   impl Database for Local {\n\
                   \x20 fn save(&mut self, u: int -> unit) { self.n = 2 }\n\
                   }\n\
                   fn handle(db: &mut Local -> int) {\n\
                   \x20 &mut db.save(0)\n\
                   \x20 db.n\n\
                   }\n\
                   fn main(-> int) {\n\
                   \x20 let mut slot = Slot { n = 0 }\n\
                   \x20 with db(&mut slot) {\n\
                   \x20   let mut local = Local { n = 7 }\n\
                   \x20   let result = handle(&mut local)\n\
                   \x20   result * 10\n\
                   \x20 }\n\
                   }\n";
        assert_eq!(shown(src, "main"), "20");
    }

    #[test]
    fn withの右辺は外側で本体はスロットとして解決する() {
        let src = "trait Database { fn save(&mut self, u: int -> unit) }\n\
                   effect db: Database\n\
                   struct Store { n: int }\n\
                   impl Database for Store {\n\
                   \x20 fn save(&mut self, u: int -> unit) { self.n = u }\n\
                   }\n\
                   fn main(-> int) {\n\
                   \x20 let mut db = Store { n = 0 }\n\
                   \x20 with db(&mut db) { db.save(9) }\n\
                   \x20 db.n\n\
                   }\n";
        assert_eq!(shown(src, "main"), "9");
    }

    // ---- 汎用宣言の具体化を走らせる(MAP-060) ----
    //
    // 具体化は普通の `hir::Callable` で、`CheckedInterp::call` は
    // `CallableId` の出どころを見ない。ここはその契約を評価器の側から
    // 押さえる焦点テスト
    // (specs/generic-instantiation-execution)。

    /// 同じ generic 本体を2つの型引数で呼ぶと、具体化ごとに独立に走る
    #[test]
    fn 汎用関数は型引数ごとに独立して走る() {
        let src = "fn identity<T>(x: T -> T) { x }\n\
                   fn main(-> int) {\n\
                   \x20 let flag = identity(true)\n\
                   \x20 let n = identity(41)\n\
                   \x20 if flag: n + 1 else: 0\n\
                   }\n";
        assert_eq!(int(src), 42);
    }

    /// 同じ型引数でも、束縛した callback ごとに別の具体化が走る
    #[test]
    fn 汎用関数はcallback束縛ごとに自分のものを呼ぶ() {
        let src = "fn double(value: int -> int) { value * 2 }\n\
                   fn negate(value: int -> int) { 0 - value }\n\
                   fn apply<T, U>(f: fn(T -> U), x: T -> U) { f(x) }\n\
                   fn main(-> int) { apply(double, 22) + apply(negate, 2) }\n";
        assert_eq!(int(src), 42);
    }

    /// generic な `impl` のメソッドも、通常のメソッド呼び出しとして走る
    #[test]
    fn 汎用traitメソッドの具体化がメソッド呼び出しで走る() {
        let src = "trait Box<T> { fn wrap<U>(&self, value: T, f: fn(T -> U) -> U) }\n\
                   struct Container { tag: int }\n\
                   impl<T> Box<T> for Container {\n\
                   \x20 fn wrap<U>(&self, value: T, f: fn(T -> U) -> U) { f(value) }\n\
                   }\n\
                   fn double(value: int -> int) { value * 2 }\n\
                   fn flip(value: bool -> bool) { value == false }\n\
                   fn main(-> int) {\n\
                   \x20 let c = Container { tag = 1 }\n\
                   \x20 let flag = c.wrap(false, flip)\n\
                   \x20 let doubled = c.wrap(21, double)\n\
                   \x20 if flag: doubled else: 0\n\
                   }\n";
        assert_eq!(int(src), 42);
    }

    /// 非 Copy な値を消費する callback へ move しても走り切る。
    /// 走り終えた `Store` に取り残しが無いことは `dispose()` の
    /// `debug_assert!` が見ている
    #[test]
    fn 汎用関数越しに非copyな値をcallbackへmoveできる() {
        let src = "struct User { id: int }\n\
                   fn take(u: User -> int) { u.id }\n\
                   fn apply<T, U>(f: fn(T -> U), x: T -> U) { f(move x) }\n\
                   fn main(-> int) {\n\
                   \x20 let u = User { id = 42 }\n\
                   \x20 apply(take, move u)\n\
                   }\n";
        assert_eq!(int(src), 42);
    }

    /// 2つの具体化がそれぞれ自分の非 Copy な値を move して drop する
    #[test]
    fn 具体化ごとに自分の非copyな値をmoveする() {
        let src = "struct User { id: int }\n\
                   struct Tag { at: int }\n\
                   fn take_user(u: User -> int) { u.id }\n\
                   fn take_tag(t: Tag -> int) { t.at }\n\
                   fn apply<T, U>(f: fn(T -> U), x: T -> U) { f(move x) }\n\
                   fn main(-> int) {\n\
                   \x20 let a = User { id = 40 }\n\
                   \x20 let b = Tag { at = 2 }\n\
                   \x20 apply(take_user, move a) + apply(take_tag, move b)\n\
                   }\n";
        assert_eq!(int(src), 42);
    }

    /// ambient を要る callback と要らない callback が同じ generic 宣言に
    /// 同居する。`quiet` の側は `with` の外から呼べる
    fn generic_ambient_src() -> String {
        format!(
            "{CLOCK}\
             fn ticked(value: int -> int) {{ value + clock.now() }}\n\
             fn plain(value: int -> int) {{ value + 1 }}\n\
             fn apply<T, U>(f: fn(T -> U), x: T -> U) {{ f(x) }}\n\
             fn quiet(-> int) {{ apply(plain, 1) }}\n\
             fn main(-> int) {{\n\
             \x20 with clock(Frozen::at(40)) {{ apply(ticked, quiet()) }}\n\
             }}\n"
        )
    }

    /// generic な呼び出しを跨いでも callback の ambient 要求が届く
    #[test]
    fn 汎用関数のcallbackがambientを受け取る() {
        assert_eq!(int(&generic_ambient_src()), 42);
    }

    /// 同じ宣言でも ambient を要らない具体化は、提供が1つも無い所で走る
    #[test]
    fn ambientを要らない具体化は提供無しで走る() {
        assert!(matches!(
            run(&generic_ambient_src(), "quiet"),
            Ok(Value::Int(2))
        ));
    }

    // ---- 組み込みの `push`(MAP-075 決定4) ----

    /// 繰り返し押し込むと、書いた順にそのまま並ぶ
    #[test]
    fn pushは順序と長さを保つ() {
        let src = "fn main(-> int) {\n\
                   \x20 let mut xs = [1]\n\
                   \x20 xs.push(2)\n\
                   \x20 xs.push(3)\n\
                   \x20 xs.push(4)\n\
                   \x20 let mut seen = 0\n\
                   \x20 let mut count = 0\n\
                   \x20 for x in &xs { seen = seen * 10 + x }\n\
                   \x20 for x in &xs { count = count + 1 }\n\
                   \x20 seen * 10 + count\n\
                   }\n";
        assert!(matches!(checked_run(src, "main"), Ok(Value::Int(12344))));
    }

    /// 容量 0 の配列へも押し込める
    #[test]
    fn 空の配列へもpushできる() {
        let src = "fn main(-> int) {\n\
                   \x20 let mut xs: [int] = []\n\
                   \x20 xs.push(9)\n\
                   \x20 let mut total = 0\n\
                   \x20 for x in xs { total = total + x }\n\
                   \x20 total\n\
                   }\n";
        assert!(matches!(checked_run(src, "main"), Ok(Value::Int(9))));
    }

    /// 非 Copy の値は配列の中へ移る。移った先から読めることで、所有が
    /// 配列側にあることを見る(元の束縛が使えないことは所有権検査が断る)
    #[test]
    fn pushした非copyの値は配列が持つ() {
        let src = "struct Item { name: str, n: int }\n\
                   fn main(-> int) {\n\
                   \x20 let mut xs = [Item { name = \"a\", n = 1 }]\n\
                   \x20 let it = Item { name = \"b\", n = 2 }\n\
                   \x20 xs.push(move it)\n\
                   \x20 let mut total = 0\n\
                   \x20 for i in &xs { if i.name == \"b\" { total = total + i.n } }\n\
                   \x20 for i in &xs { total = total + i.n }\n\
                   \x20 total\n\
                   }\n";
        assert!(matches!(checked_run(src, "main"), Ok(Value::Int(5))));
    }

    // ---- 汎用 `map`(MAP-080) ----

    /// `Map<T>` と `[T]` の実装。本体は通常の Rhodolite コードだけ
    const MAP_SRC: &str = "trait Map<T> { fn map<U>(self, f: fn(T -> U) -> [U]) }\n\
         impl<T> Map<T> for [T] {\n\
         \x20 fn map<U>(self, f: fn(T -> U) -> [U]) {\n\
         \x20   let mut result: [U] = []\n\
         \x20   for x in move self { result.push(f(move x)) }\n\
         \x20   move result\n\
         \x20 }\n\
         }\n";

    /// 呼び出しの**順序**を観測するための ambient なカウンタ。`f` が呼ばれる
    /// たびに 1 から順に番号を返す
    const TICK_SRC: &str = "trait Tick { fn next(&mut self -> int) }\n\
         struct Counter { n: int }\n\
         impl Tick for Counter { fn next(&mut self -> int) { self.n = self.n + 1\n self.n } }\n\
         effect tick: Tick\n";

    /// Copy な要素型。結果は入力と同じ並びで、`f` は要素ごとにちょうど1度
    #[test]
    fn mapはcopyな要素を順に写す() {
        let src = format!(
            "{MAP_SRC}fn double(n: int -> int) {{ n * 2 }}\n\
             fn main(-> int) {{\n\
             \x20 let xs = [1, 2, 3]\n\
             \x20 let ys = move xs.map(double)\n\
             \x20 let mut seen = 0\n\
             \x20 let mut count = 0\n\
             \x20 for y in &ys {{ seen = seen * 10 + y }}\n\
             \x20 for y in &ys {{ count = count + 1 }}\n\
             \x20 seen * 10 + count\n\
             }}\n"
        );
        assert!(matches!(checked_run(&src, "main"), Ok(Value::Int(2463))));
    }

    /// `f` は左から順に1度ずつ呼ばれる。ambient なカウンタが返す番号が
    /// 要素の並びと一致することで、結果の並びではなく**呼び出し順**を見る
    #[test]
    fn mapのcallbackは左から順に一度ずつ呼ばれる() {
        let src = format!(
            "{MAP_SRC}{TICK_SRC}fn stamp(n: int -> int) {{ n * 10 + tick.next() }}\n\
             fn fold(xs: &[int] -> int) {{ let mut t = 0\n for x in xs {{ t = t * 1000 + x }}\n t }}\n\
             fn main(-> int) {{\n\
             \x20 with tick(Counter {{ n = 0 }}) {{\n\
             \x20   let xs = [7, 8, 9]\n\
             \x20   let ys = move xs.map(stamp)\n\
             \x20   fold(&ys)\n\
             \x20 }}\n\
             }}\n"
        );
        assert!(matches!(
            checked_run(&src, "main"),
            Ok(Value::Int(71_082_093))
        ));
    }

    /// 空の配列では `f` を1度も呼ばず、結果も空
    #[test]
    fn 空の配列のmapはcallbackを呼ばない() {
        let src = format!(
            "{MAP_SRC}{TICK_SRC}fn stamp(n: int -> int) {{ n * 10 + tick.next() }}\n\
             fn main(-> int) {{\n\
             \x20 with tick(Counter {{ n = 0 }}) {{\n\
             \x20   let xs: [int] = []\n\
             \x20   let ys = move xs.map(stamp)\n\
             \x20   let mut count = 0\n\
             \x20   for y in &ys {{ count = count + 1 }}\n\
             \x20   count * 10 + tick.next()\n\
             \x20 }}\n\
             }}\n"
        );
        // 長さ 0、かつ写像後の最初の `tick.next()` がまだ 1 = 1度も呼んでいない
        assert!(matches!(checked_run(&src, "main"), Ok(Value::Int(1))));
    }

    /// 非 Copy の要素は `f` の中へ move される。結果の配列が新しい所有を持つ
    #[test]
    fn mapは非copyな要素をcallbackへmoveする() {
        let src = format!(
            "{MAP_SRC}struct Tag {{ name: str, weight: int }}\n\
             fn weigh(t: Tag -> int) {{ t.weight * 2 }}\n\
             fn main(-> int) {{\n\
             \x20 let tags = [Tag {{ name = \"a\", weight = 1 }}, Tag {{ name = \"b\", weight = 2 }}]\n\
             \x20 let ws = move tags.map(weigh)\n\
             \x20 let mut total = 0\n\
             \x20 for w in &ws {{ total = total * 10 + w }}\n\
             \x20 total\n\
             }}\n"
        );
        assert!(matches!(checked_run(&src, "main"), Ok(Value::Int(24))));
    }

    /// `clone()` してから写せば、元の配列はそのまま残る
    #[test]
    fn cloneしたmapは元の配列を残す() {
        let src = format!(
            "{MAP_SRC}fn double(n: int -> int) {{ n * 2 }}\n\
             fn main(-> int) {{\n\
             \x20 let xs = [1, 2, 3]\n\
             \x20 let ys = xs.clone().map(double)\n\
             \x20 let mut original = 0\n\
             \x20 let mut mapped = 0\n\
             \x20 for x in &xs {{ original = original * 10 + x }}\n\
             \x20 for y in &ys {{ mapped = mapped * 10 + y }}\n\
             \x20 mapped * 1000 + original\n\
             }}\n"
        );
        assert!(matches!(checked_run(&src, "main"), Ok(Value::Int(246_123))));
    }
}
