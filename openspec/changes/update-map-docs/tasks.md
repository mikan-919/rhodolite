## 1. README.md を更新する

- [ ] 1.1 「型パラメータ、総称的な `map` はまだ無い」という記述を、型パラメータ
  付き `fn`/`trait`/`impl` の宣言・検査・具体化・実行(HIR インタプリタと Core
  Wasm 両方)が完了していることが分かる記述に書き換える
- [ ] 1.2 クロージャ(捕捉のある無名関数)は引き続き未実装であることを明示のまま
  残す

## 2. docs/overview.md を更新する

- [ ] 2.1 「現状」節末尾(名前付き関数値の段落以降)を、型パラメータ・`Push<T>`・
  `Map<T>` が実装済みであることが分かる記述に書き換える
- [ ] 2.2 「9. 次の一歩」節を、MAP-000〜090(型パラメータ、whole-program 特殊化、
  `Push<T>`、`Map<T>`、正典 fixture、決定性検証)が完了した後の状態に合わせて
  書き換え、次の一歩として MAP-100(本文書更新)とその先のクロージャを挙げる
- [ ] 2.3 owned data の Wasm 表現が「次段」であるかのような古い記述
  (`compile-wasm-owned-data-values` 着手前の文言)を除去する

## 3. docs/grammar.md を更新する

- [ ] 3.1 「型パラメータ」節末尾の「本体検査・呼び出し地点の型引数推論・具体化は
  まだ行わない」という記述を、実装済みの内容(呼び出し地点での型引数推論、
  具体化した HIR での実行、インタプリタと Core Wasm の両方での一致)に書き換える
- [ ] 3.2 「型パラメータ」節の直後に、`trait Map<T>`/`impl<T> Map<T> for [T]` を
  通常の Rhodolite ソースとして書き、`move xs.map(f)` で呼ぶ現在の利用形を
  `src/differential.rs` の `MAP_COPY_FILES` 相当の最小例で追記する(`move`
  必須、借用版 `map` は無く `xs.clone().map(f)` が代替であることを含む)
- [ ] 3.3 「配列」節に、組み込み `push`(`&mut` 修飾子なしで排他借用、doubling
  capacity growth)を1〜2文で追記する(3.2 の `map` 例が `push` を前提にするため)
- [ ] 3.4 明示型引数、generic `struct`/`enum`、借用 `map` が未実装であることを
  該当箇所(「型パラメータ」節・「まだ決めていない」節)に明示のまま残す

## 4. docs/compiler-roadmap.md を更新する

- [ ] 4.1 「compiled v1 より後」節の「そこに入っていないもの」列挙から
  「型パラメータと総称的な `map`、配列の高階 API」を外し、達成済みであることを
  順序図(「型パラメータと高階関数」以降)に反映する
- [ ] 4.2 「そこに入っていないもの」に、クロージャ・明示型引数・generic
  `struct`/`enum`・借用 `map` を ROADMAP.md の語彙で明示する
- [ ] 4.3 ROADMAP.md の MAP トラック(Milestones)への参照リンクを追加する
  (8段階の compiled v1 到達図と表はそのまま変更しない)

## 5. ROADMAP.md を更新する

- [ ] 5.1 「Current State」の「現在の大きな制約」から、型パラメータ・汎用的な
  `map` に関する達成済み記述を外し、残る制約(クロージャ、明示型引数、generic
  struct/enum、借用 `map`、配列の `len()`/添字アクセス)だけを列挙する
- [ ] 5.2 「Milestones」冒頭の「次の到達点は…汎用 `map` である」という導入を、
  実績として書き換え、例コードを実際の呼び出し規約(`move` 修飾子、明示的な
  `trait Map<T>`/`impl<T> Map<T> for [T]` 宣言)に合わせる

## 6. 整合性の確認と検証

- [ ] 6.1 README・overview・grammar・compiler-roadmap・ROADMAP.md の間で、
  「まだ無い」機能の語彙(クロージャ/明示型引数/generic struct・enum/借用
  `map`)が同じ言葉で揃っていることをレビューする
- [ ] 6.2 各文書間の相互参照リンク(README → overview/grammar/compiler-roadmap、
  overview → compiler-roadmap、ROADMAP.md → overview/compiler-roadmap)が
  正しく解決することを確認する
- [ ] 6.3 `bunx @fission-ai/openspec validate --all --strict` を実行し、
  main specs(既に MAP-080/MAP-090 で sync 済み)が引き続き通ることを確認する
- [ ] 6.4 `cargo fmt --check` と `cargo clippy -- -D warnings` を実行する
  (本 change は `.rs` を変更しないため、既存の合格状態が壊れていないことの
  確認のみ)
- [ ] 6.5 インタプリタ/Wasm の差分検証と Wasm の byte 決定性は、本 change が
  コード変更を伴わないため該当しないことを明記する(共通完了条件の該当外)
- [ ] 6.6 検証済みの安定状態(文書更新のみ)を単独のコミットとして記録する
