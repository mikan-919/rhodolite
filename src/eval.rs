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
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;

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
    Struct(Rc<RefCell<Obj>>),
    /// `Gold` / `Lookup.Found(user)`。variant の宣言と宣言順の payload を持つ
    /// **不変**な値。fieldless は payload が空。
    ///
    /// struct を流用しないのは、フィールドアクセス・メソッド解決・表示が
    /// enum と struct を取り違えない不変条件を作るため(design.md 決定3)
    Enum {
        variant: hir::VariantId,
        payload: Vec<Value>,
    },
    /// `[alice]`。struct と同じく**参照**。`let a = b` は別物にならない。
    /// 「複合値は参照」の規則1本で済ませるため(型によって代入の意味が変わらない)
    Array(Rc<RefCell<Vec<Value>>>),
}

/// struct の実体。
///
/// `Rc<RefCell<_>>` にしてあるのは `stamp(u)` の中の `u.promoted_at = ...` が
/// **呼び出し元から見えないといけない**ため。値のコピーだと差し替えのデモが成立しない。
///
/// ponytail: 参照カウント。`u.x = u` で循環が作れるので**漏れる**。
/// 短命なプロセスなので放置している。常駐させるなら GC か arena + 世代 index に替える。
/// (漏れは放置できるが、循環を辿る比較は落ちるので `eq_at` で深さを見ている)
#[derive(Debug)]
pub struct Obj {
    pub type_: hir::StructId,
    /// 宣言フィールド → 値。宣言順に並ぶので比較も表示も宣言順
    pub fields: BTreeMap<hir::FieldId, Value>,
}

impl Value {
    /// `u.rank == Gold` のための等値。struct は**中身**で比べる。
    fn eq(&self, other: &Value) -> Result<bool, Flow> {
        self.eq_at(other, 0)
    }

    /// 循環は `u.x = u` で作れてしまう。深さを見ていないと**プロセスが落ちる**
    /// (スタックオーバーフローは catch できない)ので、上限でエラーに変える。
    ///
    /// ponytail: 深さ上限。到達可能な実体を覚えて回る方が正確だが、上限に
    /// 当たるのは循環しているときだけなので足りている
    fn eq_at(&self, other: &Value, depth: u32) -> Result<bool, Flow> {
        const MAX_DEPTH: u32 = 100;
        if depth > MAX_DEPTH {
            return fail("値の比較が深すぎます(循環している可能性)");
        }

        use Value::*;
        Ok(match (self, other) {
            (Int(a), Int(b)) => a == b,
            (Str(a), Str(b)) => a == b,
            (Bool(a), Bool(b)) => a == b,
            (Unit, Unit) | (Nil, Nil) => true,
            // 同じ variant で payload が対応ごとに等しいときだけ等しい。
            // `VariantId` は所属 enum ごと一意なので、別の enum の同名 variant は
            // 別の ID になる(design.md 決定6)
            (
                Enum {
                    variant: va,
                    payload: pa,
                },
                Enum {
                    variant: vb,
                    payload: pb,
                },
            ) => {
                if va != vb || pa.len() != pb.len() {
                    return Ok(false);
                }
                for (x, y) in pa.iter().zip(pb.iter()) {
                    if !x.eq_at(y, depth + 1)? {
                        return Ok(false);
                    }
                }
                true
            }
            (Struct(a), Struct(b)) => {
                // 同じ実体なら中身を見ない。循環していても答えが出る
                if Rc::ptr_eq(a, b) {
                    return Ok(true);
                }
                let (a, b) = (a.borrow(), b.borrow());
                if a.type_ != b.type_ || a.fields.len() != b.fields.len() {
                    return Ok(false);
                }
                for ((ka, va), (kb, vb)) in a.fields.iter().zip(b.fields.iter()) {
                    if ka != kb || !va.eq_at(vb, depth + 1)? {
                        return Ok(false);
                    }
                }
                true
            }
            (Array(a), Array(b)) => {
                if Rc::ptr_eq(a, b) {
                    return Ok(true);
                }
                let (a, b) = (a.borrow(), b.borrow());
                if a.len() != b.len() {
                    return Ok(false);
                }
                for (x, y) in a.iter().zip(b.iter()) {
                    if !x.eq_at(y, depth + 1)? {
                        return Ok(false);
                    }
                }
                true
            }
            _ => false,
        })
    }
}

// ---------------------------------------------------------------------------
// 脱出
// ---------------------------------------------------------------------------

/// 式の評価を途中で終わらせるもの。`Result` の `Err` 側に載せて `?` で運ぶ。
#[derive(Debug)]
pub enum Flow {
    /// `return` — 関数の境界で受け止める。エラーではないので診断にしない
    Return(Value),
    /// 実行時エラー。実行前の段と同じ `Diag` に載せる。span は式の境界
    /// (`Interp::eval`)で内側から1回だけ埋まる
    Error(Diag),
}

impl std::fmt::Display for Flow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Flow::Error(d) => write!(f, "{d}"),
            Flow::Return(_) => write!(f, "`return` が関数の外に出ました"),
        }
    }
}

pub type Eval = Result<Value, Flow>;

/// 位置なしで失敗する。span は評価中の式が `eval` の境界で付ける。
fn fail<T>(msg: impl Into<String>) -> Result<T, Flow> {
    Err(Flow::Error(Diag::msg(msg)))
}

/// 字句的な束縛。**関数呼び出しで切れる。**
///
/// `LocalId` は本体の中で一意なので、名前解決も入れ子のスコープも要らない。
/// ブロックが束縛を外へ漏らすかどうかは型検査が決めていて、ここは既に
/// 決まった宛先へ書くだけ(design.md 決定5)。
type Env = HashMap<hir::LocalId, Value>;

/// ambient 束縛。型だけ選んだ状態と、実体まで置いた状態を区別する。
///
/// どちらも実装は `TraitImplId` で持つ。スロット呼び出しはそこから契約メソッドの
/// 本体を引くので、実行時に名前で候補を探す必要がない(design.md 決定9)。
#[derive(Clone)]
enum AmbientBinding {
    Type(hir::TraitImplId),
    Value {
        implementation: hir::TraitImplId,
        value: Value,
    },
}

impl AmbientBinding {
    fn implementation(&self) -> hir::TraitImplId {
        match self {
            AmbientBinding::Type(implementation) | AmbientBinding::Value { implementation, .. } => {
                *implementation
            }
        }
    }
}

/// ambient 束縛。スロット → 選ばれた実装。**関数呼び出しで切れない。**
///
/// `Env` との差はこれだけ: `call` が `Env` を作り直すのに対し、`Ambient` は
/// そのまま渡す。CONTEXT.md「ambient」の「関数呼び出しで切れないもの」の実装が
/// この1行の違い。requirement.rs の `scan` が `provided` を引数で運ぶのと同じ形で、
/// スコープの終わりを書く必要がない(戻った時点で `inner` は消えている)。
type Ambient = BTreeMap<hir::SlotId, AmbientBinding>;

// ---------------------------------------------------------------------------
// インタプリタ
// ---------------------------------------------------------------------------

pub struct Interp<'p> {
    program: &'p hir::Program,
}

impl<'p> Interp<'p> {
    pub fn new(program: &'p hir::Program) -> Self {
        Interp { program }
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
            Value::Struct(o) => self.program.structs[o.borrow().type_].name.clone(),
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
            Value::Array(xs) => format!("[{} 要素]", xs.borrow().len()),
        }
    }

    /// エントリ(`main`)を呼ぶ。
    ///
    /// **ambient は空から始まる。**提供されていないものは何も届かない、が出発点。
    pub fn run(&self, entry: &str) -> Eval {
        let Some(callable) = self.program.free_callable(entry) else {
            return fail(format!("関数 `{entry}` がありません"));
        };
        self.call(callable, None, Vec::new(), &Ambient::new())
    }

    /// `test` の本体を走らせる。関数と同じ扱いで、`Env` も `Ambient` も空から。
    pub fn run_test(&self, id: hir::TestId) -> Eval {
        let body = &self.program.tests[id].body;
        let mut env = Env::new();
        match self.body(body, &mut env, &Ambient::new()) {
            Err(Flow::Return(v)) => Ok(v),
            other => other,
        }
    }

    /// 本体を新しい `Env` で走らせる。
    ///
    /// **`Env` をここで作り直す。**呼び出し元のローカル束縛は届かない。
    /// `Ambient` はこの境界を越える — それが推移性。
    fn call(
        &self,
        callable: hir::CallableId,
        recv: Option<Value>,
        args: Vec<Value>,
        ambient: &Ambient,
    ) -> Eval {
        let declared = &self.program.callables[callable];
        let mut env = Env::new();
        // `self` は普通の束縛。ambient と違って**関数呼び出しで切れる**
        if let (Some(receiver), Some(value)) = (declared.body.receiver, recv) {
            env.insert(receiver, value);
        }
        // 個数は型検査が合わせている(design.md 決定6)
        for (local, value) in declared.params.iter().zip(args) {
            env.insert(*local, value);
        }
        // **ここが言語の全部。**`env` は上で新しく作った。`ambient` はそのまま渡す
        match self.body(&declared.body, &mut env, ambient) {
            Err(Flow::Return(v)) => Ok(v),
            other => other,
        }
    }

    /// 本体の式の列。値は最後の式(CONTEXT.md「値ベース」)。
    fn body(&self, body: &hir::Body, env: &mut Env, ambient: &Ambient) -> Eval {
        self.sequence(body, &body.root, env, ambient)
    }

    fn sequence(
        &self,
        body: &hir::Body,
        ids: &[hir::ExprId],
        env: &mut Env,
        ambient: &Ambient,
    ) -> Eval {
        let mut last = Value::Unit;
        for id in ids {
            last = self.eval(body, *id, env, ambient)?;
        }
        Ok(last)
    }

    /// 式の評価。**位置を持つのはここだけ。**
    ///
    /// 失敗がまだ位置を持っていなければ、いま評価している式の span を入れる。
    /// 再帰も呼び出し先の本体もこの境界を通るので、最初に失敗を見た内側の式が
    /// 埋め、外側(ブロック・呼び出し元・別モジュール)は上書きしない
    /// (design.md 決定2)。
    fn eval(&self, body: &hir::Body, id: hir::ExprId, env: &mut Env, ambient: &Ambient) -> Eval {
        let expr = body.expr(id);
        self.eval_kind(body, expr, env, ambient)
            .map_err(|flow| match flow {
                Flow::Error(mut d) if d.span.is_none() => {
                    d.span = Some(expr.span);
                    d.label = Some("ここで失敗しました".to_string());
                    Flow::Error(d)
                }
                other => other,
            })
    }

    fn eval_kind(
        &self,
        body: &hir::Body,
        expr: &hir::Expr,
        env: &mut Env,
        ambient: &Ambient,
    ) -> Eval {
        match &expr.kind {
            hir::ExprKind::Int(n) => Ok(Value::Int(*n)),
            hir::ExprKind::Str(s) => Ok(Value::Str(s.clone())),
            hir::ExprKind::Bool(b) => Ok(Value::Bool(*b)),
            hir::ExprKind::Nil => Ok(Value::Nil),

            // 実行されなかった `let` の宛先を読むことがある(`if false { let x = 1 }`
            // の後の `x`)。型は付いていても値はまだ無い
            hir::ExprKind::Local(local) => match env.get(local) {
                Some(value) => Ok(value.clone()),
                None => fail(format!(
                    "`{}` が束縛されていません",
                    body.local(*local).name
                )),
            },

            hir::ExprKind::UnitStruct(id) => Ok(new_obj(*id, BTreeMap::new())),
            hir::ExprKind::Variant(variant) => Ok(Value::Enum {
                variant: *variant,
                payload: Vec::new(),
            }),

            hir::ExprKind::Field {
                recv,
                field,
                optional,
            } => {
                let value = self.eval(body, *recv, env, ambient)?;
                // `.?` は nil をそのまま伝播する
                if *optional && matches!(value, Value::Nil) {
                    return Ok(Value::Nil);
                }
                self.read_field(&value, *field)
            }

            hir::ExprKind::StructLit { struct_, fields } => {
                let mut obj = BTreeMap::new();
                for (field, value) in fields {
                    let value = self.eval(body, *value, env, ambient)?;
                    obj.insert(*field, value);
                }
                Ok(new_obj(*struct_, obj))
            }

            hir::ExprKind::Array(items) => {
                let mut xs = Vec::with_capacity(items.len());
                for item in items {
                    xs.push(self.eval(body, *item, env, ambient)?);
                }
                Ok(Value::Array(Rc::new(RefCell::new(xs))))
            }

            hir::ExprKind::Let { local, value } | hir::ExprKind::AssignLocal { local, value } => {
                let value = self.eval(body, *value, env, ambient)?;
                env.insert(*local, value);
                Ok(Value::Unit)
            }

            hir::ExprKind::AssignField { recv, field, value } => {
                let value = self.eval(body, *value, env, ambient)?;
                let recv = self.eval(body, *recv, env, ambient)?;
                let Value::Struct(o) = recv else {
                    return fail("フィールドを持たない値には代入できません");
                };
                o.borrow_mut().fields.insert(*field, value);
                Ok(Value::Unit)
            }

            hir::ExprKind::Neg(inner) => match self.eval(body, *inner, env, ambient)? {
                Value::Int(n) => Ok(Value::Int(n.wrapping_neg())),
                other => fail(format!("`-` は整数だけです ({})", self.show(&other))),
            },

            hir::ExprKind::Arith { op, lhs, rhs } => {
                let l = self.eval(body, *lhs, env, ambient)?;
                let r = self.eval(body, *rhs, env, ambient)?;
                match (l, r) {
                    (Value::Int(a), Value::Int(b)) => match op {
                        // int は符号付き64bit。加減乗と単項マイナスはラップし、
                        // 除算だけが実行時失敗を持つ (0除算 / MIN / -1)
                        hir::ArithOp::Add => Ok(Value::Int(a.wrapping_add(b))),
                        hir::ArithOp::Sub => Ok(Value::Int(a.wrapping_sub(b))),
                        hir::ArithOp::Mul => Ok(Value::Int(a.wrapping_mul(b))),
                        hir::ArithOp::Div => match a.checked_div(b) {
                            Some(q) => Ok(Value::Int(q)),
                            None if b == 0 => fail("0 で割れません"),
                            None => fail("この割り算は int の範囲を超えます"),
                        },
                    },
                    (a, b) => fail(format!(
                        "{} を {} と {} には使えません",
                        op.spelling(),
                        self.show(&a),
                        self.show(&b)
                    )),
                }
            }

            hir::ExprKind::Eq { lhs, rhs } => {
                let l = self.eval(body, *lhs, env, ambient)?;
                let r = self.eval(body, *rhs, env, ambient)?;
                Ok(Value::Bool(l.eq(&r)?))
            }

            // `??` は短絡する。`db.find(id) ?? return false` の右辺は
            // 左辺が nil のときだけ走らないといけない
            hir::ExprKind::Coalesce { lhs, rhs } => {
                let l = self.eval(body, *lhs, env, ambient)?;
                if matches!(l, Value::Nil) {
                    self.eval(body, *rhs, env, ambient)
                } else {
                    Ok(l)
                }
            }

            hir::ExprKind::Return(value) => {
                let value = match value {
                    Some(value) => self.eval(body, *value, env, ambient)?,
                    None => Value::Unit,
                };
                Err(Flow::Return(value))
            }

            hir::ExprKind::Assert(inner) => match self.eval(body, *inner, env, ambient)? {
                Value::Bool(true) => Ok(Value::Unit),
                Value::Bool(false) => fail("assert が偽になりました"),
                other => fail(format!(
                    "assert には bool が必要です ({})",
                    self.show(&other)
                )),
            },

            hir::ExprKind::Block(ids) => self.sequence(body, ids, env, ambient),

            hir::ExprKind::If { cond, then, orelse } => {
                if self.cond(body, *cond, env, ambient)? {
                    self.eval(body, *then, env, ambient)
                } else if let Some(orelse) = orelse {
                    self.eval(body, *orelse, env, ambient)
                } else {
                    Ok(Value::Unit)
                }
            }

            hir::ExprKind::While { cond, body: inner } => {
                while self.cond(body, *cond, env, ambient)? {
                    self.eval(body, *inner, env, ambient)?;
                }
                Ok(Value::Unit)
            }

            hir::ExprKind::For {
                var,
                iter,
                body: inner,
            } => {
                let Value::Array(xs) = self.eval(body, *iter, env, ambient)? else {
                    return fail("for で回せるのは配列だけです");
                };
                // ponytail: 開始時点のスナップショットを回す。本体が同じ配列を
                // 触っても RefCell が二重借用で落ちない。回している最中の追加は見えない
                let snapshot: Vec<Value> = xs.borrow().clone();
                for value in snapshot {
                    env.insert(*var, value);
                    self.eval(body, *inner, env, ambient)?;
                }
                Ok(Value::Unit)
            }

            // 対象は一度だけ評価し、選ばれた arm の本体だけを走らせる。payload は
            // その arm の束縛へ入れ、`_` は捨てる(design.md 決定5)。guard があれば
            // payload を束縛した後に一度だけ評価し、偽なら `_` へ落ちる
            hir::ExprKind::Match { subject, arms } => {
                let value = self.eval(body, *subject, env, ambient)?;
                let Value::Enum { variant, payload } = &value else {
                    return fail(format!(
                        "`match` の対象は enum だけです ({})",
                        self.show(&value)
                    ));
                };
                let exact = arms.iter().find(|arm| {
                    matches!(&arm.pattern, hir::Pattern::Variant { variant: v, .. } if v == variant)
                });
                if let Some(arm) = exact {
                    let hir::Pattern::Variant { bindings, .. } = &arm.pattern else {
                        unreachable!("`exact` は限定 arm だけを拾う")
                    };
                    // 束縛は `LocalId` なので、arm の外から読まれることはない
                    for (binding, value) in bindings.iter().zip(payload) {
                        if let Some(local) = binding {
                            env.insert(*local, value.clone());
                        }
                    }
                    // guard は payload が見えるこの状態で一度だけ走る
                    let selected = match arm.guard {
                        Some(guard) => self.cond(body, guard, env, ambient)?,
                        None => true,
                    };
                    if selected {
                        return self.eval(body, arm.body, env, ambient);
                    }
                }

                // 限定 arm が無いか guard が偽のときだけ `_` へ落ちる。型検査は
                // `_` を最後に強制するが、選択規則そのものを順序に頼らせない
                // (design.md 決定4)
                let catch_all = arms
                    .iter()
                    .find(|arm| matches!(arm.pattern, hir::Pattern::CatchAll));
                match catch_all {
                    Some(arm) => self.eval(body, arm.body, env, ambient),
                    None => fail(format!("{} に一致する arm がありません", self.show(&value))),
                }
            }

            hir::ExprKind::With {
                provisions,
                body: inner,
            } => {
                // 提供する値は**提供の外**で評価する。`db(make(clock.now()))` の
                // clock は db が立つ前に走る(requirement.rs の手順4と同じ規則)
                let mut next = ambient.clone();
                for provision in provisions {
                    let binding = match provision.value {
                        Some(value) => AmbientBinding::Value {
                            implementation: provision.implementation,
                            value: self.eval(body, value, env, ambient)?,
                        },
                        None => AmbientBinding::Type(provision.implementation),
                    };
                    // 内側勝ち。同じスロットの入れ子は上書きになる
                    next.insert(provision.slot, binding);
                }
                self.eval(body, *inner, env, &next)
            }

            hir::ExprKind::Call(call) => self.call_expr(body, call, env, ambient),

            hir::ExprKind::Clone(inner) => {
                let value = self.eval(body, *inner, env, ambient)?;
                deep_clone(&value, 0)
            }

            // ponytail: 修飾は場所をそのまま評価する。所有・借用・移動を
            // 意味として実装するのは introduce-ownership-and-borrowing の
            // フェーズ7(いまの評価器は複合値を共有したままなので、
            // `&` も `move` も観測できる違いを持たない)
            hir::ExprKind::Access { place, .. } => self.eval(body, *place, env, ambient),

            // 型検査を通った HIR に `Poison` は残らない(design.md 決定7)
            hir::ExprKind::Poison => fail("型が決まらない式を実行しようとしました"),
        }
    }

    /// 呼び出し。宛先は既に選ばれているので、名前で候補を探すことはしない。
    fn call_expr(
        &self,
        body: &hir::Body,
        call: &hir::Call,
        env: &mut Env,
        ambient: &Ambient,
    ) -> Eval {
        match call {
            hir::Call::Direct { callable, args } | hir::Call::Associated { callable, args } => {
                let args = self.args(body, args, env, ambient)?;
                self.call(*callable, None, args, ambient)
            }
            // レシーバ → 引数 の順に評価する(左から右)
            hir::Call::Method {
                callable,
                recv,
                args,
            } => {
                let recv = self.eval(body, *recv, env, ambient)?;
                let args = self.args(body, args, env, ambient)?;
                self.call(*callable, Some(recv), args, ambient)
            }
            hir::Call::Ctor { variant, args } => Ok(Value::Enum {
                variant: *variant,
                payload: self.args(body, args, env, ambient)?,
            }),
            // スロット経由。実体はその場の提供から来て、走る本体はその実装の
            // 契約メソッドから引く。**スロットは常に名前を持つので曖昧性が
            // 発生しない**(CONTEXT.md)
            hir::Call::Slot {
                slot,
                method,
                receiver,
                args,
                ..
            } => {
                let name = &self.program.slots[*slot].name;
                let member = &self.program.trait_methods[*method].name;
                let Some(binding) = ambient.get(slot) else {
                    return fail(format!(
                        "`{name}` が提供されていません(`{}{member}` の呼び出し)",
                        dot(*receiver)
                    ));
                };
                let recv = match (receiver, binding) {
                    (hir::SlotReceiver::Value, AmbientBinding::Value { value, .. }) => {
                        Some(value.clone())
                    }
                    (hir::SlotReceiver::Value, AmbientBinding::Type(_)) => {
                        return fail(format!(
                            "`{name}` は型だけが提供されています。実体が必要です(`.{member}` の呼び出し)"
                        ));
                    }
                    (hir::SlotReceiver::Type, _) => None,
                };
                let Some(callable) = self
                    .program
                    .implementation_of(binding.implementation(), *method)
                else {
                    return fail(format!(
                        "`{name}` に提供された実装が `{member}` を持っていません"
                    ));
                };
                let args = self.args(body, args, env, ambient)?;
                self.call(callable, recv, args, ambient)
            }
        }
    }

    fn args(
        &self,
        body: &hir::Body,
        args: &[hir::ExprId],
        env: &mut Env,
        ambient: &Ambient,
    ) -> Result<Vec<Value>, Flow> {
        let mut values = Vec::with_capacity(args.len());
        for arg in args {
            values.push(self.eval(body, *arg, env, ambient)?);
        }
        Ok(values)
    }

    fn read_field(&self, value: &Value, field: hir::FieldId) -> Eval {
        let Value::Struct(o) = value else {
            return fail(format!(
                "`.{}` を読めません。struct ではありません",
                self.program.fields[field].name
            ));
        };
        match o.borrow().fields.get(&field) {
            Some(value) => Ok(value.clone()),
            None => fail(format!(
                "フィールド `{}` がありません",
                self.program.fields[field].name
            )),
        }
    }

    fn cond(
        &self,
        body: &hir::Body,
        id: hir::ExprId,
        env: &mut Env,
        ambient: &Ambient,
    ) -> Result<bool, Flow> {
        match self.eval(body, id, env, ambient)? {
            Value::Bool(b) => Ok(b),
            other => fail(format!("条件には bool が必要です ({})", self.show(&other))),
        }
    }
}

/// スロット呼び出しの綴り。診断の文言でだけ使う
fn dot(receiver: hir::SlotReceiver) -> &'static str {
    match receiver {
        hir::SlotReceiver::Value => ".",
        hir::SlotReceiver::Type => "::",
    }
}

fn new_obj(type_: hir::StructId, fields: BTreeMap<hir::FieldId, Value>) -> Value {
    Value::Struct(Rc::new(RefCell::new(Obj { type_, fields })))
}

/// `value.clone()` の中身。struct・enum payload・optional の中身・配列を辿って
/// **独立した実体**を作る(design.md 決定4)。
///
/// ponytail: 深さ上限は `eq_at` と同じ理由。いまの値は `u.x = u` で循環を作れる
/// ので、辿り切る前に落ちるのを診断に変えている。所有の場所へ置き換わる phase 7
/// (tasks 7.4)では循環そのものが作れなくなるので、上限も消える
fn deep_clone(value: &Value, depth: u32) -> Eval {
    const MAX_DEPTH: u32 = 100;
    if depth > MAX_DEPTH {
        return fail("値の複製が深すぎます(循環している可能性)");
    }
    Ok(match value {
        Value::Struct(obj) => {
            let obj = obj.borrow();
            let mut fields = BTreeMap::new();
            for (field, value) in &obj.fields {
                fields.insert(*field, deep_clone(value, depth + 1)?);
            }
            new_obj(obj.type_, fields)
        }
        Value::Array(items) => {
            let items = items.borrow();
            let mut cloned = Vec::with_capacity(items.len());
            for item in items.iter() {
                cloned.push(deep_clone(item, depth + 1)?);
            }
            Value::Array(Rc::new(RefCell::new(cloned)))
        }
        Value::Enum { variant, payload } => {
            let mut cloned = Vec::with_capacity(payload.len());
            for value in payload {
                cloned.push(deep_clone(value, depth + 1)?);
            }
            Value::Enum {
                variant: *variant,
                payload: cloned,
            }
        }
        // スカラーと `nil` は実体を共有しない
        scalar => scalar.clone(),
    })
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
        let checked = crate::typecheck::check_and_lower(&program)
            .unwrap_or_else(|d| panic!("型検査を通るはず: {d:?}"));
        Interp::new(&checked).run(entry).map_err(|f| match f {
            Flow::Error(d) => d,
            Flow::Return(_) => panic!("return が関数境界を越えた"),
        })
    }

    /// 走らせた結果の綴り。値が名前を持たなくなったので、表示は
    /// プログラムを知っている側から引く
    fn shown(src: &str, entry: &str) -> String {
        let program = parse::parse(&join(lex(src).unwrap())).expect("パースできるはず");
        let checked = crate::typecheck::check_and_lower(&program)
            .unwrap_or_else(|d| panic!("型検査を通るはず: {d:?}"));
        let interp = Interp::new(&checked);
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
  let a = Outer { inner = Inner { n = 1 }, xs = [Inner { n = 2 }] }
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
  let inner = Inner { n = 1 }
  let a = Box::Full(inner)
  let b = a.clone()
  inner.n = 9
  match b { Box::Full(i): i.n
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
        let errors = crate::typecheck::check_and_lower(&program).expect_err("実行前に止まるはず");
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
                   \x20 let u = User { rank = 1 }\n\
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
                   fn next(c: Counter -> Box?) {\n\
                   \x20 c.n = c.n + 1\n\
                   \x20 Box { n = c.n }\n\
                   }\n\
                   fn main(-> int) {\n\
                   \x20 let c = Counter { n = 0 }\n\
                   \x20 let n = next(c).?n\n\
                   \x20 c.n * 10 + (n ?? 0)\n\
                   }\n";
        assert_eq!(int(src), 11);
    }

    /// **差し替えが成立する条件。**呼び出し先での変更が呼び出し元から見えること
    #[test]
    fn 呼び出し先でのフィールド変更が呼び出し元に見える() {
        let src = "struct User { rank: int }\n\
                   fn stamp(u: User) {\n\
                   \x20 u.rank = 99\n\
                   }\n\
                   fn main(-> int) {\n\
                   \x20 let u = User { rank = 1 }\n\
                   \x20 stamp(u)\n\
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
        assert!(!a.eq(&b).unwrap());
        assert!(a.eq(&a.clone()).unwrap());
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
             fn next(c: Counter -> Rank) {{\n\
             \x20 c.n = c.n + 1\n\
             \x20 Gold\n\
             }}\n\
             fn main(-> int) {{\n\
             \x20 let c = Counter {{ n = 0 }}\n\
             \x20 let picked = match next(c) {{ Rank::Bronze: 0\nRank::Gold: 1 }}\n\
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
             fn bump(c: Counter -> bool) {{\n\
             \x20 c.n = c.n + 1\n\
             \x20 true\n\
             }}\n\
             fn main(-> int) {{\n\
             \x20 let c = Counter {{ n = 0 }}\n\
             \x20 let picked = match Gold {{\n\
             \x20   Rank::Bronze if bump(c): 1\n\
             \x20   Rank::Gold if bump(c): 2\n\
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
                   fn bump(c: Counter -> int) {\n c.n = c.n + 1\n c.n\n}\n\
                   fn main(-> int) {\n\
                   \x20 let c = Counter { n = 0 }\n\
                   \x20 let p = Pair::Two(bump(c), bump(c))\n\
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
             \x20 let v = User {{ id = 1 }}\n\
             \x20 assert Lookup::Found(u, 2) == Lookup::Found(v, 2)\n\
             \x20 assert (Lookup::Found(u, 2) == Lookup::Found(v, 3)) == false\n\
             \x20 v.id = 9\n\
             \x20 Lookup::Found(u, 2) == Lookup::Found(v, 2)\n\
             }}\n"
        );
        assert!(matches!(run(&src, "main"), Ok(Value::Bool(false))));
    }

    /// 複合 payload は既存の「複合値は参照」の規則にそのまま乗る
    #[test]
    fn 共有された複合payloadは同じ実体を指す() {
        let src = format!(
            "{LOOKUP}fn main(-> int) {{\n\
             \x20 let u = User {{ id = 1 }}\n\
             \x20 let l = Lookup::Found(u, 0)\n\
             \x20 u.id = 7\n\
             \x20 match l {{\n\
             \x20   Lookup::Found(found, _): found.id\n\
             \x20   Lookup::Missing(_): 0\n\
             \x20   Lookup::Skipped: 0\n\
             \x20 }}\n\
             }}\n"
        );
        assert_eq!(int(&src), 7);
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
             \x20   Lookup::Missing(reason): reason\n\
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
                   \x20 let n = 0\n\
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
    fn 配列は参照() {
        // `let b = a` でコピーされない。for が両方から同じものを見る
        let src = "fn main(-> bool) {\n\
                   \x20 let a = [1, 2]\n\
                   \x20 let b = a\n\
                   \x20 a == b\n\
                   }\n";
        assert!(matches!(run(src, "main"), Ok(Value::Bool(true))));
    }

    #[test]
    fn forで配列を回す() {
        let src = "fn main(-> int) {\n\
                   \x20 let total = 0\n\
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
        let src = "trait Clock { fn now(self -> int) }\n\
                   struct Frozen { t: int }\n\
                   impl Clock for Frozen {\n\
                   \x20 fn now(self -> int) {\n\
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
                   \x20 fn save(self, x: int -> unit)\n\
                   \x20 fn count(self -> int)\n\
                   }\n\
                   struct Store { xs: [int] }\n\
                   impl Store {\n\
                   \x20 fn new(-> Store) {\n\
                   \x20   let empty: [int] = []\n\
                   \x20   Store { xs = empty }\n\
                   \x20 }\n\
                   }\n\
                   impl Db for Store {\n\
                   \x20 fn save(self, x: int -> unit) {\n\
                   \x20   self.xs = [x]\n\
                   \x20 }\n\
                   \x20 fn count(self -> int) {\n\
                   \x20   let n = 0\n\
                   \x20   for y in self.xs: n = n + 1\n\
                   \x20   n\n\
                   \x20 }\n\
                   }\n\
                   fn main(-> int) {\n\
                   \x20 let s = Store::new()\n\
                   \x20 s.save(7)\n\
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
        let checked = crate::typecheck::check_and_lower(&program).expect("型検査を通る");
        let interp = Interp::new(&checked);

        assert_eq!(checked.tests.len(), 1, "正典の test は1つ");
        for (id, declared) in checked.tests.iter() {
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
                   \x20 fn value(self -> int)\n\
                   }\n\
                   effect db: Database\n\
                   struct Postgres {}\n\
                   impl Database for Postgres {\n\
                   \x20 fn new(-> Postgres) { Postgres {} }\n\
                   \x20 fn value(self -> int) { 7 }\n\
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

    /// `u.x = u` で循環が作れる。比較でプロセスが落ちないこと
    #[test]
    fn 自己参照structを比較しても落ちない() {
        let src = "struct Node { indirect x: Node? }\n\
                   fn main(-> bool) {\n\
                   \x20 let u = Node { x = nil }\n\
                   \x20 u.x = u\n\
                   \x20 u == u\n\
                   }\n";
        // 同じ実体なので中身を見ずに真
        assert!(matches!(run(src, "main"), Ok(Value::Bool(true))));
    }

    /// 相互に参照し合う2つを比べる。上限に当たってエラーになる(落ちない)
    #[test]
    fn 相互循環の比較はエラーになる() {
        let src = "struct Node { indirect x: Node? }\n\
                   fn main(-> bool) {\n\
                   \x20 let a = Node { x = nil }\n\
                   \x20 let b = Node { x = nil }\n\
                   \x20 a.x = b\n\
                   \x20 b.x = a\n\
                   \x20 a == b\n\
                   }\n";
        assert!(run(src, "main").is_err());
    }

    #[test]
    fn 同名の引数はスロットを一貫して隠す() {
        let src = "trait Database { fn save(self, u: int -> unit) }\n\
                   effect db: Database\n\
                   struct Slot { n: int }\n\
                   impl Database for Slot {\n\
                   \x20 fn save(self, u: int -> unit) { self.n = 1 }\n\
                   }\n\
                   struct Local { n: int }\n\
                   impl Database for Local {\n\
                   \x20 fn save(self, u: int -> unit) { self.n = 2 }\n\
                   }\n\
                   fn handle(db: Local -> int) {\n\
                   \x20 db.save(0)\n\
                   \x20 db.n\n\
                   }\n\
                   fn main(-> int) {\n\
                   \x20 let slot = Slot { n = 0 }\n\
                   \x20 with db(slot) {\n\
                   \x20   let result = handle(Local { n = 7 })\n\
                   \x20   result * 10 + slot.n\n\
                   \x20 }\n\
                   }\n";
        assert_eq!(shown(src, "main"), "20");
    }

    #[test]
    fn withの右辺は外側で本体はスロットとして解決する() {
        let src = "trait Database { fn save(self, u: int -> unit) }\n\
                   effect db: Database\n\
                   struct Store { n: int }\n\
                   impl Database for Store {\n\
                   \x20 fn save(self, u: int -> unit) { self.n = u }\n\
                   }\n\
                   fn main(-> int) {\n\
                   \x20 let db = Store { n = 0 }\n\
                   \x20 with db(db) { db.save(9) }\n\
                   \x20 db.n\n\
                   }\n";
        assert_eq!(shown(src, "main"), "9");
    }
}
