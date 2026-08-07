## Context

See `proposal.md` - Why. 実装済みの境界を確認した内容:

- 型パラメータの構文・型表現(`fn`/`trait`/`impl` の `<T, U>`)は宣言・検査・
  呼び出し地点の型引数推論・具体化・実行(HIR インタプリタと Core Wasm 両方)まで
  MAP-010〜070 で完了している。ただし明示的な型引数指定、generic `struct`/`enum`、
  制約付き型パラメータ/overload resolution は今回のスコープ外のまま(ROADMAP.md
  Decisions MAP-Q1/Q2、Non-goals)。
- `Push<T>` は `src/typecheck.rs` 上、`trait Push<T> { fn push(&mut self, x: T) }`
  を宣言すると `impl<T> Push<T> for [T]` を **コンパイラ組み込み実装** として
  登録する(パースした本体を要求しない)。`xs.push(y)` は `&mut` 修飾子なしで
  排他借用され、doubling capacity growth を持つ(`openspec/specs/array-push/spec.md`)。
- `Map<T>` は組み込みではない。`trait Map<T> { fn map<U>(self, f: fn(T -> U) -> [U]) }`
  と `impl<T> Map<T> for [T] { ... }` を **通常の Rhodolite ソース**として
  プログラム側が宣言し、その本体は `push`・`for x in move self`・ローカル `[U]` を
  使って書く(`openspec/specs/generic-map/spec.md`、`src/differential.rs` の
  `MAP_COPY_FILES` 等)。呼び出しは `move xs.map(f)`(消費・受け取り側は `move`
  修飾子必須)。借用版 `map` は無く、元配列を残したい場合は `xs.clone().map(f)`。
- `docs/grammar.md` の「型パラメータ」節は既に宣言構文(`Map<T>`/`impl<T> Map<T>
  for [T]` を例に)を持つが、末尾の一文「型パラメータを持つ宣言の本体検査・
  呼び出し地点の型引数推論・具体化はまだ行わないので、generic 宣言は実行経路に
  繋がらない」は MAP-020〜070 で覆されており、事実と反する。
- `docs/overview.md` の「9. 次の一歩」は `compile-wasm-owned-data-values` 着手前の
  文言のまま止まっており、MAP-000〜090 の内容(型パラメータ・`Push<T>`・`Map<T>`・
  差分実行・決定性)に一切触れていない。
- `docs/compiler-roadmap.md` の「compiled v1 より後」節は「入っていないもの」に
  「型パラメータと総称的な `map`」を挙げているが、これは達成済み。この文書は
  ROADMAP.md の MAP トラックへの参照を持たない。
- `ROADMAP.md` の「Current State」は「型パラメータ、汎用的な高階関数は未実装」と
  書き、「Milestones」冒頭は汎用 `map` をこれから実装する到達点として導入している。
  例コードは `move` 修飾子も明示的な `trait`/`impl` 宣言もない簡略形で、
  実装済みの実際の呼び出し規約と食い違う。

## Goals / Non-Goals

**Goals:**
- README・overview・grammar・compiler-roadmap・ROADMAP.md が、汎用 `map` の
  現在の境界(できること/できないこと)を同じ言葉で説明する状態にする。
- クロージャ、明示型引数、generic `struct`/`enum`、借用 `map` が「未実装」だと
  各文書で明示され続けるようにする(実装済みと誤解されないように)。
- 例コードは実際に動く構文(`move`、`trait Map<T>`/`impl` の明示宣言、
  `push` の暗黙借用)に合わせる。

**Non-Goals:**
- 新しい言語仕様・設計判断は導入しない(ROADMAP.md Decisions MAP-Q1〜Q6 が
  そのまま正典)。
- `openspec/specs/` 配下の main spec は変更しない(MAP-080/MAP-090 の feat
  コミットで sync 済みで、今回変更する要件はない)。
- `docs/requirement-map.md`、ADR、`docs/design-notes/`、research 文書
  (`docs/*-research.md`)は今回のスコープ外(ROADMAP.md が名指しした対象は
  README / overview / grammar / compiler roadmap であり、これらは Current
  State に触れない独立文書)。

## Decisions

- **各ファイルの改訂範囲は「現状描写のみ」とし、構成・見出し・文体は変えない。**
  差分レビューのコストを下げるため、既存の文書構造(README の「現状」節、
  overview の節番号、grammar の節、compiler-roadmap の表と図)を保ったまま
  該当パラグラフだけ書き換える。新しい節を足すのは、grammar.md の配列節に
  `push` を、型パラメータ節の直後に `Map<T>` の利用形を短く足す場合だけ
  (現状どちらも一文も触れていない実装済み機能のため)。
  *代替案として検討:* 独立した「generic map」章を各文書に新設する案は、
  ROADMAP.md が要求する「同じ言葉で説明する」がむしろ崩れる(文書ごとに
  節構成が変わり、リンクが辿りにくくなる)ため採らない。
- **例コードは `src/differential.rs` の維持 fixture(`MAP_COPY_FILES` など)か
  `docs/grammar.md` 既存の `Map<T>` 宣言例から取る。** 新規に例を創作しない。
  実装が実際に検査・実行する形と文書の例が乖離しないようにするため。
- **「まだ無い」列挙は ROADMAP.md の Decisions/Non-goals の語彙をそのまま使う。**
  各文書で独自の言い回しを作らず、「明示型引数」「generic struct/enum」
  「借用 `map`」「クロージャ(捕捉のある無名関数)」の4つを共通の否定リストとして
  README・overview・grammar・compiler-roadmap の該当箇所に揃える。
- **`push` は `Map<T>` を理解する前提として grammar.md に最小限だけ足す。**
  `impl<T> Map<T> for [T]` の本体自体が `push` を呼ぶため、`push` に一文も
  触れないまま `map` の例を出すと文書内で未定義語になる。範囲は「組み込みで
  `&mut` 修飾子なしに動く」ことと doubling growth の1〜2文に絞り、
  `array-push` spec の全文を複製しない。
- **compiler-roadmap.md は ROADMAP.md への参照リンクを追加するに留め、
  8段階の compiled v1 到達図は書き換えない。** compiled v1 は既に到達済みの
  マイルストーンで、MAP トラックはその後続(「compiled v1 より後」節)に
  位置づけられる。段階図に9段階目を足すと、compiled v1 の完了線の意味が
  変わって見えるため、「入っていないもの」表の更新とリンク追加だけにする。

## Risks / Trade-offs

- [文書更新のみでコード検証がない] → タスクの検証は文書間の用語・リンクの
  レビューと `openspec validate --all --strict` に限定される(ROADMAP.md の
  MAP-100 検証方法どおり)。共通完了条件のうち「自動テスト」「差分検証」
  「byte 決定性」は本タスクの性質上コード変更を伴わないため該当しないと
  tasks.md に明記する。
- [例コードの陳腐化] → 将来 `Map<T>`/`push` の契約が変わった場合、文書の例も
  追随が要る。`src/differential.rs` の fixture から転記する方針にすることで、
  fixture 変更時に文書の乖離に気付きやすくする(意図的な軽減であり、
  自動同期は導入しない)。
