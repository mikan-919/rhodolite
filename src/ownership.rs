//! 所有権解析。型検査を通った HIR に「誰がいつ値を持っているか」を付ける段。
//!
//! 型検査は「どんな値か」を決めるが、「その値の所有がどこへ動いたか」は決めない。
//! ここが決めるのは4つ:
//!
//!   1. スコープ  … 局所束縛がどの字句スコープに属するか(design.md 決定8)
//!   2. CFG       … 本体の制御の流れを点と辺で表した**解析用の**表現(決定5)
//!   3. アクセス  … 各場所式が Copy 読み・共有借用・排他借用・move のどれか
//!   4. 破棄      … どの辺でどの所有値が落ちるか(決定8)
//!
//! CFG は解析データであって第二の下ろしではない。評価器が読むのは今までどおり
//! 構造化された HIR で、ここが作る点と辺は `ExprId` へ鍵で戻るだけ。
//!
//! # スコープは括弧の数ではない
//!
//! Rhodolite のブロックは一様な字句スコープではない。第二級ブロック
//! (`{ ... }`)の中の `let` は外へ漏れる — これは型検査の `Locals` が
//! ブロックで複製されないことで既に決まっている既存の規則で、所有権解析が
//! 変えてよいものではない(決定8)。逆に `if`/`else`/`while`/`for`/`with`/
//! match の arm と guard は型検査が `locals.clone()` する境界で、そこだけが
//! 新しいスコープになる。この対応は `scopes` の作り方1箇所に閉じている。
//!
//! # まだやらないこと
//!
//! 借用の生存区間(非字句リージョン)・戻り値 provenance・借用の衝突検査は
//! phase 4 の仕事。ここでは「参照を束縛したら、その束縛が生きている間は
//! 借用元も借りられている」という字句的な上界だけを持ち、`move` との衝突を
//! 見るのに使う。

use crate::diag::Diag;
use crate::hir::{self, AccessMode, Id as _, ReceiverMode};
use crate::lex::Span;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

// ---------------------------------------------------------------------------
// 計画の語彙
// ---------------------------------------------------------------------------

/// 本体1つの中で一意な字句スコープ。
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct ScopeId(u32);

/// 本体1つの CFG の中で一意な点。
#[derive(Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct PointId(u32);

impl ScopeId {
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

impl PointId {
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

/// 所有権が動く単位。局所束縛の根と、そこからの射影の列(design.md 決定5)。
///
/// 射影しても根の同一性は変わらない。フィールドを消費すると struct 全体が
/// 消費されるので、部分的に move された状態は表現できない(tasks 3.5)。
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Place {
    pub root: hir::LocalId,
    pub path: Vec<Projection>,
}

/// 場所の射影1段。
///
/// ponytail: phase 3 が作るのは `Field` だけ。optional の中身・enum payload・
/// 配列要素の射影は `match`/`for`/`??` の所有モードと一緒に phase 6 が入れる。
/// 変種をここに書いておくのは、phase 4 の重なり判定がこの形を前提に書かれる
/// ため(design.md 決定5)。
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Projection {
    Field(hir::FieldId),
    OptionalPayload,
    EnumPayload(hir::VariantId, usize),
    /// 定数添字が分かっていれば `Some`。分からない添字は容器全体と重なる
    ArrayElement(Option<i64>),
}

/// 場所へのアクセスの分類(tasks 3.1)。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    /// Copy な値の読み。所有も借用も動かない
    Read,
    /// `&place`、および借用を受ける位置への自動共有借用
    Shared,
    /// `&mut place`、フィールド代入のレシーバ、`&mut self` レシーバ
    Mutable,
    /// 所有の移動。根ごと消費する
    Move,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Access {
    pub place: Place,
    pub mode: Mode,
}

/// CFG の点1つが起こすこと。
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Effect {
    Access(Access),
    /// 局所束縛が所有を得る(引数・`self`・`let`・ループ変数・payload 束縛)
    Init(hir::LocalId),
    /// 初期化済みの局所束縛への再代入。古い値はここで落ちる。
    /// ponytail: 上書きの破棄はこの効果そのものが表すので、辺には別の
    /// `Drop` を積まない。破棄経路を全部並べ切るのは phase 6(6.6)
    Assign(hir::LocalId),
    /// 分岐・合流・入口・出口
    Nop,
}

/// CFG の点。`expr` で HIR へ戻る(合流点など、式に対応しない点は `None`)。
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Point {
    pub expr: Option<hir::ExprId>,
    pub scope: ScopeId,
    pub effect: Effect,
}

/// 破棄1件。計画の出力であって、構造ではない(design.md 決定8)。
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Drop {
    /// スコープを抜ける所有束縛
    Local(hir::LocalId),
    /// フィールドを消費した根の、残り全部(tasks 3.5)。`consumed` は消費した
    /// **経路**なので、`move u.inner.name` でも「`u` から `inner.name` を除いた
    /// 残り」を一意に指せる。最後の1段だけを持つと `inner` を丸ごと落として
    /// しまい、既に持ち出された `name` を二度落とすことになる
    Remaining {
        root: hir::LocalId,
        consumed: Vec<Projection>,
    },
}

/// 制御の辺。`exits` は抜ける字句スコープ、`drops` は解析が確定した破棄。
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Edge {
    pub from: PointId,
    pub to: PointId,
    pub exits: Vec<ScopeId>,
    pub drops: Vec<Drop>,
}

/// 字句スコープ1つ。`locals` は宣言順で、破棄はこの逆順。
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Scope {
    pub parent: Option<ScopeId>,
    pub locals: Vec<hir::LocalId>,
}

/// 本体1つ分の計画。
#[derive(Debug, Default)]
pub struct BodyPlan {
    scopes: Vec<Scope>,
    points: Vec<Point>,
    edges: Vec<Edge>,
    /// 場所式 → その点。名前で引き直さずに分類を取れる
    accesses: BTreeMap<hir::ExprId, PointId>,
    /// 破棄の対象になる所有束縛。借用の束縛とループ変数は入らない
    owners: BTreeSet<hir::LocalId>,
    entry: PointId,
    exit: PointId,
}

impl BodyPlan {
    pub fn scopes(&self) -> impl Iterator<Item = (ScopeId, &Scope)> {
        self.scopes
            .iter()
            .enumerate()
            .map(|(i, s)| (ScopeId(i as u32), s))
    }

    pub fn points(&self) -> impl Iterator<Item = (PointId, &Point)> {
        self.points
            .iter()
            .enumerate()
            .map(|(i, p)| (PointId(i as u32), p))
    }

    pub fn edges(&self) -> &[Edge] {
        &self.edges
    }

    /// この式が場所へのアクセスなら、その分類。
    pub fn access(&self, expr: hir::ExprId) -> Option<&Access> {
        let point = *self.accesses.get(&expr)?;
        match &self.points[point.index()].effect {
            Effect::Access(access) => Some(access),
            _ => None,
        }
    }
}

/// 本体ごとの計画。並びは宣言順。
#[derive(Debug, Default)]
pub struct Plan {
    bodies: BTreeMap<hir::BodyId, BodyPlan>,
}

impl Plan {
    pub fn body(&self, id: hir::BodyId) -> &BodyPlan {
        self.bodies.get(&id).expect("全ての本体に計画がある")
    }
}

/// 所有権検査を通ったプログラム(design.md 決定3)。
///
/// これを作れるのは `check` だけなので、この型を受け取る段は「全てのアクセスに
/// 所有モードが付いている」と仮定してよい。
pub struct CheckedProgram {
    pub hir: hir::Program,
    pub plan: Plan,
}

/// 型検査を通った HIR の所有権を閉じる。
///
/// 本体は宣言順に1つずつ独立に見る。この段は本体をまたがない(呼び出し先の
/// 戻り値 provenance は phase 4)。
pub fn check(hir: hir::Program) -> Result<CheckedProgram, Vec<Diag>> {
    let mut plan = Plan::default();
    let mut diagnostics = Vec::new();
    for id in &hir.bodies {
        let (body_plan, mut found) = analyze(&hir, *id);
        diagnostics.append(&mut found);
        plan.bodies.insert(*id, body_plan);
    }
    if diagnostics.is_empty() {
        Ok(CheckedProgram { hir, plan })
    } else {
        Err(diagnostics)
    }
}

// ---------------------------------------------------------------------------
// CFG の構築
// ---------------------------------------------------------------------------

/// 束縛の出自。診断の直し方と、借用で束ねた名前の見分けに使う。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Bound {
    /// `let` / `let mut`
    Let,
    /// 引数と `self`
    Param,
    /// 素の `for` のループ変数と素の match の payload。対象を借用しているので
    /// 所有者ではない(design.md 決定7)
    Borrowed,
}

/// 値を要求している側が何を要るか。場所式の分類はここで決まる。
#[derive(Clone, Copy)]
enum Need {
    /// 読むだけ。非 Copy なら自動の共有借用
    Read,
    /// 排他アクセス。フィールド代入のレシーバと `&mut self`
    Mutate,
    /// 所有が要る。束縛・戻り値・構築は暗黙に move してよい
    Take,
    /// 所有が要る呼び出しの実引数。束縛済みの非 Copy には `move` の明示が要る
    /// (index が `None` ならレシーバ)
    Argument(&'static str, Option<usize>),
}

struct Build<'a> {
    program: &'a hir::Program,
    body: &'a hir::Body,
    ctx: String,
    plan: BodyPlan,
    /// いま制御が居る点。抜けたら `None`
    cur: Option<PointId>,
    /// その点の後に走る破棄。フィールド消費の残余だけがここに入る
    residue: BTreeMap<PointId, Vec<Drop>>,
    /// `let v = &u` の記録。v が生きている間 u は借りられている
    borrows: BTreeMap<hir::LocalId, (hir::LocalId, Span)>,
    /// 名前で参照できる束縛とその出自。ここに無いのは「知らない名前への代入」が
    /// 作った書き込み専用の束縛で、代入は初期化として扱う
    bindings: BTreeMap<hir::LocalId, Bound>,
    /// いま解決中の呼び出し先の表示名(`Need::Argument` の診断に使う)
    callee: Option<String>,
    /// 所有を要求されたのに `move` が書かれていなかった場所式と、その位置の綴り
    demanded: BTreeMap<hir::ExprId, (String, &'static str, Option<usize>)>,
    diagnostics: Vec<Diag>,
}

impl<'a> Build<'a> {
    fn new(program: &'a hir::Program, id: hir::BodyId) -> Self {
        Build {
            program,
            body: program.body(id),
            ctx: program.show_body(id),
            plan: BodyPlan::default(),
            cur: None,
            residue: BTreeMap::new(),
            borrows: BTreeMap::new(),
            bindings: BTreeMap::new(),
            callee: None,
            demanded: BTreeMap::new(),
            diagnostics: Vec::new(),
        }
    }

    // ---- 骨組み ----

    fn scope(&mut self, parent: Option<ScopeId>) -> ScopeId {
        let id = ScopeId(self.plan.scopes.len() as u32);
        self.plan.scopes.push(Scope {
            parent,
            locals: Vec::new(),
        });
        id
    }

    /// 点を確保するだけ。制御は繋がない
    fn alloc(&mut self, expr: Option<hir::ExprId>, scope: ScopeId, effect: Effect) -> PointId {
        let id = PointId(self.plan.points.len() as u32);
        self.plan.points.push(Point {
            expr,
            scope,
            effect,
        });
        id
    }

    /// 点を確保して、いまの制御から繋ぐ。
    ///
    /// 制御が既に抜けている(`cur` が `None`)ところでも点は作り、そこから先は
    /// 繋ぎ直す。到達しない領域にも完全な計画を残すためで、`accesses` は
    /// 「全ての場所式に分類が付く」を保てる(tasks 3.1)。入口から辿れない
    /// 領域なので、解析はその点を作業キューへ入れない
    fn point(&mut self, expr: Option<hir::ExprId>, scope: ScopeId, effect: Effect) -> PointId {
        let id = self.alloc(expr, scope, effect);
        if let Some(prev) = self.cur {
            self.edge(prev, id, Vec::new());
        }
        self.cur = Some(id);
        id
    }

    fn edge(&mut self, from: PointId, to: PointId, exits: Vec<ScopeId>) {
        let drops = self.residue.get(&from).cloned().unwrap_or_default();
        self.plan.edges.push(Edge {
            from,
            to,
            exits,
            drops,
        });
    }

    /// スコープ `from` から `to`(排他)までの、抜ける順のスコープ列。
    /// `to` が `None` なら根まで全部
    fn open(&self, from: ScopeId, to: Option<ScopeId>) -> Vec<ScopeId> {
        let mut exits = Vec::new();
        let mut cursor = Some(from);
        while let Some(id) = cursor.filter(|id| Some(*id) != to) {
            exits.push(id);
            cursor = self.plan.scopes[id.index()].parent;
        }
        exits
    }

    /// 局所束縛をスコープへ登録する。`Bound::Borrowed` は借用なので所有者に
    /// ならず、破棄もされない
    fn declare(&mut self, local: hir::LocalId, scope: ScopeId, bound: Bound) {
        self.plan.scopes[scope.index()].locals.push(local);
        self.bindings.insert(local, bound);
        if bound != Bound::Borrowed && self.owning(self.body.local(local).ty.as_ref()) {
            self.plan.owners.insert(local);
        }
    }

    /// 破棄の対象になる型か。参照は所有していないので落ちない。Copy な値は
    /// 落とすものが無いので計画に出さない
    fn owning(&self, ty: Option<&hir::Type>) -> bool {
        ty.is_some_and(|ty| ty.reference.is_none() && !self.program.is_copy(ty))
    }

    // ---- 場所 ----

    /// 場所として書ける式ならその場所。呼び出しの結果のような一時値は `None`
    fn place_of(&self, id: hir::ExprId) -> Option<Place> {
        match &self.body.expr(id).kind {
            hir::ExprKind::Local(local) => Some(Place {
                root: *local,
                path: Vec::new(),
            }),
            hir::ExprKind::Field {
                recv,
                field,
                optional: false,
            } => {
                let mut place = self.place_of(*recv)?;
                place.path.push(Projection::Field(*field));
                Some(place)
            }
            // ponytail: `.?` は optional の中身への射影。`OptionalPayload` を
            // 作るのは `??`/`match` の所有モードと同じ phase 6
            _ => None,
        }
    }

    /// 診断に出す綴り。ソースの見た目に寄せるので ID は出さない
    fn show_place(&self, place: &Place) -> String {
        format!(
            "{}{}",
            self.body.local(place.root).name,
            show_path(self.program, &place.path)
        )
    }

    fn access(&mut self, expr: hir::ExprId, place: Place, mode: Mode, scope: ScopeId) {
        // 射影を消費したら根ごと消費され、残りはその場で落ちる
        let residue = (mode == Mode::Move && !place.path.is_empty()).then(|| Drop::Remaining {
            root: place.root,
            consumed: place.path.clone(),
        });
        let point = self.point(Some(expr), scope, Effect::Access(Access { place, mode }));
        self.plan.accesses.insert(expr, point);
        if let Some(residue) = residue {
            self.residue.insert(point, vec![residue]);
        }
    }

    // ---- 走査 ----

    fn value(&mut self, id: hir::ExprId, scope: ScopeId, need: Need) {
        if let hir::ExprKind::Access { mode, place } = &self.body.expr(id).kind {
            let (mode, place) = (*mode, *place);
            let Some(resolved) = self.place_of(place) else {
                // `&f()` のように場所でないものを修飾している。所有権解析は
                // 場所にだけ効くので、中身を一時値として評価するだけ。
                // ponytail: 場所でない借用の拒否は phase 4(4.3)
                return self.value(place, scope, Need::Take);
            };
            let mode = match mode {
                AccessMode::Shared => Mode::Shared,
                AccessMode::Mutable => Mode::Mutable,
                AccessMode::Move => Mode::Move,
            };
            return self.access(id, resolved, mode, scope);
        }
        if let Some(place) = self.place_of(id) {
            return self.bare_place(id, place, scope, need);
        }
        self.compound(id, scope, need);
    }

    /// 修飾の付いていない場所。何を要求されているかで分類が決まる
    fn bare_place(&mut self, id: hir::ExprId, place: Place, scope: ScopeId, need: Need) {
        let ty = self.body.expr(id).result.ty();
        let copy = ty.is_some_and(|ty| self.program.is_copy(ty));
        let reborrow = ty.and_then(|ty| ty.reference);
        let mode = match need {
            Need::Mutate => Mode::Mutable,
            // Copy は暗黙に複製してよい。`&T` もここに入る
            _ if copy => Mode::Read,
            Need::Read => Mode::Shared,
            // 参照そのものを渡すのは再借用。所有は動かない
            // ponytail: 再借用の制約(元の借用より長く生きないこと)は phase 4(4.1)
            _ if reborrow == Some(hir::RefKind::Mutable) => Mode::Mutable,
            Need::Take => Mode::Move,
            // 診断はここでは出さない。既に move 済みの値には別の診断が付くので、
            // データフローの結果を見てから `report_access` が判断する
            Need::Argument(what, index) => {
                self.demanded
                    .insert(id, (self.callee.clone().unwrap_or_default(), what, index));
                Mode::Move
            }
        };
        self.access(id, place, mode, scope);
    }

    fn compound(&mut self, id: hir::ExprId, scope: ScopeId, need: Need) {
        let body = self.body;
        let program = self.program;
        match &body.expr(id).kind {
            hir::ExprKind::Int(_)
            | hir::ExprKind::Str(_)
            | hir::ExprKind::Bool(_)
            | hir::ExprKind::Nil
            | hir::ExprKind::UnitStruct(_)
            | hir::ExprKind::Variant(_)
            | hir::ExprKind::Local(_)
            | hir::ExprKind::Access { .. }
            | hir::ExprKind::Poison => {}

            // 場所として畳めなかった読み(一時値のフィールド、`.?`)
            hir::ExprKind::Field { recv, .. } => self.value(*recv, scope, Need::Read),

            // ponytail: 構築(struct リテラル・配列リテラル・enum 構築・`with` の
            // 提供値)は所有を暗黙に受け取る。仕様が `move` を要求しているのは
            // 「所有の引数」と「消費レシーバ」だけなので、構築位置は4つとも
            // 揃えて暗黙にしてある。構築にも要求するなら phase 6(6.1/6.3/6.4)
            // と phase 5(5.4)で一緒に変える
            hir::ExprKind::StructLit { fields, .. } => {
                for (_, value) in fields {
                    self.value(*value, scope, Need::Take);
                }
            }
            hir::ExprKind::Array(items) => {
                for item in items {
                    self.value(*item, scope, Need::Take);
                }
            }

            hir::ExprKind::Let { local, value } => {
                self.value(*value, scope, Need::Take);
                // 参照を束縛したら、その束縛が生きている間は借用元も借りられて
                // いる(効かせるのは `report_access`)
                if let hir::ExprKind::Access {
                    mode: AccessMode::Shared | AccessMode::Mutable,
                    place,
                } = &body.expr(*value).kind
                    && let Some(source) = self.place_of(*place)
                {
                    self.borrows
                        .insert(*local, (source.root, body.expr(*value).span));
                }
                self.declare(*local, scope, Bound::Let);
                self.point(Some(id), scope, Effect::Init(*local));
            }

            hir::ExprKind::AssignLocal { local, value } => {
                self.value(*value, scope, Need::Take);
                self.point(Some(id), scope, Effect::Assign(*local));
            }

            // 評価順は型検査と同じくレシーバ → 値。書き込む場所はフィールドまで
            hir::ExprKind::AssignField { recv, field, value } => {
                match self.place_of(*recv) {
                    Some(mut place) => {
                        place.path.push(Projection::Field(*field));
                        self.access(id, place, Mode::Mutable, scope);
                    }
                    None => self.value(*recv, scope, Need::Read),
                }
                self.value(*value, scope, Need::Take);
            }

            hir::ExprKind::Neg(inner) | hir::ExprKind::Assert(inner) => {
                self.value(*inner, scope, Need::Read)
            }
            hir::ExprKind::Arith { lhs, rhs, .. } | hir::ExprKind::Eq { lhs, rhs } => {
                self.value(*lhs, scope, Need::Read);
                self.value(*rhs, scope, Need::Read);
            }

            // 右辺は左辺が `nil` のときだけ走る。
            // ponytail: `??` の所有モード(借用・複製・消費)は phase 6(6.2)
            hir::ExprKind::Coalesce { lhs, rhs } => {
                self.value(*lhs, scope, Need::Read);
                let branch = self.point(Some(id), scope, Effect::Nop);
                self.value(*rhs, scope, need);
                let fallback = self.cur;
                let join = self.alloc(Some(id), scope, Effect::Nop);
                self.edge(branch, join, Vec::new());
                if let Some(end) = fallback.filter(|end| *end != branch) {
                    self.edge(end, join, Vec::new());
                }
                self.cur = Some(join);
            }

            hir::ExprKind::Return(value) => {
                if let Some(value) = value {
                    // 戻り先が所有を宣言しているので、返す move は自動
                    self.value(*value, scope, Need::Take);
                }
                if let Some(from) = self.cur {
                    let exits = self.open(scope, None);
                    let exit = self.plan.exit;
                    self.edge(from, exit, exits);
                }
                self.cur = None;
            }

            // 第二級ブロック。周りのスコープをそのまま使う(design.md 決定8)
            hir::ExprKind::Block(ids) => {
                let Some((last, init)) = ids.split_last() else {
                    return;
                };
                for e in init {
                    self.value(*e, scope, Need::Read);
                }
                self.value(*last, scope, need);
            }

            hir::ExprKind::If { cond, then, orelse } => {
                self.value(*cond, scope, Need::Read);
                let branch = self.point(Some(id), scope, Effect::Nop);
                let taken_scope = self.scope(Some(scope));
                self.cur = Some(branch);
                self.value(*then, taken_scope, need);
                let taken = self.cur;
                let (other, other_scope) = match orelse {
                    Some(orelse) => {
                        let other_scope = self.scope(Some(scope));
                        self.cur = Some(branch);
                        self.value(*orelse, other_scope, need);
                        (self.cur, Some(other_scope))
                    }
                    None => (Some(branch), None),
                };
                if taken.is_none() && other.is_none() {
                    self.cur = None;
                    return;
                }
                let join = self.alloc(Some(id), scope, Effect::Nop);
                if let Some(from) = taken {
                    self.edge(from, join, vec![taken_scope]);
                }
                if let Some(from) = other {
                    self.edge(from, join, other_scope.into_iter().collect());
                }
                self.cur = Some(join);
            }

            hir::ExprKind::While { cond, body: inner } => {
                let head = self.point(Some(id), scope, Effect::Nop);
                self.value(*cond, scope, Need::Read);
                let test = self.cur;
                let after = self.alloc(Some(id), scope, Effect::Nop);
                if let Some(test) = test {
                    let inner_scope = self.scope(Some(scope));
                    self.cur = Some(test);
                    self.value(*inner, inner_scope, Need::Read);
                    if let Some(from) = self.cur {
                        self.edge(from, head, vec![inner_scope]);
                    }
                    self.edge(test, after, Vec::new());
                }
                self.cur = Some(after);
            }

            // ponytail: 素の `for` は配列を借用し、要素も借用で束縛する
            // (design.md 決定7)。`&mut`/`move` の反復は phase 6(6.4)
            hir::ExprKind::For {
                var,
                iter,
                body: inner,
            } => {
                self.value(*iter, scope, Need::Read);
                let head = self.point(Some(id), scope, Effect::Nop);
                let after = self.alloc(Some(id), scope, Effect::Nop);
                let inner_scope = self.scope(Some(scope));
                self.declare(*var, inner_scope, Bound::Borrowed);
                self.cur = Some(head);
                self.point(Some(id), inner_scope, Effect::Init(*var));
                self.value(*inner, inner_scope, Need::Read);
                if let Some(from) = self.cur {
                    self.edge(from, head, vec![inner_scope]);
                }
                self.edge(head, after, Vec::new());
                self.cur = Some(after);
            }

            // ponytail: 提供値の所有モード(共有・排他・move・一時所有)は
            // phase 5(5.4)。ここでは提供のスコープだけを作る
            hir::ExprKind::With {
                provisions,
                body: inner,
            } => {
                for provision in provisions {
                    if let Some(value) = provision.value {
                        self.value(value, scope, Need::Take);
                    }
                }
                let inner_scope = self.scope(Some(scope));
                self.value(*inner, inner_scope, need);
                if let Some(from) = self.cur {
                    let join = self.alloc(Some(id), scope, Effect::Nop);
                    self.edge(from, join, vec![inner_scope]);
                    self.cur = Some(join);
                }
            }

            // arm は上から順に試される。pattern が外れても guard が偽でも次の
            // arm へ進むので、その連鎖をそのまま辺にする。畳んで「合流点へ抜ける」
            // にすると、guard が動かした所有が後続 arm に見えなくなる。
            //
            // ponytail: 素の `match` は対象全体を借用し、payload も借用で
            // 束縛する(design.md 決定7)。`&mut`/`move` は phase 6(6.3)
            hir::ExprKind::Match { subject, arms } => {
                self.value(*subject, scope, Need::Read);
                let branch = self.point(Some(id), scope, Effect::Nop);
                // 「ここまでの arm がどれも取らなかった」点
                let mut fallthrough = branch;
                let mut ends: Vec<(PointId, Vec<ScopeId>)> = Vec::new();
                for arm in arms {
                    let arm_scope = self.scope(Some(scope));
                    let next = self.alloc(Some(id), scope, Effect::Nop);
                    // pattern が合わなければ、この arm には入らずに次を試す
                    self.edge(fallthrough, next, Vec::new());
                    self.cur = Some(fallthrough);
                    if let hir::Pattern::Variant { bindings, .. } = &arm.pattern {
                        for bound in bindings.iter().flatten() {
                            self.declare(*bound, arm_scope, Bound::Borrowed);
                            self.point(Some(id), arm_scope, Effect::Init(*bound));
                        }
                    }
                    if let Some(guard) = arm.guard {
                        self.value(guard, arm_scope, Need::Read);
                        // guard が偽なら、この arm のスコープを抜けて次を試す。
                        // guard も pattern 束縛も点を作らなかったなら、上で張った
                        // pattern 不一致の辺と同じものになるので張り直さない
                        if let Some(tested) = self.cur.filter(|at| *at != fallthrough) {
                            self.edge(tested, next, vec![arm_scope]);
                        }
                    }
                    self.value(arm.body, arm_scope, need);
                    if let Some(from) = self.cur {
                        ends.push((from, vec![arm_scope]));
                    }
                    fallthrough = next;
                }
                // 全ての arm に guard が付いていると、どれも取らずに抜ける経路が
                // 実在する。網羅していれば必ずどれかが取るので、その経路は無い
                if !arms.is_empty() && arms.iter().all(|arm| arm.guard.is_some()) {
                    ends.push((fallthrough, Vec::new()));
                }
                if ends.is_empty() {
                    self.cur = None;
                    return;
                }
                let join = self.alloc(Some(id), scope, Effect::Nop);
                for (from, exits) in ends {
                    self.edge(from, join, exits);
                }
                self.cur = Some(join);
            }

            hir::ExprKind::Call(call) => match call {
                hir::Call::Direct { callable, args } | hir::Call::Associated { callable, args } => {
                    self.callee = Some(program.show_callable(*callable));
                    let params = self.declared_params(*callable);
                    self.arguments(scope, &params, args);
                }
                hir::Call::Method {
                    callable,
                    recv,
                    args,
                } => {
                    self.callee = Some(program.show_callable(*callable));
                    let need = match program.callables[*callable].receiver {
                        Some(ReceiverMode::Owned) => Need::Argument("レシーバ", None),
                        Some(ReceiverMode::Mutable) => Need::Mutate,
                        _ => Need::Read,
                    };
                    self.value(*recv, scope, need);
                    let params = self.declared_params(*callable);
                    self.callee = Some(program.show_callable(*callable));
                    self.arguments(scope, &params, args);
                }
                hir::Call::Slot { method, args, .. } => {
                    self.callee = Some(program.show_trait_method(*method));
                    let params: Vec<Option<hir::Type>> = program.trait_methods[*method]
                        .params
                        .iter()
                        .map(|ty| Some(ty.clone()))
                        .collect();
                    self.arguments(scope, &params, args);
                }
                // enum の構築。呼び出しの綴りだが受け取るのは payload の所有で、
                // struct リテラルや配列リテラルと同じ構築位置なので暗黙 move
                hir::Call::Ctor { args, .. } => {
                    for arg in args {
                        self.value(*arg, scope, Need::Take);
                    }
                }
            },
        }
    }

    /// 呼び出し先の宣言引数型。引数の局所束縛は呼び出し**先**の本体にある
    fn declared_params(&self, callable: hir::CallableId) -> Vec<Option<hir::Type>> {
        let callee = &self.program.callables[callable];
        callee
            .params
            .iter()
            .map(|local| callee.body.local(*local).ty.clone())
            .collect()
    }

    fn arguments(&mut self, scope: ScopeId, params: &[Option<hir::Type>], args: &[hir::ExprId]) {
        let callee = self.callee.take();
        for (index, arg) in args.iter().enumerate() {
            let need = match params.get(index).and_then(|ty| ty.as_ref()?.reference) {
                // 排他借用を受け取る引数は排他アクセス。`&T` の位置なら読むだけ
                Some(hir::RefKind::Mutable) => Need::Mutate,
                Some(hir::RefKind::Shared) => Need::Read,
                None => Need::Argument("引数", Some(index)),
            };
            self.callee.clone_from(&callee);
            self.value(*arg, scope, need);
        }
        self.callee = None;
    }
}

// ---------------------------------------------------------------------------
// 初期化と move のデータフロー
// ---------------------------------------------------------------------------

/// 1つの局所束縛の状態。2つの may 情報の組なので、合流は or で閉じる。
///
/// `owned=false, moved=None` は「まだ・もう居ない」(スコープの外)。
/// `owned=true, moved=Some` が「経路によっては move 済み」。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
struct Init {
    owned: bool,
    /// move した地点。複数あるときは決定的に最小の span
    moved: Option<Span>,
}

type Facts = BTreeMap<hir::LocalId, Init>;

/// 合流。単調で高さが有限なので、繰り返しは必ず止まる
fn join(into: &mut Option<Facts>, next: Facts) -> bool {
    let Some(current) = into else {
        *into = Some(next);
        return true;
    };
    let mut changed = false;
    for (local, fact) in next {
        let slot = current.entry(local).or_default();
        let before = *slot;
        slot.owned |= fact.owned;
        slot.moved = match (slot.moved, fact.moved) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        changed |= *slot != before;
    }
    changed
}

impl Build<'_> {
    fn apply(&self, mut facts: Facts, point: &Point) -> Facts {
        match &point.effect {
            Effect::Init(local) | Effect::Assign(local) => {
                facts.insert(
                    *local,
                    Init {
                        owned: true,
                        moved: None,
                    },
                );
            }
            Effect::Access(access) if access.mode == Mode::Move => {
                facts.insert(
                    access.place.root,
                    Init {
                        owned: false,
                        moved: point.expr.map(|e| self.body.expr(e).span),
                    },
                );
            }
            Effect::Access(_) | Effect::Nop => {}
        }
        facts
    }

    /// 決定的な作業キューで不動点まで回す。
    ///
    /// `requirement::analyze` と同じく「増える一方の情報を、変化が無くなるまで
    /// 流す」形。違いは辺を持っているので全点を舐め直す必要が無いことで、
    /// 次に見る点は `BTreeSet` の最小要素 = 点 ID 順に固定される。
    fn solve(&self) -> Vec<Option<Facts>> {
        let mut outgoing: Vec<Vec<usize>> = vec![Vec::new(); self.plan.points.len()];
        for (index, edge) in self.plan.edges.iter().enumerate() {
            outgoing[edge.from.index()].push(index);
        }
        let mut input: Vec<Option<Facts>> = vec![None; self.plan.points.len()];
        input[self.plan.entry.index()] = Some(Facts::new());
        let mut queue: BTreeSet<PointId> = BTreeSet::from([self.plan.entry]);
        while let Some(point) = queue.pop_first() {
            let Some(facts) = input[point.index()].clone() else {
                continue;
            };
            let output = self.apply(facts, &self.plan.points[point.index()]);
            for index in &outgoing[point.index()] {
                let edge = &self.plan.edges[*index];
                let mut next = output.clone();
                // 抜けたスコープの束縛はもう居ない。借用もここで終わる
                for scope in &edge.exits {
                    for local in &self.plan.scopes[scope.index()].locals {
                        next.remove(local);
                    }
                }
                let to = edge.to;
                if join(&mut input[to.index()], next) {
                    queue.insert(to);
                }
            }
        }
        input
    }

    /// 可変に触れる場所か。`let mut` の束縛か、既に `&mut` である参照だけ
    fn mutable_root(&self, root: hir::LocalId) -> bool {
        let local = self.body.local(root);
        local.mutable
            || local
                .ty
                .as_ref()
                .and_then(|ty| ty.reference)
                .is_some_and(|kind| kind == hir::RefKind::Mutable)
    }

    /// 全ての診断はここから出る。点 ID 順 = 評価順なので、並びは決定的で、
    /// 型検査が診断を積む順(走査順)と同じ約束になる
    fn report(&mut self, input: &[Option<Facts>]) {
        let mut found = Vec::new();
        for (facts, point) in input.iter().zip(&self.plan.points) {
            // 到達しない点。状態が無いので何も言えない
            let Some(facts) = facts else { continue };
            let span = point.expr.map(|e| self.body.expr(e).span);
            match &point.effect {
                Effect::Access(access) => {
                    self.report_access(&mut found, facts, access, point.expr, span)
                }
                Effect::Assign(local) => self.report_assign(&mut found, facts, *local, span),
                Effect::Init(_) | Effect::Nop => {}
            }
        }
        self.diagnostics.extend(found);
    }

    fn report_access(
        &self,
        found: &mut Vec<Diag>,
        facts: &Facts,
        access: &Access,
        expr: Option<hir::ExprId>,
        span: Option<Span>,
    ) {
        let ctx = &self.ctx;
        let root = access.place.root;
        let state = facts.get(&root).copied().unwrap_or_default();
        let shown = self.show_place(&access.place);
        // 所有の引数に `move` が書かれていない。既に move 済みならそちらの診断が
        // 出るので、同じ式に2件は積まない
        if let Some((callee, what, index)) = expr.and_then(|e| self.demanded.get(&e))
            && state.moved.is_none()
        {
            let position = match index {
                Some(index) => format!("第 {} {what}", index + 1),
                None => what.to_string(),
            };
            let local = self.body.local(root);
            found.push(
                Diag::from_span(
                    span,
                    format!(
                        "{ctx}: `{callee}` の{position}は所有を受け取りますが、束縛済みの `{shown}` をそのまま渡しています"
                    ),
                )
                .label("所有が渡りません")
                .help(format!(
                    "`move {shown}` と書いて移動を見えるようにしてください"
                ))
                .related(vec![Diag::at(
                    local.span,
                    format!("`{}` はここで束縛されています", local.name),
                )]),
            );
        }
        if let Some(moved) = state.moved {
            let diagnostic = if Some(moved) == span {
                // 自分自身が付けた move が入口に居る = 背辺から戻ってきている
                Diag::from_span(
                    span,
                    format!("{ctx}: `{shown}` はループの2周目には move 済みです"),
                )
                .label("毎周 move している")
                .help("ループの外で move するか、周ごとに新しい値を束縛してください")
                .related(vec![Diag::at(
                    self.body.local(root).span,
                    format!("`{}` はここで束縛されています", self.body.local(root).name),
                )])
            } else if state.owned {
                Diag::from_span(
                    span,
                    format!("{ctx}: `{shown}` は move される経路があるのでここでは使えません"),
                )
                .label("経路によっては move 済み")
                .related(vec![Diag::at(moved, "この経路でここが move します")])
            } else {
                Diag::from_span(
                    span,
                    format!("{ctx}: `{shown}` は既に move されているので使えません"),
                )
                .label("move 済みの値")
                .related(vec![Diag::at(moved, "ここで move されました")])
            };
            found.push(diagnostic);
        }
        // 素の `for`/`match` は対象を借用するので、要素や payload の所有は
        // 取り出せない。部分 move は受け付けない(design.md 決定7)
        if access.mode == Mode::Move && self.bindings.get(&root) == Some(&Bound::Borrowed) {
            let local = self.body.local(root);
            found.push(
                Diag::from_span(
                    span,
                    format!("{ctx}: `{shown}` は借用で束ねた名前なので move できません"),
                )
                .label("借用された束縛の move")
                .help("素の `for` と `match` は対象を借用します。要素や payload だけを取り出すことはできません")
                .related(vec![Diag::at(
                    local.span,
                    format!("`{}` はここで束ねられています", local.name),
                )]),
            );
        }
        if access.mode == Mode::Move {
            // 借用されたままの場所は move できない。
            // ponytail: いま loan を作るのは `let v = &place` と直接書いた形だけで、
            // 生存区間も束縛のスコープで上から抑えている。全ての借用に loan を
            // 作って最終使用まで縮めるのは phase 4(4.1/4.3)
            for (borrower, (source, borrowed_at)) in &self.borrows {
                if *source == root && facts.get(borrower).is_some_and(|fact| fact.owned) {
                    found.push(
                        Diag::from_span(
                            span,
                            format!("{ctx}: `{shown}` は借用されているので move できません"),
                        )
                        .label("借用中の値の move")
                        .related(vec![Diag::at(
                            *borrowed_at,
                            format!(
                                "`{}` がここで借用しています",
                                self.body.local(*borrower).name
                            ),
                        )]),
                    );
                    break;
                }
            }
        }
        if access.mode == Mode::Mutable && !self.mutable_root(root) {
            let local = self.body.local(root);
            let shared = local
                .ty
                .as_ref()
                .and_then(|ty| ty.reference)
                .is_some_and(|kind| kind == hir::RefKind::Shared);
            let diagnostic = if shared {
                Diag::from_span(
                    span,
                    format!("{ctx}: `{shown}` は共有借用を通しているので変更できません"),
                )
                .label("共有借用への書き込み")
            } else {
                let base = Diag::from_span(
                    span,
                    format!("{ctx}: `{shown}` は可変な束縛ではないので変更できません"),
                )
                .label("不変な束縛への書き込み");
                match self.bindings.get(&root) {
                    Some(Bound::Let) => {
                        base.help(format!("`let mut {}` と宣言してください", local.name))
                    }
                    // 引数と `self` に `mut` は書けない。可変にしたいなら署名側
                    Some(Bound::Param) => {
                        base.help(format!("`{}` を `&mut` で受け取ってください", local.name))
                    }
                    _ => base,
                }
            };
            found.push(diagnostic.related(vec![Diag::at(
                local.span,
                format!("`{}` はここで束縛されています", local.name),
            )]));
        }
    }

    fn report_assign(
        &self,
        found: &mut Vec<Diag>,
        facts: &Facts,
        local: hir::LocalId,
        span: Option<Span>,
    ) {
        let state = facts.get(&local).copied().unwrap_or_default();
        let known = self.bindings.contains_key(&local);
        let initialized = state.owned || state.moved.is_some();
        let declaration = self.body.local(local);
        // 知らない名前への代入が作った束縛と、まだ値の無い束縛は初期化。
        // 束縛そのものを差し替えるので、`&mut` を通した変更とは違って
        // 束縛が `let mut` でなければならない
        if !known || !initialized || declaration.mutable {
            return;
        }
        found.push(
            Diag::from_span(
                span,
                format!(
                    "{}: `{}` は不変な束縛なので代入できません",
                    self.ctx, declaration.name
                ),
            )
            .label("不変な束縛への代入")
            .help(format!("`let mut {}` と宣言してください", declaration.name))
            .related(vec![Diag::at(
                declaration.span,
                format!("`{}` はここで束縛されています", declaration.name),
            )]),
        );
    }

    /// 破棄を確定する。計画に残るのは「その辺を通る時点で持っている」ものだけ
    fn settle(&mut self, input: &[Option<Facts>]) {
        let mut settled = Vec::with_capacity(self.plan.edges.len());
        for edge in &self.plan.edges {
            let Some(before) = &input[edge.from.index()] else {
                // 到達しない辺。破棄も起きない
                settled.push(Vec::new());
                continue;
            };
            let after = self.apply(before.clone(), &self.plan.points[edge.from.index()]);
            let owns = |facts: &Facts, local: hir::LocalId| {
                facts.get(&local).is_some_and(|fact| fact.owned)
            };
            let mut drops: Vec<Drop> = edge
                .drops
                .iter()
                // 残余は射影を消費した点の後。持っていたかは消費**前**で見る
                .filter(|drop| match drop {
                    Drop::Remaining { root, .. } => owns(before, *root),
                    Drop::Local(local) => owns(&after, *local),
                })
                .cloned()
                .collect();
            for scope in &edge.exits {
                drops.extend(
                    self.plan.scopes[scope.index()]
                        .locals
                        .iter()
                        .rev()
                        // ponytail: 経路によっては move 済み(`owned` は真だが
                        // `moved` も付いている)でも計画には残す。実行時の
                        // drop フラグは phase 7 が持つ
                        .filter(|local| self.plan.owners.contains(local) && owns(&after, **local))
                        .map(|local| Drop::Local(*local)),
                );
            }
            settled.push(drops);
        }
        for (edge, drops) in self.plan.edges.iter_mut().zip(settled) {
            edge.drops = drops;
        }
    }
}

/// 本体1つを解析する。
fn analyze(program: &hir::Program, id: hir::BodyId) -> (BodyPlan, Vec<Diag>) {
    let mut build = Build::new(program, id);
    let body = build.body;
    let root = build.scope(None);
    let entry = build.alloc(None, root, Effect::Nop);
    let exit = build.alloc(None, root, Effect::Nop);
    build.plan.entry = entry;
    build.plan.exit = exit;
    build.cur = Some(entry);

    // `self` と引数は呼び出しの時点で所有を得ている。入口の直後に初期化を置くと、
    // 入口の状態が空のままで済む
    let params: Vec<hir::LocalId> = match id {
        hir::BodyId::Callable(callable) => body
            .receiver
            .iter()
            .chain(program.callables[callable].params.iter())
            .copied()
            .collect(),
        hir::BodyId::Test(_) => Vec::new(),
    };
    for local in params {
        build.declare(local, root, Bound::Param);
        build.point(None, root, Effect::Init(local));
    }

    // 本体は値ベース。最後の式だけが戻り値なので、そこだけ所有を要求する
    if let Some((last, init)) = body.root.split_last() {
        for e in init {
            build.value(*e, root, Need::Read);
        }
        build.value(*last, root, Need::Take);
    }
    if let Some(from) = build.cur {
        build.edge(from, exit, vec![root]);
    }

    let input = build.solve();
    build.report(&input);
    build.settle(&input);
    (build.plan, build.diagnostics)
}

// ---------------------------------------------------------------------------
// 決定的な描画
// ---------------------------------------------------------------------------

impl Plan {
    /// 宣言順に全本体の計画を書き出す。同じプログラムからは常に同じ文字列が
    /// 出るので、計画のスナップショットテストはこれを比べる(tasks 3.7)。
    ///
    /// span は載せない(バイト位置は無関係な編集で動くため)。
    pub fn dump(&self, program: &hir::Program) -> String {
        let mut out = String::new();
        for id in &program.bodies {
            let plan = self.body(*id);
            let body = program.body(*id);
            let _ = writeln!(out, "body {}", program.show_body(*id));
            for (id, scope) in plan.scopes() {
                let locals: Vec<String> = scope
                    .locals
                    .iter()
                    .map(|local| {
                        format!(
                            "local#{}{}",
                            local.index(),
                            if plan.owners.contains(local) { "*" } else { "" }
                        )
                    })
                    .collect();
                let parent = match scope.parent {
                    Some(parent) => format!(" < scope#{}", parent.index()),
                    None => String::new(),
                };
                let _ = writeln!(out, "  scope#{}{parent} [{}]", id.index(), locals.join(" "));
            }
            for (id, point) in plan.points() {
                let expr = match point.expr {
                    Some(expr) => format!("expr#{}", expr.index()),
                    None => "-".to_string(),
                };
                let _ = writeln!(
                    out,
                    "  point#{} {expr} scope#{} {}",
                    id.index(),
                    point.scope.index(),
                    show_effect(program, body, &point.effect)
                );
            }
            for edge in plan.edges() {
                let exits: Vec<String> = edge
                    .exits
                    .iter()
                    .map(|scope| format!("scope#{}", scope.index()))
                    .collect();
                let drops: Vec<String> = edge.drops.iter().map(|d| show_drop(program, d)).collect();
                let mut line = format!("  edge #{} -> #{}", edge.from.index(), edge.to.index());
                if !exits.is_empty() {
                    let _ = write!(line, " exits [{}]", exits.join(" "));
                }
                if !drops.is_empty() {
                    let _ = write!(line, " drops [{}]", drops.join(" "));
                }
                let _ = writeln!(out, "{line}");
            }
            let _ = writeln!(
                out,
                "  entry #{} exit #{}",
                plan.entry.index(),
                plan.exit.index()
            );
        }
        out
    }
}

fn show_effect(program: &hir::Program, body: &hir::Body, effect: &Effect) -> String {
    match effect {
        Effect::Nop => "nop".to_string(),
        Effect::Init(local) => format!("init local#{}", local.index()),
        Effect::Assign(local) => format!("assign local#{}", local.index()),
        Effect::Access(access) => {
            let mode = match access.mode {
                Mode::Read => "read",
                Mode::Shared => "&",
                Mode::Mutable => "&mut",
                Mode::Move => "move",
            };
            format!(
                "{mode} local#{}({}){}",
                access.place.root.index(),
                body.local(access.place.root).name,
                show_path(program, &access.place.path)
            )
        }
    }
}

/// 射影の列の綴り。診断も dump もここだけを共有する
fn show_path(program: &hir::Program, path: &[Projection]) -> String {
    let mut out = String::new();
    for step in path {
        match step {
            Projection::Field(field) => {
                let _ = write!(out, ".{}", program.fields[*field].name);
            }
            Projection::OptionalPayload => out.push_str(".?"),
            Projection::EnumPayload(_, n) => {
                let _ = write!(out, ".{n}");
            }
            Projection::ArrayElement(Some(n)) => {
                let _ = write!(out, "[{n}]");
            }
            Projection::ArrayElement(None) => out.push_str("[_]"),
        }
    }
    out
}

fn show_drop(program: &hir::Program, drop: &Drop) -> String {
    match drop {
        Drop::Local(local) => format!("local#{}", local.index()),
        Drop::Remaining { root, consumed } => format!(
            "rest(local#{} - {})",
            root.index(),
            show_path(program, consumed)
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lex::{join as continue_lines, lex};
    use crate::parse;
    use crate::typecheck;

    /// 型検査までは通る前提で、所有権検査だけを見る
    fn analyze_source(src: &str) -> Result<CheckedProgram, Vec<Diag>> {
        let program = parse::parse(&continue_lines(lex(src).unwrap())).expect("パースできるはず");
        let hir = typecheck::check_and_lower(&program).expect("型検査を通るはず");
        check(hir)
    }

    fn accepted(src: &str) -> CheckedProgram {
        match analyze_source(src) {
            Ok(checked) => checked,
            Err(errors) => panic!("受理されるはず: {errors:#?}"),
        }
    }

    fn rejected(src: &str) -> Vec<Diag> {
        match analyze_source(src) {
            Ok(_) => panic!("拒否されるはず"),
            Err(errors) => errors,
        }
    }

    /// 診断1件だけを見る。件数も文言も固定する
    fn only(src: &str) -> Diag {
        let mut errors = rejected(src);
        assert_eq!(errors.len(), 1, "{errors:#?}");
        errors.pop().unwrap()
    }

    fn slice(src: &str, span: Span) -> &str {
        &src[span.start as usize..span.end as usize]
    }

    /// 主 span が指す範囲
    fn at<'s>(src: &'s str, diagnostic: &Diag) -> &'s str {
        slice(src, diagnostic.span.expect("実行前の診断は位置を持つ"))
    }

    /// 従属診断の (文言, 指す範囲)
    fn related<'s>(src: &'s str, diagnostic: &Diag) -> Vec<(String, &'s str)> {
        diagnostic
            .related
            .iter()
            .map(|r| {
                (
                    r.msg.clone(),
                    slice(src, r.span.expect("従属診断も位置を持つ")),
                )
            })
            .collect()
    }

    fn dump(src: &str) -> String {
        let checked = accepted(src);
        checked.plan.dump(&checked.hir)
    }

    const USER: &str = "struct User { id: int, name: str }
fn take(u: User -> int) { u.id }
fn make(-> User) { User { id = 1, name = \"a\" } }
";

    /// 本体を `main` に包む。末尾は常に `0` なので、渡した行は全て文の位置に来る
    fn with_user(body: &str) -> String {
        format!("{USER}fn main(-> int) {{\n{body}\n  0\n}}\n")
    }

    // -----------------------------------------------------------------------
    // 3.2 スコープ
    // -----------------------------------------------------------------------

    /// 第二級ブロックは新しいスコープを作らない。所有権解析が名前の見え方を
    /// 黙って変えていないことを、型検査が通ることと計画の両方で押さえる
    #[test]
    fn 第二級ブロックは周りのスコープを共有する() {
        let src = "fn main(-> int) {
  { let x = 1 }
  x
}
";
        let dumped = dump(src);
        assert!(
            dumped.contains("scope#0 [local#0]"),
            "ブロックの中の let も根のスコープに属する: {dumped}"
        );
        assert!(
            !dumped.contains("scope#1"),
            "ブロックはスコープを増やさない: {dumped}"
        );
    }

    /// 逆に、型検査が `locals` を複製する境界はそのままスコープになる
    #[test]
    fn 枝とループと提供とarmは自分のスコープを持つ() {
        let src = "enum Rank { Bronze Gold }
trait Clock { fn now(self -> int) }
struct SystemClock {}
impl Clock for SystemClock { fn now(self -> int) { 0 } }
effect clock: Clock
fn main(r: Rank, xs: [int] -> int) {
  if true { let a = 1 } else { let b = 2 }
  while false { let c = 3 }
  for x in xs { let d = 4 }
  with clock(SystemClock {}) { let e = 5 }
  match r {
    Rank::Bronze: 1
    Rank::Gold: 2
  }
}
";
        let dumped = dump(src);
        let main = dumped.split("body main").nth(1).expect("main の計画がある");
        let scopes: Vec<&str> = main
            .lines()
            .filter(|line| line.trim_start().starts_with("scope#"))
            .map(str::trim)
            .collect();
        assert_eq!(
            scopes,
            vec![
                "scope#0 [local#0 local#1*]",
                "scope#1 < scope#0 [local#2]",
                "scope#2 < scope#0 [local#3]",
                "scope#3 < scope#0 [local#4]",
                "scope#4 < scope#0 [local#5 local#6]",
                "scope#5 < scope#0 [local#7]",
                "scope#6 < scope#0 []",
                "scope#7 < scope#0 []",
            ],
            "{dumped}"
        );
    }

    /// 関数・メソッド・test はそれぞれ別の本体で、局所束縛は混ざらない
    #[test]
    fn 本体ごとに計画が分かれる() {
        let src = "fn main(-> int) { let a = 1\n a }
test \"t\" { let b = 2 }
";
        let dumped = dump(src);
        assert!(dumped.contains("body main"), "{dumped}");
        assert!(dumped.contains("body test \"t\""), "{dumped}");
    }

    // -----------------------------------------------------------------------
    // 3.3 初期化と move のデータフロー
    // -----------------------------------------------------------------------

    #[test]
    fn 前向きの制御では移動元がもう読めない() {
        let src = with_user("  let u = make()\n  let v = u\n  take(move u)");
        let diagnostic = only(&src);
        assert_eq!(
            diagnostic.msg,
            "main: `u` は既に move されているので使えません"
        );
    }

    /// 片方の枝でだけ move すると、合流点では「経路によっては move 済み」
    #[test]
    fn 枝の合流でmaybe_movedになる() {
        let src = with_user(
            "  let u = make()\n  if true { take(move u) }\n  let v = take(move u)\n  assert v == 1",
        );
        let diagnostic = only(&src);
        assert_eq!(
            diagnostic.msg,
            "main: `u` は move される経路があるのでここでは使えません"
        );
    }

    /// 抜ける枝で move しても、続く経路には move が流れてこない
    #[test]
    fn 抜ける枝のmoveは合流に流れない() {
        let src = format!(
            "{USER}fn main(-> int) {{
  let u = make()
  if true {{ return take(move u) }}
  take(move u)
}}
"
        );
        accepted(&src);
    }

    /// ループの背辺まで含めた不動点。2周目には move 済みになる
    #[test]
    fn ループが持ち越すmoveを拒否する() {
        let src = with_user("  let u = make()\n  while true { take(move u) }");
        let diagnostic = only(&src);
        assert_eq!(
            diagnostic.msg,
            "main: `u` はループの2周目には move 済みです"
        );
    }

    /// 周ごとに作り直す値はループを回っても持ち越さない
    #[test]
    fn 周ごとに束縛し直す値はループを通る() {
        accepted(&with_user("  while true { let u = make()\n take(move u) }"));
    }

    // -----------------------------------------------------------------------
    // 3.4 束縛・Copy・呼び出し・戻り値
    // -----------------------------------------------------------------------

    #[test]
    fn 不変な束縛への代入を拒否する() {
        let src = "fn main() { let x = 1\n x = 2 }\n";
        let diagnostic = only(src);
        assert_eq!(diagnostic.msg, "main: `x` は不変な束縛なので代入できません");
    }

    #[test]
    fn let_mutの束縛には代入できる() {
        accepted("fn main(-> int) { let mut x = 1\n x = 2\n x }\n");
    }

    /// `&mut` は「中身を変えてよい」であって「束縛を差し替えてよい」ではない
    #[test]
    fn 排他参照の束縛自体の差し替えには_let_mutが要る() {
        let src = "struct User { id: int }
fn swap(r: &mut User, other: &mut User) { r = other }
";
        let diagnostic = only(src);
        assert_eq!(diagnostic.msg, "swap: `r` は不変な束縛なので代入できません");
    }

    /// Copy な値は再束縛しても元が残る
    #[test]
    fn copyな値の再束縛は元を残す() {
        accepted("fn main(-> int) { let a = 1\n let b = a\n a + b }\n");
    }

    /// 一時値は既に所有者なので `move` は要らない
    #[test]
    fn 一時値の引数はmoveを要らない() {
        accepted(&with_user("  let n = take(make())\n  assert n == 1"));
    }

    #[test]
    fn 所有を受け取る引数には明示のmoveが要る() {
        let src = with_user("  let u = make()\n  let n = take(u)\n  assert n == 1");
        let diagnostic = only(&src);
        assert_eq!(
            diagnostic.msg,
            "main: `take` の第 1 引数は所有を受け取りますが、束縛済みの `u` をそのまま渡しています"
        );
    }

    /// 戻り値の move は自動。境界が既に所有を宣言している
    #[test]
    fn 戻り値の所有移動は自動() {
        let src = format!("{USER}fn hand(-> User) {{ let u = make()\n u }}\n");
        let checked = accepted(&src);
        let dumped = checked.plan.dump(&checked.hir);
        let hand = dumped.split("body hand").nth(1).expect("hand の計画がある");
        assert!(
            hand.contains("move local#0(u)"),
            "戻り値は明示なしで move する: {dumped}"
        );
        assert!(!hand.contains("drops"), "move 済みの値は落ちない: {dumped}");
    }

    /// 束縛からの束縛は既定で move
    #[test]
    fn 非copyの局所束縛は既定でmoveする() {
        let src =
            format!("{USER}fn main(-> int) {{ let u = make()\n let v = u\n take(move v) }}\n");
        let checked = accepted(&src);
        let dumped = checked.plan.dump(&checked.hir);
        assert!(dumped.contains("move local#0(u)"), "{dumped}");
    }

    // -----------------------------------------------------------------------
    // 3.5 フィールド射影は struct 全体を消費する
    // -----------------------------------------------------------------------

    #[test]
    fn フィールドの消費は根ごと消費して残りを落とす() {
        let src = format!("{USER}fn main(-> str) {{ let u = make()\n move u.name }}\n");
        let checked = accepted(&src);
        let dumped = checked.plan.dump(&checked.hir);
        let main = dumped.split("body main").nth(1).expect("main の計画がある");
        assert!(
            main.contains("move local#0(u).name"),
            "消費するのは射影した場所: {dumped}"
        );
        assert!(
            main.contains("drops [rest(local#0 - .name)]"),
            "残りのフィールドはその場で落ちる: {dumped}"
        );
    }

    #[test]
    fn フィールドを消費した根はもう使えない() {
        let src = with_user("  let u = make()\n  let n = move u.name\n  assert u.id == 1");
        let diagnostic = only(&src);
        assert_eq!(
            diagnostic.msg,
            "main: `u.id` は既に move されているので使えません"
        );
    }

    // -----------------------------------------------------------------------
    // 3.6 診断と関連位置
    // -----------------------------------------------------------------------

    #[test]
    fn move後の使用は移動元を指す() {
        let src = with_user("  let u = make()\n  take(move u)\n  take(move u)");
        let diagnostic = only(&src);
        assert_eq!(at(&src, &diagnostic), "move u");
        assert_eq!(diagnostic.label.as_deref(), Some("move 済みの値"));
        assert_eq!(
            related(&src, &diagnostic),
            vec![("ここで move されました".to_string(), "move u")]
        );
    }

    #[test]
    fn maybe_movedは分岐側のmoveを指す() {
        let src = with_user("  let u = make()\n  if true { take(move u) }\n  take(move u)");
        let diagnostic = only(&src);
        assert_eq!(
            diagnostic.label.as_deref(),
            Some("経路によっては move 済み")
        );
        assert_eq!(
            related(&src, &diagnostic),
            vec![("この経路でここが move します".to_string(), "move u")]
        );
    }

    #[test]
    fn ループが持ち越すmoveは直し方を添える() {
        let src = with_user("  let u = make()\n  while true { take(move u) }");
        let diagnostic = only(&src);
        assert_eq!(at(&src, &diagnostic), "move u");
        assert_eq!(diagnostic.label.as_deref(), Some("毎周 move している"));
        assert_eq!(
            diagnostic.help.as_deref(),
            Some("ループの外で move するか、周ごとに新しい値を束縛してください")
        );
    }

    #[test]
    fn moveの欠落は束縛の宣言を指す() {
        let src = with_user("  let u = make()\n  let n = take(u)\n  assert n == 1");
        let diagnostic = only(&src);
        assert_eq!(at(&src, &diagnostic), "u");
        assert_eq!(diagnostic.label.as_deref(), Some("所有が渡りません"));
        assert_eq!(
            diagnostic.help.as_deref(),
            Some("`move u` と書いて移動を見えるようにしてください")
        );
        assert_eq!(
            related(&src, &diagnostic),
            vec![("`u` はここで束縛されています".to_string(), "let u = make()")]
        );
    }

    #[test]
    fn 不変な束縛への代入は宣言を指す() {
        let src = "fn main() { let x = 1\n x = 2 }\n";
        let diagnostic = only(src);
        assert_eq!(at(src, &diagnostic), "x = 2");
        assert_eq!(diagnostic.label.as_deref(), Some("不変な束縛への代入"));
        assert_eq!(
            diagnostic.help.as_deref(),
            Some("`let mut x` と宣言してください")
        );
        assert_eq!(
            related(src, &diagnostic),
            vec![("`x` はここで束縛されています".to_string(), "let x = 1")]
        );
    }

    #[test]
    fn 借用されている場所のmoveは借用地点を指す() {
        let src =
            with_user("  let u = make()\n  let view = &u\n  take(move u)\n  assert view.id == 1");
        let diagnostic = only(&src);
        assert_eq!(
            diagnostic.msg,
            "main: `u` は借用されているので move できません"
        );
        assert_eq!(diagnostic.label.as_deref(), Some("借用中の値の move"));
        assert_eq!(
            related(&src, &diagnostic),
            vec![("`view` がここで借用しています".to_string(), "&u")]
        );
    }

    /// 借用が抜けたスコープと一緒に終われば、その後の move は通る
    #[test]
    fn スコープを抜けた借用は_moveを塞がない() {
        accepted(&with_user(
            "  let u = make()\n  if true { let view = &u\n assert view.id == 1 }\n  take(move u)",
        ));
    }

    /// 不変な束縛への `&mut` と、そこを通した書き込みは拒否する
    #[test]
    fn 不変な束縛は可変に触れない() {
        let src = with_user("  let u = make()\n  u.id = 2");
        let diagnostic = only(&src);
        assert_eq!(
            diagnostic.msg,
            "main: `u.id` は可変な束縛ではないので変更できません"
        );
        assert_eq!(at(&src, &diagnostic), "u.id = 2");
        assert_eq!(
            diagnostic.help.as_deref(),
            Some("`let mut u` と宣言してください")
        );
        assert_eq!(
            related(&src, &diagnostic),
            vec![("`u` はここで束縛されています".to_string(), "let u = make()")]
        );
    }

    #[test]
    fn 可変な束縛のフィールドは書き換えられる() {
        accepted(&with_user("  let mut u = make()\n  u.id = 2"));
    }

    #[test]
    fn 共有借用を通した書き込みを拒否する() {
        let src = format!("{USER}fn touch(u: &User) {{ u.id = 2 }}\n");
        let diagnostic = only(&src);
        assert_eq!(
            diagnostic.msg,
            "touch: `u.id` は共有借用を通しているので変更できません"
        );
    }

    #[test]
    fn 排他借用を通した書き込みは通る() {
        accepted(&format!("{USER}fn touch(u: &mut User) {{ u.id = 2 }}\n"));
    }

    // -----------------------------------------------------------------------
    // 3.7 計画のスナップショットと決定性
    // -----------------------------------------------------------------------

    /// 再帰する本体も本体ごとに独立に閉じる(この段は本体をまたがない)
    #[test]
    fn 再帰する本体も計画を持つ() {
        let src = "fn down(n: int -> int) { if n == 0 { 0 } else { down(n - 1) } }\n";
        let dumped = dump(src);
        assert!(dumped.contains("body down"), "{dumped}");
    }

    #[test]
    fn 入れ子のスコープは逆宣言順に落ちる() {
        let src = format!(
            "{USER}fn main() {{
  let a = make()
  if true {{
    let b = make()
    let c = make()
  }}
}}
"
        );
        let checked = accepted(&src);
        let dumped = checked.plan.dump(&checked.hir);
        let main = dumped.split("body main").nth(1).expect("main の計画がある");
        assert!(
            main.contains("exits [scope#1] drops [local#2 local#1]"),
            "内側は逆宣言順: {dumped}"
        );
        assert!(
            main.contains("exits [scope#0] drops [local#0]"),
            "外側は本体を抜けるときに落ちる: {dumped}"
        );
    }

    /// `return` は抜ける全てのスコープの破棄を積んでから戻る
    #[test]
    fn 早期returnは抜ける全スコープを落とす() {
        let src = format!(
            "{USER}fn main(-> int) {{
  let a = make()
  if true {{
    let b = make()
    return 0
  }}
  1
}}
"
        );
        let checked = accepted(&src);
        let dumped = checked.plan.dump(&checked.hir);
        assert!(
            dumped.contains("exits [scope#1 scope#0] drops [local#1 local#0]"),
            "{dumped}"
        );
    }

    /// ループの背辺は本体スコープだけを落とす
    #[test]
    fn ループの背辺は本体スコープを落とす() {
        let src = format!("{USER}fn main() {{ while true {{ let u = make() }} }}\n");
        let checked = accepted(&src);
        let dumped = checked.plan.dump(&checked.hir);
        assert!(
            dumped.contains("exits [scope#1] drops [local#0]"),
            "{dumped}"
        );
    }

    /// Copy な値は落とすものが無いので計画に出ない
    #[test]
    fn copyな束縛は破棄の計画に出ない() {
        let dumped = dump("fn main() { let x = 1 }\n");
        assert!(!dumped.contains("drops"), "{dumped}");
        assert!(
            dumped.contains("scope#0 [local#0]"),
            "所有者印は付かない: {dumped}"
        );
    }

    /// 同じソースからは常に同じ計画が出る。`dump` は毎回ソースから
    /// 読み込み直すので、比べているのは走査と不動点の並び全体
    #[test]
    fn 計画は繰り返しても同じ文字列になる() {
        let src = format!(
            "{USER}enum Rank {{ Bronze Gold }}
trait Clock {{ fn now(&self -> int) }}
struct SystemClock {{}}
impl Clock for SystemClock {{ fn now(&self -> int) {{ 0 }} }}
effect clock: Clock
fn pick(flag: bool, u: User -> User) {{ if flag {{ u }} else {{ make() }} }}
fn loops(xs: [int] -> int) {{
  let mut total = 0
  for x in xs {{ while false {{ total = total + x }} }}
  total
}}
fn arms(r: Rank -> int) {{ match r {{ Rank::Bronze: 1
 Rank::Gold: 2 }} }}
fn provided(-> int) {{ with clock(SystemClock {{}}) {{ clock.now() }} }}
fn main(-> int) {{
  let mut n = 0
  let u = make()
  if true {{ n = take(move u) }} else {{ n = 1 }}
  n + loops([1, 2]) + arms(Bronze) + provided() + take(move pick(true, make()))
}}
test \"t\" {{ let b = make()
 assert b.id == 1 }}
"
        );
        let first = dump(&src);
        let second = dump(&src);
        assert_eq!(first, second);
        assert!(first.lines().count() > 100, "{first}");
    }

    /// 素の `match` と `for` は対象を借用する(所有モードは phase 6)
    #[test]
    fn 素のmatchとforは対象を借用する() {
        let src = "enum Lookup { Found(int) Missing }
fn main(l: Lookup, xs: [int] -> int) {
  for x in xs { assert x == x }
  match l {
    Lookup::Found(n): n
    Lookup::Missing: 0
  }
}
";
        let dumped = dump(src);
        assert!(dumped.contains("& local#1(xs)"), "{dumped}");
        assert!(dumped.contains("& local#0(l)"), "{dumped}");
    }

    // -----------------------------------------------------------------------
    // 導かれた位置の所有モード
    //
    // 明示の `move` が書かれていないのに所有が動くのはここだけなので、
    // 位置ごとに固定しておく
    // -----------------------------------------------------------------------

    /// 枝の末尾がそのまま戻り値になる。`move` は書かれていないが所有は動く
    #[test]
    fn 枝の末尾は暗黙にmoveする() {
        let src = "struct User { id: int }
fn make(-> User) { User { id = 1 } }
fn pick(flag: bool, u: User -> User) { if flag { u } else { make() } }
";
        let checked = accepted(src);
        let dumped = checked.plan.dump(&checked.hir);
        assert!(dumped.contains("move local#1(u)"), "{dumped}");
    }

    /// 第二級ブロックの末尾も同じ。括弧は所有を止めない
    #[test]
    fn ブロックの末尾も暗黙にmoveする() {
        let src = "struct User { id: int }
fn hand(u: User -> User) { { u } }
";
        let checked = accepted(src);
        let dumped = checked.plan.dump(&checked.hir);
        assert!(dumped.contains("move local#0(u)"), "{dumped}");
    }

    /// 枝の末尾で move すると、続く経路では使えない
    #[test]
    fn 枝の末尾のmoveも合流に効く() {
        let src = "struct User { id: int }
fn make(-> User) { User { id = 1 } }
fn take(u: User -> int) { u.id }
fn main(-> int) {
  let u = make()
  let v = if true { u } else { make() }
  take(move u)
}
";
        let diagnostic = only(src);
        assert_eq!(
            diagnostic.msg,
            "main: `u` は move される経路があるのでここでは使えません"
        );
    }

    /// `let e = &mut u` は不変な `u` からは作れない
    #[test]
    fn 不変な束縛からは排他借用を作れない() {
        let src = "struct User { id: int }
fn make(-> User) { User { id = 1 } }
fn main() { let u = make()
 let e = &mut u
 e.id = 2 }
";
        let diagnostic = only(src);
        assert_eq!(
            diagnostic.msg,
            "main: `u` は可変な束縛ではないので変更できません"
        );
    }

    #[test]
    fn 可変な束縛からは排他借用を作れる() {
        accepted(
            "struct User { id: int }
fn make(-> User) { User { id = 1 } }
fn main() { let mut u = make()
 let e = &mut u
 e.id = 2 }
",
        );
    }

    // -----------------------------------------------------------------------
    // arm の順次探索・入れ子の射影・借用で束ねた名前
    // -----------------------------------------------------------------------

    /// guard が偽なら次の arm へ進むので、guard が動かした所有はそこに見える
    #[test]
    fn armのguardが動かした所有は次のarmに見える() {
        let src = format!(
            "{USER}enum Rank {{ Bronze Gold }}
fn main(r: Rank -> int) {{
  let u = make()
  match r {{
    Rank::Bronze if take(move u) == 1: 1
    _: take(move u)
  }}
}}
"
        );
        let diagnostic = only(&src);
        assert_eq!(
            diagnostic.msg,
            "main: `u` は move される経路があるのでここでは使えません"
        );
    }

    /// pattern が外れても次の arm へ進む
    #[test]
    fn armのpatternが外れたら次のarmへ進む() {
        let src = format!(
            "{USER}enum Rank {{ Bronze Gold }}
fn main(r: Rank -> int) {{
  let u = make()
  match r {{
    Rank::Bronze: take(move u)
    Rank::Gold: take(move u)
  }}
}}
"
        );
        // 別々の arm は排他なので、同じ値を両方で move してよい
        accepted(&src);
    }

    /// 入れ子の射影を消費したら、消した経路ごと残余に載る
    #[test]
    fn 入れ子の射影の残余は経路ごと記録する() {
        let src = "struct Inner { name: str }
struct Outer { id: int, inner: Inner }
fn main(-> str) {
  let u = Outer { id = 1, inner = Inner { name = \"a\" } }
  move u.inner.name
}
";
        let checked = accepted(src);
        let dumped = checked.plan.dump(&checked.hir);
        assert!(dumped.contains("move local#0(u).inner.name"), "{dumped}");
        assert!(
            dumped.contains("drops [rest(local#0 - .inner.name)]"),
            "残余は消した経路ごと。`name` だけだと `inner` を丸ごと落としてしまう: {dumped}"
        );
    }

    /// 素の `for` は配列を借用するので、要素の所有は取り出せない
    #[test]
    fn ループ変数からはmoveできない() {
        let src = format!(
            "{USER}fn main(us: [User] -> int) {{
  for u in us {{ take(move u) }}
  0
}}
"
        );
        let diagnostic = only(&src);
        assert_eq!(
            diagnostic.msg,
            "main: `u` は借用で束ねた名前なので move できません"
        );
        assert_eq!(diagnostic.label.as_deref(), Some("借用された束縛の move"));
    }

    /// 素の `match` も同じ。payload だけを取り出す部分 move は無い
    #[test]
    fn payload束縛からはmoveできない() {
        let src = format!(
            "{USER}enum Lookup {{ Found(User) Missing }}
fn main(l: Lookup -> int) {{
  match l {{
    Lookup::Found(u): take(move u)
    Lookup::Missing: 0
  }}
}}
"
        );
        let diagnostic = only(&src);
        assert_eq!(
            diagnostic.msg,
            "main: `u` は借用で束ねた名前なので move できません"
        );
        assert_eq!(
            related(&src, &diagnostic),
            vec![(
                "`u` はここで束ねられています".to_string(),
                "Lookup::Found(u): take(move u)"
            )]
        );
    }

    /// `&mut T` の引数へ渡した `&mut` 束縛は排他アクセスとして記録される
    #[test]
    fn 排他参照の引数渡しは排他アクセスになる() {
        let src = "struct User { id: int }
fn bump(u: &mut User) { u.id = u.id + 1 }
fn outer(r: &mut User) { bump(r) }
";
        let checked = accepted(src);
        let dumped = checked.plan.dump(&checked.hir);
        let outer = dumped
            .split("body outer")
            .nth(1)
            .expect("outer の計画がある");
        assert!(outer.contains("&mut local#0(r)"), "{dumped}");
    }

    /// 消費レシーバに `move` が無い場合の文言
    #[test]
    fn 消費レシーバにもmoveが要る() {
        let src = "struct User { id: int }
impl User {
  fn burn(self -> int) { self.id }
}
fn main(-> int) { let u = User { id = 1 }
 u.burn() }
";
        let diagnostic = only(src);
        assert_eq!(
            diagnostic.msg,
            "main: `impl User::burn` のレシーバは所有を受け取りますが、束縛済みの `u` をそのまま渡しています"
        );
    }

    /// move 済みの値に `move` の欠落を重ねて言わない
    #[test]
    fn move済みの値には診断を1件だけ出す() {
        let src = with_user("  let u = make()\n  take(move u)\n  let n = take(u)\n  assert n == 1");
        let diagnostic = only(&src);
        assert_eq!(
            diagnostic.msg,
            "main: `u` は既に move されているので使えません"
        );
    }

    /// 引数と `self` に `mut` は書けないので、直し方は署名を指す
    #[test]
    fn 引数への書き込みは署名の直し方を出す() {
        let src = format!("{USER}fn touch(u: User) {{ u.id = 2 }}\n");
        let diagnostic = only(&src);
        assert_eq!(
            diagnostic.help.as_deref(),
            Some("`u` を `&mut` で受け取ってください")
        );
    }

    /// ループが持ち越す move も束縛の宣言を指す
    #[test]
    fn ループが持ち越すmoveは束縛を指す() {
        let src = with_user("  let u = make()\n  while true { take(move u) }");
        let diagnostic = only(&src);
        assert_eq!(
            related(&src, &diagnostic),
            vec![("`u` はここで束縛されています".to_string(), "let u = make()")]
        );
    }

    /// 計画の全文。点・辺・スコープの並びと入口出口まで固定する
    #[test]
    fn 計画の全文を固定する() {
        let src = "struct User { id: int, name: str }
fn take(u: User -> int) { u.id }
fn main(-> int) {
  let u = User { id = 1, name = \"a\" }
  if true { take(move u) } else { 0 }
}
";
        assert_eq!(
            dump(src),
            "body take
  scope#0 [local#0*]
  point#0 - scope#0 nop
  point#1 - scope#0 nop
  point#2 - scope#0 init local#0
  point#3 expr#1 scope#0 read local#0(u).id
  edge #0 -> #2
  edge #2 -> #3
  edge #3 -> #1 exits [scope#0] drops [local#0]
  entry #0 exit #1
body main
  scope#0 [local#0*]
  scope#1 < scope#0 []
  scope#2 < scope#0 []
  point#0 - scope#0 nop
  point#1 - scope#0 nop
  point#2 expr#3 scope#0 init local#0
  point#3 expr#12 scope#0 nop
  point#4 expr#6 scope#1 move local#0(u)
  point#5 expr#12 scope#0 nop
  edge #0 -> #2
  edge #2 -> #3
  edge #3 -> #4
  edge #4 -> #5 exits [scope#1]
  edge #3 -> #5 exits [scope#2]
  edge #5 -> #1 exits [scope#0] drops [local#0]
  entry #0 exit #1
"
        );
    }

    /// 再帰する本体も、呼び出しの引数まで含めて閉じる
    #[test]
    fn 再帰する呼び出しの引数も検査する() {
        let src = format!(
            "{USER}fn down(n: int, u: User -> int) {{
  if n == 0 {{ take(move u) }} else {{ down(n - 1, u) }}
}}
"
        );
        let diagnostic = only(&src);
        assert_eq!(
            diagnostic.msg,
            "down: `down` の第 2 引数は所有を受け取りますが、束縛済みの `u` をそのまま渡しています"
        );
        accepted(&format!(
            "{USER}fn down(n: int, u: User -> int) {{
  if n == 0 {{ take(move u) }} else {{ down(n - 1, move u) }}
}}
"
        ));
    }

    /// 分類済みアクセスは `ExprId` で引ける(tasks 3.1)
    #[test]
    fn 分類済みアクセスは式idで引ける() {
        let src = "struct User { id: int }
fn main(-> int) { let mut u = User { id = 1 }
 u.id = 2
 u.id }
";
        let checked = accepted(src);
        let plan = checked.plan.body(checked.hir.bodies[0]);
        let modes: Vec<Mode> = checked
            .hir
            .body(checked.hir.bodies[0])
            .exprs()
            .filter_map(|(id, _)| plan.access(id).map(|a| a.mode))
            .collect();
        assert_eq!(modes, vec![Mode::Mutable, Mode::Read]);
    }

    /// 診断が式の位置の順に並ぶ(`render::report` は並べ替えない)
    #[test]
    fn 診断はソースの位置の順に出る() {
        let src = with_user("  let a = make()\n  let b = make()\n  a.id = 2\n  b.id = 3");
        let spans: Vec<u32> = rejected(&src)
            .iter()
            .map(|d| d.span.expect("位置を持つ").start)
            .collect();
        assert_eq!(spans.len(), 2, "{spans:?}");
        assert!(spans[0] < spans[1], "{spans:?}");
    }

    /// 未移行の実例。落ちるのは移行項目だけで、余計な診断は出ない
    #[test]
    fn 未移行のcanonicalは移行項目だけで落ちる() {
        let src = std::fs::read_to_string("examples/canonical.rd").expect("読める");
        let messages: Vec<String> = rejected(&src).into_iter().map(|d| d.msg).collect();
        assert_eq!(
            messages,
            vec![
                "stamp: `u.promoted_at` は可変な束縛ではないので変更できません",
                "stamp: `impl Database::save` の第 1 引数は所有を受け取りますが、束縛済みの `u` をそのまま渡しています",
                "promote: `u.rank` は可変な束縛ではないので変更できません",
                "promote: `stamp` の第 1 引数は所有を受け取りますが、束縛済みの `u` をそのまま渡しています",
                "impl InMemoryDb::get: `impl InMemoryDb::find` のレシーバは所有を受け取りますが、束縛済みの `store` をそのまま渡しています",
                // 素の `for` はループ変数を借用で束ねるので、要素を返せない
                "impl InMemoryDb::find: `u` は借用で束ねた名前なので move できません",
                "test \"昇格すると Gold になり時刻が刻まれる\": `alice.id` は既に move されているので使えません",
                "test \"昇格すると Gold になり時刻が刻まれる\": `store` は既に move されているので使えません",
                "test \"昇格すると Gold になり時刻が刻まれる\": `alice.id` は既に move されているので使えません",
            ]
        );
    }
}
