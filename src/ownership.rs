//! 所有権解析。型検査を通った HIR に「誰がいつ値を持っているか」を付ける段。
//!
//! 型検査は「どんな値か」を決めるが、「その値の所有がどこへ動いたか」は決めない。
//! ここが決めるのは6つ:
//!
//!   1. スコープ  … 局所束縛がどの字句スコープに属するか(design.md 決定8)
//!   2. CFG       … 本体の制御の流れを点と辺で表した**解析用の**表現(決定5)
//!   3. アクセス  … 各場所式が Copy 読み・共有借用・排他借用・move のどれか
//!   4. 破棄      … どの辺でどの所有値が落ちるか(決定8)
//!   5. 借用      … どの借用がどの点で生きているか(非字句リージョン、決定6)
//!   6. provenance … 参照を返す本体が、どの入力の場所を借りて返すか(決定6)
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
//! # リージョンは字句スコープではない
//!
//! 借用は場所へのアクセスと同じ点で生まれ、**最後の使用まで**しか生きない
//! (design.md 決定6)。生存区間は2つの単調な流れの交わりで決める:
//!
//!   - 前向き … 生まれた点から到達できる点
//!   - 後ろ向き … その借用を要る点へ到達できる点
//!
//! 「要る点」は借用を持つ参照束縛を触った点・その借用を実引数に取る呼び出しの
//! 点・戻り値に載るなら出口。だから `let v = &u` の後で `v` を使わなくなれば、
//! そこから先は `u` を排他的に触れる。
//!
//! # 破棄計画が言うこと・言わないこと
//!
//! 辺に載る `Drop` は**場所**の破棄だけ(design.md 決定8)。局所束縛と、
//! 射影を消費した根の残りがそれで、抜けるスコープの宣言の逆順に並ぶ。落ちる
//! 辺は素通し・枝の合流・周回の背辺と脱出・提供本体の脱出・`return`(抜ける
//! 全スコープぶん)・本体末尾の6つで、`return` だけが複数スコープを積む。
//!
//! 場所を持たない値 — 一時値、`with` が提供した実体、消費する `match` の
//! 選ばれなかった中身、消費する `for` が使い終わった buffer、上書きされた
//! 古い値 — は計画に並ばない。それらを手放すのは**それを作った構文**で、
//! 対応する HIR ノードと、その構文の所有モード(`BodyPlan::access` で引ける
//! 対象のアクセス)から一意に決まる。所有の移動を場所の間で追うのが計画の
//! 仕事で、寿命の一覧を作るのは仕事ではない。
//!
//! 実行時に要るもの(phase 7、tasks 7.4):経路によっては move 済みの場所は
//! 計画に残るので、場所ごとの実行時 drop フラグが要る(design.md 決定8)。
//! 実行時失敗は言語のスコープを巻き戻さない。
//!
//! # まだやらないこと
//!
//! 評価器の所有(phase 7)、ソースの移行(phase 8)。

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

/// 場所の射影1段(design.md 決定5)。
///
/// 4つとも作られる。`Field` はフィールド射影、`OptionalPayload` は `.?` の
/// optional 射影、`EnumPayload` は `match` が束ねた payload、`ArrayElement` は
/// `for` が束ねた要素。後ろ3つは重なり判定で保守的に容器全体と重なる。
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Projection {
    Field(hir::FieldId),
    OptionalPayload,
    EnumPayload(hir::VariantId, usize),
    /// 定数添字が分かっていれば `Some`。分からない添字は容器全体と重なる
    ArrayElement(Option<i64>),
}

/// 2つの場所が同じ記憶に触れうるか(design.md 決定5、tasks 4.2)。
///
/// 根が違えば重ならない。根が同じなら、片方の射影列がもう片方の**接頭辞**で
/// ある限り重なる — つまり全体は子の全てと重なり、`p` と `p.left` は重なる。
pub fn overlaps(a: &Place, b: &Place) -> bool {
    a.root == b.root
        && a.path
            .iter()
            .zip(&b.path)
            .all(|(x, y)| projections_overlap(x, y))
}

/// 射影1段どうしの重なり。言い分けられないものは全部「重なる」へ倒す。
///
/// 保守的な側は**受理を狭める**側なので、判定を足せるようになったら
/// 拒否が減るだけで、通っていたものが落ちることはない。
fn projections_overlap(a: &Projection, b: &Projection) -> bool {
    match (a, b) {
        // 宣言の違うフィールドは別の記憶。ここだけが「重ならない」を言える
        (Projection::Field(x), Projection::Field(y)) => x == y,
        // 定数添字が両方分かっていれば言い分けられる(design.md 決定5 の
        // 「narrow constant rule」)。記号的な不等式は後の最適化
        (Projection::ArrayElement(Some(x)), Projection::ArrayElement(Some(y))) => x == y,
        // enum payload・optional の中身・不明な添字・種類の違う射影は保守的に重なる
        _ => true,
    }
}

/// 本体1つの中で一意な借用。
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct LoanId(u32);

impl LoanId {
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

/// 借用1つ(design.md 決定6、tasks 4.1)。
///
/// 明示の `&place` / `&mut place` と、借用を受け取る位置への自動共有借用が
/// 場所へのアクセスと同じ点で作る。生存区間は `BodyPlan::region` が持つ。
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Loan {
    pub place: Place,
    pub kind: hir::RefKind,
    /// 借用が生まれた点
    pub point: PointId,
    pub span: Span,
}

/// 戻り値 provenance の根。呼び出し側で実引数へ置き換わる(design.md 決定6)。
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Input {
    Receiver,
    Param(usize),
}

/// 入力の場所。参照はローカル・引数・戻り値にしか置けないので、provenance は
/// 「入力の根 + 射影」の**有限集合**で閉じる(design.md 決定6)。
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct InputPlace {
    pub input: Input,
    pub path: Vec<Projection>,
}

/// 参照を返す本体の要約(design.md 決定6、tasks 4.4)。
///
/// 分岐は origin を合流する。排他で返すなら候補は全部排他に予約されたままに
/// なる — `&mut` を作れるのは排他な場所からだけなので、候補に共有の借用が
/// 混ざることは構成上起きない(tasks 4.5)。
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ReturnProvenance {
    pub kind: hir::RefKind,
    pub origins: BTreeSet<InputPlace>,
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
    ///
    /// 上書きの破棄はこの効果そのものが表すので、辺には別の `Drop` を積まない。
    /// フィールドの差し替え(`u.name = ...`)も同じで、そこは
    /// `Effect::Access(Mutable)` と `ExprKind::AssignField` が表す(tasks 6.6)
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
    /// 作った順 = 評価順の借用
    loans: Vec<Loan>,
    /// 借用ごとの生存区間。`loans` と同じ並び
    regions: Vec<BTreeSet<PointId>>,
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

    /// 式に対応する CFG 点。ownership-aware evaluator の debug assertion と
    /// 計画スナップショットが、分類済み access に点があることを照合する。
    pub fn point_of(&self, expr: hir::ExprId) -> Option<PointId> {
        self.accesses.get(&expr).copied().or_else(|| {
            self.points
                .iter()
                .position(|point| point.expr == Some(expr))
                .map(|index| PointId(index as u32))
        })
    }

    pub fn loans(&self) -> impl Iterator<Item = (LoanId, &Loan)> {
        self.loans
            .iter()
            .enumerate()
            .map(|(i, loan)| (LoanId(i as u32), loan))
    }

    /// その借用が生きている点。最後の使用より先には伸びない(design.md 決定6)
    pub fn region(&self, loan: LoanId) -> &BTreeSet<PointId> {
        &self.regions[loan.index()]
    }
}

/// 本体ごとの計画。並びは宣言順。
#[derive(Debug, Default)]
pub struct Plan {
    bodies: BTreeMap<hir::BodyId, BodyPlan>,
    provenance: BTreeMap<hir::CallableId, ReturnProvenance>,
}

impl Plan {
    pub fn body(&self, id: hir::BodyId) -> &BodyPlan {
        self.bodies.get(&id).expect("全ての本体に計画がある")
    }

    /// 参照を返す callable の要約。所有を返す callable は持たない
    pub fn provenance(&self, id: hir::CallableId) -> Option<&ReturnProvenance> {
        self.provenance.get(&id)
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
/// 3段。(1) 本体ごとに CFG・借用・carrier を1度だけ作る。(2) 戻り値
/// provenance を全プログラムで不動点まで閉じる。(3) その要約を使って本体ごとに
/// リージョンと衝突を確定する。(1) と (3) が本体を跨がないので、本体を跨ぐ
/// 反復は (2) の集合演算だけで済む。
pub fn check(hir: hir::Program) -> Result<CheckedProgram, Vec<Diag>> {
    let mut plan = Plan::default();
    let mut diagnostics = Vec::new();
    {
        let mut builds: Vec<Build> = hir.bodies.iter().map(|id| walk_body(&hir, *id)).collect();
        let summaries = solve_provenance(&hir, &mut builds);
        for (id, build) in hir.bodies.iter().zip(builds) {
            if let hir::BodyId::Callable(callable) = id
                && let Some(kind) = hir.callables[*callable].ret.reference
            {
                plan.provenance.insert(
                    *callable,
                    ReturnProvenance {
                        kind,
                        origins: summaries
                            .get(&Callee::Body(*callable))
                            .cloned()
                            .unwrap_or_default(),
                    },
                );
            }
            let (body_plan, mut found) = build.finish();
            diagnostics.append(&mut found);
            plan.bodies.insert(*id, body_plan);
        }
    }
    if diagnostics.is_empty() {
        Ok(CheckedProgram { hir, plan })
    } else {
        Err(diagnostics)
    }
}

// ---------------------------------------------------------------------------
// 戻り値 provenance の不動点(tasks 4.4 / 4.5)
// ---------------------------------------------------------------------------

/// 呼び出し先の同一性。契約メソッドは、それを実装する全ての本体の provenance が
/// 合流した仮想の本体(`requirement.rs` の `BodyKey` と同じ形)。
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum Callee {
    Body(hir::CallableId),
    Trait(hir::TraitMethodId),
}

type Summaries = BTreeMap<Callee, BTreeSet<InputPlace>>;

/// 空から始めて、変化がなくなるまで回す。
///
/// origin は「入力の根 + ソースに書かれた射影」しか作らないので候補は有限で、
/// 要約は増える一方(単調)。だから再帰も相互再帰も必ず止まる(design.md の
/// リスク「whole-program region inference … fails to converge」)。
///
/// ponytail: `requirement::analyze` と同じ素朴な反復。呼び出しグラフを SCC で
/// 縮約すれば反復は減るが、遅くなってからでよい。
fn solve_provenance(program: &hir::Program, builds: &mut [Build]) -> Summaries {
    // 実装本体 → その本体が実装する契約メソッド
    let mut contracts: BTreeMap<hir::CallableId, Vec<hir::TraitMethodId>> = BTreeMap::new();
    for (_, decl) in program.trait_impls.iter() {
        for (method, callable) in &decl.methods {
            contracts.entry(*callable).or_default().push(*method);
        }
    }
    let mut summaries = Summaries::new();
    loop {
        let mut changed = false;
        for build in builds.iter_mut() {
            build.resolve(&summaries);
            let hir::BodyId::Callable(callable) = build.id else {
                continue;
            };
            let origins = build.provenance_origins();
            changed |= merge_origins(&mut summaries, Callee::Body(callable), &origins);
            for method in contracts.get(&callable).into_iter().flatten() {
                changed |= merge_origins(&mut summaries, Callee::Trait(*method), &origins);
            }
        }
        if !changed {
            break;
        }
    }
    summaries
}

fn merge_origins(into: &mut Summaries, key: Callee, origins: &BTreeSet<InputPlace>) -> bool {
    let slot = into.entry(key).or_default();
    let before = slot.len();
    slot.extend(origins.iter().cloned());
    slot.len() != before
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
    /// `for` のループ変数と match の payload が、対象を借りて束ねた名前
    /// (design.md 決定7)。所有者ではないので破棄もされない。`Mutable` なら
    /// 中身は変えられるが、名前そのものから所有を持ち出すことはできない
    Borrowed(hir::RefKind),
    /// 消費する `for` / `match` が所有ごと束ねた名前。周回・arm のスコープが
    /// 所有者になるので、抜けるときに落ちる(design.md 決定7)
    Owned,
}

impl Bound {
    /// 破棄の対象になる束縛か。借用で束ねた名前は所有者ではない
    fn owns(self) -> bool {
        !matches!(self, Bound::Borrowed(_))
    }
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

/// 値が運ぶ借用の出どころ。解決は `Build::expand`。
///
/// `Local` と `Call` だけが後回しで、それ以外は走査中に決まる。`Local` は
/// 束縛の不動点、`Call` は呼び出し先の要約を要る。
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum Carrier {
    Loan(LoanId),
    Local(hir::LocalId),
    Call(hir::ExprId),
}

/// 解決済みの借用の出どころ。この本体の借用か、参照で受け取った入力そのもの。
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum Origin {
    Loan(LoanId),
    Input(Input),
}

/// 借用元を呼び出し側で名指せない理由。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Unrooted {
    /// 一時値からの借用。所有者が式の中で終わる
    Temporary,
    /// スロットのレシーバ = 提供された実体で、その所有をこの本体で名指せない。
    /// 提供が外側の本体にあるか、提供そのものが一時値のとき(tasks 5.4)
    Provider,
}

/// 実体を運ぶ提供の運び方(design.md 決定11、tasks 5.4)。
///
/// 型だけの提供(`with db<Postgres>`)は実体を運ばないので、`Provision::value`
/// が `None` であることがそのままそのモードになる。それ以外はソースの修飾と
/// 「値が場所か」だけで決まるので、HIR に別の欄を持たない。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Provider {
    /// `with db(store)` — 修飾を書かない束縛済みの場所は共有借用
    Shared,
    /// `with db(&mut store)`
    Mutable,
    /// `with db(move store)`
    Moved,
    /// `with db(Postgres::new(...))` — 提供スコープが持つ一時値
    Temporary,
}

impl Provider {
    fn spelling(self) -> &'static str {
        match self {
            Provider::Shared => "共有借用",
            Provider::Mutable => "排他借用",
            Provider::Moved => "move した所有",
            Provider::Temporary => "一時値の所有",
        }
    }

    /// 提供スコープが所有を握っているか
    fn owns(self) -> bool {
        matches!(self, Provider::Moved | Provider::Temporary)
    }
}

/// 呼び出し1つ。結果の provenance を実引数へ置き換えるのに要る
struct CallSite {
    callee: Callee,
    recv: Option<hir::ExprId>,
    args: Vec<hir::ExprId>,
}

struct Build<'a> {
    program: &'a hir::Program,
    body: &'a hir::Body,
    id: hir::BodyId,
    ctx: String,
    plan: BodyPlan,
    /// いま制御が居る点。抜けたら `None`
    cur: Option<PointId>,
    /// その点の後に走る破棄。フィールド消費の残余だけがここに入る
    residue: BTreeMap<PointId, Vec<Drop>>,
    /// 借用が生きていなければならない点。作成点は常に入る
    loan_uses: Vec<BTreeSet<PointId>>,
    /// 借用ごとの「借り直さずに使用へ届く」点。借用元の破棄と突き合わせる
    outliving: Vec<BTreeSet<PointId>>,
    /// 式の値が運ぶ借用の出どころ
    carriers: BTreeMap<hir::ExprId, BTreeSet<Carrier>>,
    /// 束縛へ流れ込む carrier。ponytail: 経路を区別せず和で閉じる。借用が
    /// 実際より長く生きる側にしか動かないので、拒否が増えるだけ
    flows: BTreeMap<hir::LocalId, BTreeSet<Carrier>>,
    /// 呼び出し式 → 呼び先と実引数
    calls: BTreeMap<hir::ExprId, CallSite>,
    /// 戻り値へ流れる carrier(明示の `return` と本体の末尾)
    returns: BTreeSet<Carrier>,
    /// 参照で受け取る引数・レシーバ。provenance の根になる
    inputs: BTreeMap<hir::LocalId, Input>,
    /// `resolve` の結果
    holds: BTreeMap<hir::LocalId, BTreeSet<Origin>>,
    origins: BTreeMap<hir::ExprId, BTreeSet<Origin>>,
    returned: BTreeSet<Origin>,
    /// 借用を返す呼び出しなのに、借用元をこの本体で名指せないもの(tasks 4.3)
    unrooted: BTreeMap<hir::ExprId, Unrooted>,
    /// 名前で参照できる束縛とその出自。ここに無いのは「知らない名前への代入」が
    /// 作った書き込み専用の束縛で、代入は初期化として扱う
    bindings: BTreeMap<hir::LocalId, Bound>,
    /// いま効いている提供のスタック。内側が外側を隠すので後ろから引く
    /// (tasks 5.4 の入れ子の提供)。第3要素は提供を積んだ時点の周回の深さで、
    /// 「その提供を周回のたびに消費していないか」を見るのに使う(tasks 6.6)
    provided: Vec<(hir::SlotId, Option<hir::ExprId>, u32)>,
    /// 消費レシーバで手放した提供と、手放した呼び出し。提供された実体には
    /// 局所束縛が無いので、move 済みを覚える場所がここになる(tasks 6.6)
    consumed: Vec<(hir::SlotId, hir::ExprId, hir::ExprId)>,
    /// いま入っているループの深さ。提供を消費する呼び出しが周回の中にあるかを
    /// 言うためだけに持つ
    depth: u32,
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
            id,
            ctx: program.show_body(id),
            plan: BodyPlan::default(),
            cur: None,
            residue: BTreeMap::new(),
            loan_uses: Vec::new(),
            outliving: Vec::new(),
            carriers: BTreeMap::new(),
            flows: BTreeMap::new(),
            calls: BTreeMap::new(),
            returns: BTreeSet::new(),
            inputs: BTreeMap::new(),
            holds: BTreeMap::new(),
            origins: BTreeMap::new(),
            returned: BTreeSet::new(),
            unrooted: BTreeMap::new(),
            bindings: BTreeMap::new(),
            provided: Vec::new(),
            consumed: Vec::new(),
            depth: 0,
            callee: None,
            demanded: BTreeMap::new(),
            diagnostics: Vec::new(),
        }
    }

    // ---- carrier ----

    /// 式の値が運ぶ借用を記録する
    fn carry(&mut self, expr: hir::ExprId, carrier: Carrier) {
        self.carriers.entry(expr).or_default().insert(carrier);
    }

    /// 部分式が運ぶものをそのまま親へ引き継ぐ(枝の合流もここ)
    fn carry_from(&mut self, expr: hir::ExprId, from: hir::ExprId) {
        let Some(carried) = self.carriers.get(&from).cloned() else {
            return;
        };
        self.carriers.entry(expr).or_default().extend(carried);
    }

    /// 束縛へ流れ込む carrier を足す
    fn flow(&mut self, local: hir::LocalId, from: hir::ExprId) {
        let Some(carried) = self.carriers.get(&from).cloned() else {
            return;
        };
        self.flows.entry(local).or_default().extend(carried);
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

    /// `mark` 以降に作った借用を、`at` まで生かす。
    ///
    /// 呼び出しの実引数だけに使う。`at` は呼び出しの点で、実引数が枝を抜けて
    /// いても必ず作られる(`point` は `cur` が無くても点を確保する)ので、
    /// 要求点が消えることがない。消えうる合流点には `hold_through` を使う
    fn hold_until(&mut self, mark: usize, at: PointId) {
        for loan in mark..self.plan.loans.len() {
            self.loan_uses[loan].insert(at);
        }
    }

    /// `mark..held` の借用を、`from` 以降に作った点**すべて**で生かす。
    ///
    /// `for` / `match` / `with` は構文の区間そのものが借用の区間なので、合流点や
    /// 背辺の行き先を1つだけ要求点にすると、その区間を `return` で抜ける経路を
    /// 取りこぼす(合流点が生まれず、背辺も張られない)。区間の点は `alloc` の順に
    /// 連続しているので、区間を丸ごと要求点にできる。到達しない点はリージョンの
    /// 交わり(生誕から到達できる)で落ちるので、要求点を増やす側は**必ず拒否が
    /// 増える**だけになる。
    ///
    /// 伸ばすのは**区間に入る前に生まれた**借用だけ(`held` が上限)。区間の
    /// 中で生まれた借用まで伸ばすと、その借用元も区間の中で落ちるので、
    /// 「所有者より長生きする借用」を自分で作ってしまう
    fn hold_through(&mut self, mark: usize, held: usize, from: usize) {
        let through: Vec<PointId> = (from..self.plan.points.len())
            .map(|p| PointId(p as u32))
            .collect();
        for loan in mark..held {
            self.loan_uses[loan].extend(through.iter().copied());
        }
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
        if bound.owns() && self.owning(self.body.local(local).ty.as_ref()) {
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
            // `u.name` はフィールド射影、`u.?name` は optional の中身を1段
            // 挟んだフィールド射影(optional-field-access)
            hir::ExprKind::Field {
                recv,
                field,
                optional,
            } => {
                let mut place = self.place_of(*recv)?;
                if *optional {
                    place.path.push(Projection::OptionalPayload);
                }
                place.path.push(Projection::Field(*field));
                Some(place)
            }
            _ => None,
        }
    }

    /// 修飾を剥がした場所。`&mut xs` / `move l` の下にある場所を指す
    fn qualified_place(&self, expr: hir::ExprId) -> Option<Place> {
        match &self.body.expr(expr).kind {
            hir::ExprKind::Access { place, .. } => self.place_of(*place),
            _ => self.place_of(expr),
        }
    }

    /// 制御構文が**対象全体**に持つ所有モード(design.md 決定7、tasks 6.2〜6.4)。
    ///
    /// `Some(kind)` は借用、`None` は明示された消費。arm も周回も1つのモードを受け継ぐので、
    /// pattern ごと・要素ごとの部分 move は表現そのものが存在しない。
    ///
    /// 参照を先に見るのは、参照は所有を運ばないから。`match move r` の `r` が
    /// `&mut T` なら、動くのは参照であって借用先ではない。
    fn whole_mode(&self, subject: hir::ExprId) -> Option<hir::RefKind> {
        let expr = self.body.expr(subject);
        if let Some(kind) = expr.result.ty().and_then(|ty| ty.reference) {
            return Some(kind);
        }
        match &expr.kind {
            hir::ExprKind::Access { mode, .. } => match mode {
                AccessMode::Move => None,
                AccessMode::Mutable => Some(hir::RefKind::Mutable),
                AccessMode::Shared => Some(hir::RefKind::Shared),
            },
            // 修飾なしは、場所か一時値かにかかわらず共有借用。一時値が既に
            // 所有者であることは、消費を暗黙に選ぶ理由にはならない。所有を
            // arm/周回へ渡すのは `move` だけ(design.md 決定7)。
            _ => Some(hir::RefKind::Shared),
        }
    }

    /// `??` が所有を産むか(optional-core-type-checking、tasks 6.2)。
    ///
    /// 束縛済みの非 Copy な optional を素で開くと、取れるのは中身の借用だけで
    /// 所有は元の場所に残る。`move` を書いた場所と、そもそも所有者である一時値
    /// だけが所有を産む。Copy は複製なので所有が動かなくても値として渡せる。
    fn coalesce_owns(&self, lhs: hir::ExprId) -> bool {
        let expr = self.body.expr(lhs);
        if let hir::ExprKind::Access {
            mode: AccessMode::Move,
            ..
        } = expr.kind
        {
            return true;
        }
        self.place_of(lhs).is_none() || expr.result.ty().is_some_and(|ty| self.program.is_copy(ty))
    }

    /// 借用しか産まない `??` を所有の要る位置で使っている
    fn report_borrowed_coalesce(&mut self, lhs: hir::ExprId) {
        let Some(place) = self.place_of(lhs) else {
            return;
        };
        let shown = self.show_place(&place);
        let owner = self.body.local(place.root);
        self.diagnostics.push(
            Diag::at(
                self.body.expr(lhs).span,
                format!(
                    "{}: 束縛済みの `{shown}` を素の `??` で開いても取れるのは借用で、所有は動きません",
                    self.ctx
                ),
            )
            .label("借用しか産まない `??`")
            .help(format!("`move {shown} ?? ...` と書いてください"))
            .related(vec![Diag::at(
                owner.span,
                format!("`{}` はここで束縛されています", owner.name),
            )]),
        );
    }

    /// `match` の payload / `for` の要素を束ねる。モードは構文全体から来る。
    ///
    /// 借用のモードでは、束縛は対象の**射影**への借用になる。射影は容器全体と
    /// 保守的に重なる(`projections_overlap`)ので、対象そのものを触る式とは
    /// これまでどおり衝突する。消費のモードでは、束縛がその値の所有者になり、
    /// スコープを抜けるときに落ちる。
    fn bind_content(
        &mut self,
        at: hir::ExprId,
        local: hir::LocalId,
        scope: ScopeId,
        mode: Option<hir::RefKind>,
        subject: hir::ExprId,
        step: Projection,
    ) {
        let bound = match mode {
            Some(kind) => Bound::Borrowed(kind),
            None => Bound::Owned,
        };
        self.declare(local, scope, bound);
        if mode.is_some() {
            // 対象が運んでいた借用もそのまま引き継ぐ(参照越しの `match` など)
            self.flow(local, subject);
        }
        let point = self.point(Some(at), scope, Effect::Init(local));
        if let Some(kind) = mode
            && let Some(mut place) = self.qualified_place(subject)
        {
            place.path.push(step);
            let span = self.body.expr(subject).span;
            let loan = self.loan_at(place, kind, point, span);
            self.flows
                .entry(local)
                .or_default()
                .insert(Carrier::Loan(loan));
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

    /// 場所へのアクセスを1点として積む。借りるアクセスなら借用も作って返す。
    /// その借用を**値として運ぶ**かは呼び出し側が決める(代入先の排他アクセスは
    /// 誰にも渡らない)
    fn access(
        &mut self,
        expr: hir::ExprId,
        place: Place,
        mode: Mode,
        scope: ScopeId,
    ) -> Option<LoanId> {
        // 射影を消費したら根ごと消費され、残りはその場で落ちる
        let residue = (mode == Mode::Move && !place.path.is_empty()).then(|| Drop::Remaining {
            root: place.root,
            consumed: place.path.clone(),
        });
        // 借りるアクセスはその場で借用を作る。明示の `&`/`&mut` も、借用を
        // 受け取る位置への自動共有借用も同じ扱い(tasks 4.1)
        let kind = match mode {
            Mode::Shared => Some(hir::RefKind::Shared),
            Mode::Mutable => Some(hir::RefKind::Mutable),
            Mode::Read | Mode::Move => None,
        };
        let borrowed = kind.map(|kind| (kind, place.clone()));
        let point = self.point(Some(expr), scope, Effect::Access(Access { place, mode }));
        self.plan.accesses.insert(expr, point);
        let span = self.body.expr(expr).span;
        let loan = borrowed.map(|(kind, place)| self.loan_at(place, kind, point, span));
        if let Some(residue) = residue {
            self.residue.insert(point, vec![residue]);
        }
        loan
    }

    /// 借用を1つ作る。生誕点は既にある点で、生存区間は `regions` が決める
    fn loan_at(&mut self, place: Place, kind: hir::RefKind, point: PointId, span: Span) -> LoanId {
        let id = LoanId(self.plan.loans.len() as u32);
        self.plan.loans.push(Loan {
            place,
            kind,
            point,
            span,
        });
        self.plan.regions.push(BTreeSet::new());
        self.loan_uses.push(BTreeSet::from([point]));
        id
    }

    // ---- 走査 ----

    fn value(&mut self, id: hir::ExprId, scope: ScopeId, need: Need) {
        if let hir::ExprKind::Access { mode, place } = &self.body.expr(id).kind {
            let (mode, place) = (*mode, *place);
            let Some(resolved) = self.place_of(place) else {
                // `&f()` のように場所でないものを修飾している。借りる先も
                // 移す先も無いので断る(tasks 4.3)。計画は完全に残したいので
                // 中身は一時値として歩き続ける
                self.report_not_a_place(id, mode);
                return self.value(place, scope, Need::Take);
            };
            let mode = match mode {
                AccessMode::Shared => Mode::Shared,
                AccessMode::Mutable => Mode::Mutable,
                AccessMode::Move => Mode::Move,
            };
            if let Some(loan) = self.access(id, resolved, mode, scope) {
                self.carry(id, Carrier::Loan(loan));
            }
            return;
        }
        if let Some(place) = self.place_of(id) {
            return self.bare_place(id, place, scope, need);
        }
        self.compound(id, scope, need);
    }

    /// `&f()` / `&mut f()` / `move f()`。修飾は場所にしか掛けられない
    fn report_not_a_place(&mut self, id: hir::ExprId, mode: AccessMode) {
        let (what, help) = match mode {
            AccessMode::Shared => ("共有借用", "借りるものを先に束縛してください"),
            AccessMode::Mutable => ("排他借用", "借りるものを先に束縛してください"),
            AccessMode::Move => (
                "`move`",
                "一時値は既に所有者なので `move` は要りません。修飾を外してください",
            ),
        };
        let span = self.body.expr(id).span;
        self.diagnostics.push(
            Diag::at(
                span,
                format!("{}: 場所ではない値に{what}は掛けられません", self.ctx),
            )
            .label("場所ではない値への所有権修飾")
            .help(help),
        );
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
            // 参照そのものを渡すのは再借用。所有は動かない。元の借用より
            // 長く生きられないことは、下の `Carrier::Local` が元の借用を
            // 引き継ぐことで効く(tasks 4.1)
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
        // 参照束縛をそのまま渡す/再借用するときは、元が持っていた借用も
        // 一緒に運ばれる(tasks 4.1 の再借用の制約)
        let inherits = place.path.is_empty() && reborrow.is_some();
        let root = place.root;
        if let Some(loan) = self.access(id, place, mode, scope) {
            self.carry(id, Carrier::Loan(loan));
        }
        if inherits {
            self.carry(id, Carrier::Local(root));
        }
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

            // 場所として畳めなかった読み(レシーバが一時値のフィールド)
            hir::ExprKind::Field { recv, .. } => self.value(*recv, scope, Need::Read),

            // 構築(struct リテラル・配列リテラル・enum 構築)は所有を暗黙に
            // 受け取る。`move` を要求している仕様の文は「所有の**引数**」と
            // 「消費レシーバ」だけ(ownership-and-borrowing「Transfer and copying
            // remain visible」)で、構築は所有が値の中へ**入る**位置なので、
            // 枝の末尾や戻り値と同じく境界がそれを言っている
            // (struct-shape-checking「Structs are owned values」、
            // data-bearing-enums「Payload ownership」、tasks 6.1 / 6.3 / 6.4)。
            // `with` の提供値は所有モードを持つのでここには居ない(tasks 5.4)
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
                self.flow(*local, *value);
                self.declare(*local, scope, Bound::Let);
                self.point(Some(id), scope, Effect::Init(*local));
            }

            hir::ExprKind::AssignLocal { local, value } => {
                self.value(*value, scope, Need::Take);
                self.flow(*local, *value);
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

            // `clone()` はレシーバを共有借用で読んで**新しい所有**を産む。
            // 借用は結果へ引き継がない — そこが借用を運ぶ他の全ての式との
            // 違いで、`carry_from` を呼ばないことがそのまま所有の独立性になる
            // (design.md 決定4、tasks 6.5)
            hir::ExprKind::Clone(inner) => self.value(*inner, scope, Need::Read),
            hir::ExprKind::Arith { lhs, rhs, .. } | hir::ExprKind::Eq { lhs, rhs } => {
                self.value(*lhs, scope, Need::Read);
                self.value(*rhs, scope, Need::Read);
            }

            // 右辺は左辺が `nil` のときだけ走る。所有モードは左辺が決める
            // (optional-core-type-checking「Coalescing follows its ownership
            // mode」、tasks 6.2)。Copy は複製、束縛済みの非 Copy を素で開くと
            // 借用、`move` と一時値は所有。結果が所有なら右辺の値も所有で要る
            hir::ExprKind::Coalesce { lhs, rhs } => {
                self.value(*lhs, scope, Need::Read);
                let owning = self.coalesce_owns(*lhs);
                if !owning && matches!(need, Need::Take | Need::Argument(..)) {
                    self.report_borrowed_coalesce(*lhs);
                }
                let branch = self.point(Some(id), scope, Effect::Nop);
                // 右辺は左辺が空のときだけ評価され、そのときだけ所有が動く。
                // 辺が `branch` から合流点へ直行するので、その経路は計画にある
                self.value(*rhs, scope, if owning { Need::Take } else { Need::Read });
                self.carry_from(id, *lhs);
                self.carry_from(id, *rhs);
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
                    if let Some(carried) = self.carriers.get(value).cloned() {
                        self.returns.extend(carried);
                    }
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
                self.carry_from(id, *last);
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
                self.carry_from(id, *then);
                if let Some(orelse) = orelse {
                    self.carry_from(id, *orelse);
                }
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
                    self.depth += 1;
                    self.value(*inner, inner_scope, Need::Read);
                    self.depth -= 1;
                    if let Some(from) = self.cur {
                        self.edge(from, head, vec![inner_scope]);
                    }
                    self.edge(test, after, Vec::new());
                }
                self.cur = Some(after);
            }

            // 反復のモードは対象全体に1つ(design.md 決定7、tasks 6.4)。
            // 素の `for` は配列を共有借用して要素も借用で束ね、`&mut` は排他、
            // `move` は配列を消費して要素の所有を周回へ渡す。配列そのものの
            // 借用は周回のあいだずっと生きているので、反復中の構造変更は
            // そこで衝突する
            hir::ExprKind::For {
                var,
                iter,
                body: inner,
            } => {
                let mark = self.plan.loans.len();
                self.value(*iter, scope, Need::Read);
                let mode = self.whole_mode(*iter);
                let head = self.point(Some(id), scope, Effect::Nop);
                let after = self.alloc(Some(id), scope, Effect::Nop);
                let inner_scope = self.scope(Some(scope));
                self.cur = Some(head);
                let (opened, held) = (self.plan.points.len(), self.plan.loans.len());
                self.bind_content(
                    id,
                    *var,
                    inner_scope,
                    mode,
                    *iter,
                    Projection::ArrayElement(None),
                );
                self.depth += 1;
                self.value(*inner, inner_scope, Need::Read);
                self.depth -= 1;
                if let Some(from) = self.cur {
                    self.edge(from, head, vec![inner_scope]);
                }
                self.edge(head, after, Vec::new());
                // 対象は周回中ずっと借りられている。`head` だけを要求点にすると
                // 背辺の張られない本体(全経路が抜ける)を取りこぼす
                self.hold_through(mark, held, opened);
                self.cur = Some(after);
            }

            // 提供値の所有モードは `Need::Read` がそのまま表す(tasks 5.4)。
            // 修飾を書かない束縛済みの場所は共有借用になり、所有者は `with` の
            // 後も残る。一時値は場所ではないので提供スコープの所有のまま。
            // 明示の `&mut` / `move` は `Access` としてそのまま通る
            hir::ExprKind::With {
                provisions,
                body: inner,
            } => {
                let mark = self.plan.loans.len();
                for provision in provisions {
                    if let Some(value) = provision.value {
                        self.value(value, scope, Need::Read);
                    }
                }
                let (opened, held) = (self.plan.points.len(), self.plan.loans.len());
                // 同じ `with` の提供は互いを見ない。全部評価してから積む
                let depth = self.provided.len();
                for provision in provisions {
                    self.provided
                        .push((provision.slot, provision.value, self.depth));
                }
                let inner_scope = self.scope(Some(scope));
                self.value(*inner, inner_scope, need);
                self.carry_from(id, *inner);
                self.provided.truncate(depth);
                if let Some(from) = self.cur {
                    let join = self.alloc(Some(id), scope, Effect::Nop);
                    self.edge(from, join, vec![inner_scope]);
                    self.cur = Some(join);
                }
                // 提供の借用は提供本体の間ずっと生きている(tasks 5.4)
                self.hold_through(mark, held, opened);
            }

            // arm は上から順に試される。pattern が外れても guard が偽でも次の
            // arm へ進むので、その連鎖をそのまま辺にする。畳んで「合流点へ抜ける」
            // にすると、guard が動かした所有が後続 arm に見えなくなる。
            //
            // モードは対象全体に1つ(design.md 決定7、tasks 6.3)。素の `match`
            // は対象を共有借用して payload も借用で束ね、`&mut` は排他、`move`
            // は enum ごと消費して選ばれた payload の所有を arm へ渡す。選ばれ
            // なかった中身は消費した `match` そのものが手放すので、根が部分的に
            // move された状態は現れない
            hir::ExprKind::Match { subject, arms } => {
                let mark = self.plan.loans.len();
                self.value(*subject, scope, Need::Read);
                let mode = self.whole_mode(*subject);
                let branch = self.point(Some(id), scope, Effect::Nop);
                let (opened, held) = (self.plan.points.len(), self.plan.loans.len());
                // 「ここまでの arm がどれも取らなかった」点
                let mut fallthrough = branch;
                let mut ends: Vec<(PointId, Vec<ScopeId>)> = Vec::new();
                for arm in arms {
                    let arm_scope = self.scope(Some(scope));
                    let next = self.alloc(Some(id), scope, Effect::Nop);
                    // pattern が合わなければ、この arm には入らずに次を試す
                    self.edge(fallthrough, next, Vec::new());
                    self.cur = Some(fallthrough);
                    if let hir::Pattern::Variant { variant, bindings } = &arm.pattern {
                        for (index, bound) in bindings.iter().enumerate() {
                            let Some(bound) = bound else { continue };
                            self.bind_content(
                                id,
                                *bound,
                                arm_scope,
                                mode,
                                *subject,
                                Projection::EnumPayload(*variant, index),
                            );
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
                    self.carry_from(id, arm.body);
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
                    // どの arm も抜ける。合流点は生まれないが、対象は arm の
                    // 間ずっと借りられている
                    self.hold_through(mark, held, opened);
                    self.cur = None;
                    return;
                }
                let join = self.alloc(Some(id), scope, Effect::Nop);
                for (from, exits) in ends {
                    self.edge(from, join, exits);
                }
                // 対象は match の間ずっと借りられている(design.md 決定7)
                self.hold_through(mark, held, opened);
                self.cur = Some(join);
            }

            // 実引数の借用は呼び出しの間ずっと生きている。だから引数の評価を
            // 始める前に印を付けて、終わったら「呼び出しの点」を要求点として
            // 全部に足す。これで `f(&mut u, &u)` が衝突として見える(tasks 4.3)
            hir::ExprKind::Call(call) => {
                let mark = self.plan.loans.len();
                let site = match call {
                    hir::Call::Direct { callable, args }
                    | hir::Call::Associated { callable, args } => {
                        self.callee = Some(program.show_callable(*callable));
                        let params = self.declared_params(*callable);
                        self.arguments(scope, &params, args);
                        Some(CallSite {
                            callee: Callee::Body(*callable),
                            recv: None,
                            args: args.clone(),
                        })
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
                        // レシーバの中に呼び出しがあると、その `arguments` が
                        // 表示名を消していく。実引数のために置き直す
                        self.callee = Some(program.show_callable(*callable));
                        self.arguments(scope, &params, args);
                        Some(CallSite {
                            callee: Callee::Body(*callable),
                            recv: Some(*recv),
                            args: args.clone(),
                        })
                    }
                    hir::Call::Slot {
                        slot, method, args, ..
                    } => {
                        self.callee = Some(program.show_trait_method(*method));
                        let params: Vec<Option<hir::Type>> = program.trait_methods[*method]
                            .params
                            .iter()
                            .map(|ty| Some(ty.clone()))
                            .collect();
                        self.arguments(scope, &params, args);
                        self.check_provider(id, *slot, *method);
                        Some(CallSite {
                            callee: Callee::Trait(*method),
                            // レシーバは提供が持つ実体。この本体の `with` が
                            // 置いたものなら、その提供値がそのまま借用元になる
                            // (tasks 5.4)
                            recv: self.provision(*slot).and_then(|(value, _)| value),
                            args: args.clone(),
                        })
                    }
                    // enum の構築。呼び出しの綴りだが受け取るのは payload の所有で、
                    // struct リテラルや配列リテラルと同じ構築位置なので暗黙 move
                    hir::Call::Ctor { args, .. } => {
                        for arg in args {
                            self.value(*arg, scope, Need::Take);
                        }
                        None
                    }
                };
                let at = self.point(Some(id), scope, Effect::Nop);
                self.hold_until(mark, at);
                if let Some(site) = site {
                    // 参照を返す呼び出しだけが借用を運ぶ
                    if body
                        .expr(id)
                        .result
                        .ty()
                        .is_some_and(|ty| ty.reference.is_some())
                    {
                        self.carry(id, Carrier::Call(id));
                    }
                    self.calls.insert(id, site);
                }
            }
        }
    }

    // ---- 提供(tasks 5.4) ----

    /// このスロットへいま効いている提供。内側の `with` が外側を隠す。
    /// 外側の `Option` は「この本体が提供しているか」、内側は「実体を運ぶか」
    fn provision(&self, slot: hir::SlotId) -> Option<(Option<hir::ExprId>, u32)> {
        self.provided
            .iter()
            .rev()
            .find(|(s, _, _)| *s == slot)
            .map(|(_, value, depth)| (*value, *depth))
    }

    /// 実体を運ぶ提供の運び方。
    ///
    /// 判定はソースの形ではなく**走査が実際に何をしたか**から取る。構文だけを
    /// 見ると、場所を素通しする形(`{ s }`、枝の末尾)や参照を返す呼び出しが
    /// 「一時値 = 提供スコープの所有」に落ちて、借用しか無い実体に消費レシーバを
    /// 通してしまう。
    ///
    /// 順に:明示の修飾 → 結果が参照型ならその強さ → **借用を運んでいるか**
    /// (`carriers` は「この値は誰かの記憶を指している」の唯一の定義)。
    /// どれでもなければ、この式が作った所有の一時値。
    fn provider_mode(&self, value: hir::ExprId) -> Provider {
        match &self.body.expr(value).kind {
            hir::ExprKind::Access {
                mode: AccessMode::Shared,
                ..
            } => return Provider::Shared,
            hir::ExprKind::Access {
                mode: AccessMode::Mutable,
                ..
            } => return Provider::Mutable,
            hir::ExprKind::Access {
                mode: AccessMode::Move,
                ..
            } => return Provider::Moved,
            _ => {}
        }
        match self
            .body
            .expr(value)
            .result
            .ty()
            .and_then(|ty| ty.reference)
        {
            Some(hir::RefKind::Shared) => Provider::Shared,
            Some(hir::RefKind::Mutable) => Provider::Mutable,
            // 所有の形をしていても、借用を運んでいるなら実体は他人のもの
            None if self.carriers.contains_key(&value) => Provider::Shared,
            None => Provider::Temporary,
        }
    }

    /// 契約のレシーバが要る強さを、いま効いている提供が満たしているか
    /// (with-provision-type-checking「Shared provider」/「Mutable provider」)。
    ///
    /// 提供がこの本体に無ければ、強さを決めたのは呼び出し元の `with` なので
    /// そちらで見る。型だけの提供に値レシーバを呼ぶ食い違いは要求解析が報告する。
    ///
    /// 消費レシーバ(`self`)は提供の実体を手放すので、その後の使用も見る
    /// (tasks 6.6)。提供された実体には局所束縛が無いので、move 済みは
    /// `consumed` が覚える。
    ///
    /// ponytail: `consumed` は経路を区別しない。枝ごとに1度ずつ消費する形も
    /// 2件目として断る。消費が**1本の経路で二度**起きないことだけを保証すれば
    /// よいので、経路を畳んで見る側は必ず拒否が増えるだけになる。周回は経路の
    /// 畳み込みで消えてしまうため、深さで別に見る
    fn check_provider(&mut self, at: hir::ExprId, slot: hir::SlotId, method: hir::TraitMethodId) {
        let Some((Some(value), provided_at)) = self.provision(slot) else {
            return;
        };
        let Some(receiver) = self.program.trait_methods[method].receiver else {
            return;
        };
        if let Some((_, _, consumer)) = self
            .consumed
            .iter()
            .find(|(s, v, _)| *s == slot && *v == value)
        {
            let name = &self.program.slots[slot].name;
            let consumer = self.body.expr(*consumer).span;
            self.diagnostics.push(
                Diag::at(
                    self.body.expr(at).span,
                    format!(
                        "{}: `{name}` へ提供された実体は既に消費されているので使えません",
                        self.ctx
                    ),
                )
                .label("消費済みの提供の使用")
                .related(vec![Diag::at(consumer, "ここで消費されました")]),
            );
            return;
        }
        let mode = self.provider_mode(value);
        if receiver == ReceiverMode::Owned && mode.owns() {
            if self.depth > provided_at {
                let name = &self.program.slots[slot].name;
                self.diagnostics.push(
                    Diag::at(
                        self.body.expr(at).span,
                        format!(
                            "{}: `{name}` へ提供された実体を周回のたびに消費しています",
                            self.ctx
                        ),
                    )
                    .label("周回の中の消費レシーバ")
                    .help("`with` をループの中へ入れて、周回ごとに提供し直してください")
                    .related(vec![Diag::at(
                        self.body.expr(value).span,
                        format!("`{name}` はループの外で提供されています"),
                    )]),
                );
            }
            self.consumed.push((slot, value, at));
        }
        let (enough, want, fix) = match receiver {
            // 所有からも排他からも共有は取れる
            ReceiverMode::Shared => (true, "&self", "&"),
            ReceiverMode::Mutable => (
                mode == Provider::Mutable || mode.owns(),
                "&mut self",
                "&mut",
            ),
            ReceiverMode::Owned => (mode.owns(), "self", "move"),
        };
        if enough {
            return;
        }
        let name = &self.program.slots[slot].name;
        self.diagnostics.push(
            Diag::at(
                self.body.expr(at).span,
                format!(
                    "{}: `{}` のレシーバは `{want}` ですが、`{name}` への提供は{}です",
                    self.ctx,
                    self.program.show_trait_method(method),
                    mode.spelling()
                ),
            )
            .label("提供の所有モードが足りない")
            .help(format!("`with {name}({fix} ...)` と書いてください"))
            .related(vec![Diag::at(
                self.body.expr(value).span,
                format!("`{name}` はここで提供されています"),
            )]),
        );
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

    // ---- carrier の解決(tasks 4.4) ----

    /// carrier を借用の出どころへ展開する。
    ///
    /// 呼び出し先の要約がまだ空なら何も出さない。要約は増える一方なので、
    /// 空から始めても外側の反復で必ず追いつく
    fn expand(
        &self,
        carriers: &BTreeSet<Carrier>,
        holds: &BTreeMap<hir::LocalId, BTreeSet<Origin>>,
        origins: &BTreeMap<hir::ExprId, BTreeSet<Origin>>,
        summaries: &Summaries,
        unrooted: &mut BTreeMap<hir::ExprId, Unrooted>,
    ) -> BTreeSet<Origin> {
        let mut out = BTreeSet::new();
        for carrier in carriers {
            match carrier {
                Carrier::Loan(loan) => {
                    out.insert(Origin::Loan(*loan));
                }
                Carrier::Local(local) => {
                    out.extend(holds.get(local).into_iter().flatten().copied());
                }
                Carrier::Call(expr) => {
                    let Some(site) = self.calls.get(expr) else {
                        continue;
                    };
                    let Some(callee) = summaries.get(&site.callee) else {
                        continue;
                    };
                    for place in callee {
                        let arg = match place.input {
                            Input::Receiver => site.recv,
                            Input::Param(index) => site.args.get(index).copied(),
                        };
                        // ponytail: origin の射影で狭めず、実引数の場所を丸ごと
                        // 借りたことにする。親の場所は子と重なる全てと重なる
                        // (`overlaps` は接頭辞判定)ので、狭めない方は必ず
                        // より多く拒否する。狭めるのは後の最適化
                        // スロットのレシーバ。提供がこの本体の `with` に無ければ
                        // 借用元を名指せない(tasks 5.4)
                        let provided = place.input == Input::Receiver
                            && matches!(site.callee, Callee::Trait(_));
                        let Some(arg) = arg else {
                            unrooted.insert(*expr, Unrooted::Provider);
                            continue;
                        };
                        let found = origins.get(&arg).cloned().unwrap_or_default();
                        // 場所でない実引数から借りたのに借用が付いていない =
                        // 借用元は評価の途中で作られた一時値。提供の実体なら
                        // 直し方が違うので言い分ける
                        if found.is_empty() && self.place_of(arg).is_none() {
                            unrooted.insert(
                                *expr,
                                if provided {
                                    Unrooted::Provider
                                } else {
                                    Unrooted::Temporary
                                },
                            );
                        }
                        out.extend(found);
                    }
                }
            }
        }
        out
    }

    /// 与えられた要約のもとで、束縛と式が持つ借用を不動点まで閉じる。
    ///
    /// 参照の引数だけを種にする。所有の引数は本体の中で落ちるので、そこからの
    /// 借用は入力の provenance にならない(戻せば `report_escapes` が断る)
    fn resolve(&mut self, summaries: &Summaries) {
        let mut holds: BTreeMap<hir::LocalId, BTreeSet<Origin>> = self
            .inputs
            .iter()
            .map(|(local, input)| (*local, BTreeSet::from([Origin::Input(*input)])))
            .collect();
        let mut origins: BTreeMap<hir::ExprId, BTreeSet<Origin>> = BTreeMap::new();
        let mut unrooted = BTreeMap::new();
        loop {
            let mut changed = false;
            for (expr, carriers) in &self.carriers {
                let next = self.expand(carriers, &holds, &origins, summaries, &mut unrooted);
                let slot = origins.entry(*expr).or_default();
                let before = slot.len();
                slot.extend(next);
                changed |= slot.len() != before;
            }
            for (local, carriers) in &self.flows {
                let next = self.expand(carriers, &holds, &origins, summaries, &mut unrooted);
                let slot = holds.entry(*local).or_default();
                let before = slot.len();
                slot.extend(next);
                changed |= slot.len() != before;
            }
            if !changed {
                break;
            }
        }
        // 途中の反復では要約がまだ育っていないので、最後にもう一度だけ判定する
        unrooted.clear();
        self.returned = self.expand(
            &self.returns.clone(),
            &holds,
            &origins,
            summaries,
            &mut unrooted,
        );
        for carriers in self.carriers.values() {
            self.expand(carriers, &holds, &origins, summaries, &mut unrooted);
        }
        self.unrooted = unrooted;
        self.holds = holds;
        self.origins = origins;
    }

    /// 借用の出どころを入力の場所へ戻す。入力に根を持たないなら `None` で、
    /// それは本体の中で終わる場所の借用 = 返せない借用
    fn input_place(&self, origin: Origin) -> Option<InputPlace> {
        match origin {
            Origin::Input(input) => Some(InputPlace {
                input,
                path: Vec::new(),
            }),
            Origin::Loan(loan) => Some(InputPlace {
                input: *self.inputs.get(&self.plan.loans[loan.index()].place.root)?,
                path: self.plan.loans[loan.index()].place.path.clone(),
            }),
        }
    }

    fn provenance_origins(&self) -> BTreeSet<InputPlace> {
        self.returned
            .iter()
            .filter_map(|origin| self.input_place(*origin))
            .collect()
    }

    // ---- リージョン(tasks 4.1) ----

    /// 借用ごとの生存区間を決める。
    ///
    /// 「生まれた点から到達できる」かつ「要る点へ到達できる」点だけ。これが
    /// 制約を満たす**最小**の点集合になる(design.md 決定6)。字句スコープは
    /// 一切見ない
    fn regions(&mut self) {
        // 参照束縛を触った点は、その束縛が持つ借用の要求点
        for (point, effect) in self.plan.points.iter().enumerate() {
            let Effect::Access(access) = &effect.effect else {
                continue;
            };
            for origin in self.holds.get(&access.place.root).into_iter().flatten() {
                if let Origin::Loan(loan) = origin {
                    self.loan_uses[loan.index()].insert(PointId(point as u32));
                }
            }
        }
        // 戻り値に載る借用は出口まで生きる
        for origin in &self.returned {
            if let Origin::Loan(loan) = origin {
                self.loan_uses[loan.index()].insert(self.plan.exit);
            }
        }

        let count = self.plan.points.len();
        let mut successors: Vec<BTreeSet<PointId>> = vec![BTreeSet::new(); count];
        let mut predecessors: Vec<BTreeSet<PointId>> = vec![BTreeSet::new(); count];
        for edge in &self.plan.edges {
            successors[edge.from.index()].insert(edge.to);
            predecessors[edge.to.index()].insert(edge.from);
        }
        self.outliving = Vec::with_capacity(self.plan.loans.len());
        for loan in 0..self.plan.loans.len() {
            let born = self.plan.loans[loan].point;
            let forward = spread(&successors, BTreeSet::from([born]), None);
            let backward = spread(&predecessors, self.loan_uses[loan].clone(), None);
            let region: BTreeSet<PointId> = forward.intersection(&backward).copied().collect();
            // 「借用が生まれた点を通らずに使用へ届く」点だけ。ループの中では
            // 背辺の先も点としてはリージョンに入るが、そこから使用へ行くには
            // もう一度借り直すしかない。**次の周の**借用と混ざらないよう、
            // 借用元の破棄を見るときはこちらを使う
            let again = spread(&predecessors, self.loan_uses[loan].clone(), Some(born));
            self.outliving
                .push(region.intersection(&again).copied().collect());
            self.plan.regions[loan] = region;
        }
    }

    /// その点で生きている借用。点 ID で引ける形に畳む
    fn live_loans(&self) -> Vec<BTreeSet<LoanId>> {
        let mut live: Vec<BTreeSet<LoanId>> = vec![BTreeSet::new(); self.plan.points.len()];
        for (loan, region) in self.plan.regions.iter().enumerate() {
            for point in region {
                live[point.index()].insert(LoanId(loan as u32));
            }
        }
        live
    }
}

/// 決定的な作業キューで到達集合を広げる。`edges` は隣接、`seed` は出発点。
/// `blocked` を通る経路は数えない
fn spread(
    edges: &[BTreeSet<PointId>],
    seed: BTreeSet<PointId>,
    blocked: Option<PointId>,
) -> BTreeSet<PointId> {
    let mut reached: BTreeSet<PointId> = seed.into_iter().filter(|p| Some(*p) != blocked).collect();
    let mut queue = reached.clone();
    while let Some(point) = queue.pop_first() {
        for next in &edges[point.index()] {
            if Some(*next) != blocked && reached.insert(*next) {
                queue.insert(*next);
            }
        }
    }
    reached
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

    /// 可変に触れる場所か。`let mut` の束縛、既に `&mut` である参照、そして
    /// `&mut` で反復・match した中身の束縛(design.md 決定7)
    fn mutable_root(&self, root: hir::LocalId) -> bool {
        let local = self.body.local(root);
        local.mutable
            || self.bindings.get(&root) == Some(&Bound::Borrowed(hir::RefKind::Mutable))
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
        let live = self.live_loans();
        for (index, (facts, point)) in input.iter().zip(&self.plan.points).enumerate() {
            // 到達しない点。状態が無いので何も言えない
            let Some(facts) = facts else { continue };
            let span = point.expr.map(|e| self.body.expr(e).span);
            let at = PointId(index as u32);
            match &point.effect {
                Effect::Access(access) => {
                    self.report_access(&mut found, facts, access, point.expr, span);
                    self.report_conflicts(&mut found, at, &access.place, access.mode, span, &live);
                }
                Effect::Assign(local) => {
                    self.report_assign(&mut found, facts, *local, span);
                    // 代入は古い値を落とす。束縛そのものへの排他アクセス
                    let place = Place {
                        root: *local,
                        path: Vec::new(),
                    };
                    self.report_conflicts(&mut found, at, &place, Mode::Mutable, span, &live);
                }
                Effect::Init(_) | Effect::Nop => {}
            }
        }
        self.diagnostics.extend(found);
    }

    /// 生きている借用との衝突(tasks 4.3)。
    ///
    /// 自分自身が作った借用は数えない。それ以外で場所が重なるものだけを見る
    fn report_conflicts(
        &self,
        found: &mut Vec<Diag>,
        at: PointId,
        place: &Place,
        mode: Mode,
        span: Option<Span>,
        live: &[BTreeSet<LoanId>],
    ) {
        let ctx = &self.ctx;
        let shown = self.show_place(place);
        for loan in &live[at.index()] {
            let held = &self.plan.loans[loan.index()];
            if held.point == at || !overlaps(&held.place, place) {
                continue;
            }
            let (msg, label, note) = match (mode, held.kind) {
                (Mode::Move, hir::RefKind::Shared) | (Mode::Move, hir::RefKind::Mutable) => (
                    format!("{ctx}: `{shown}` は借用されているので move できません"),
                    "借用中の値の move",
                    "ここで借用しています",
                ),
                (Mode::Mutable, hir::RefKind::Shared) => (
                    format!("{ctx}: `{shown}` は共有借用されている間は排他的に触れません"),
                    "共有借用との衝突",
                    "ここで共有借用しています",
                ),
                (Mode::Mutable, hir::RefKind::Mutable) => (
                    format!(
                        "{ctx}: `{shown}` は既に排他借用されているので、もう一度は借りられません"
                    ),
                    "排他借用の重なり",
                    "ここで排他借用しています",
                ),
                (Mode::Read | Mode::Shared, hir::RefKind::Mutable) => (
                    format!("{ctx}: `{shown}` は排他借用されている間は読めません"),
                    "排他借用との衝突",
                    "ここで排他借用しています",
                ),
                // 共有どうしは重なってよい
                (Mode::Read | Mode::Shared, hir::RefKind::Shared) => continue,
            };
            found.push(
                Diag::from_span(span, msg)
                    .label(label)
                    .related(vec![Diag::at(held.span, note)]),
            );
            // 同じアクセスに衝突を並べても直し方は増えない
            break;
        }
    }

    /// 所有者より長く生きる借用(tasks 4.3 / 4.6)。
    ///
    /// 借用1つにつき1件。戻り値へ漏れるものを先に見て、残りを破棄と突き合わせる
    fn report_escapes(&mut self) {
        let ctx = self.ctx.clone();
        let mut found = Vec::new();
        for (expr, why) in &self.unrooted {
            let (msg, label, help) = match why {
                Unrooted::Temporary => (
                    "この呼び出しは実引数から借りて返しますが、その実引数は式の中で終わる一時値です",
                    "一時値からの借用",
                    "借用元を先に束縛してから渡してください",
                ),
                Unrooted::Provider => (
                    "スロット経由の呼び出しはレシーバから借りて返しますが、提供された実体の借用元をこの本体では名指せません",
                    "提供された実体からの借用",
                    "束縛した値を同じ本体の `with` で提供するか、借用を返さない契約にしてください",
                ),
            };
            found.push(
                Diag::at(self.body.expr(*expr).span, format!("{ctx}: {msg}"))
                    .label(label)
                    .help(help),
            );
        }
        let escaping: BTreeSet<LoanId> = self
            .returned
            .iter()
            .filter(|origin| self.input_place(**origin).is_none())
            .filter_map(|origin| match origin {
                Origin::Loan(loan) => Some(*loan),
                Origin::Input(_) => None,
            })
            .collect();
        for (index, loan) in self.plan.loans.iter().enumerate() {
            let id = LoanId(index as u32);
            let shown = self.show_place(&loan.place);
            if escaping.contains(&id) {
                found.push(
                    Diag::at(
                        loan.span,
                        format!(
                            "{ctx}: `{shown}` の借用は返せません。借用元がこの本体の中で終わります"
                        ),
                    )
                    .label("所有者より長生きする借用")
                    .help("所有ごと返すか、入力から借りたものを返してください"),
                );
                continue;
            }
            // 借用元が落ちる辺。その先でも借用が生きているなら宙に浮く
            let region = &self.outliving[index];
            let Some(edge) = self.plan.edges.iter().find(|edge| {
                region.contains(&edge.to)
                    && edge.drops.iter().any(|drop| match drop {
                        Drop::Local(local) => *local == loan.place.root,
                        Drop::Remaining { root, .. } => *root == loan.place.root,
                    })
            }) else {
                continue;
            };
            let owner = self.body.local(loan.place.root);
            let _ = edge;
            found.push(
                Diag::at(
                    loan.span,
                    format!("{ctx}: `{shown}` の借用は借用元より長く生きています"),
                )
                .label("所有者より長生きする借用")
                .related(vec![Diag::at(
                    owner.span,
                    format!("`{}` はスコープの終わりで落ちます", owner.name),
                )]),
            );
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
        // 借用で反復・match した対象からは、要素や payload の所有を取り出せない。
        // 部分 move は受け付けない(design.md 決定7)
        if access.mode == Mode::Move && matches!(self.bindings.get(&root), Some(Bound::Borrowed(_)))
        {
            let local = self.body.local(root);
            found.push(
                Diag::from_span(
                    span,
                    format!("{ctx}: `{shown}` は借用で束ねた名前なので move できません"),
                )
                .label("借用された束縛の move")
                .help("要素や payload の所有が要るなら、対象そのものに `move` を付けて反復・match してください")
                .related(vec![Diag::at(
                    local.span,
                    format!("`{}` はここで束ねられています", local.name),
                )]),
            );
        }
        // optional の中身は「あるかもしれない」場所なので、そこから所有を
        // 持ち出す形は無い(optional-field-access)。所有が要るなら先に `??`
        if access.mode == Mode::Move && access.place.path.contains(&Projection::OptionalPayload) {
            let local = self.body.local(root);
            found.push(
                Diag::from_span(
                    span,
                    format!("{ctx}: `{shown}` は optional の中身への射影なので move できません"),
                )
                .label("optional 射影からの move")
                .help("`??` で中身を取り出してから所有を動かしてください")
                .related(vec![Diag::at(
                    local.span,
                    format!("`{}` はここで束縛されています", local.name),
                )]),
            );
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
                    // 共有で束ねた要素・payload は、対象の側のモードで決まる
                    Some(Bound::Borrowed(hir::RefKind::Shared)) => {
                        base.help("対象に `&mut` を付けて反復・match してください")
                    }
                    // ponytail: 消費で束ねた名前(`for u in move xs`)は不変。
                    // 所有はあるので変えてよい値だが、`mut` を書ける構文が無い。
                    // 拒否が増えるだけなので、要ると分かったら `for mut u in ...`
                    // を足す
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
                        // 経路によっては move 済み(`owned` は真だが `moved` も
                        // 付いている)場所も計画には残す(design.md 決定8)。
                        // 計画だけでは取った経路で move 済みの値を落とすので、
                        // 場所ごとの実行時 drop フラグが要る — それは評価器の
                        // 持ち物(phase 7、tasks 7.4)
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

/// 本体1つを歩いて、要約に依らない事実を全部作る(check の第1段)。
fn walk_body(program: &hir::Program, id: hir::BodyId) -> Build<'_> {
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
    let params: Vec<(hir::LocalId, Input)> = match id {
        hir::BodyId::Callable(callable) => body
            .receiver
            .iter()
            .map(|local| (*local, Input::Receiver))
            .chain(
                program.callables[callable]
                    .params
                    .iter()
                    .enumerate()
                    .map(|(index, local)| (*local, Input::Param(index))),
            )
            .collect(),
        hir::BodyId::Test(_) => Vec::new(),
    };
    for (local, input) in params {
        build.declare(local, root, Bound::Param);
        // 参照で受け取った入力は、呼び出し側の場所の代理。この本体の中に
        // 借用元が無いので、provenance の根になる(design.md 決定6)
        if body
            .local(local)
            .ty
            .as_ref()
            .is_some_and(|ty| ty.reference.is_some())
        {
            build.inputs.insert(local, input);
        }
        build.point(None, root, Effect::Init(local));
    }

    // 本体は値ベース。最後の式だけが戻り値なので、そこだけ所有を要求する
    if let Some((last, init)) = body.root.split_last() {
        for e in init {
            build.value(*e, root, Need::Read);
        }
        build.value(*last, root, Need::Take);
        if let Some(carried) = build.carriers.get(last).cloned() {
            build.returns.extend(carried);
        }
    }
    if let Some(from) = build.cur {
        build.edge(from, exit, vec![root]);
    }
    build
}

impl Build<'_> {
    /// 要約が閉じた後に、リージョン・破棄・診断を確定する(check の第3段)。
    fn finish(mut self) -> (BodyPlan, Vec<Diag>) {
        let input = self.solve();
        self.settle(&input);
        self.regions();
        self.report(&input);
        self.report_escapes();
        (self.plan, self.diagnostics)
    }
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
            for (loan_id, loan) in plan.loans() {
                let points: Vec<String> = plan
                    .region(loan_id)
                    .iter()
                    .map(|p| format!("#{}", p.index()))
                    .collect();
                let _ = writeln!(
                    out,
                    "  loan#{} {} local#{}({}){} @ #{} region [{}]",
                    loan_id.index(),
                    loan.kind.spelling().trim_end(),
                    loan.place.root.index(),
                    body.local(loan.place.root).name,
                    show_path(program, &loan.place.path),
                    loan.point.index(),
                    points.join(" ")
                );
            }
            if let hir::BodyId::Callable(callable) = id
                && let Some(provenance) = self.provenance(*callable)
            {
                let origins: Vec<String> = provenance
                    .origins
                    .iter()
                    .map(|origin| show_input(program, origin))
                    .collect();
                let _ = writeln!(
                    out,
                    "  provenance {} [{}]",
                    provenance.kind.spelling().trim_end(),
                    origins.join(" ")
                );
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

fn show_input(program: &hir::Program, place: &InputPlace) -> String {
    let root = match place.input {
        Input::Receiver => "self".to_string(),
        Input::Param(index) => format!("param#{index}"),
    };
    format!("{root}{}", show_path(program, &place.path))
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
    // optional の射影は**次の1段に掛かる印**。ソースの `u.?name` と同じ綴りに
    // なるよう、単独では出さずに次の射影へ畳む
    let mut optional = false;
    for step in path {
        let marker = if std::mem::take(&mut optional) {
            "?"
        } else {
            ""
        };
        match step {
            Projection::Field(field) => {
                let _ = write!(out, ".{marker}{}", program.fields[*field].name);
            }
            Projection::OptionalPayload => optional = true,
            Projection::EnumPayload(_, n) => {
                let _ = write!(out, ".{marker}{n}");
            }
            Projection::ArrayElement(Some(n)) => {
                let _ = write!(out, "{marker}[{n}]");
            }
            Projection::ArrayElement(None) => {
                let _ = write!(out, "{marker}[_]");
            }
        }
    }
    if optional {
        out.push_str(".?");
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

    /// 周ごとに作って周ごとに借りるだけなら、背辺の破棄は借用を殺さない。
    /// 点の集合だけでは次の周の借用と区別が付かないので、借用が生まれた点を
    /// 通らずに使用へ届くかで見る
    #[test]
    fn 周ごとに借り直す値はループを通る() {
        accepted(&with_user(
            "  while true { let u = make()\n let v = &u\n assert v.id == 1 }",
        ));
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
            vec![("ここで借用しています".to_string(), "&u")]
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
  let u2 = make()
  if true {{ n = take(move u) }} else {{ n = 1 }}
  n + loops([1, 2]) + arms(Bronze) + provided() + take(pick(true, move u2))
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
  point#5 expr#7 scope#1 nop
  point#6 expr#12 scope#0 nop
  edge #0 -> #2
  edge #2 -> #3
  edge #3 -> #4
  edge #4 -> #5
  edge #5 -> #6 exits [scope#1]
  edge #3 -> #6 exits [scope#2]
  edge #6 -> #1 exits [scope#0] drops [local#0]
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

    // -----------------------------------------------------------------------
    // 4.1 借用と非字句リージョン
    // -----------------------------------------------------------------------

    /// 射影を持つ場所を借りるための道具立て
    const PAIR: &str = "struct Inner { n: int }
struct Pair { left: Inner, right: Inner }
fn bump(x: &mut Inner) { x.n = x.n + 1 }
fn peek(x: &Inner -> int) { x.n }
fn pair(-> Pair) { Pair { left = Inner { n = 1 }, right = Inner { n = 2 } } }
";

    /// 借用は最後の使用で終わる。括弧もリージョン注釈も要らない
    #[test]
    fn 共有借用は最後の使用で終わる() {
        accepted(&with_user(
            "  let mut u = make()\n  let view = &u\n  let n = view.id\n  u.id = 2\n  assert n == 1",
        ));
    }

    /// 逆に、最後の使用が後ろにあれば同じ形が塞がる
    #[test]
    fn 使用が後ろにあれば共有借用は塞ぐ() {
        let src =
            with_user("  let mut u = make()\n  let view = &u\n  u.id = 2\n  assert view.id == 2");
        let diagnostic = only(&src);
        assert_eq!(
            diagnostic.msg,
            "main: `u.id` は共有借用されている間は排他的に触れません"
        );
        assert_eq!(diagnostic.label.as_deref(), Some("共有借用との衝突"));
        assert_eq!(
            related(&src, &diagnostic),
            vec![("ここで共有借用しています".to_string(), "&u")]
        );
    }

    /// リージョンは点の集合として計画に載る。使わなくなった先には伸びない
    #[test]
    fn リージョンは計画に載る() {
        let src = with_user(
            "  let mut u = make()\n  let view = &u\n  let n = view.id\n  u.id = 2\n  assert n == 1",
        );
        let checked = accepted(&src);
        let plan = checked.plan.body(checked.hir.bodies[2]);
        let (id, loan) = plan.loans().next().expect("借用が1つある");
        assert_eq!(loan.kind, hir::RefKind::Shared);
        let region = plan.region(id);
        let write = plan
            .points()
            .find(
                |(_, point)| matches!(&point.effect, Effect::Access(a) if a.mode == Mode::Mutable),
            )
            .expect("`u.id = 2` の排他アクセスがある")
            .0;
        assert!(region.contains(&loan.point), "作成点は必ず入る");
        assert!(
            !region.contains(&write),
            "最後の使用より後には伸びない: {region:?}"
        );
    }

    /// 再借用した参照が生きている間は、元の借用元も借りられたまま
    #[test]
    fn 再借用は元の借用も生かす() {
        let src = format!(
            "{PAIR}fn main() {{
  let mut p = pair()
  let e = &mut p
  let r = e
  p.left.n = 2
  bump(&mut r.right)
}}
"
        );
        let diagnostic = only(&src);
        assert_eq!(
            diagnostic.msg,
            "main: `p.left.n` は既に排他借用されているので、もう一度は借りられません"
        );
    }

    /// 実引数の借用は呼び出しの間ずっと生きている。だから同じ呼び出しの中で
    /// 排他と共有が並ぶと衝突する
    #[test]
    fn 同じ呼び出しの実引数どうしが衝突する() {
        let src = format!(
            "{PAIR}fn both(a: &mut Inner, b: &Inner -> int) {{ a.n + b.n }}
fn main(-> int) {{
  let mut p = pair()
  both(&mut p.left, &p.left)
}}
"
        );
        let diagnostic = only(&src);
        assert_eq!(
            diagnostic.msg,
            "main: `p.left` は排他借用されている間は読めません"
        );
    }

    /// 素の `for` はループ変数が対象の借用を持ち回るので、周回中は対象を
    /// 排他的に触れない
    #[test]
    fn ループ変数は対象の借用を持ち回る() {
        let src = "struct Holder { xs: [int] }
fn main(-> int) {
  let mut h = Holder { xs = [1, 2] }
  for x in h.xs { h.xs = [3] }
  0
}
";
        let diagnostic = only(src);
        assert_eq!(
            diagnostic.msg,
            "main: `h.xs` は共有借用されている間は排他的に触れません"
        );
    }

    /// arm が全部抜けても対象は借りられたまま。合流点が生まれない経路で
    /// 借用が縮まないこと(CFG の端の穴を塞ぐ回帰)
    #[test]
    fn 抜けるarmでも対象は借りられたまま() {
        let src = "enum Lookup { Found(int) Missing }
struct Holder { r: Lookup, n: int }
fn main(-> int) {
  let mut h = Holder { r = Lookup::Missing, n = 1 }
  match h.r {
    Lookup::Found(x): return x
    Lookup::Missing {
      h.r = Lookup::Missing
      return h.n
    }
  }
}
";
        let diagnostic = only(src);
        assert_eq!(
            diagnostic.msg,
            "main: `h.r` は共有借用されている間は排他的に触れません"
        );
        assert_eq!(
            related(src, &diagnostic),
            vec![("ここで共有借用しています".to_string(), "h.r")]
        );
    }

    /// `for` の本体が全経路で抜けると背辺が張られない。対象の借用を背辺の
    /// 行き先に繋いでいると、そこで借用が死ぬ(`match` と同じ端の穴の回帰)
    #[test]
    fn 抜けるループ本体でも対象は借りられたまま() {
        let src = "struct Holder { xs: [int] }
fn eat(h: Holder -> int) { 0 }
fn main(-> int) {
  let h = Holder { xs = [1, 2] }
  for x in h.xs {
    return eat(move h)
  }
  0
}
";
        let diagnostic = only(src);
        assert_eq!(
            diagnostic.msg,
            "main: `h` は借用されているので move できません"
        );
        assert_eq!(
            related(src, &diagnostic),
            vec![("ここで借用しています".to_string(), "h.xs")]
        );
    }

    // -----------------------------------------------------------------------
    // 4.2 場所の重なり
    // -----------------------------------------------------------------------

    #[test]
    fn 別のフィールドは独立に排他借用できる() {
        accepted(&format!(
            "{PAIR}fn main() {{
  let mut p = pair()
  let a = &mut p.left
  let b = &mut p.right
  bump(a)
  bump(b)
}}
"
        ));
    }

    #[test]
    fn 全体は子と重なる() {
        let src = format!(
            "{PAIR}fn main() {{
  let mut p = pair()
  let a = &mut p.left
  let w = &mut p
  bump(a)
  bump(&mut w.right)
}}
"
        );
        let diagnostic = only(&src);
        assert_eq!(
            diagnostic.msg,
            "main: `p` は既に排他借用されているので、もう一度は借りられません"
        );
    }

    /// 重なり規則そのもの。ソースに添字構文が無いので配列と enum payload は
    /// ここで直接固定する(tasks 4.2)
    #[test]
    fn 重なり規則は保守的な側へ倒れる() {
        let root = hir::LocalId::from_index(0);
        let other = hir::LocalId::from_index(1);
        let left = Projection::Field(hir::FieldId::from_index(0));
        let right = Projection::Field(hir::FieldId::from_index(1));
        let at = |path: Vec<Projection>| Place { root, path };

        assert!(overlaps(&at(vec![]), &at(vec![])));
        // 根が違えば重ならない
        assert!(!overlaps(
            &at(vec![]),
            &Place {
                root: other,
                path: vec![]
            }
        ));
        // 全体は子の全てと重なる
        assert!(overlaps(&at(vec![]), &at(vec![left.clone()])));
        assert!(overlaps(&at(vec![left.clone()]), &at(vec![])));
        // 宣言の違うフィールドは独立
        assert!(!overlaps(&at(vec![left.clone()]), &at(vec![right])));
        assert!(!overlaps(
            &at(vec![left.clone(), left.clone()]),
            &at(vec![
                left.clone(),
                Projection::Field(hir::FieldId::from_index(2))
            ])
        ));
        // 定数添字が両方分かっていれば言い分けられる
        assert!(!overlaps(
            &at(vec![Projection::ArrayElement(Some(0))]),
            &at(vec![Projection::ArrayElement(Some(1))])
        ));
        assert!(overlaps(
            &at(vec![Projection::ArrayElement(Some(0))]),
            &at(vec![Projection::ArrayElement(Some(0))])
        ));
        // 添字が分からなければ保守的に重なる
        assert!(overlaps(
            &at(vec![Projection::ArrayElement(None)]),
            &at(vec![Projection::ArrayElement(Some(7))])
        ));
        // enum payload と optional の中身も保守的に重なる
        let one = Projection::EnumPayload(hir::VariantId::from_index(0), 0);
        let two = Projection::EnumPayload(hir::VariantId::from_index(1), 1);
        assert!(overlaps(&at(vec![one]), &at(vec![two])));
        assert!(overlaps(
            &at(vec![Projection::OptionalPayload]),
            &at(vec![left])
        ));
    }

    // -----------------------------------------------------------------------
    // 4.3 借用の衝突と漏れ
    // -----------------------------------------------------------------------

    #[test]
    fn 排他借用の重なりを拒否する() {
        let src = format!(
            "{PAIR}fn main() {{
  let mut p = pair()
  let a = &mut p.left
  let b = &mut p.left
  bump(a)
  bump(b)
}}
"
        );
        let diagnostic = only(&src);
        assert_eq!(
            diagnostic.msg,
            "main: `p.left` は既に排他借用されているので、もう一度は借りられません"
        );
        assert_eq!(diagnostic.label.as_deref(), Some("排他借用の重なり"));
        assert_eq!(
            related(&src, &diagnostic),
            vec![("ここで排他借用しています".to_string(), "&mut p.left")]
        );
    }

    #[test]
    fn 排他借用中の読みを拒否する() {
        let src = format!(
            "{PAIR}fn main(-> int) {{
  let mut p = pair()
  let e = &mut p.left
  let n = p.left.n
  bump(e)
  n
}}
"
        );
        let diagnostic = only(&src);
        assert_eq!(
            diagnostic.msg,
            "main: `p.left.n` は排他借用されている間は読めません"
        );
        assert_eq!(diagnostic.label.as_deref(), Some("排他借用との衝突"));
    }

    /// 借用が残ったまま所有者のスコープが終わる
    #[test]
    fn 所有者より長生きする借用を拒否する() {
        let src = format!(
            "{PAIR}fn main(outer: &Pair -> int) {{
  let mut view = outer
  if true {{
    let inner = pair()
    view = &inner
  }}
  peek(&view.left)
}}
"
        );
        let diagnostic = only(&src);
        assert_eq!(
            diagnostic.msg,
            "main: `inner` の借用は借用元より長く生きています"
        );
        assert_eq!(
            diagnostic.label.as_deref(),
            Some("所有者より長生きする借用")
        );
        assert_eq!(
            related(&src, &diagnostic),
            vec![(
                "`inner` はスコープの終わりで落ちます".to_string(),
                "let inner = pair()"
            )]
        );
    }

    /// 周回の緩和で穴を開けていないこと。ループの外へ持ち出す借用は塞ぐ
    #[test]
    fn ループの外へ持ち出す借用は拒否する() {
        let src = format!(
            "{PAIR}fn main(outer: &Pair -> int) {{
  let mut view = outer
  while true {{
    let inner = pair()
    view = &inner
  }}
  peek(&view.left)
}}
"
        );
        let diagnostic = only(&src);
        assert_eq!(
            diagnostic.msg,
            "main: `inner` の借用は借用元より長く生きています"
        );
    }

    /// 局所の借用は返せない
    #[test]
    fn 局所からの借用を返すのを拒否する() {
        let src = format!("{PAIR}fn leak(-> &Inner) {{ let p = pair()\n &p.left }}\n");
        let diagnostic = only(&src);
        assert_eq!(
            diagnostic.msg,
            "leak: `p.left` の借用は返せません。借用元がこの本体の中で終わります"
        );
        assert_eq!(at(&src, &diagnostic), "&p.left");
        assert_eq!(
            diagnostic.help.as_deref(),
            Some("所有ごと返すか、入力から借りたものを返してください")
        );
    }

    /// 所有で受け取った引数も本体の中で終わるので、そこからの借用は返せない
    #[test]
    fn 所有の引数からの借用も返せない() {
        let src = format!("{PAIR}fn leak(p: Pair -> &Inner) {{ &p.left }}\n");
        let diagnostic = only(&src);
        assert_eq!(
            diagnostic.msg,
            "leak: `p.left` の借用は返せません。借用元がこの本体の中で終わります"
        );
    }

    /// 一時値から借りて返す呼び出しは、借用元が式の中で終わる
    #[test]
    fn 一時値からの借用を返す呼び出しを拒否する() {
        let src = format!(
            "{PAIR}impl Pair {{ fn first(&self -> &Inner) {{ &self.left }} }}
fn main(-> int) {{ peek(pair().first()) }}
"
        );
        let diagnostic = only(&src);
        assert_eq!(
            diagnostic.msg,
            "main: この呼び出しは実引数から借りて返しますが、その実引数は式の中で終わる一時値です"
        );
        assert_eq!(diagnostic.label.as_deref(), Some("一時値からの借用"));
    }

    /// 一時値を提供したスロットからレシーバを借りて返すと、借用元が提供の
    /// 実体で、この本体に場所が無い(tasks 5.4)
    #[test]
    fn 一時値の提供からの借用返しを拒否する() {
        let src = "struct Inner { n: int }
struct Store { one: Inner }
trait Peek { fn look(&self -> &Inner) }
impl Peek for Store { fn look(&self -> &Inner) { &self.one } }
effect store: Peek
fn main(-> int) { with store(Store { one = Inner { n = 1 } }) { store.look().n } }
";
        let diagnostic = only(src);
        assert_eq!(
            diagnostic.msg,
            "main: スロット経由の呼び出しはレシーバから借りて返しますが、提供された実体の借用元をこの本体では名指せません"
        );
    }

    /// 提供がこの本体の外にあるときも同じ。借用元を名指す `with` が無い
    #[test]
    fn 外側の提供からの借用返しを拒否する() {
        let src = "struct Inner { n: int }
struct Store { one: Inner }
trait Peek { fn look(&self -> &Inner) }
impl Peek for Store { fn look(&self -> &Inner) { &self.one } }
effect store: Peek
fn borrowed(-> &Inner) { store.look() }
";
        let diagnostic = only(src);
        assert_eq!(
            diagnostic.msg,
            "borrowed: スロット経由の呼び出しはレシーバから借りて返しますが、提供された実体の借用元をこの本体では名指せません"
        );
    }

    /// 束縛を提供すれば借用元を名指せる。共有借用の提供から借りて返す形は
    /// そのまま通る(phase 4 が丸ごと拒否していた穴、tasks 5.4)
    #[test]
    fn 束縛を提供したスロットからは借りて返せる() {
        accepted(
            "struct Inner { n: int }
struct Store { one: Inner }
trait Peek { fn look(&self -> &Inner) }
impl Peek for Store { fn look(&self -> &Inner) { &self.one } }
effect store: Peek
fn main(-> int) {
  let s = Store { one = Inner { n = 1 } }
  with store(s) { store.look().n }
}
",
        );
    }

    /// 借用元を名指せるようになった分、その借用は他の使用と衝突しなければ
    /// ならない。受理だけ増えて拒否が付いてこないと穴になる(tasks 5.4)
    #[test]
    fn 提供から借りた結果は借用元を塞ぐ() {
        const PEEK: &str = "struct Inner { n: int }
struct Store { one: Inner }
trait Peek { fn look(&self -> &Inner) }
impl Peek for Store { fn look(&self -> &Inner) { &self.one } }
effect store: Peek
fn read(x: &Inner -> int) { x.n }
fn eat(s: Store -> int) { s.one.n }
fn make(-> Store) { Store { one = Inner { n = 1 } } }
";
        // 提供の借用は `with` で終わるが、そこから借りて返った結果は生き残る。
        // その間、借用元は move できない
        let src = format!(
            "{PEEK}fn main(-> int) {{
  let s = make()
  let got = with store(s) {{ store.look() }}
  let n = eat(move s)
  n + read(got)
}}
"
        );
        let diagnostic = only(&src);
        assert_eq!(
            diagnostic.msg,
            "main: `s` は借用されているので move できません"
        );

        // 書き換えることもできない
        let src = format!(
            "{PEEK}fn main(-> int) {{
  let mut s = make()
  let got = with store(s) {{ store.look() }}
  s.one.n = 5
  read(got)
}}
"
        );
        let diagnostic = only(&src);
        assert_eq!(
            diagnostic.msg,
            "main: `s.one.n` は共有借用されている間は排他的に触れません"
        );
    }

    #[test]
    fn 場所でない値への借用を拒否する() {
        let src = format!("{PAIR}fn main(-> int) {{ peek(&pair().left) }}\n");
        let diagnostic = only(&src);
        assert_eq!(
            diagnostic.msg,
            "main: 場所ではない値に共有借用は掛けられません"
        );
        assert_eq!(
            diagnostic.label.as_deref(),
            Some("場所ではない値への所有権修飾")
        );
    }

    #[test]
    fn 場所でない値へのmoveを拒否する() {
        let src = with_user("  let n = take(move make())\n  assert n == 1");
        let diagnostic = only(&src);
        assert_eq!(
            diagnostic.msg,
            "main: 場所ではない値に`move`は掛けられません"
        );
        assert_eq!(
            diagnostic.help.as_deref(),
            Some("一時値は既に所有者なので `move` は要りません。修飾を外してください")
        );
    }

    // -----------------------------------------------------------------------
    // 5.1 / 5.2 引数とレシーバの所有モード
    // -----------------------------------------------------------------------

    /// 引数3種とレシーバ3種の道具立て
    const MODES: &str = "struct User { n: int }
impl User {
  fn look(&self -> int) { self.n }
  fn bump(&mut self -> int) { self.n = self.n + 1
    self.n }
  fn finish(self -> int) { self.n }
}
fn read(u: &User -> int) { u.n }
fn write(u: &mut User -> int) { u.n = 1
  u.n }
fn own(u: User -> int) { u.n }
fn make(-> User) { User { n = 1 } }
";

    /// 引数とレシーバで選ばれた所有モードが、そのまま計画に出る。共有だけが
    /// 自動で挿さり、排他と消費は書いたときにしか現れない
    /// (design.md 決定2、tasks 5.1 / 5.2)
    #[test]
    fn 選ばれた所有モードが計画に出る() {
        let src = format!(
            "{MODES}fn main(-> int) {{
  let mut u = make()
  let a = read(u)
  let b = write(&mut u)
  let c = u.look()
  let d = &mut u.bump()
  let e = own(move u)
  a + b + c + d + e
}}
"
        );
        let checked = accepted(&src);
        let dumped = checked.plan.dump(&checked.hir);
        let main = dumped.split("body main").nth(1).expect("main の計画がある");
        let modes: Vec<&str> = main
            .lines()
            .filter_map(|line| line.split(" scope#0 ").nth(1))
            .filter(|effect| effect.contains("local#0(u)"))
            .collect();
        assert_eq!(
            modes,
            vec![
                // `read(u)` — 共有借用が自動で挿さる
                "& local#0(u)",
                // `write(&mut u)` — 排他は書いたところにだけ
                "&mut local#0(u)",
                // `u.look()` — `&self` レシーバも自動の共有借用
                "& local#0(u)",
                // `&mut u.bump()` — `&mut self` は修飾が要る
                "&mut local#0(u)",
                // `own(move u)` — 所有の引数は `move` が要る
                "move local#0(u)",
            ],
            "{dumped}"
        );
    }

    /// 共有借用は所有者を残す。同じ束縛を何度でも読める
    #[test]
    fn 自動の共有借用は所有者を残す() {
        accepted(&format!(
            "{MODES}fn main(-> int) {{
  let u = make()
  read(u) + u.look() + own(move u)
}}
"
        ));
    }

    /// 排他レシーバの修飾が抜けていれば型検査が断るので、所有権解析まで来ない。
    /// 消費レシーバだけは静的型で言い分けられないのでここが見る
    #[test]
    fn 消費レシーバの_move_忘れをここで断る() {
        let src = format!("{MODES}fn main(-> int) {{ let u = make()\n u.finish() }}\n");
        let diagnostic = only(&src);
        assert_eq!(
            diagnostic.msg,
            "main: `impl User::finish` のレシーバは所有を受け取りますが、束縛済みの `u` をそのまま渡しています"
        );
    }

    /// レシーバの中の呼び出しが表示名を消していかないこと。消えると `move` の
    /// 欠落が空の呼び出し先を名指す
    #[test]
    fn レシーバに呼び出しが挟まっても呼び出し先を名指す() {
        let src = format!(
            "{MODES}impl User {{ fn eat(&self, other: User -> int) {{ other.n }} }}
fn wrap(u: User -> User) {{ u }}
fn main(-> int) {{
  let a = make()
  let b = make()
  wrap(move a).eat(b)
}}
"
        );
        let diagnostic = only(&src);
        assert_eq!(
            diagnostic.msg,
            "main: `impl User::eat` の第 1 引数は所有を受け取りますが、束縛済みの `b` をそのまま渡しています"
        );
    }

    /// 一時値のレシーバと実引数は既に所有者なので、修飾はどこにも要らない
    #[test]
    fn 一時値のレシーバと引数は修飾を要らない() {
        accepted(&format!(
            "{MODES}fn main(-> int) {{ make().look() + make().finish() + read(make()) + own(make()) }}\n"
        ));
    }

    // -----------------------------------------------------------------------
    // 5.3 呼び出しの形ごとの伝播
    // -----------------------------------------------------------------------

    /// 直接・関連・具体メソッド・trait 実装メソッド・スロットの5形すべてで
    /// 同じ道具立てを共有する。呼び出し先は型検査が既に選んでいるので、ここは
    /// `hir::Call` の解決済み ID しか見ない(tasks 5.3)
    const FORMS: &str = "struct Inner { n: int }
struct Pair { left: Inner, right: Inner }
trait Peek { fn look(&self -> &Inner) }
impl Peek for Pair { fn look(&self -> &Inner) { &self.left } }
impl Pair {
  fn first(&self -> &Inner) { &self.left }
  fn of(p: &Pair -> &Inner) { &p.left }
}
effect peek: Peek
fn direct(p: &Pair -> &Inner) { &p.left }
fn read(x: &Inner -> int) { x.n }
fn pair(-> Pair) { Pair { left = Inner { n = 1 }, right = Inner { n = 2 } } }
";

    /// どの形でも、返った借用が生きている間は借用元が借りられたまま。
    /// 実引数とレシーバはどちらも自動の共有借用で埋まる
    #[test]
    fn 借用を返す呼び出しは形を問わず出自を引き継ぐ() {
        for call in [
            "direct(p)",
            "Pair::of(p)",
            "p.first()",
            // trait 実装のメソッドを具体型から直接呼ぶ形
            "p.look()",
        ] {
            let src = format!(
                "{FORMS}fn main(-> int) {{
  let mut p = pair()
  let got = {call}
  p.left.n = 5
  read(got)
}}
"
            );
            let diagnostic = only(&src);
            assert_eq!(
                diagnostic.msg, "main: `p.left.n` は共有借用されている間は排他的に触れません",
                "{call}"
            );
        }
    }

    /// 借用を使い終わっていれば、同じ形でも後の排他アクセスは通る
    #[test]
    fn 使い終わった借用は形を問わず塞がない() {
        for call in ["direct(p)", "Pair::of(p)", "p.first()", "p.look()"] {
            accepted(&format!(
                "{FORMS}fn main(-> int) {{
  let mut p = pair()
  let got = {call}
  let n = read(got)
  p.left.n = 5
  n
}}
"
            ));
        }
    }

    /// 契約メソッドの provenance は、それを実装する全ての本体の合流。
    /// スロット経由の呼び出しは提供の場所へ置き換わる(tasks 5.3 / 5.4)
    #[test]
    fn スロット経由でも出自は提供へ置き換わる() {
        let src = format!(
            "{FORMS}fn main(-> int) {{
  let p = pair()
  with peek(p) {{
    let got = peek.look()
    read(got)
  }}
}}
"
        );
        accepted(&src);
    }

    // -----------------------------------------------------------------------
    // 5.4 提供の所有モード
    // -----------------------------------------------------------------------

    /// 共有・排他・消費の3つの契約を持つスロット1つ分の道具立て
    const SLOT: &str = "struct Store { n: int }
trait Ops {
  fn look(&self -> int)
  fn bump(&mut self -> int)
  fn finish(self -> int)
  fn zero(-> int)
}
impl Ops for Store {
  fn look(&self -> int) { self.n }
  fn bump(&mut self -> int) { self.n = self.n + 1
    self.n }
  fn finish(self -> int) { self.n }
  fn zero(-> int) { 0 }
}
effect store: Ops
fn take(s: Store -> int) { s.n }
fn peek(s: &Store -> int) { s.n }
fn make(-> Store) { Store { n = 1 } }
fn borrow(s: &Store -> &Store) { s }
fn borrow_mut(s: &mut Store -> &mut Store) { s }
";

    /// 修飾を書かない束縛済みの提供は共有借用。所有者は `with` の後も残る
    #[test]
    fn 束縛の提供は共有借用で所有者が残る() {
        accepted(&format!(
            "{SLOT}fn main(-> int) {{
  let s = make()
  let a = with store(s) {{ store.look() }}
  a + take(move s)
}}
"
        ));
    }

    /// 一時値の提供は提供スコープの所有。`move` は要らない
    #[test]
    fn 一時値の提供はmoveを要らない() {
        accepted(&format!(
            "{SLOT}fn main(-> int) {{ with store(make()) {{ store.finish() }} }}\n"
        ));
    }

    /// `move` した提供は提供スコープへ所有が移る。元の束縛はもう使えない
    #[test]
    fn moveした提供は元の束縛を消費する() {
        let src = format!(
            "{SLOT}fn main(-> int) {{
  let s = make()
  let a = with store(move s) {{ store.finish() }}
  a + take(move s)
}}
"
        );
        let diagnostic = only(&src);
        assert_eq!(
            diagnostic.msg,
            "main: `s` は既に move されているので使えません"
        );
    }

    /// 排他の提供は提供本体の間ずっと生きている。その間、外から触れない
    #[test]
    fn 排他の提供は提供本体の間ずっと塞ぐ() {
        let src = format!(
            "{SLOT}fn main(-> int) {{
  let mut s = make()
  with store(&mut s) {{ store.bump() + peek(&s) }}
}}
"
        );
        let diagnostic = only(&src);
        assert_eq!(
            diagnostic.msg,
            "main: `s` は排他借用されている間は読めません"
        );
        assert_eq!(
            related(&src, &diagnostic),
            vec![("ここで排他借用しています".to_string(), "&mut s")]
        );
    }

    /// 本体が `return` で抜けても提供の借用は塞ぐ。合流点が生まれない経路で
    /// 借用が縮まないこと(CFG の端の穴を塞ぐ回帰)
    #[test]
    fn 抜ける提供本体でも排他の提供は塞ぐ() {
        let src = format!(
            "{SLOT}fn main(-> int) {{
  let mut s = make()
  with store(&mut s) {{ return peek(&s) }}
}}
"
        );
        let diagnostic = only(&src);
        assert_eq!(
            diagnostic.msg,
            "main: `s` は排他借用されている間は読めません"
        );
    }

    /// 逆に共有の提供なら、提供本体の中でも外からの読みは通る
    #[test]
    fn 共有の提供は本体の中の読みを塞がない() {
        accepted(&format!(
            "{SLOT}fn main(-> int) {{
  let s = make()
  with store(s) {{ store.look() + peek(&s) }}
}}
"
        ));
    }

    /// 型だけの提供は実体を運ばないので、借用も所有も動かない
    #[test]
    fn 型だけの提供は実体を持たない() {
        let checked = accepted(&format!(
            "{SLOT}fn main(-> int) {{ with store<Store> {{ store::zero() }} }}\n"
        ));
        let dumped = checked.plan.dump(&checked.hir);
        let main = dumped.split("body main").nth(1).expect("main の計画がある");
        assert!(!main.contains("loan#"), "借用は生まれない: {dumped}");
    }

    /// 契約が要る強さを提供が満たしていなければ断る
    #[test]
    fn 提供の所有モードが足りなければ断る() {
        for (provision, call, expected, fix) in [
            (
                "s",
                "store.bump()",
                "main: `impl Ops::bump` のレシーバは `&mut self` ですが、`store` への提供は共有借用です",
                "`with store(&mut ...)` と書いてください",
            ),
            (
                "s",
                "store.finish()",
                "main: `impl Ops::finish` のレシーバは `self` ですが、`store` への提供は共有借用です",
                "`with store(move ...)` と書いてください",
            ),
            (
                "&mut s",
                "store.finish()",
                "main: `impl Ops::finish` のレシーバは `self` ですが、`store` への提供は排他借用です",
                "`with store(move ...)` と書いてください",
            ),
        ] {
            let src = format!(
                "{SLOT}fn main(-> int) {{
  let mut s = make()
  with store({provision}) {{ {call} }}
}}
"
            );
            let diagnostic = only(&src);
            assert_eq!(diagnostic.msg, expected);
            assert_eq!(diagnostic.help.as_deref(), Some(fix));
            assert_eq!(at(&src, &diagnostic), call);
            assert_eq!(
                related(&src, &diagnostic),
                vec![("`store` はここで提供されています".to_string(), provision)]
            );
        }
    }

    /// 満たしていれば通る。所有を握る提供はどの契約にも足りる
    #[test]
    fn 満たす提供は通る() {
        for (provision, call) in [
            ("s", "store.look()"),
            ("&mut s", "store.look()"),
            ("&mut s", "store.bump()"),
            ("move s", "store.finish()"),
            ("make()", "store.bump()"),
        ] {
            accepted(&format!(
                "{SLOT}fn main(-> int) {{
  let mut s = make()
  with store({provision}) {{ {call} }}
}}
"
            ));
        }
    }

    /// 提供の強さは構文ではなく走査の結果から決まる。所有の形をしていても
    /// 借用を運んでいれば実体は他人のもので、消費レシーバには足りない
    #[test]
    fn 借用を運ぶ提供は所有として扱わない() {
        for (provision, call, mode) in [
            // 参照を返す呼び出し。結果型が `&Store`
            ("borrow(&s)", "store.finish()", "共有借用"),
            ("borrow(&s)", "store.bump()", "共有借用"),
            // 排他を返す呼び出しは消費には足りない
            ("borrow_mut(&mut s)", "store.finish()", "排他借用"),
            // 場所を素通しするブロック。所有の型だが借用しか取っていない
            ("{ s }", "store.finish()", "共有借用"),
            ("{ s }", "store.bump()", "共有借用"),
        ] {
            let src = format!(
                "{SLOT}fn main(-> int) {{
  let mut s = make()
  let a = with store({provision}) {{ {call} }}
  a + peek(&s)
}}
"
            );
            let messages: Vec<String> = rejected(&src).into_iter().map(|d| d.msg).collect();
            let want = format!("`store` への提供は{mode}です");
            assert!(
                messages.iter().any(|m| m.contains(&want)),
                "{provision} / {call}: {messages:?}"
            );
        }
    }

    /// 参照の束縛をそのまま提供したら、その参照の強さがそのまま提供の強さ
    #[test]
    fn 参照の束縛の提供はその強さで通る() {
        accepted(&format!(
            "{SLOT}fn main(-> int) {{
  let mut s = make()
  let r = &mut s
  with store(r) {{ store.bump() }}
}}
"
        ));
        let src = format!(
            "{SLOT}fn main(-> int) {{
  let s = make()
  let r = &s
  with store(r) {{ store.bump() }}
}}
"
        );
        let diagnostic = only(&src);
        assert_eq!(
            diagnostic.msg,
            "main: `impl Ops::bump` のレシーバは `&mut self` ですが、`store` への提供は共有借用です"
        );
    }

    /// 内側の提供が外側を隠す。外側だけを見ていたら `&mut self` に足りない
    #[test]
    fn 内側の提供が外側を隠す() {
        accepted(&format!(
            "{SLOT}fn main(-> int) {{
  let outer = make()
  let mut inner = make()
  with store(outer) {{
    let a = store.look()
    let b = with store(&mut inner) {{ store.bump() }}
    a + b
  }}
}}
"
        ));
    }

    /// 隠した提供は `with` を抜けたら戻る。戻った先は共有借用のまま
    #[test]
    fn 隠した提供は抜けたら戻る() {
        let src = format!(
            "{SLOT}fn main(-> int) {{
  let outer = make()
  let mut inner = make()
  with store(outer) {{
    let a = with store(&mut inner) {{ store.bump() }}
    a + store.bump()
  }}
}}
"
        );
        let diagnostic = only(&src);
        assert_eq!(
            diagnostic.msg,
            "main: `impl Ops::bump` のレシーバは `&mut self` ですが、`store` への提供は共有借用です"
        );
    }

    // -----------------------------------------------------------------------
    // 5.6 モジュールを跨ぐ形と再帰
    // -----------------------------------------------------------------------

    /// 複数モジュールをまとめて検査する。所有権解析はプログラム全体で1回
    fn analyze_files(files: &[(&str, &str)]) -> Result<CheckedProgram, Vec<Diag>> {
        let loaded = crate::module::load_files(files).expect("ロードできる");
        let hir = typecheck::check_and_lower(&loaded.program).expect("型検査を通るはず");
        check(hir)
    }

    /// 別モジュールの署名でも所有モードは同じ契約。共有は自動で借り、所有には
    /// `move` が要り、排他には `&mut` が要る(tasks 5.6)
    #[test]
    fn モジュールを跨いでも所有モードは同じ契約() {
        const DEP: &str = "struct User { n: int }\n\
                           impl User {\n\
                           \x20 fn look(&self -> int) { self.n }\n\
                           \x20 fn finish(self -> int) { self.n }\n\
                           }\n\
                           fn read(u: &User -> int) { u.n }\n\
                           fn own(u: User -> int) { u.n }\n\
                           fn make(-> User) { User { n = 1 } }\n";
        analyze_files(&[
            (
                "main.rd",
                "use dep::{User, read, own, make}\n\
                 fn main(-> int) {\n\
                 \x20 let u = make()\n\
                 \x20 read(u) + u.look() + own(move u)\n\
                 }\n",
            ),
            ("dep.rd", DEP),
        ])
        .expect("受理されるはず");

        let Err(errors) = analyze_files(&[
            (
                "main.rd",
                "use dep::{User, own, make}\n\
                 fn main(-> int) {\n\
                 \x20 let u = make()\n\
                 \x20 own(u)\n\
                 }\n",
            ),
            ("dep.rd", DEP),
        ]) else {
            panic!("拒否されるはず")
        };
        let messages: Vec<String> = errors.iter().map(|d| d.msg.clone()).collect();
        assert_eq!(
            messages,
            vec![
                "main::main: `dep::own` の第 1 引数は所有を受け取りますが、束縛済みの `u` をそのまま渡しています"
            ]
        );
        // 主 span は呼び出し側、related は束縛側。どちらも main.rd
        assert_eq!(errors[0].span.expect("位置を持つ").src, 0);
        assert_eq!(errors[0].related.len(), 1);
    }

    /// 別モジュールの提供でも所有モードの照合は効く
    #[test]
    fn モジュールを跨ぐ提供の所有モードも照合する() {
        let Err(errors) = analyze_files(&[
            (
                "main.rd",
                "use dep::{Store, make, store}\n\
                 fn main(-> int) {\n\
                 \x20 let s = make()\n\
                 \x20 with store(s) { store.bump() }\n\
                 }\n",
            ),
            (
                "dep.rd",
                "struct Store { n: int }\n\
                 trait Ops { fn bump(&mut self -> int) }\n\
                 impl Ops for Store { fn bump(&mut self -> int) { self.n = self.n + 1\n self.n } }\n\
                 effect store: Ops\n\
                 fn make(-> Store) { Store { n = 1 } }\n",
            ),
        ]) else {
            panic!("拒否されるはず")
        };
        let messages: Vec<String> = errors.iter().map(|d| d.msg.clone()).collect();
        assert_eq!(
            messages,
            vec![
                "main::main: `impl dep::Ops::bump` のレシーバは `&mut self` ですが、`dep::store` への提供は共有借用です"
            ]
        );
    }

    /// 再帰する本体でも、引数の自動借用と `move` の要求はそのまま効く
    #[test]
    fn 再帰でも引数の所有モードは効く() {
        accepted(&format!(
            "{MODES}fn walk(u: &User, n: int -> int) {{
  if n == 0: read(u)
  else: walk(u, n - 1)
}}
fn main(-> int) {{ let u = make()\n walk(u, 3) }}
"
        ));
        let src = format!(
            "{MODES}fn drain(u: User, n: int -> int) {{
  if n == 0: own(move u)
  else: drain(u, n - 1)
}}
"
        );
        let diagnostic = only(&src);
        assert_eq!(
            diagnostic.msg,
            "drain: `drain` の第 1 引数は所有を受け取りますが、束縛済みの `u` をそのまま渡しています"
        );
    }

    /// 相互再帰する借用返しも不動点で閉じる。呼び出し側では実引数へ置き換わる
    #[test]
    fn 相互再帰する借用返しも呼び出し側で塞ぐ() {
        let src = format!(
            "{FORMS}fn ping(p: &Pair, n: int -> &Inner) {{
  if n == 0 {{ &p.left }} else {{ pong(p, n - 1) }}
}}
fn pong(p: &Pair, n: int -> &Inner) {{
  if n == 0 {{ &p.right }} else {{ ping(p, n - 1) }}
}}
fn main(-> int) {{
  let mut p = pair()
  let got = ping(p, 3)
  p.left.n = 5
  read(got)
}}
"
        );
        let diagnostic = only(&src);
        assert_eq!(
            diagnostic.msg,
            "main: `p.left.n` は共有借用されている間は排他的に触れません"
        );
    }

    // -----------------------------------------------------------------------
    // 4.4 / 4.5 戻り値 provenance
    // -----------------------------------------------------------------------

    fn provenance(src: &str, name: &str) -> String {
        let checked = accepted(src);
        let callable = checked.hir.free_callable(name).expect("宣言がある");
        let found = checked.plan.provenance(callable).expect("参照を返す");
        let origins: Vec<String> = found
            .origins
            .iter()
            .map(|origin| show_input(&checked.hir, origin))
            .collect();
        format!(
            "{} [{}]",
            found.kind.spelling().trim_end(),
            origins.join(" ")
        )
    }

    #[test]
    fn 単一の入力から借りた戻り値の出自() {
        let src = format!("{PAIR}fn left(p: &Pair -> &Inner) {{ &p.left }}\n");
        assert_eq!(provenance(&src, "left"), "& [param#0.left]");
    }

    #[test]
    fn 分岐は出自を合流する() {
        let src = format!(
            "{PAIR}fn pick(a: &Inner, b: &Inner, flag: bool -> &Inner) {{
  if flag {{ a }} else {{ b }}
}}
"
        );
        assert_eq!(provenance(&src, "pick"), "& [param#0 param#1]");
    }

    /// 再帰は不動点で閉じる。1周目は `param#0` だけだが、再帰呼び出しが
    /// 実引数を入れ替えているので `param#1` も候補になる
    #[test]
    fn 再帰する出自は不動点で閉じる() {
        let src = format!(
            "{PAIR}fn walk(a: &Inner, b: &Inner, n: int -> &Inner) {{
  if n == 0 {{ a }} else {{ walk(b, a, n - 1) }}
}}
"
        );
        assert_eq!(provenance(&src, "walk"), "& [param#0 param#1]");
    }

    #[test]
    fn 相互再帰する出自も閉じる() {
        let src = format!(
            "{PAIR}fn ping(a: &Inner, b: &Inner, n: int -> &Inner) {{
  if n == 0 {{ a }} else {{ pong(b, a, n - 1) }}
}}
fn pong(a: &Inner, b: &Inner, n: int -> &Inner) {{
  if n == 0 {{ b }} else {{ ping(a, b, n - 1) }}
}}
"
        );
        assert_eq!(provenance(&src, "ping"), "& [param#0 param#1]");
        assert_eq!(provenance(&src, "pong"), "& [param#0 param#1]");
    }

    /// メソッドのレシーバも出自になる
    #[test]
    fn レシーバからの借用も出自になる() {
        let src = format!(
            "{PAIR}impl Pair {{ fn first(&self -> &Inner) {{ &self.left }} }}
"
        );
        let checked = accepted(&src);
        let dumped = checked.plan.dump(&checked.hir);
        assert!(dumped.contains("provenance & [self.left]"), "{dumped}");
    }

    /// 呼び出し側では出自が実引数へ置き換わる。結果が生きている間、元は
    /// 借りられたまま
    #[test]
    fn 呼び出し側で出自が実引数に置き換わる() {
        let src = format!(
            "{PAIR}fn left(p: &Pair -> &Inner) {{ &p.left }}
fn main(-> int) {{
  let mut p = pair()
  let got = left(&p)
  p.left.n = 5
  peek(got)
}}
"
        );
        let diagnostic = only(&src);
        assert_eq!(
            diagnostic.msg,
            "main: `p.left.n` は共有借用されている間は排他的に触れません"
        );
    }

    /// 候補が2つなら両方が借りられたまま(design.md 決定6)
    #[test]
    fn 候補が複数なら全部が借りられたまま() {
        let src = format!(
            "{PAIR}fn pick(a: &Inner, b: &Inner, flag: bool -> &Inner) {{
  if flag {{ a }} else {{ b }}
}}
fn main(-> int) {{
  let mut x = pair()
  let mut y = pair()
  let got = pick(&x.left, &y.left, true)
  y.left.n = 5
  peek(got)
}}
"
        );
        let diagnostic = only(&src);
        assert_eq!(
            diagnostic.msg,
            "main: `y.left.n` は共有借用されている間は排他的に触れません"
        );
    }

    /// 排他で返すなら候補は全部排他に予約されたまま(tasks 4.5)
    #[test]
    fn 排他で返した借用は候補を全部予約する() {
        let src = format!(
            "{PAIR}fn pick(a: &mut Inner, b: &mut Inner, flag: bool -> &mut Inner) {{
  if flag {{ a }} else {{ b }}
}}
fn main() {{
  let mut x = pair()
  let mut y = pair()
  let got = pick(&mut x.left, &mut y.left, true)
  peek(&y.left)
  bump(got)
}}
"
        );
        assert_eq!(
            provenance(
                &format!(
                    "{PAIR}fn pick(a: &mut Inner, b: &mut Inner, flag: bool -> &mut Inner) {{ if flag {{ a }} else {{ b }} }}\n"
                ),
                "pick"
            ),
            "&mut [param#0 param#1]"
        );
        let diagnostic = only(&src);
        assert_eq!(
            diagnostic.msg,
            "main: `y.left` は排他借用されている間は読めません"
        );
    }

    // -----------------------------------------------------------------------
    // 4.6 参照が使える位置
    // -----------------------------------------------------------------------

    /// 局所・引数・戻り値・フィールド再借用・要素再借用は全部そのまま通る
    #[test]
    fn 参照は局所引数戻り値と射影の再借用で使える() {
        accepted(&format!(
            "{PAIR}fn field(p: &Pair -> &Inner) {{ &p.left }}
fn element(xs: [Pair] -> int) {{
  let mut total = 0
  for p in xs {{ total = total + peek(&p.left) }}
  total
}}
fn main(-> int) {{
  let p = pair()
  let local = &p
  let n = peek(field(local))
  n + element([pair()])
}}
"
        ));
    }

    // -----------------------------------------------------------------------
    // 4.7 計画のスナップショット
    // -----------------------------------------------------------------------

    /// 借用・リージョン・provenance の全文。並びまで固定する
    #[test]
    fn 借用とリージョンの全文を固定する() {
        let src = "struct Inner { n: int }
struct Pair { left: Inner, right: Inner }
fn left(p: &Pair -> &Inner) { &p.left }
";
        assert_eq!(
            dump(src),
            "body left
  scope#0 [local#0]
  point#0 - scope#0 nop
  point#1 - scope#0 nop
  point#2 - scope#0 init local#0
  point#3 expr#2 scope#0 & local#0(p).left
  edge #0 -> #2
  edge #2 -> #3
  edge #3 -> #1 exits [scope#0]
  loan#0 & local#0(p).left @ #3 region [#1 #3]
  provenance & [param#0.left]
  entry #0 exit #1
"
        );
    }

    /// 借用を作るようになっても、いま有効な形は全部そのまま通る。
    /// `for`・`match`・`with`・test・可変な累算を1本に詰めてある
    #[test]
    fn 現行の有効な形は借用を作っても通る() {
        accepted(
            "enum Rank { Bronze Gold }
struct Point { x: int, y: int }
trait Clock { fn now(self -> int) }
struct Frozen { t: int }
impl Frozen { fn at(t: int -> Frozen) { Frozen { t = t } } }
impl Clock for Frozen { fn now(self -> int) { self.t } }
effect clock: Clock
fn sum(xs: [int] -> int) {
  let mut total = 0
  for x in xs { total = total + x }
  total
}
fn rank(r: Rank -> int) {
  match r {
    Rank::Bronze: 1
    Rank::Gold: 2
  }
}
fn stamped(-> int) { clock.now() }
fn main(-> int) {
  let p = Point { x = 3, y = 4 }
  let n = sum([1, 2, 3]) + rank(Gold) + p.x * p.y
  with clock(Frozen::at(1000)) { n + stamped() }
}
test \"合計\" { assert sum([1, 2]) == 3 }
",
        );
    }

    /// プロセスを跨いだ決定性の見本。借用・リージョン・provenance と、
    /// それを閉じる不動点を全部通る
    fn fingerprint_source() -> String {
        format!(
            "{PAIR}fn ping(a: &Inner, b: &Inner, n: int -> &Inner) {{
  if n == 0 {{ a }} else {{ pong(b, a, n - 1) }}
}}
fn pong(a: &Inner, b: &Inner, n: int -> &Inner) {{
  if n == 0 {{ b }} else {{ ping(a, b, n - 1) }}
}}
fn field(p: &Pair -> &Inner) {{ &p.left }}
fn main(-> int) {{
  let mut p = pair()
  let a = &mut p.left
  let b = &mut p.right
  bump(a)
  bump(b)
  let view = &p
  peek(field(view)) + peek(ping(&p.left, &p.right, 3))
}}
"
        )
    }

    const FINGERPRINT: &str = "ownership::tests::計画の指紋を出す";

    /// 子プロセスから呼ばれる。標準出力に計画をそのまま出すだけ
    #[test]
    fn 計画の指紋を出す() {
        println!("<<<plan\n{}plan>>>", dump(&fingerprint_source()));
    }

    /// 別々のプロセスで走らせても同じ計画が出る。
    ///
    /// 同じプロセスで2回呼ぶだけでは、走るたびに変わる種(ハッシュの初期値、
    /// アドレス)を掴んでいても気付けない。自分自身のテストバイナリを2回
    /// 起動して突き合わせる
    #[test]
    fn 計画はプロセスを跨いでも同じ文字列になる() {
        let exe = std::env::current_exe().expect("テストバイナリの位置が分かる");
        let run = || {
            let out = std::process::Command::new(&exe)
                .args(["--exact", "--nocapture", FINGERPRINT])
                .output()
                .expect("子プロセスが起動する");
            let stdout = String::from_utf8(out.stdout).expect("UTF-8");
            let (_, rest) = stdout.split_once("<<<plan\n").expect("指紋が出ている");
            let (plan, _) = rest.split_once("plan>>>").expect("指紋が閉じている");
            plan.to_string()
        };
        let first = run();
        assert_eq!(first, run());
        assert_eq!(first, dump(&fingerprint_source()));
        assert!(first.contains("provenance & [param#0 param#1]"), "{first}");
        assert!(first.contains("loan#"), "{first}");
    }

    // -----------------------------------------------------------------------
    // 6.1 所有 struct の構築とフィールドの射影
    // -----------------------------------------------------------------------

    /// 所有のデータ4形(struct・optional・payload enum・配列)の道具立て
    const DATA: &str = "struct User { id: int, name: str }
enum Lookup { Found(User) Missing }
fn peek(u: &User -> int) { u.id }
fn edit(u: &mut User -> int) { u.id = u.id + 1
  u.id }
fn take(u: User -> int) { u.id }
fn make(-> User) { User { id = 1, name = \"a\" } }
";

    /// `main` に包む。末尾は常に `0`
    fn with_data(body: &str) -> String {
        format!("{DATA}fn main(-> int) {{\n{body}\n  0\n}}\n")
    }

    /// 本体1つ分の計画から、効果の行だけを取り出す
    fn effects(dumped: &str, body: &str) -> Vec<String> {
        dumped
            .split(&format!("body {body}\n"))
            .nth(1)
            .expect("その本体の計画がある")
            .lines()
            .take_while(|line| line.starts_with("  "))
            .filter(|line| line.trim_start().starts_with("point#"))
            .filter_map(|line| line.split_once(" scope#"))
            .map(|(_, rest)| rest.split_once(' ').expect("効果がある").1.to_string())
            .collect()
    }

    /// struct リテラルは所有を作り、その所有はスコープの終わりで落ちる
    #[test]
    fn 構築したstructは所有される() {
        let checked = accepted(&with_data("  let u = make()\n  assert peek(u) == 1"));
        let dumped = checked.plan.dump(&checked.hir);
        let main = dumped.split("body main").nth(1).expect("main の計画がある");
        assert!(main.contains("scope#0 [local#0*"), "{dumped}");
        assert!(main.contains("drops [local#0]"), "{dumped}");
    }

    /// フィールドの射影は要求されたものでモードが決まる。Copy は複製、素の
    /// 読みは共有借用、`&mut` は排他、`move` は根ごと消費
    /// (struct-shape-checking「Field access follows value mode」)
    #[test]
    fn フィールドの射影はモードごとに分かれる() {
        let src = with_data(
            "  let mut u = make()\n  \
             let a = u.id\n  \
             let b = &u.name\n  \
             let c = &mut u.name\n  \
             let d = move u.name\n  \
             assert a == 1",
        );
        let checked = accepted(&src);
        let dumped = checked.plan.dump(&checked.hir);
        let modes: Vec<String> = effects(&dumped, "main")
            .into_iter()
            .filter(|effect| effect.contains("local#0(u)"))
            .collect();
        assert_eq!(
            modes,
            vec![
                "read local#0(u).id",
                "& local#0(u).name",
                "&mut local#0(u).name",
                "move local#0(u).name",
            ],
            "{dumped}"
        );
        // 消費した射影の残りはその場で落ちる。根は二度落ちない
        let main = dumped.split("body main").nth(1).expect("main の計画がある");
        assert!(main.contains("drops [rest(local#0 - .name)]"), "{dumped}");
        assert!(
            !main.contains("drops [local#0]"),
            "消費した根は落とさない: {dumped}"
        );
    }

    /// フィールドの差し替えは排他アクセスで、右辺の非 Copy な所有は move する
    /// (struct-shape-checking「Field assignment preserves ownership」)
    #[test]
    fn フィールドの差し替えは右辺を消費する() {
        let src = with_data(
            "  let mut u = make()\n  let v = make()\n  u.name = v.name\n  assert peek(u) == 1",
        );
        let checked = accepted(&src);
        let dumped = checked.plan.dump(&checked.hir);
        let modes: Vec<String> = effects(&dumped, "main")
            .into_iter()
            .filter(|effect| effect.contains(".name"))
            .collect();
        assert_eq!(
            modes,
            vec!["&mut local#0(u).name", "move local#1(v).name"],
            "{dumped}"
        );
    }

    /// `indirect` な所有エッジも計画では普通の所有。落ちるのは根1つで、
    /// 中身を辿るのは破棄そのものの仕事(indirect-recursive-values)
    #[test]
    fn indirectなフィールドを持つ値も所有として落ちる() {
        let src = "struct Node { id: int, indirect next: Node? }
fn main(-> int) {
  let leaf = Node { id = 2, next = nil }
  let root = Node { id = 1, next = leaf }
  root.id
}
";
        let checked = accepted(src);
        let dumped = checked.plan.dump(&checked.hir);
        // `leaf` は `root` の中へ move したので、落ちるのは `root` だけ
        assert!(dumped.contains("drops [local#1]"), "{dumped}");
        assert!(
            !dumped.contains("drops [local#1 local#0]"),
            "move した値は落とさない: {dumped}"
        );
    }

    /// 構築の評価順はソース順。点の並びがそのまま評価順なので、要素と
    /// フィールドの所有はソースに書いた順に動く
    #[test]
    fn 構築の評価順はソース順() {
        for build in [
            "[a, b]",
            "User { id = take(move a), name = name_of(move b) }",
        ] {
            let src = format!(
                "{DATA}fn name_of(u: User -> str) {{ u.name }}
fn main(-> int) {{
  let a = make()
  let b = make()
  let v = {build}
  0
}}
"
            );
            let checked = accepted(&src);
            let dumped = checked.plan.dump(&checked.hir);
            let moves: Vec<String> = effects(&dumped, "main")
                .into_iter()
                .filter(|effect| effect.starts_with("move "))
                .collect();
            assert_eq!(
                moves,
                vec!["move local#0(a)", "move local#1(b)"],
                "{build}: {dumped}"
            );
        }
    }

    // -----------------------------------------------------------------------
    // 6.2 optional の注入・射影・`??`
    // -----------------------------------------------------------------------

    /// `.?` は optional の中身への射影。場所として畳めるので、借用は
    /// フィールドまで指す(optional-field-access)
    #[test]
    fn optional射影は場所になる() {
        let src = with_data("  let u: User? = make()\n  assert u.?name == nil");
        let checked = accepted(&src);
        let dumped = checked.plan.dump(&checked.hir);
        assert!(dumped.contains("& local#0(u).?name @"), "{dumped}");
    }

    /// optional 射影は所有者を消費しない。読んだ後も所有者はそのまま残り、
    /// 差し替えも通る(借用は最後の使用で終わるため、optional-field-access)
    #[test]
    fn optional射影は所有者を消費しない() {
        accepted(&with_data(
            "  let mut u: User? = make()\n  \
             let seen = u.?name == nil\n  \
             u = make()\n  \
             assert seen == false",
        ));
    }

    /// optional の中身から所有は持ち出せない。所有が要るなら先に `??`
    #[test]
    fn optional射影からはmoveできない() {
        let src = with_data("  let u: User? = make()\n  let s = u.?name\n  assert s == nil");
        let diagnostic = only(&src);
        assert_eq!(
            diagnostic.msg,
            "main: `u.?name` は optional の中身への射影なので move できません"
        );
        assert_eq!(
            diagnostic.help.as_deref(),
            Some("`??` で中身を取り出してから所有を動かしてください")
        );
    }

    /// Copy な `??` は複製。左辺の束縛はそのまま残る
    #[test]
    fn copyのcoalesceは複製する() {
        accepted(&with_data(
            "  let n: int? = 1\n  let a = n ?? 0\n  let b = n ?? 0\n  assert a == b",
        ));
    }

    /// 束縛済みの非 Copy を素の `??` で開いても、取れるのは借用だけ
    #[test]
    fn 素のcoalesceは所有を産まない() {
        let src =
            with_data("  let u: User? = make()\n  let v = u ?? make()\n  assert peek(v) == 1");
        let diagnostic = only(&src);
        assert_eq!(
            diagnostic.msg,
            "main: 束縛済みの `u` を素の `??` で開いても取れるのは借用で、所有は動きません"
        );
        assert_eq!(
            diagnostic.help.as_deref(),
            Some("`move u ?? ...` と書いてください")
        );
    }

    /// `move` を書けば所有が動く。動かした後の左辺はもう使えない
    #[test]
    fn 消費するcoalesceは所有を産む() {
        accepted(&with_data(
            "  let u: User? = make()\n  let v = move u ?? make()\n  assert take(move v) == 1",
        ));
        let src = with_data(
            "  let u: User? = make()\n  \
             let v = move u ?? make()\n  \
             let w = move u ?? make()\n  \
             assert take(move v) + take(move w) == 2",
        );
        let diagnostic = only(&src);
        assert_eq!(
            diagnostic.msg,
            "main: `u` は既に move されているので使えません"
        );
    }

    /// 一時値の左辺は既に所有者なので `move` は要らない
    #[test]
    fn 一時値のcoalesceは修飾を要らない() {
        accepted(&format!(
            "{DATA}fn find(-> User?) {{ make() }}
fn main(-> int) {{ take(find() ?? make()) }}
"
        ));
    }

    /// 右辺は左辺が空のときだけ走る。だから右辺の move は「経路によっては」
    /// にしかならない(optional-core-type-checking「Fallback remains
    /// short-circuited」)
    #[test]
    fn coalesceの右辺は経路が分かれる() {
        let src = with_data(
            "  let o: User? = make()\n  \
             let fallback = make()\n  \
             let v = move o ?? fallback\n  \
             assert take(move fallback) + take(move v) == 2",
        );
        let diagnostic = only(&src);
        assert_eq!(
            diagnostic.msg,
            "main: `fallback` は move される経路があるのでここでは使えません"
        );
    }

    // -----------------------------------------------------------------------
    // 6.3 payload enum の構築と match の3モード
    // -----------------------------------------------------------------------

    /// 構築は payload の所有を受け取る。渡した束縛はもう使えない
    #[test]
    fn payloadの構築は所有を受け取る() {
        let src =
            with_data("  let u = make()\n  let l = Lookup::Found(u)\n  assert take(move u) == 1");
        let diagnostic = only(&src);
        assert_eq!(
            diagnostic.msg,
            "main: `u` は既に move されているので使えません"
        );
    }

    /// 素の `match` は対象を共有借用し、payload も対象の射影への借用で束ねる。
    /// 抜けた後も対象はそのまま使える
    #[test]
    fn 素のmatchは対象を借用する() {
        let src = with_data(
            "  let l = Lookup::Found(make())\n  \
             let n = match l {\n    \
             Lookup::Found(u): peek(u)\n    \
             Lookup::Missing: 0\n  \
             }\n  \
             assert n == 1",
        );
        let checked = accepted(&src);
        let dumped = checked.plan.dump(&checked.hir);
        assert!(dumped.contains("& local#0(l).0 @"), "{dumped}");
        assert!(dumped.contains("drops [local#0]"), "{dumped}");
    }

    /// 一時値にも素の `match` は共有モードを選ぶ。既に所有されていることは
    /// payload を暗黙に arm へ move する根拠にならない。
    #[test]
    fn 一時値への素のmatchはpayloadを借用する() {
        let src = with_data(
            "  let n = match Lookup::Found(make()) {\n    \
             Lookup::Found(u): take(move u)\n    \
             Lookup::Missing: 0\n  \
             }\n  \
             assert n == 1",
        );
        let diagnostic = only(&src);
        assert_eq!(
            diagnostic.msg,
            "main: `u` は借用で束ねた名前なので move できません"
        );
    }

    /// `&mut` の `match` は payload を排他で束ねるので、中身を変えられる
    #[test]
    fn 排他のmatchはpayloadを変更できる() {
        let src = with_data(
            "  let mut l = Lookup::Found(make())\n  \
             let n = match &mut l {\n    \
             Lookup::Found(u): edit(&mut u)\n    \
             Lookup::Missing: 0\n  \
             }\n  \
             assert n == 2",
        );
        let checked = accepted(&src);
        let dumped = checked.plan.dump(&checked.hir);
        assert!(dumped.contains("&mut local#0(l).0 @"), "{dumped}");
        // payload の束縛は借用なので所有者ではない
        let main = dumped.split("body main").nth(1).expect("main の計画がある");
        assert!(main.contains("scope#1 < scope#0 [local#1]"), "{dumped}");
    }

    /// 素の `match` の payload は借用なので、そこから所有は取り出せない
    #[test]
    fn 借用のmatchのpayloadからはmoveできない() {
        let src = with_data(
            "  let l = Lookup::Found(make())\n  \
             let n = match l {\n    \
             Lookup::Found(u): take(move u)\n    \
             Lookup::Missing: 0\n  \
             }\n  \
             assert n == 1",
        );
        let diagnostic = only(&src);
        assert_eq!(
            diagnostic.msg,
            "main: `u` は借用で束ねた名前なので move できません"
        );
        assert_eq!(
            diagnostic.help.as_deref(),
            Some(
                "要素や payload の所有が要るなら、対象そのものに `move` を付けて反復・match してください"
            )
        );
        assert_eq!(
            related(&src, &diagnostic),
            vec![(
                "`u` はここで束ねられています".to_string(),
                "Lookup::Found(u): take(move u)"
            )]
        );
    }

    /// `move` の `match` は enum ごと消費し、選ばれた payload の所有を arm へ
    /// 渡す。対象はもう使えない
    #[test]
    fn 消費するmatchはpayloadを所有する() {
        accepted(&with_data(
            "  let l = Lookup::Found(make())\n  \
             let n = match move l {\n    \
             Lookup::Found(u): take(move u)\n    \
             Lookup::Missing: 0\n  \
             }\n  \
             assert n == 1",
        ));
        let src = with_data(
            "  let l = Lookup::Found(make())\n  \
             let n = match move l {\n    \
             Lookup::Found(u): take(move u)\n    \
             Lookup::Missing: 0\n  \
             }\n  \
             let m = match move l {\n    \
             Lookup::Found(u): take(move u)\n    \
             Lookup::Missing: 0\n  \
             }\n  \
             assert n + m == 1",
        );
        let diagnostic = only(&src);
        assert_eq!(
            diagnostic.msg,
            "main: `l` は既に move されているので使えません"
        );
    }

    /// arm が payload を持ち出さなければ、その中身は arm を抜けるときに落ちる。
    /// 対象は既に丸ごと消費されているので、根が部分的に move された状態は
    /// 現れない(design.md 決定7)
    #[test]
    fn 消費するmatchは持ち出さなかった中身を落とす() {
        let src = with_data(
            "  let l = Lookup::Found(make())\n  \
             let n = match move l {\n    \
             Lookup::Found(u): peek(u)\n    \
             Lookup::Missing: 0\n  \
             }\n  \
             assert n == 1",
        );
        let checked = accepted(&src);
        let dumped = checked.plan.dump(&checked.hir);
        let main = dumped.split("body main").nth(1).expect("main の計画がある");
        // payload の束縛は arm スコープの所有者で、arm を抜ける辺で落ちる
        assert!(main.contains("scope#1 < scope#0 [local#1*]"), "{dumped}");
        assert_eq!(
            main.lines()
                .filter(|line| line.contains("drops [local#1]"))
                .count(),
            1,
            "落ちるのはちょうど1回: {dumped}"
        );
        // 対象そのものは消費済みなので落ちない
        assert!(!main.contains("drops [local#0]"), "{dumped}");
    }

    // -----------------------------------------------------------------------
    // 6.4 配列の構築と `for` の3モード
    // -----------------------------------------------------------------------

    /// 素の `for` は配列を共有借用する。抜けた後も配列は使える
    #[test]
    fn 素のforは配列を借用する() {
        let src = with_data(
            "  let xs = [make()]\n  \
             let mut t = 0\n  \
             for u in xs { t = t + peek(u) }\n  \
             assert t == 1",
        );
        let checked = accepted(&src);
        let dumped = checked.plan.dump(&checked.hir);
        assert!(dumped.contains("& local#0(xs)[_] @"), "{dumped}");
        assert!(dumped.contains("drops [local#0]"), "{dumped}");
    }

    /// 一時配列も素の `for` では共有モードになる。要素の所有を取るには、
    /// 既存の `for u in move xs` のように束縛済みの対象を明示して消費する。
    #[test]
    fn 一時値への素のforは要素を借用する() {
        let src = with_data("  for u in [make()] { assert take(move u) == 1 }");
        let diagnostic = only(&src);
        assert_eq!(
            diagnostic.msg,
            "main: `u` は借用で束ねた名前なので move できません"
        );
    }

    /// `&mut` の `for` は要素を排他で束ねるので、要素を変えられる
    #[test]
    fn 排他のforは要素を変更できる() {
        let src = with_data(
            "  let mut xs = [make()]\n  \
             let mut t = 0\n  \
             for u in &mut xs { t = t + edit(&mut u) }\n  \
             assert t == 2",
        );
        let checked = accepted(&src);
        let dumped = checked.plan.dump(&checked.hir);
        assert!(dumped.contains("&mut local#0(xs)[_] @"), "{dumped}");
    }

    /// `move` の `for` は配列を消費し、要素の所有を周回へ渡す。周回を抜ける
    /// 辺で落ちるので、持ち出さなかった要素も1度だけ落ちる
    #[test]
    fn 消費するforは要素を所有する() {
        let src = with_data(
            "  let xs = [make()]\n  \
             let mut t = 0\n  \
             for u in move xs { t = t + take(move u) }\n  \
             assert t == 1",
        );
        let checked = accepted(&src);
        let dumped = checked.plan.dump(&checked.hir);
        let main = dumped.split("body main").nth(1).expect("main の計画がある");
        assert!(main.contains("move local#0(xs)"), "{dumped}");
        // 要素を持ち出したので周回の背辺では落ちない
        assert!(!main.contains("drops [local#2]"), "{dumped}");

        let kept = with_data(
            "  let xs = [make()]\n  \
             let mut t = 0\n  \
             for u in move xs { t = t + peek(u) }\n  \
             assert t == 1",
        );
        let checked = accepted(&kept);
        let dumped = checked.plan.dump(&checked.hir);
        let main = dumped.split("body main").nth(1).expect("main の計画がある");
        assert_eq!(
            main.lines()
                .filter(|line| line.contains("drops [local#2]"))
                .count(),
            1,
            "持ち出さなかった要素は周回ごとに1度落ちる: {dumped}"
        );
    }

    /// 消費した配列はもう使えない
    #[test]
    fn 消費した配列は後で使えない() {
        let src = with_data(
            "  let xs = [make()]\n  \
             for u in move xs { assert peek(u) == 1 }\n  \
             for u in xs { assert peek(u) == 1 }",
        );
        let diagnostic = only(&src);
        assert_eq!(
            diagnostic.msg,
            "main: `xs` は既に move されているので使えません"
        );
    }

    /// 反復のあいだ配列は借りられたままなので、構造を変える操作は衝突する
    #[test]
    fn 反復中の構造変更を拒否する() {
        let src = with_data(
            "  let mut xs = [make()]\n  \
             for u in xs { xs = [make()] }",
        );
        let messages: Vec<String> = rejected(&src).into_iter().map(|d| d.msg).collect();
        assert!(
            messages.contains(&"main: `xs` は共有借用されている間は排他的に触れません".to_string()),
            "{messages:?}"
        );
    }

    /// 借用で束ねた要素からは所有を取り出せない
    #[test]
    fn 借用のforのループ変数からはmoveできない() {
        let src = with_data("  let xs = [make()]\n  for u in xs { assert take(move u) == 1 }");
        let diagnostic = only(&src);
        assert_eq!(
            diagnostic.msg,
            "main: `u` は借用で束ねた名前なので move できません"
        );
    }

    /// 借用で束ねた要素・payload は変更できない。直し方は対象の側のモード
    #[test]
    fn 借用で束ねた中身は変更できない() {
        let src = with_data("  let xs = [make()]\n  for u in xs { u.id = 2 }");
        let diagnostic = only(&src);
        assert_eq!(
            diagnostic.msg,
            "main: `u.id` は可変な束縛ではないので変更できません"
        );
        assert_eq!(
            diagnostic.help.as_deref(),
            Some("対象に `&mut` を付けて反復・match してください")
        );
        // 消費で束ねた名前も同じく不変。`mut` を書ける構文が無い
        let src = with_data("  let xs = [make()]\n  for u in move xs { u.id = 2 }");
        let diagnostic = only(&src);
        assert_eq!(
            diagnostic.msg,
            "main: `u.id` は可変な束縛ではないので変更できません"
        );
        assert_eq!(diagnostic.help, None);
    }

    // -----------------------------------------------------------------------
    // 6.5 構造的な等値と明示の深い複製
    // -----------------------------------------------------------------------

    /// `==` は両辺を共有借用で読む。比べた後もどちらも所有されたまま
    #[test]
    fn 等値は両辺を借用する() {
        let src = with_data(
            "  let a = make()\n  let b = make()\n  \
             let same = a == b\n  \
             assert take(move a) + take(move b) == 2\n  assert same",
        );
        let checked = accepted(&src);
        let dumped = checked.plan.dump(&checked.hir);
        let modes: Vec<String> = effects(&dumped, "main")
            .into_iter()
            .filter(|effect| effect.starts_with("& local"))
            .collect();
        assert_eq!(modes, vec!["& local#0(a)", "& local#1(b)"], "{dumped}");
    }

    /// `clone()` は所有を産む。元の束縛は共有借用されるだけで残る
    #[test]
    fn cloneは所有を産んで元を残す() {
        let src = with_data(
            "  let mut u = make()\n  \
             let c = u.clone()\n  \
             u.id = 2\n  \
             assert take(move c) + take(move u) == 3",
        );
        let checked = accepted(&src);
        let dumped = checked.plan.dump(&checked.hir);
        let main = dumped.split("body main").nth(1).expect("main の計画がある");
        // レシーバは共有借用。複製の結果は元の借用を運ばないので、直後の
        // 排他アクセスが通る
        assert!(main.contains("& local#0(u)\n"), "{dumped}");
        assert!(main.contains("&mut local#0(u).id"), "{dumped}");
    }

    /// 共有借用からの `clone()` は借用先を所有の値へ複製する
    #[test]
    fn 共有借用からのcloneは所有になる() {
        accepted(&format!(
            "{DATA}fn copy_of(u: &User -> User) {{ u.clone() }}
fn main(-> int) {{ let u = make()\n take(copy_of(u)) + take(move u) }}
"
        ));
    }

    /// 型が `clone` を自分で宣言していればそちらが勝つ
    #[test]
    fn 宣言されたcloneが組み込みより優先する() {
        accepted(&format!(
            "{DATA}impl User {{ fn clone(&self -> int) {{ self.id }} }}
fn main(-> int) {{ let u = make()\n u.clone() }}
"
        ));
    }

    // -----------------------------------------------------------------------
    // 6.6 破棄計画
    // -----------------------------------------------------------------------

    /// 提供本体の中の束縛も、提供を抜ける辺で落ちる
    #[test]
    fn 提供本体を抜ける辺で束縛が落ちる() {
        let src = format!(
            "{SLOT}fn main(-> int) {{
  let s = make()
  with store(s) {{ let inner = make()\n peek(&inner) }}
}}
"
        );
        let checked = accepted(&src);
        let dumped = checked.plan.dump(&checked.hir);
        let main = dumped.split("body main").nth(1).expect("main の計画がある");
        assert!(
            main.lines()
                .any(|line| line.contains("exits [scope#1] drops [local#1]")),
            "{dumped}"
        );
    }

    /// 提供された実体を消費レシーバで手放したら、その後は使えない
    #[test]
    fn 消費した提供の再使用を拒否する() {
        let src = format!(
            "{SLOT}fn main(-> int) {{
  with store(make()) {{ store.finish() + store.look() }}
}}
"
        );
        let diagnostic = only(&src);
        assert_eq!(
            diagnostic.msg,
            "main: `store` へ提供された実体は既に消費されているので使えません"
        );
        assert_eq!(
            related(&src, &diagnostic),
            vec![("ここで消費されました".to_string(), "store.finish()")]
        );
    }

    /// 周回のたびに提供を消費する形も断る。経路を畳んだ `consumed` では
    /// 見えないので、提供を積んだときの周回の深さと突き合わせる
    #[test]
    fn 周回のたびに提供を消費する形を拒否する() {
        let src = format!(
            "{SLOT}fn main(-> int) {{
  let mut n = 0
  with store(make()) {{ while true {{ n = n + store.finish() }} }}
  n
}}
"
        );
        let diagnostic = only(&src);
        assert_eq!(
            diagnostic.msg,
            "main: `store` へ提供された実体を周回のたびに消費しています"
        );
    }

    /// 破棄経路の全文。素通し・枝・周回の背辺と脱出・`return`・提供の脱出・
    /// 消費した射影の残りを1本に詰めて、どこで何がちょうど1度落ちるかを固定する
    #[test]
    fn 破棄経路の全文を固定する() {
        let src = "struct User { id: int, name: str }
fn make(-> User) { User { id = 1, name = \"a\" } }
fn drops(flag: bool -> int) {
  let a = make()
  if flag { let b = make()
    return b.id }
  let c = move a.name
  0
}
";
        assert_eq!(
            dump(src).split("body drops\n").nth(1).expect("計画がある"),
            "  scope#0 [local#0 local#1* local#3*]
  scope#1 < scope#0 [local#2*]
  point#0 - scope#0 nop
  point#1 - scope#0 nop
  point#2 - scope#0 init local#0
  point#3 expr#0 scope#0 nop
  point#4 expr#1 scope#0 init local#1
  point#5 expr#2 scope#0 read local#0(flag)
  point#6 expr#9 scope#0 nop
  point#7 expr#3 scope#1 nop
  point#8 expr#4 scope#1 init local#2
  point#9 expr#6 scope#1 read local#2(b).id
  point#10 expr#9 scope#0 nop
  point#11 expr#12 scope#0 move local#1(a).name
  point#12 expr#13 scope#0 init local#3
  edge #0 -> #2
  edge #2 -> #3
  edge #3 -> #4
  edge #4 -> #5
  edge #5 -> #6
  edge #6 -> #7
  edge #7 -> #8
  edge #8 -> #9
  edge #9 -> #1 exits [scope#1 scope#0] drops [local#2 local#1]
  edge #6 -> #10
  edge #10 -> #11
  edge #11 -> #12 drops [rest(local#1 - .name)]
  edge #12 -> #1 exits [scope#0] drops [local#3]
  entry #0 exit #1
"
        );
    }

    /// 6.1〜6.5 の形を1本に詰めた受理側。データの4形・3つの所有モード・
    /// 複製・等値・消費の取り出しが同じプログラムの中で噛み合うこと
    #[test]
    fn 所有データの形は一本にまとめても通る() {
        accepted(&format!(
            "{DATA}fn consuming_match(-> int) {{
  let l = Lookup::Found(make())
  match move l {{ Lookup::Found(u): take(move u)
    Lookup::Missing: 0 }}
}}
fn mutable_match(-> int) {{
  let mut l = Lookup::Found(make())
  match &mut l {{ Lookup::Found(u): edit(&mut u)
    Lookup::Missing: 0 }}
}}
fn consuming_for(-> int) {{
  let xs = [make(), make()]
  let mut t = 0
  for u in move xs {{ t = t + take(move u) }}
  t
}}
fn mutable_for(-> int) {{
  let mut xs = [make()]
  let mut t = 0
  for u in &mut xs {{ t = t + edit(&mut u) }}
  t
}}
fn consuming_coalesce(-> int) {{
  let o: User? = make()
  take(move o ?? make())
}}
fn cloning(-> int) {{
  let u = make()
  let c = u.clone()
  peek(u) + take(move c) + take(move u)
}}
fn equality(-> bool) {{
  let a = make()
  let b = make()
  let same = a == b
  assert take(move a) + take(move b) == 2
  same
}}
fn main(-> int) {{
  consuming_match() + mutable_match() + consuming_for() + mutable_for()
    + consuming_coalesce() + cloning() + rank(equality())
}}
fn rank(b: bool -> int) {{ if b {{ 1 }} else {{ 0 }} }}
"
        ));
    }

    /// 正典は所有権構文へ移行済みなので、通常入口へ渡す前に受理される。
    #[test]
    fn 移行済みcanonicalは所有権検査を通る() {
        let src = std::fs::read_to_string("examples/canonical.rd").expect("読める");
        let _ = accepted(&src);
    }
}
