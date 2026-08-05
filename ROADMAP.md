# Roadmap

## Vision
- 書きやすいRubyやJavascriptのような書き心地のまま、Rustのような機能を使えること
- environment(ambient)をつかってプロップのバケツリレーを阻止すること


## Current State

Rhodolite は compiled v1 に到達している。正典プログラム
[`examples/canonical.rd`](./examples/canonical.rd) に対して、ambient の差し替え、
関数ごとの要求推論、提供忘れの到達経路付き診断が動作する。同じ検査済み
プログラムを HIR インタプリタと Core WebAssembly の両方で実行でき、
維持された fixture 群で結果と失敗の分類を差分検証している。

現在の処理系は次のパイプラインが縦につながっている。

```text
source
  → lex / parse / module load
  → static type check + typed HIR lowering
  → ownership / borrowing check
  → CheckedProgram
      ├─ requirement analysis
      ├─ HIR interpreter
      └─ ambient specialization → Core Wasm
```

実装済みの主な能力:

- `effect` slot、trait、`with` による ambient の宣言・使用・提供
- モジュール読み込みと `pub use` による公開関数の選択
- `int` / `bool` / `str` / `unit`、struct、enum、optional、配列
- 分岐、ループ、`match`、method、trait method、名前付き関数値
- 単独所有、明示的な `move` / `clone()`、`&T` / `&mut T` の借用検査、決定的 drop
- owned data を含む Core Wasm 生成、組み込み allocator、公開 ABI v0 / v1
- source span と要求の到達経路を伴う実行前診断

現在の大きな制約:

- 型・ambient 要求・借用の解決は whole-program 前提で、モジュール単独型検査は行わない
- 関数値は名前付きトップレベル関数のみ。クロージャ、型パラメータ、汎用的な高階関数は未実装
- borrow を aggregate に格納できず、shared ownership、GC、raw pointer、`unsafe` は持たない
- 公開 ABI に borrow や callable 値を出せない。ABI はまだ安定化の対象ではない
- async/await、ジェネレータ、バックトラッキング、language-level unwinding はサポートしない
- パッケージマネージャ、LSP、増分ビルド、最適化パイプライン、ネイティブ生成はまだない

この状態の基準線は `cargo test` で検証する。言語の全体像と実装の地図は
[`docs/overview.md`](./docs/overview.md)、compiled v1 までの経緯と完了条件は
[`docs/compiler-roadmap.md`](./docs/compiler-roadmap.md)を参照する。


## Principles

開発時に優先する判断基準。

- シンプルさを優先する
- 後方互換性を守る
- ランタイムを小さく保つ
- 実験的機能は安定APIから分離する
- なるべく読みやすいコードを記述する
  - 名前は言語の概念と役割を表す
  - モジュールの公開面を小さく保ち、実装詳細を内側に隠す
  - コメントには処理の言い換えではなく、理由や制約を書く
  - 実際の重複や利用例が現れる前に抽象化しない

### 自動実装の境界

ROADMAP は AI Agent が繰り返し実装するための作業キューでもある。
Agent は次の条件をすべて満たすタスクだけを自動で開始する。

- 状態が `ready` である
- 依存タスクがすべて `done` である
- 「設計判断」が `不要` である
- 完了条件と検証方法が明記されている

Agent は対象タスクを `in-progress` にし、1タスクずつ実装する。
完了条件を検証し、テストが通った安定状態をコミットしてから `done` にする。

実装中に新しい言語仕様、公開 API、データ表現、ランタイム契約の選択が
必要になった場合、Agent は推測で決めない。タスクを `blocked` にし、
判断が必要な問いを Open Questions に追加して実装を止める。

## Milestones

次の到達点は、名前付き関数値と型パラメータを使った汎用 `map` である。
ambient を要求する関数を `map` へ渡すと、その要求が呼び出し元まで推論される。
エフェクト変数はソース上の型に出さない。クロージャはこの到達点に含めない。

```rhodolite
fn fetch(id: int -> User?) {
    db.find(id)
}

fn main(-> [User?]) {
    with db<Postgres> {
        [1, 2, 3].map(fetch)
    }
}
```

状態は `needs-design` / `planned` / `ready` / `in-progress` / `blocked` / `done` の
6種類とする。`needs-design` は mikan の判断待ち、`planned` はまだ
自動実装できる粒度になっていないこと、`ready` は契約が固定済みであることを
表す。`ready` でも依存タスクが `done` になるまでは選択されない。

| タスクID | 状態 | タスク名 | 依存 | 工数 | 設計判断 |
|---|---|---|---|---|---|
| MAP-000 | `done` | 汎用 `map` の観測可能な契約を決める | なし | M | 必要 |
| MAP-010 | `done` | 関数・trait・impl の型パラメータ構文と型表現を追加する | MAP-000 | L | 不要 |
| MAP-020 | `done` | 汎用関数の型検査と呼び出し時の具体化を追加する | MAP-010 | L | 不要 |
| MAP-025 | `done` | 汎用 trait / impl の契約検査と method resolution を追加する | MAP-010, MAP-020 | L | 不要 |
| MAP-030 | `done` | 具体化された型を ownership 検査へ渡す | MAP-020, MAP-025 | M | 不要 |
| MAP-040 | `done` | 型引数と trait impl を whole-program 特殊化キーに加える | MAP-020, MAP-025 | L | 不要 |
| MAP-050 | `done` | 関数値と型引数ごとに ambient 要求を推論する | MAP-040 | L | 不要 |
| MAP-060 | `in-progress` | 汎用関数と trait method の具体化を HIR インタプリタで実行する | MAP-030, MAP-040, MAP-050 | M | 不要 |
| MAP-070 | `ready` | 汎用関数と trait method の具体化を Core Wasm へ生成する | MAP-030, MAP-040, MAP-050 | L | 不要 |
| MAP-075 | `needs-design` | 通常コードから使える最小の配列構築手段を追加する | MAP-030, MAP-060, MAP-070 | M | 必要 |
| MAP-080 | `ready` | 汎用 `map` trait と配列用 impl を Rhodolite で実装する | MAP-050, MAP-075 | M | 不要 |
| MAP-090 | `ready` | 正典 fixture、差分実行、決定性検証を追加する | MAP-080 | M | 不要 |
| MAP-100 | `ready` | 仕様と利用者向け文書を更新する | MAP-090 | S | 不要 |

### MAP-000 — 汎用 `map` の契約

目的:

- 実装が新しい構造を勝手に選ばないよう、最小の言語契約を先に固定する

完了条件:

- MAP-Q1 〜 MAP-Q5 がすべて Decisions へ移っている
- 構文、型推論、trait、ownership、単相化の境界が Decisions に明記されている

検証方法:

- MAP-Q1 〜 MAP-Q5 が Open Questions に残っていないことをレビューする
- MAP-Q1 〜 MAP-Q5 の決定が Decisions にあることをレビューする

### MAP-010 — 型パラメータの構文と型表現

完了条件:

- `fn name<T>(...)`、`trait Name<T>`、`impl<T> Name<T> for Type` を parse できる
- 型パラメータは宣言内だけで有効な安定 ID として AST から型検査へ渡る
- 重複する型パラメータとスコープ外の参照を source span 付きで拒否する
- generic 本体を保持する型表現と、具体化後の HIR に型変数を残さない不変条件が検査できる
- 既存の non-generic 宣言の AST / HIR dump と振る舞いが変わらない

検証方法:

- lexer / parser / module / HIR の焦点テスト
- generic 宣言の dump snapshot と不正構文の CLI 診断テスト

### MAP-020 — 汎用関数の型検査と具体化

完了条件:

- generic 本体を剛体型変数のまま全域検査する
- 呼び出し引数と callable 署名からすべての型引数を一意に推論する
- 未解決、矛盾、明示的な型引数指定を実行前に診断する
- `identity<T>` と `apply<T, U>` を複数の具体型で検査できる
- 呼び出されない generic 本体の型エラーも報告する

検証方法:

- typecheck の型推論・署名不一致・未解決の焦点テスト
- generic `identity` / `apply` の CLI 成功・失敗テスト

### MAP-025 — 汎用 trait / impl の解決

完了条件:

- generic trait method と generic impl の署名を型パラメータ置換後に契約検査する
- 具体的な receiver 型と引数から trait impl と method 型引数を一意に解決する
- impl の不足・重複・曖昧性・署名不一致を source span 付きで拒否する
- non-generic trait / impl の現行の method resolution を保つ

検証方法:

- typecheck / module の generic trait 契約と method resolution の焦点テスト
- 同名 trait method と不適合 impl の CLI 診断テスト

### MAP-030 — generic と ownership の境界

完了条件:

- ownership pass は型引数がすべて具体化された HIR だけを受け取る
- 具体型の Copy / owned 分類で move、borrow、clone、drop を計画する
- `apply<T, U>` の consuming callback が non-Copy 値を一度だけ消費する
- 型変数が ownership 以降へ漏れた場合は internal invariant violation として検出する

検証方法:

- ownership の generic Copy / non-Copy / move-after-use / drop 焦点テスト
- concrete HIR / CheckedProgram に型変数がないことの invariant test

### MAP-040 — whole-program 特殊化

完了条件:

- generic 宣言 ID、正規化した型引数、callback 束縛を特殊化キーに含める
- 同じキーは一つの instance を共有し、異なる型引数は別 instance になる
- 同じキーの再帰は先に枠を確保して有限に閉じる
- polymorphic recursion は型引数の変化を示す到達経路付きで拒否する
- 到達しない具体化 instance は生成しない

検証方法:

- 特殊化キーの決定性・共有・再帰・到達性の snapshot test
- polymorphic recursion の CLI 診断テスト

### MAP-050 — generic callback の ambient 要求推論

完了条件:

- 要求解析を generic 型引数と callback 束縛ごとの正確な特殊化に対して行う
- ambient を要求する callback の要求が `apply<T, U>` を経由して呼び出し元へ届く
- ambient 不要の callback の特殊化に不要な slot が混ざらない
- 提供忘れは generic helper と callback を含む到達経路付きで診断する

検証方法:

- requirement / ambient ABI の型引数×callback×provider 組み合わせテスト
- 提供忘れと不要 slot 非伝播の CLI 診断テスト

### MAP-060 — HIR インタプリタ

完了条件:

- generic 関数と generic trait method の具体化 instance を実行できる
- 同じ generic 本体を異なる型引数で呼び分けられる
- consuming callback の値・drop・ambient の振る舞いが検査済み計画と一致する

検証方法:

- eval の generic `identity` / `apply` / generic trait method 焦点テスト
- Copy、non-Copy、ambient callback の実行結果テスト

### MAP-070 — Core Wasm 生成

完了条件:

- 到達した generic 関数と trait method の具体化 instance だけを生成する
- 型引数・callback・provider が異なる instance を決定的な直接呼び出しへ下ろす
- table、`funcref`、クロージャ確保、新しい host import を追加しない
- インタプリタと戻り値、失敗分類、所有値の最終状態が一致する

検証方法:

- Wasm の generic instance、直接 call、hidden ambient record の snapshot test
- 独立 engine による差分実行と byte-identical rebuild test

### MAP-075 — 配列構築手段

実装前に、通常の Rhodolite コードが `[U]` を組み立てる最小 API と
その ownership、評価順、OOM 時の振る舞いを決める。決定後に個別の完了条件と
検証方法を追加し、`ready` / `不要` へ変更する。

### MAP-080 — `Map<T>` trait と配列 impl

完了条件:

- `Map<T>` が consuming `map<U>(self, f: fn(T -> U) -> [U])` を宣言する
- `impl<T> Map<T> for [T]` の本体を通常の Rhodolite コードで記述する
- `move xs.map(f)` が各要素を左から一度ずつ callback へ渡し、`[U]` を返す
- `xs.clone().map(f)` は元の配列を残し、借用版 `map` は導入しない
- callback の ambient 要求が `map` と trait dispatch を経由して正確に伝播する

検証方法:

- 配列の Copy / non-Copy / empty / ambient callback の焦点テスト
- インタプリタと Wasm の差分 fixture

### MAP-090 — 正典 fixture と決定性

完了条件:

- scalar、owned data、ambient callback、提供忘れを維持された fixture に含める
- 戻り値、実行時失敗、最終状態、宣言 test の結果を両実行系で照合する
- 代表的な generic `map` の Wasm snapshot を固定する
- 同じ source tree と option から byte 単位で同じ Wasm が生成される

検証方法:

- differential corpus と representative snapshot test
- 連続する2回の build の byte 比較

### MAP-100 — 仕様と利用者向け文書

完了条件:

- OpenSpec の delta specs が main specs へ sync されている
- README、overview、grammar、compiler roadmap が generic `map` の現在の境界を同じ言葉で説明する
- クロージャ、明示型引数、generic data type、借用 `map` が未実装と明示される
- Current State と Milestones が実装後の状態に更新されている

検証方法:

- 文書間の用語とリンクのレビュー
- `bunx @fission-ai/openspec validate --all --strict`

### MAP-010 〜 MAP-100 共通の完了条件

各実装タスクは MAP-000 で作る OpenSpec tasks の対応範囲を実装する。
次の条件をすべて満たしたときだけ `done` にできる。

- 対応する OpenSpec scenario に自動テストがある
- 新規テストと既存テストが通る
- `cargo fmt --check` と warning をエラーにした Clippy が通る
- インタプリタと Wasm の両方に関わる振る舞いは差分検証されている
- 同じ入力から生成する Wasm が byte 単位で決定的である
- 検証済みの安定状態が単独のスナップショットとしてコミットされている


## Future Directions

まだ実施を約束していない長期案。

- 捕捉を持つ無名関数・クロージャ
- callable 値の aggregate 格納と公開 ABI
- 関数単位キャッシュと増分ビルド
- LSP と IDE 連携

## Non-goals

少なくとも現在は実装しないもの。

- クロージャとその実行時 ABI
- 型に表れるエフェクト変数や effect row
- モジュール単独型検査と汎用バイナリの配布
- generic struct / enum
- 制約付き型パラメータと overload resolution

## Open Questions

まだ設計判断が終わっていない問題。

- **MAP-Q6:** 通常の Rhodolite コードが結果配列を組み立てる最小 API をどうするか

## Decisions

- **MAP-Q1 — 型パラメータの宣言構文:** 宣言名の後ろに `<T, U>` を書く。
  例: `fn map<T, U>(...)`、`trait Map<T> { ... }`、`impl<T> Map<T> for [T] { ... }`。
  今回は generic function / trait / impl を対象とし、generic struct / enum には広げない。
- **MAP-Q2 — 型引数の指定:** 型引数は具体的な呼び出し引数からすべて推論する。
  明示的な型引数指定、部分的な明示指定、推論できない型パラメータは今回扱わない。
- **MAP-Q3 — `map` の提供形態:** `map` は generic trait の method として宣言し、
  配列用の generic impl を通常の Rhodolite コードで実装する。`xs.map(f)` は
  コンパイル時に trait impl へ解決し、prototype chain や実行時のメソッド書き換えは導入しない。
- **MAP-Q3A — `map` trait の抽象度:** 今回は `Map<T>` trait と method 側の
  `map<U>` を持ち、どの型の impl でも結果型を `[U]` に固定する。配列以外の型が
  将来 impl することは禁止しないが、入力と同じコンテナ形状を結果に保つ一般化、
  associated type constructor、higher-kinded type は今回扱わない。
- **MAP-Q4 — `map` の ownership:** `map` は `self` で入力を消費し、
  callback は `fn(T -> U)` で各要素の所有権を受け取る。既存 local を渡すときは
  `move xs.map(f)` と書き、元の配列を残す利用者は `xs.clone().map(f)` を明示する。
  `&self` / `fn(&T -> U)` の借用版は今回追加しない。
- **MAP-Q5 — 型置換と単相化の境界:** generic 本体は型パラメータを
  剛体変数としてまず全域検査する。具体的な呼び出し地点で型引数を推論し、
  generic 宣言 ID、型引数、callback 束縛をキーに具体化した HIR を作る。
  ownership 以降へ渡す HIR と `CheckedProgram` に型変数は残さない。
  同じキーの再帰は具体化枠を先に確保して共有する。一つの再帰循環で同じ
  generic 宣言が異なる型引数を要求する polymorphic recursion は、
  無限具体化を避けるため実行前に診断する。
