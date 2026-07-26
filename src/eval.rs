//! 評価。構文木を走らせる。
//!
//! 段は4つに割ってあり、これは段1(ambient なし)。
//!
//!   1. struct・フィールド・演算・`if`・`return`・関数呼び出し  ← いまここ
//!   2. `impl` とメソッド呼び出し・配列
//!   3. **ambient** — `db(store): { ... }` が `db.save(u)` に届く
//!   4. `test` の実行。`examples/canonical.rd` 完走
//!
//! 段3が本番だが、そこで足すのは `Ambient` を引数で下へ渡す1本だけになる。
//! `Env` は関数呼び出しで作り直し、`Ambient` は渡す — **この差が言語の全部。**

use crate::ast::{BinOp, Expr, ExprKind, Head, Item, Program, Sig, UnOp};
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
            _ => false,
        })
    }

    fn show(&self) -> String {
        match self {
            Value::Int(n) => n.to_string(),
            Value::Str(s) => format!("{s:?}"),
            Value::Bool(b) => b.to_string(),
            Value::Unit => "unit".to_string(),
            Value::Nil => "nil".to_string(),
            Value::Struct(o) => o.borrow().type_name.clone(),
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

pub type Eval = Result<Value, Flow>;

fn fail<T>(msg: impl Into<String>) -> Result<T, Flow> {
    Err(Flow::Error(msg.into()))
}

/// ローカル束縛。**関数呼び出しで切れる。**
type Env = HashMap<String, Value>;

// ---------------------------------------------------------------------------
// インタプリタ
// ---------------------------------------------------------------------------

pub struct Interp<'a> {
    fns: HashMap<&'a str, (&'a Sig, &'a [Expr])>,
    /// 型名の集合。`Gold` のようなフィールド0個の struct を名前だけで値にするのに使う。
    ///
    /// ponytail: 宣言されたフィールドは検査しない。struct 生成で与えたものが
    /// そのまま入る。型検査を入れるときに突き合わせる
    structs: HashSet<&'a str>,
}

impl<'a> Interp<'a> {
    pub fn new(program: &'a Program) -> Self {
        let mut fns = HashMap::new();
        let mut structs = HashSet::new();
        for item in &program.items {
            match item {
                Item::Fn { sig, body, .. } => {
                    fns.insert(sig.name.as_str(), (sig, body.as_slice()));
                }
                Item::Struct { name, .. } => {
                    structs.insert(name.as_str());
                }
                _ => {}
            }
        }
        Interp { fns, structs }
    }

    /// 名前で関数を呼ぶ。
    ///
    /// **`Env` をここで作り直す。**呼び出し元のローカル束縛は届かない。
    /// 段3で足す `Ambient` はこの境界を越える — それが推移性。
    pub fn call(&self, name: &str, args: Vec<Value>) -> Eval {
        let Some((sig, body)) = self.fns.get(name) else {
            return fail(format!("関数 `{name}` がありません"));
        };
        if sig.params.len() != args.len() {
            return fail(format!(
                "`{name}` は引数 {} 個ですが {} 個渡されました",
                sig.params.len(),
                args.len()
            ));
        }

        let mut env = Env::new();
        for (p, a) in sig.params.iter().zip(args) {
            env.insert(p.name.clone(), a);
        }

        match self.block(body, &mut env) {
            Err(Flow::Return(v)) => Ok(v),
            other => other,
        }
    }

    /// ブロックの値は最後の式(CONTEXT.md「値ベース」)。
    ///
    /// ponytail: ブロックごとに新しいスコープを作らない。`let` は外へ漏れる。
    /// 外の変数への代入が消えないほうを優先した。シャドーイングが要るときに分ける
    fn block(&self, body: &[Expr], env: &mut Env) -> Eval {
        let mut last = Value::Unit;
        for e in body {
            last = self.eval(e, env)?;
        }
        Ok(last)
    }

    fn eval(&self, e: &Expr, env: &mut Env) -> Eval {
        match &e.kind {
            ExprKind::Int(n) => Ok(Value::Int(*n)),
            ExprKind::Str(s) => Ok(Value::Str(s.clone())),
            ExprKind::Bool(b) => Ok(Value::Bool(*b)),

            ExprKind::Ident(name) => match env.get(name) {
                Some(v) => Ok(v.clone()),
                // `Gold` — フィールド0個の struct は名前だけで値になる。
                // ponytail: enum は無い。列挙が要るまでこれで足りる
                None if self.structs.contains(name.as_str()) => Ok(new_obj(name, BTreeMap::new())),
                None => fail(format!("`{name}` が束縛されていません")),
            },

            ExprKind::Let { name, value } => {
                let v = self.eval(value, env)?;
                env.insert(name.clone(), v);
                Ok(Value::Unit)
            }

            ExprKind::StructLit { name, fields } => {
                let mut obj = BTreeMap::new();
                for (k, v) in fields {
                    let v = self.eval(v, env)?;
                    obj.insert(k.clone(), v);
                }
                Ok(new_obj(name, obj))
            }

            ExprKind::Field(recv, name) => {
                let Value::Struct(o) = self.eval(recv, env)? else {
                    return fail(format!("`.{name}` を読めません。struct ではありません"));
                };
                match o.borrow().fields.get(name) {
                    Some(v) => Ok(v.clone()),
                    None => fail(format!("フィールド `{name}` がありません")),
                }
            }

            ExprKind::Assign { target, value } => {
                let v = self.eval(value, env)?;
                match &target.kind {
                    ExprKind::Ident(n) => {
                        env.insert(n.clone(), v);
                    }
                    ExprKind::Field(recv, f) => {
                        let Value::Struct(o) = self.eval(recv, env)? else {
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
                    Some(e) => self.eval(e, env)?,
                    None => Value::Unit,
                };
                Err(Flow::Return(v))
            }

            ExprKind::Assert(inner) => match self.eval(inner, env)? {
                Value::Bool(true) => Ok(Value::Unit),
                Value::Bool(false) => fail("assert が偽になりました"),
                other => fail(format!("assert には bool が必要です ({})", other.show())),
            },

            ExprKind::Unary(UnOp::Neg, inner) => match self.eval(inner, env)? {
                Value::Int(n) => Ok(Value::Int(-n)),
                other => fail(format!("`-` は整数だけです ({})", other.show())),
            },

            ExprKind::Binary { op, lhs, rhs } => self.binary(*op, lhs, rhs, env),

            ExprKind::Block(body) => self.block(body, env),

            ExprKind::Call(callee, args) => {
                let ExprKind::Ident(name) = &callee.kind else {
                    return fail("段2で実装: メソッド呼び出しとパス呼び出し");
                };
                let mut vals = Vec::with_capacity(args.len());
                for a in args {
                    vals.push(self.eval(a, env)?);
                }
                self.call(name, vals)
            }

            ExprKind::Head { head, body, orelse } => {
                self.head(head, body, orelse.as_deref(), env)
            }

            ExprKind::Array(_) => fail("段2で実装: 配列"),
            ExprKind::Path(_) => fail("段2で実装: パス"),
        }
    }

    fn binary(&self, op: BinOp, lhs: &Expr, rhs: &Expr, env: &mut Env) -> Eval {
        // `??` は短絡する。`db.find(id) ?? return false` の右辺は
        // 左辺が nil のときだけ走らないといけない
        if let BinOp::Coalesce = op {
            let l = self.eval(lhs, env)?;
            return if matches!(l, Value::Nil) {
                self.eval(rhs, env)
            } else {
                Ok(l)
            };
        }

        let l = self.eval(lhs, env)?;
        let r = self.eval(rhs, env)?;

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

    fn head(&self, head: &Head, body: &Expr, orelse: Option<&Expr>, env: &mut Env) -> Eval {
        match head {
            Head::If(c) | Head::Elif(c) => {
                if self.cond(c, env)? {
                    self.eval(body, env)
                } else if let Some(o) = orelse {
                    self.eval(o, env)
                } else {
                    Ok(Value::Unit)
                }
            }
            Head::Else => self.eval(body, env),
            Head::While(c) => {
                while self.cond(c, env)? {
                    self.eval(body, env)?;
                }
                Ok(Value::Unit)
            }
            Head::For { .. } => fail("段2で実装: for(配列が要る)"),
            Head::Ambient(_) => fail("段3で実装: ambient の提供"),
        }
    }

    fn cond(&self, c: &Expr, env: &mut Env) -> Result<bool, Flow> {
        match self.eval(c, env)? {
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
        Interp::new(&program).call(entry, vec![]).map_err(|f| match f {
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
        // `Gold` を enum なしで書けるようにする回避。同じ型なら等しい
        let src = "struct Gold {}\n\
                   struct Silver {}\n\
                   fn main() {\n\
                   \x20 assert Gold == Gold\n\
                   \x20 Gold == Silver\n\
                   }\n";
        assert!(matches!(run(src, "main"), Ok(Value::Bool(false))));
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

    #[test]
    fn ambientはまだ段3() {
        let src = "effect db: Database\n\
                   fn main() {\n\
                   \x20 db(pg): {\n\
                   \x20   1\n\
                   \x20 }\n\
                   }\n";
        assert!(run(src, "main").is_err());
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
}
