//! 評価。構文木を走らせる。
//!
//! 段は4つに割って実装した。
//!
//!   1. struct・フィールド・演算・`if`・`return`・関数呼び出し
//!   2. `impl` とメソッド呼び出し・配列
//!   3. **ambient** — `with db(store) { ... }` が `db.save(u)` に届く
//!   4. `test` の実行。`examples/canonical.rd` 完走
//!
//! `Env` は `invoke` で作り直し、`Ambient` はそのまま渡す。
//! **この差1行が言語の全部**(CONTEXT.md「ambient」)。

use crate::ast::{BinOp, Expr, ExprKind, Head, Item, Program, Provision, Sig, UnOp};
use crate::requirement::{Slots, collect_slots};
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::rc::Rc;

// ---------------------------------------------------------------------------
// 値
// ---------------------------------------------------------------------------

/// 実行時の値。
///
/// ハンドラ専用の変種は**作らない**。`Postgres::new(url)` が返すのは
/// `type_name` を持つただの `Struct` で、`db.save(u)` の解決はその型名で
/// `impl` を引く(段2)。「ハンドラはただの impl」を値の側でも守る形。
#[derive(Debug, Clone)]
pub enum Value {
    Int(i64),
    Str(String),
    Bool(bool),
    Unit,
    /// `User?` の無い方
    Nil,
    Struct(Rc<RefCell<Obj>>),
    /// `Gold`。所属 enum と variant の名前だけを持つ**不変**な値。
    ///
    /// struct を流用しないのは、フィールドアクセス・メソッド解決・表示が
    /// enum と struct を取り違えない不変条件を作るため(design.md 決定3)
    Enum {
        enum_name: String,
        variant: String,
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
    pub type_name: String,
    pub fields: BTreeMap<String, Value>,
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
            // 同じ enum の同じ variant だけが等しい
            (
                Enum {
                    enum_name: ea,
                    variant: va,
                },
                Enum {
                    enum_name: eb,
                    variant: vb,
                },
            ) => ea == eb && va == vb,
            (Struct(a), Struct(b)) => {
                // 同じ実体なら中身を見ない。循環していても答えが出る
                if Rc::ptr_eq(a, b) {
                    return Ok(true);
                }
                let (a, b) = (a.borrow(), b.borrow());
                if a.type_name != b.type_name || a.fields.len() != b.fields.len() {
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

    pub fn show(&self) -> String {
        match self {
            Value::Int(n) => n.to_string(),
            Value::Str(s) => format!("{s:?}"),
            Value::Bool(b) => b.to_string(),
            Value::Unit => "unit".to_string(),
            Value::Nil => "nil".to_string(),
            Value::Struct(o) => o.borrow().type_name.clone(),
            Value::Enum { enum_name, variant } => format!("{enum_name}.{variant}"),
            Value::Array(xs) => format!("[{} 要素]", xs.borrow().len()),
        }
    }
}

// ---------------------------------------------------------------------------
// 脱出
// ---------------------------------------------------------------------------

/// 式の評価を途中で終わらせるもの。`Result` の `Err` 側に載せて `?` で運ぶ。
#[derive(Debug)]
pub enum Flow {
    /// `return` — 関数の境界で受け止める
    Return(Value),
    /// 実行時エラー。ponytail: 文字列。span を付けるのは miette を入れるときに
    Error(String),
}

impl std::fmt::Display for Flow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Flow::Error(m) => write!(f, "{m}"),
            Flow::Return(_) => write!(f, "`return` が関数の外に出ました"),
        }
    }
}

pub type Eval = Result<Value, Flow>;

fn fail<T>(msg: impl Into<String>) -> Result<T, Flow> {
    Err(Flow::Error(msg.into()))
}

#[derive(Clone)]
enum Binding {
    Local(Value),
    Slot,
}

/// 字句的な束縛。内側のフレームから名前を探す。**関数呼び出しで切れる。**
struct Env {
    scopes: Vec<HashMap<String, Binding>>,
}

impl Env {
    fn new() -> Self {
        Env {
            scopes: vec![HashMap::new()],
        }
    }

    fn binding(&self, name: &str) -> Option<&Binding> {
        for scope in self.scopes.iter().rev() {
            if let Some(binding) = scope.get(name) {
                return Some(binding);
            }
        }
        None
    }

    fn insert(&mut self, name: String, value: Value) {
        self.scopes
            .last_mut()
            .expect("Env には必ずスコープが1つある")
            .insert(name, Binding::Local(value));
    }

    fn assign(&mut self, name: &str, value: Value) -> Result<(), ()> {
        for scope in self.scopes.iter_mut().rev() {
            if let Some(binding) = scope.get_mut(name) {
                return match binding {
                    Binding::Local(current) => {
                        *current = value;
                        Ok(())
                    }
                    Binding::Slot => Err(()),
                };
            }
        }
        self.insert(name.to_string(), value);
        Ok(())
    }

    fn push_slots<'s>(&mut self, slots: impl Iterator<Item = &'s str>) {
        let mut scope = HashMap::new();
        for slot in slots {
            scope.insert(slot.to_string(), Binding::Slot);
        }
        self.scopes.push(scope);
    }

    fn pop_scope(&mut self) {
        assert!(self.scopes.len() > 1, "最外スコープは外せない");
        self.scopes.pop();
    }
}

/// ambient 束縛。型だけ選んだ状態と、実体まで置いた状態を区別する。
#[derive(Clone)]
enum AmbientBinding {
    Type(String),
    Value(Value),
}

/// ambient 束縛。スロット名 → 選ばれた型または実体。**関数呼び出しで切れない。**
///
/// `Env` との差はこれだけ: `invoke` が `Env` を作り直すのに対し、`Ambient` は
/// そのまま渡す。CONTEXT.md「ambient」の「関数呼び出しで切れないもの」の実装が
/// この1行の違い。requirement.rs の `scan` が `provided` を引数で運ぶのと同じ形で、
/// スコープの終わりを書く必要がない(戻った時点で `inner` は消えている)。
type Ambient = BTreeMap<String, AmbientBinding>;

// ---------------------------------------------------------------------------
// インタプリタ
// ---------------------------------------------------------------------------

/// `impl` の中の1メソッド。
///
/// `trait_name` を**鍵にせず候補側に持つ**のは、呼び出し地点で trait が
/// 分かるとは限らないため。`db.save(u)` はスロットなので `effect db: Database`
/// から引けるが、`store.get(id)` はただの変数で、実行時にあるのは型名だけ。
///
/// 候補が複数あるとき、スロット経由なら trait で絞れて必ず一意に決まる
/// (CONTEXT.md「スロットは常に名前を持つので曖昧性が発生しない」)。
/// ただの変数で複数あるなら曖昧としてエラーにする。
struct Method<'a> {
    trait_name: Option<&'a str>,
    sig: &'a Sig,
    body: &'a [Expr],
}

pub struct Interp<'a> {
    fns: HashMap<&'a str, (&'a Sig, &'a [Expr])>,
    /// 型名 → その型の全メソッド(trait 実装も inherent も混ぜて入れる)
    methods: HashMap<&'a str, Vec<Method<'a>>>,
    /// 型名の集合。フィールド0個の struct を名前だけで値にするのに使う。
    ///
    /// フィールドの過不足は `typecheck::check` が実行前に済ませているので、ここは
    /// 名前の有無しか見ない。ponytail: フィールド**値**の型で実行前に分かっているのは
    /// 「型の分かる位置に置かれた enum の所属」だけ(design.md 決定4)
    structs: HashSet<&'a str>,
    /// variant の正準名 → 所属 enum の正準名。裸の `Gold` を値へ落とすのに使う
    variants: HashMap<&'a str, &'a str>,
    /// スロット名 → trait 名。requirement.rs のものを再利用する
    slots: Slots,
}

impl<'a> Interp<'a> {
    pub fn new(program: &'a Program) -> Self {
        let mut fns = HashMap::new();
        let mut methods: HashMap<_, Vec<_>> = HashMap::new();
        let mut structs = HashSet::new();
        let mut variants = HashMap::new();
        for item in &program.items {
            match item {
                Item::Fn { sig, body, .. } => {
                    fns.insert(sig.name.as_str(), (sig, body.as_slice()));
                }
                Item::Struct { name, .. } => {
                    structs.insert(name.as_str());
                }
                Item::Enum {
                    name, variants: vs, ..
                } => {
                    for variant in vs {
                        variants.insert(variant.as_str(), name.as_str());
                    }
                }
                Item::Impl {
                    trait_name,
                    type_name,
                    methods: ms,
                    ..
                } => {
                    let entry: &mut Vec<_> = methods.entry(type_name.as_str()).or_default();
                    for (sig, body) in ms {
                        entry.push(Method {
                            trait_name: trait_name.as_deref(),
                            sig,
                            body: body.as_slice(),
                        });
                    }
                }
                _ => {}
            }
        }
        Interp {
            fns,
            methods,
            structs,
            variants,
            slots: collect_slots(program),
        }
    }

    /// 型名とメソッド名から本体を引く。
    ///
    /// `want_trait` は「この trait のものが欲しい」— スロット経由の呼び出しで
    /// だけ分かる。段3で使う。ponytail: いまの呼び出し元は常に `None`
    fn find_method(
        &self,
        type_name: &str,
        method: &str,
        want_trait: Option<&str>,
    ) -> Result<&Method<'a>, Flow> {
        let all = self
            .methods
            .get(type_name)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let mut named = all.iter().filter(|m| m.sig.name == method);

        let Some(first) = named.next() else {
            return fail(format!("`{type_name}` に `{method}` がありません"));
        };

        if let Some(want) = want_trait {
            return match all
                .iter()
                .find(|m| m.sig.name == method && m.trait_name == Some(want))
            {
                Some(m) => Ok(m),
                None => fail(format!(
                    "`{type_name}` は `{want}` の `{method}` を実装していません"
                )),
            };
        }

        // 候補が2つ以上あるのに trait が分からない。スロット経由なら
        // trait で絞れるので、ここに来るのはただの変数のときだけ
        if named.next().is_some() {
            return fail(format!(
                "`{type_name}` の `{method}` がどの trait のものか決まりません"
            ));
        }
        Ok(first)
    }

    /// 式がスロット名そのものなら、その名前。`db.save(u)` の `db` を見分ける。
    ///
    /// 最も内側の束縛が勝つ。通常はローカルがトップレベルのスロットを隠すが、
    /// `with db(value)` の本体ではスロットの束縛を1段内側へ積む。
    fn slot_of<'e>(&self, e: &'e Expr, env: &Env) -> Option<&'e str> {
        let ExprKind::Ident(name) = &e.kind else {
            return None;
        };
        self.name_is_slot(name, env).then_some(name)
    }

    fn name_is_slot(&self, name: &str, env: &Env) -> bool {
        match env.binding(name) {
            Some(Binding::Local(_)) => false,
            Some(Binding::Slot) => true,
            None => self.slots.is_slot(name),
        }
    }

    /// その型が trait を実装しているか。提供の検査に使う。
    fn implements(&self, type_name: &str, trait_name: &str) -> bool {
        self.methods
            .get(type_name)
            .is_some_and(|ms| ms.iter().any(|m| m.trait_name == Some(trait_name)))
    }

    /// エントリ(`main`)を呼ぶ。
    ///
    /// **ambient は空から始まる。**提供されていないものは何も届かない、が出発点。
    pub fn run(&self, name: &str) -> Eval {
        self.call(name, Vec::new(), &Ambient::new())
    }

    /// `test` の本体を走らせる。関数と同じ扱いで、`Env` も `Ambient` も空から。
    pub fn run_body(&self, body: &[Expr]) -> Eval {
        let mut env = Env::new();
        match self.block(body, &mut env, &Ambient::new()) {
            Err(Flow::Return(v)) => Ok(v),
            other => other,
        }
    }

    /// 名前で関数を呼ぶ。
    ///
    /// **`Env` をここで作り直す。**呼び出し元のローカル束縛は届かない。
    /// 段3で足す `Ambient` はこの境界を越える — それが推移性。
    fn call(&self, name: &str, args: Vec<Value>, ambient: &Ambient) -> Eval {
        let Some((sig, body)) = self.fns.get(name) else {
            return fail(format!("関数 `{name}` がありません"));
        };
        self.invoke(name, sig, body, args, None, ambient)
    }

    /// `Postgres::new(url)` — レシーバを取らない、型に属する関数。
    fn call_path(
        &self,
        type_name: &str,
        method: &str,
        args: Vec<Value>,
        want_trait: Option<&str>,
        ambient: &Ambient,
    ) -> Eval {
        let m = self.find_method(type_name, method, want_trait)?;
        let what = format!("{type_name}::{method}");
        if m.sig.has_self {
            return fail(format!("`{what}` はレシーバが必要です"));
        }
        self.invoke(&what, m.sig, m.body, args, None, ambient)
    }

    /// `store.get(id)` — レシーバを取るメソッド。
    ///
    /// `want_trait` はスロット経由のときだけ渡せる(段3)。
    fn call_method(
        &self,
        recv: Value,
        method: &str,
        args: Vec<Value>,
        want_trait: Option<&str>,
        ambient: &Ambient,
    ) -> Eval {
        let Value::Struct(o) = &recv else {
            return fail(format!(
                "`.{method}` を呼べません。struct ではありません ({})",
                recv.show()
            ));
        };
        let type_name = o.borrow().type_name.clone();

        let m = self.find_method(&type_name, method, want_trait)?;
        let what = format!("{type_name}.{method}");
        if !m.sig.has_self {
            return fail(format!(
                "`{what}` は self を取りません。`{type_name}::{method}` で呼びます"
            ));
        }
        self.invoke(&what, m.sig, m.body, args, Some(recv), ambient)
    }

    /// 本体を新しい `Env` で走らせる。呼び出し3種の共通部分。
    fn invoke(
        &self,
        what: &str,
        sig: &Sig,
        body: &[Expr],
        args: Vec<Value>,
        recv: Option<Value>,
        ambient: &Ambient,
    ) -> Eval {
        if sig.params.len() != args.len() {
            return fail(format!(
                "`{what}` は引数 {} 個ですが {} 個渡されました",
                sig.params.len(),
                args.len()
            ));
        }

        let mut env = Env::new();
        // `self` は普通の束縛。ambient と違って**関数呼び出しで切れる**
        if let Some(r) = recv {
            env.insert("self".to_string(), r);
        }
        for (p, a) in sig.params.iter().zip(args) {
            env.insert(p.name.clone(), a);
        }

        // **ここが言語の全部。**`env` は上で新しく作った。`ambient` はそのまま渡す
        match self.block(body, &mut env, ambient) {
            Err(Flow::Return(v)) => Ok(v),
            other => other,
        }
    }

    /// ブロックの値は最後の式(CONTEXT.md「値ベース」)。
    ///
    /// ponytail: ブロックごとに新しいスコープを作らない。`let` は外へ漏れる。
    /// 外の変数への代入が消えないほうを優先した。シャドーイングが要るときに分ける
    fn block(&self, body: &[Expr], env: &mut Env, ambient: &Ambient) -> Eval {
        let mut last = Value::Unit;
        for e in body {
            last = self.eval(e, env, ambient)?;
        }
        Ok(last)
    }

    fn eval(&self, e: &Expr, env: &mut Env, ambient: &Ambient) -> Eval {
        match &e.kind {
            ExprKind::Int(n) => Ok(Value::Int(*n)),
            ExprKind::Str(s) => Ok(Value::Str(s.clone())),
            ExprKind::Bool(b) => Ok(Value::Bool(*b)),
            ExprKind::Nil => Ok(Value::Nil),

            ExprKind::Ident(name) => match env.binding(name) {
                Some(Binding::Local(v)) => Ok(v.clone()),
                Some(Binding::Slot) => fail(format!("スロット `{name}` は値として取り出せません")),
                // `Gold` — 隠されていない variant は名前だけで値になる
                None if self.variants.contains_key(name.as_str()) => Ok(Value::Enum {
                    enum_name: self.variants[name.as_str()].to_string(),
                    variant: name.clone(),
                }),
                // フィールド0個の struct も名前だけで値になる
                None if self.structs.contains(name.as_str()) => Ok(new_obj(name, BTreeMap::new())),
                None => fail(format!("`{name}` が束縛されていません")),
            },

            ExprKind::Let { name, value } => {
                let v = self.eval(value, env, ambient)?;
                env.insert(name.clone(), v);
                Ok(Value::Unit)
            }

            ExprKind::StructLit { name, fields } => {
                let mut obj = BTreeMap::new();
                for (k, v) in fields {
                    let v = self.eval(v, env, ambient)?;
                    obj.insert(k.clone(), v);
                }
                Ok(new_obj(name, obj))
            }

            ExprKind::Field(recv, name) => {
                let Value::Struct(o) = self.eval(recv, env, ambient)? else {
                    return fail(format!("`.{name}` を読めません。struct ではありません"));
                };
                match o.borrow().fields.get(name) {
                    Some(v) => Ok(v.clone()),
                    None => fail(format!("フィールド `{name}` がありません")),
                }
            }

            ExprKind::Assign { target, value } => {
                let v = self.eval(value, env, ambient)?;
                match &target.kind {
                    ExprKind::Ident(n) => {
                        if self.name_is_slot(n, env) {
                            return fail(format!("スロット `{n}` には代入できません"));
                        }
                        if env.assign(n, v).is_err() {
                            return fail(format!("スロット `{n}` には代入できません"));
                        }
                    }
                    ExprKind::Field(recv, f) => {
                        let Value::Struct(o) = self.eval(recv, env, ambient)? else {
                            return fail(format!("`.{f}` に代入できません。struct ではありません"));
                        };
                        o.borrow_mut().fields.insert(f.clone(), v);
                    }
                    _ => return fail("代入できない左辺です"),
                }
                Ok(Value::Unit)
            }

            ExprKind::Return(v) => {
                let v = match v {
                    Some(e) => self.eval(e, env, ambient)?,
                    None => Value::Unit,
                };
                Err(Flow::Return(v))
            }

            ExprKind::Assert(inner) => match self.eval(inner, env, ambient)? {
                Value::Bool(true) => Ok(Value::Unit),
                Value::Bool(false) => fail("assert が偽になりました"),
                other => fail(format!("assert には bool が必要です ({})", other.show())),
            },

            ExprKind::Unary(UnOp::Neg, inner) => match self.eval(inner, env, ambient)? {
                Value::Int(n) => Ok(Value::Int(-n)),
                other => fail(format!("`-` は整数だけです ({})", other.show())),
            },

            ExprKind::Binary { op, lhs, rhs } => self.binary(*op, lhs, rhs, env, ambient),

            ExprKind::Block(body) => self.block(body, env, ambient),

            // レシーバ → 引数 の順に評価する(左から右)
            ExprKind::Call(callee, args) => {
                // レシーバがスロットなら、値は ambient から来て、trait が
                // `effect db: Database` から分かる。**スロット経由は必ず一意に決まる**
                // (CONTEXT.md「スロットは常に名前を持つので曖昧性が発生しない」)
                let recv = match &callee.kind {
                    ExprKind::Field(r, m) => Some(match self.slot_of(r, env) {
                        Some(slot) => {
                            let Some(binding) = ambient.get(slot) else {
                                return fail(format!(
                                    "`{slot}` が提供されていません(`.{m}` の呼び出し)"
                                ));
                            };
                            let AmbientBinding::Value(v) = binding else {
                                return fail(format!(
                                    "`{slot}` は型だけが提供されています。実体が必要です(`.{m}` の呼び出し)"
                                ));
                            };
                            (v.clone(), self.slots.trait_of(slot))
                        }
                        None => (self.eval(r, env, ambient)?, None),
                    }),
                    _ => None,
                };
                let mut vals = Vec::with_capacity(args.len());
                for a in args {
                    vals.push(self.eval(a, env, ambient)?);
                }
                match (&callee.kind, recv) {
                    (ExprKind::Ident(name), _) => self.call(name, vals, ambient),
                    (ExprKind::Path(parts), _) => match parts.as_slice() {
                        [name, method] if self.name_is_slot(name, env) => {
                            let Some(binding) = ambient.get(name) else {
                                return fail(format!(
                                    "`{name}` が提供されていません(`::{method}` の呼び出し)"
                                ));
                            };
                            let type_name = match binding {
                                AmbientBinding::Type(type_name) => type_name.clone(),
                                AmbientBinding::Value(Value::Struct(value)) => {
                                    value.borrow().type_name.clone()
                                }
                                AmbientBinding::Value(value) => {
                                    return fail(format!(
                                        "`{name}` の実体から型を選べません ({})",
                                        value.show()
                                    ));
                                }
                            };
                            self.call_path(
                                &type_name,
                                method,
                                vals,
                                self.slots.trait_of(name),
                                ambient,
                            )
                        }
                        [type_name, method] => {
                            self.call_path(type_name, method, vals, None, ambient)
                        }
                        _ => fail(format!("`{}` は呼べません", parts.join("::"))),
                    },
                    (ExprKind::Field(_, m), Some((recv, want_trait))) => {
                        self.call_method(recv, m, vals, want_trait, ambient)
                    }
                    _ => fail("呼べない式です"),
                }
            }

            ExprKind::Head { head, body, orelse } => {
                self.head(head, body, orelse.as_deref(), env, ambient)
            }

            ExprKind::Array(items) => {
                let mut xs = Vec::with_capacity(items.len());
                for i in items {
                    xs.push(self.eval(i, env, ambient)?);
                }
                Ok(Value::Array(Rc::new(RefCell::new(xs))))
            }

            ExprKind::Path(parts) => fail(format!("`{}` は値ではありません", parts.join("::"))),
        }
    }

    fn binary(&self, op: BinOp, lhs: &Expr, rhs: &Expr, env: &mut Env, ambient: &Ambient) -> Eval {
        // `??` は短絡する。`db.find(id) ?? return false` の右辺は
        // 左辺が nil のときだけ走らないといけない
        if let BinOp::Coalesce = op {
            let l = self.eval(lhs, env, ambient)?;
            return if matches!(l, Value::Nil) {
                self.eval(rhs, env, ambient)
            } else {
                Ok(l)
            };
        }

        let l = self.eval(lhs, env, ambient)?;
        let r = self.eval(rhs, env, ambient)?;

        if let BinOp::Eq = op {
            return Ok(Value::Bool(l.eq(&r)?));
        }

        match (l, r) {
            (Value::Int(a), Value::Int(b)) => match op {
                BinOp::Add => Ok(Value::Int(a + b)),
                BinOp::Sub => Ok(Value::Int(a - b)),
                BinOp::Mul => Ok(Value::Int(a * b)),
                BinOp::Div if b == 0 => fail("0 で割れません"),
                BinOp::Div => Ok(Value::Int(a / b)),
                BinOp::Eq | BinOp::Coalesce => unreachable!("上で返している"),
            },
            (a, b) => fail(format!(
                "{op:?} を {} と {} には使えません",
                a.show(),
                b.show()
            )),
        }
    }

    fn head(
        &self,
        head: &Head,
        body: &Expr,
        orelse: Option<&Expr>,
        env: &mut Env,
        ambient: &Ambient,
    ) -> Eval {
        match head {
            Head::If(c) | Head::Elif(c) => {
                if self.cond(c, env, ambient)? {
                    self.eval(body, env, ambient)
                } else if let Some(o) = orelse {
                    self.eval(o, env, ambient)
                } else {
                    Ok(Value::Unit)
                }
            }
            Head::Else => self.eval(body, env, ambient),
            Head::While(c) => {
                while self.cond(c, env, ambient)? {
                    self.eval(body, env, ambient)?;
                }
                Ok(Value::Unit)
            }
            Head::For { var, iter } => {
                let Value::Array(xs) = self.eval(iter, env, ambient)? else {
                    return fail("for で回せるのは配列だけです");
                };
                // ponytail: 開始時点のスナップショットを回す。本体が同じ配列を
                // 触っても RefCell が二重借用で落ちない。回している最中の追加は見えない
                let snapshot: Vec<Value> = xs.borrow().clone();
                for v in snapshot {
                    env.insert(var.clone(), v);
                    self.eval(body, env, ambient)?;
                }
                Ok(Value::Unit)
            }
            // `with db(store), clock(Frozen::at(1000)) { ... }` — 提供。
            //
            // requirement.rs の `scan` と同じ形。**スコープの終わりを書かない** —
            // `self.eval(body, env, &inner)` から戻れば `inner` は消えていて、
            // 呼び出し元は元の `ambient` を持ったまま。字句スコープを
            // 呼び出しスタックがそのまま表現している。
            Head::Ambient(binders) => {
                let mut inner = ambient.clone();
                for b in binders {
                    let slot = b.slot();
                    let Some(want_trait) = self.slots.trait_of(slot) else {
                        return fail(format!("`{slot}` はスロットではありません"));
                    };

                    let binding = match b {
                        Provision::Type { type_name, .. } => {
                            if !self.implements(type_name, want_trait) {
                                return fail(format!(
                                    "`{type_name}` は `{want_trait}` を実装していないので `{slot}` に指定できません"
                                ));
                            }
                            AmbientBinding::Type(type_name.clone())
                        }
                        Provision::Value { value, .. } => {
                            // 提供する値は**提供の外**で評価する。`db(make(clock.now()))` の
                            // clock は db が立つ前に走る(requirement.rs の手順4と同じ規則)
                            let v = self.eval(value, env, ambient)?;

                            let Value::Struct(o) = &v else {
                                return fail(format!(
                                    "`{slot}` に渡せるのは struct だけです ({})",
                                    v.show()
                                ));
                            };
                            let type_name = o.borrow().type_name.clone();
                            if !self.implements(&type_name, want_trait) {
                                return fail(format!(
                                    "`{type_name}` は `{want_trait}` を実装していないので `{slot}` に渡せません"
                                ));
                            }
                            AmbientBinding::Value(v)
                        }
                    };

                    // 内側勝ち。同じスロットの入れ子は上書きになる
                    inner.insert(slot.to_string(), binding);
                }
                env.push_slots(binders.iter().map(|b| b.slot()));
                let result = self.eval(body, env, &inner);
                env.pop_scope();
                result
            }
        }
    }

    fn cond(&self, c: &Expr, env: &mut Env, ambient: &Ambient) -> Result<bool, Flow> {
        match self.eval(c, env, ambient)? {
            Value::Bool(b) => Ok(b),
            other => fail(format!("条件には bool が必要です ({})", other.show())),
        }
    }
}

fn new_obj(type_name: &str, fields: BTreeMap<String, Value>) -> Value {
    Value::Struct(Rc::new(RefCell::new(Obj {
        type_name: type_name.to_string(),
        fields,
    })))
}

// ---------------------------------------------------------------------------
// テスト
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lex::{join, lex};
    use crate::parse;

    /// ソースを評価して、指定した関数を引数なしで呼ぶ
    fn run(src: &str, entry: &str) -> Result<Value, String> {
        let program = parse::parse(&join(lex(src).unwrap())).expect("パースできるはず");
        Interp::new(&program).run(entry).map_err(|f| match f {
            Flow::Error(m) => m,
            Flow::Return(v) => panic!("return が関数境界を越えた: {}", v.show()),
        })
    }

    fn int(src: &str) -> i64 {
        match run(src, "main").expect("評価が通るはず") {
            Value::Int(n) => n,
            other => panic!("整数ではない: {}", other.show()),
        }
    }

    #[test]
    fn ブロックの値は最後の式() {
        assert_eq!(int("fn main() {\n 1\n 2\n 3\n}\n"), 3);
    }

    #[test]
    fn 算術と優先順位() {
        assert_eq!(int("fn main() {\n 1 + 2 * 3\n}\n"), 7);
        assert_eq!(int("fn main() {\n -4 / 2\n}\n"), -2);
    }

    #[test]
    fn ゼロ除算はエラーになる() {
        assert!(run("fn main() {\n 1 / 0\n}\n", "main").is_err());
    }

    #[test]
    fn letと参照() {
        assert_eq!(int("fn main() {\n let x = 40\n x + 2\n}\n"), 42);
    }

    #[test]
    fn 束縛されていない名前はエラー() {
        assert!(run("fn main() {\n nope\n}\n", "main").is_err());
    }

    #[test]
    fn 呼び出しでenvは切れる() {
        // callee は呼び出し元の `x` を見られない
        let src = "fn callee() {\n x\n}\n\
                   fn main() {\n let x = 1\n callee()\n}\n";
        assert!(run(src, "main").is_err());
    }

    #[test]
    fn returnは関数境界で止まる() {
        let src = "fn f() {\n return 7\n 999\n}\n\
                   fn main() {\n f() + 1\n}\n";
        assert_eq!(int(src), 8);
    }

    #[test]
    fn structのフィールドを読み書きする() {
        let src = "struct User { rank: Rank }\n\
                   fn main() {\n\
                   \x20 let u = User { rank = 1 }\n\
                   \x20 u.rank = 2\n\
                   \x20 u.rank\n\
                   }\n";
        assert_eq!(int(src), 2);
    }

    /// **差し替えが成立する条件。**呼び出し先での変更が呼び出し元から見えること
    #[test]
    fn 呼び出し先でのフィールド変更が呼び出し元に見える() {
        let src = "struct User { rank: Rank }\n\
                   fn stamp(u: User) {\n\
                   \x20 u.rank = 99\n\
                   }\n\
                   fn main() {\n\
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
                   struct Silver {}\n\
                   fn main() {\n\
                   \x20 assert Gold == Gold\n\
                   \x20 Gold == Silver\n\
                   }\n";
        assert!(matches!(run(src, "main"), Ok(Value::Bool(false))));
    }

    // ---- enum ----

    const RANKS: &str = "enum Rank { Bronze Gold }\n\
                         enum Grade { Low High }\n";

    #[test]
    fn variantは裸の名前で値になる() {
        let src = format!("{RANKS}fn main() {{\n Gold\n}}\n");
        assert_eq!(run(&src, "main").unwrap().show(), "Rank.Gold");
    }

    #[test]
    fn 同じvariantは等しく別のvariantは等しくない() {
        let src = format!(
            "{RANKS}fn main() {{\n\
             \x20 assert Gold == Gold\n\
             \x20 Gold == Bronze\n\
             }}\n"
        );
        assert!(matches!(run(&src, "main"), Ok(Value::Bool(false))));
    }

    /// variant 名が同じでも所属 enum が違えば別の値。同名 variant は別モジュールに
    /// しか置けないので、値の比較規則としてここで固定する
    #[test]
    fn 別のenumの同名variantとは等しくない() {
        let same_in_a = Value::Enum {
            enum_name: "a::Rank".into(),
            variant: "a::Same".into(),
        };
        let same_in_b = Value::Enum {
            enum_name: "b::Grade".into(),
            variant: "b::Same".into(),
        };
        assert!(!same_in_a.eq(&same_in_b).unwrap());
        assert!(same_in_a.eq(&same_in_a.clone()).unwrap());
    }

    #[test]
    fn ローカルはvariantを隠す() {
        let src = format!("{RANKS}fn main() {{\n let Gold = 7\n Gold\n}}\n");
        assert_eq!(int(&src), 7);
    }

    #[test]
    fn enumはstructではないのでフィールドを読めない() {
        let src = format!("{RANKS}fn main() {{\n Gold.name\n}}\n");
        let e = run(&src, "main").expect_err("enum にフィールドは無い");
        assert!(e.contains("struct ではありません"), "{e}");
    }

    #[test]
    fn assertが偽ならエラー() {
        assert!(run("fn main() {\n assert 1 == 2\n}\n", "main").is_err());
    }

    #[test]
    fn ifは値を産む() {
        let src = "fn main() {\n\
                   \x20 if 1 == 1: 10\n\
                   \x20 else: 20\n\
                   }\n";
        assert_eq!(int(src), 10);
    }

    #[test]
    fn elseの側も選ばれる() {
        let src = "fn main() {\n\
                   \x20 if 1 == 2: 10\n\
                   \x20 else: 20\n\
                   }\n";
        assert_eq!(int(src), 20);
    }

    #[test]
    fn whileが回る() {
        let src = "fn main() {\n\
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
        let src = "fn main() {\n\
                   \x20 1 ?? return 999\n\
                   }\n";
        assert_eq!(int(src), 1);
    }

    // ---- 段2: impl と配列 ----

    #[test]
    fn パス呼び出しでimplの関数を呼ぶ() {
        let src = "struct Frozen { t: Time }\n\
                   impl Frozen {\n\
                   \x20 fn at(t: Time -> Frozen) {\n\
                   \x20   Frozen { t = t }\n\
                   \x20 }\n\
                   }\n\
                   fn main() {\n\
                   \x20 Frozen::at(1000).t\n\
                   }\n";
        assert_eq!(int(src), 1000);
    }

    #[test]
    fn 配列は参照() {
        // `let b = a` でコピーされない。for が両方から同じものを見る
        let src = "fn main() {\n\
                   \x20 let a = [1, 2]\n\
                   \x20 let b = a\n\
                   \x20 a == b\n\
                   }\n";
        assert!(matches!(run(src, "main"), Ok(Value::Bool(true))));
    }

    #[test]
    fn forで配列を回す() {
        let src = "fn main() {\n\
                   \x20 let total = 0\n\
                   \x20 for x in [1, 2, 3]: total = total + x\n\
                   \x20 total\n\
                   }\n";
        assert_eq!(int(src), 6);
    }

    #[test]
    fn 配列の等値は中身で決まる() {
        let src = "fn main() {\n\
                   \x20 [1, 2] == [1, 2]\n\
                   }\n";
        assert!(matches!(run(src, "main"), Ok(Value::Bool(true))));
    }

    /// trait を候補側に持つ効果。ただの変数では絞れないので曖昧エラーになる
    #[test]
    fn 同名メソッドが二つのtraitにあると曖昧() {
        let src = "struct X {}\n\
                   impl A for X {\n\
                   \x20 fn get(-> Int) {\n\
                   \x20   1\n\
                   \x20 }\n\
                   }\n\
                   impl B for X {\n\
                   \x20 fn get(-> Int) {\n\
                   \x20   2\n\
                   \x20 }\n\
                   }\n\
                   fn main() {\n\
                   \x20 X::get()\n\
                   }\n";
        let e = run(src, "main").expect_err("曖昧なのでエラー");
        assert!(e.contains("どの trait"), "{e}");
    }

    #[test]
    fn 無い型のメソッドはエラー() {
        assert!(run("fn main() {\n Nope::go()\n}\n", "main").is_err());
    }

    // ---- 段2b: self を取るメソッド ----

    /// ハンドラの本体が自分の保存先に手が届くこと。これが無いと差し替えが書けない
    #[test]
    fn メソッドはselfでレシーバに触れる() {
        let src = "struct Frozen { t: Time }\n\
                   impl Clock for Frozen {\n\
                   \x20 fn now(self -> Time) {\n\
                   \x20   self.t\n\
                   \x20 }\n\
                   }\n\
                   fn main() {\n\
                   \x20 let c = Frozen { t = 1000 }\n\
                   \x20 c.now()\n\
                   }\n";
        assert_eq!(int(src), 1000);
    }

    /// `InMemoryDb` の骨。配列を持って足せること
    #[test]
    fn メソッドがselfの配列を変更できる() {
        let src = "struct Store { xs: Ints }\n\
                   impl Store {\n\
                   \x20 fn new(-> Store) {\n\
                   \x20   Store { xs = [] }\n\
                   \x20 }\n\
                   }\n\
                   impl Db for Store {\n\
                   \x20 fn save(self, x: Int -> unit) {\n\
                   \x20   self.xs = [x]\n\
                   \x20 }\n\
                   \x20 fn count(self -> Int) {\n\
                   \x20   let n = 0\n\
                   \x20   for y in self.xs: n = n + 1\n\
                   \x20   n\n\
                   \x20 }\n\
                   }\n\
                   fn main() {\n\
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
                   \x20 fn go(-> Int) {\n\
                   \x20   1\n\
                   \x20 }\n\
                   }\n\
                   fn main() {\n\
                   \x20 let s = S {}\n\
                   \x20 s.go()\n\
                   }\n";
        let e = run(src, "main").expect_err("self が無いのでエラー");
        assert!(e.contains("self を取りません"), "{e}");
    }

    #[test]
    fn selfを取るメソッドはパスで呼べない() {
        let src = "struct S { n: Int }\n\
                   impl S {\n\
                   \x20 fn get(self -> Int) {\n\
                   \x20   self.n\n\
                   \x20 }\n\
                   }\n\
                   fn main() {\n\
                   \x20 S::get()\n\
                   }\n";
        let e = run(src, "main").expect_err("レシーバが無いのでエラー");
        assert!(e.contains("レシーバ"), "{e}");
    }

    /// `self` は ambient と違って普通の束縛。呼び出しで切れる
    #[test]
    fn selfは呼び出し先に届かない() {
        let src = "struct S { n: Int }\n\
                   fn helper(-> Int) {\n\
                   \x20 self.n\n\
                   }\n\
                   impl S {\n\
                   \x20 fn get(self -> Int) {\n\
                   \x20   helper()\n\
                   \x20 }\n\
                   }\n\
                   fn main() {\n\
                   \x20 let s = S { n = 1 }\n\
                   \x20 s.get()\n\
                   }\n";
        assert!(run(src, "main").is_err());
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
        let interp = Interp::new(&program);

        for item in &program.items {
            if let crate::ast::Item::Test { name, body, .. } = item
                && let Err(e) = interp.run_body(body)
            {
                panic!("test {name:?} が失敗: {e}");
            }
        }
    }

    /// 本番側の経路も走ること。Postgres は空の DB なので false が返る
    #[test]
    fn 正典のmainが走る() {
        let src = std::fs::read_to_string("examples/canonical.rd").unwrap();
        let program = parse::parse(&join(lex(&src).unwrap())).expect("パースできるはず");
        assert!(matches!(
            Interp::new(&program).run("main"),
            Ok(Value::Bool(false))
        ));
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
    const CLOCK: &str = "effect clock: Clock\n\
                         struct Frozen { t: Time }\n\
                         impl Frozen {\n\
                         \x20 fn at(t: Time -> Frozen) {\n\
                         \x20   Frozen { t = t }\n\
                         \x20 }\n\
                         }\n\
                         impl Clock for Frozen {\n\
                         \x20 fn now(self -> Time) {\n\
                         \x20   self.t\n\
                         \x20 }\n\
                         }\n";

    #[test]
    fn 提供したハンドラがスロット経由で呼ばれる() {
        let src = format!(
            "{CLOCK}\
             fn main() {{\n\
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
             fn stamp(-> Time) {{\n\
             \x20 clock.now()\n\
             }}\n\
             fn promote(-> Time) {{\n\
             \x20 stamp()\n\
             }}\n\
             fn handle(-> Time) {{\n\
             \x20 promote()\n\
             }}\n\
             fn main() {{\n\
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
        let src = "fn callee(-> Int) {\n\
                   \x20 c\n\
                   }\n\
                   fn main() {\n\
                   \x20 let c = 1\n\
                   \x20 callee()\n\
                   }\n";
        assert!(run(src, "main").is_err());
    }

    /// 差し替え。**呼ばれる側を一切変更しない**
    #[test]
    fn 同じ関数が提供を変えると別の答えを返す() {
        let src = format!(
            "{CLOCK}\
             fn stamp(-> Time) {{\n\
             \x20 clock.now()\n\
             }}\n\
             fn main(-> Int) {{\n\
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
             fn main() {{\n\
             \x20 with clock(Frozen::at(1000)) {{ clock.now() }}\n\
             \x20 clock.now()\n\
             }}\n"
        );
        let e = run(&src, "main").expect_err("外では提供されていない");
        assert!(e.contains("提供されていません"), "{e}");
    }

    #[test]
    fn 入れ子は内側が勝つ() {
        let src = format!(
            "{CLOCK}\
             fn main() {{\n\
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
             fn main() {{\n\
             \x20 with clock(Frozen::at(clock.now())) {{ 1 }}\n\
             }}\n"
        );
        let e = run(&src, "main").expect_err("clock はまだ立っていない");
        assert!(e.contains("提供されていません"), "{e}");
    }

    /// トレイトを実装していない値はスロットに入らない
    #[test]
    fn 実装していない値は提供できない() {
        let src = "effect clock: Clock\n\
                   struct Nope {}\n\
                   fn main() {\n\
                   \x20 with clock(Nope {}) { 1 }\n\
                   }\n";
        let e = run(src, "main").expect_err("Clock を実装していない");
        assert!(e.contains("実装していない"), "{e}");
    }

    /// 同じ型が2つの trait に同名メソッドを持っていても、スロット経由なら決まる
    #[test]
    fn スロット経由なら同名メソッドでも曖昧にならない() {
        let src = "effect clock: Clock\n\
                   struct Both {}\n\
                   impl Clock for Both {\n\
                   \x20 fn now(self -> Int) {\n\
                   \x20   1\n\
                   \x20 }\n\
                   }\n\
                   impl Other for Both {\n\
                   \x20 fn now(self -> Int) {\n\
                   \x20   2\n\
                   \x20 }\n\
                   }\n\
                   fn main() {\n\
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
                   \x20 fn value(self -> Int)\n\
                   }\n\
                   effect db: Database\n\
                   struct Postgres {}\n\
                   impl Database for Postgres {\n\
                   \x20 fn new(-> Postgres) { Postgres {} }\n\
                   \x20 fn value(self -> Int) { 7 }\n\
                   }\n\
                   fn main(-> Int) {\n\
                   \x20 with db<Postgres> {\n\
                   \x20   let db = db::new()\n\
                   \x20   with db(db) { db.value() }\n\
                   \x20 }\n\
                   }\n";
        assert_eq!(run(src, "main").unwrap().show(), "7");
    }

    #[test]
    fn 型だけの提供では値射影を使えない() {
        let src = "trait Database { fn value(self -> Int) }\n\
                   effect db: Database\n\
                   struct Postgres {}\n\
                   impl Database for Postgres {\n\
                   \x20 fn value(self -> Int) { 7 }\n\
                   }\n\
                   fn main(-> Int) {\n\
                   \x20 with db<Postgres> { db.value() }\n\
                   }\n";
        let error = run(src, "main").unwrap_err();
        assert!(error.contains("型だけ"), "{error}");
    }

    #[test]
    fn 型射影もスロットのtraitで関連関数を絞る() {
        let src = "trait Database { fn new(-> Both) }\n\
                   trait Other { fn new(-> Both) }\n\
                   effect db: Database\n\
                   struct Both { n: Int }\n\
                   impl Database for Both {\n\
                   \x20 fn new(-> Both) { Both { n = 1 } }\n\
                   }\n\
                   impl Other for Both {\n\
                   \x20 fn new(-> Both) { Both { n = 2 } }\n\
                   }\n\
                   fn main(-> Int) {\n\
                   \x20 with db<Both> { db::new().n }\n\
                   }\n";
        assert_eq!(run(src, "main").unwrap().show(), "1");
    }

    #[test]
    fn letなしの代入ではスロットを隠せない() {
        let src = "trait Database { fn save(self) }\n\
                   effect db: Database\n\
                   struct Store {}\n\
                   impl Database for Store { fn save(self) { 1 } }\n\
                   fn main() { db = Store }\n";
        let error = run(src, "main").unwrap_err();
        assert!(
            error.contains("スロット `db` には代入できません"),
            "{error}"
        );
    }

    /// `u.x = u` で循環が作れる。比較でプロセスが落ちないこと
    #[test]
    fn 自己参照structを比較しても落ちない() {
        let src = "struct Node { x: Node }\n\
                   fn main() {\n\
                   \x20 let u = Node { x = 1 }\n\
                   \x20 u.x = u\n\
                   \x20 u == u\n\
                   }\n";
        // 同じ実体なので中身を見ずに真
        assert!(matches!(run(src, "main"), Ok(Value::Bool(true))));
    }

    /// 相互に参照し合う2つを比べる。上限に当たってエラーになる(落ちない)
    #[test]
    fn 相互循環の比較はエラーになる() {
        let src = "struct Node { x: Node }\n\
                   fn main() {\n\
                   \x20 let a = Node { x = 1 }\n\
                   \x20 let b = Node { x = 1 }\n\
                   \x20 a.x = b\n\
                   \x20 b.x = a\n\
                   \x20 a == b\n\
                   }\n";
        assert!(run(src, "main").is_err());
    }

    #[test]
    fn 同名の引数はスロットを一貫して隠す() {
        let src = "trait Database { fn save(self, u: Int -> unit) }\n\
                   effect db: Database\n\
                   struct Slot { n: Int }\n\
                   impl Database for Slot {\n\
                   \x20 fn save(self, u: Int -> unit) { self.n = 1 }\n\
                   }\n\
                   struct Local { n: Int }\n\
                   impl Database for Local {\n\
                   \x20 fn save(self, u: Int -> unit) { self.n = 2 }\n\
                   }\n\
                   fn handle(db: Local -> Int) {\n\
                   \x20 db.save(0)\n\
                   \x20 db.n\n\
                   }\n\
                   fn main(-> Int) {\n\
                   \x20 let slot = Slot { n = 0 }\n\
                   \x20 with db(slot) {\n\
                   \x20   let result = handle(Local { n = 7 })\n\
                   \x20   result * 10 + slot.n\n\
                   \x20 }\n\
                   }\n";
        assert_eq!(run(src, "main").unwrap().show(), "20");
    }

    #[test]
    fn withの右辺は外側で本体はスロットとして解決する() {
        let src = "trait Database { fn save(self, u: Int -> unit) }\n\
                   effect db: Database\n\
                   struct Store { n: Int }\n\
                   impl Database for Store {\n\
                   \x20 fn save(self, u: Int -> unit) { self.n = u }\n\
                   }\n\
                   fn main(-> Int) {\n\
                   \x20 let db = Store { n = 0 }\n\
                   \x20 with db(db) { db.save(9) }\n\
                   \x20 db.n\n\
                   }\n";
        assert_eq!(run(src, "main").unwrap().show(), "9");
    }
}
