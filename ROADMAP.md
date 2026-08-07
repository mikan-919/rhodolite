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
- `fn` / `trait` / `impl` の型パラメータ、呼び出し地点の型引数推論と具体化、
  型引数を鍵に含む whole-program 特殊化
- 組み込み `push` と、通常の Rhodolite ソースで書く `Map<T>` / `impl<T> Map<T> for [T]`
- 単独所有、明示的な `move` / `clone()`、`&T` / `&mut T` の借用検査、決定的 drop
- owned data を含む Core Wasm 生成、組み込み allocator、公開 ABI v0 / v1
- source span と要求の到達経路を伴う実行前診断

現在の大きな制約:

- 型・ambient 要求・借用の解決は whole-program 前提で、モジュール単独型検査は行わない
- 関数値は名前付きトップレベル関数のみ。捕捉のある無名関数(クロージャ)は未実装
- 型引数は推論のみで明示指定できない。generic な `struct` / `enum` と
  制約付き型パラメータは持たない
- `map` は `self` を消費する形だけで、借用版 `map` は無い。配列の `len()` と
  添字アクセス `xs[i]` もまだ無い
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

名前付き関数値と型パラメータを使った汎用 `map` に到達した(MAP-000 〜 MAP-090)。
ambient を要求する関数を `map` へ渡すと、その要求が呼び出し元まで推論される。
エフェクト変数はソース上の型に出さない。クロージャはこの到達点に含めない。

`Map<T>` は組み込みではなく、通常の Rhodolite ソースとして宣言する。`map` は
`self` を消費するので、呼び出しには `move` が要る。

```rhodolite
trait Map<T> { fn map<U>(self, f: fn(T -> U) -> [U]) }

impl<T> Map<T> for [T] {
    fn map<U>(self, f: fn(T -> U) -> [U]) {
        let mut result: [U] = []
        for x in move self { result.push(f(move x)) }
        move result
    }
}

fn fetch(id: int -> User?) {
    db.find(id)
}

fn main(-> [User?]) {
    with db<Postgres> {
        let ids = [1, 2, 3]
        move ids.map(fetch)
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
| MAP-060 | `done` | 汎用関数と trait method の具体化を HIR インタプリタで実行する | MAP-030, MAP-040, MAP-050 | M | 不要 |
| MAP-070 | `done` | 汎用関数と trait method の具体化を Core Wasm へ生成する | MAP-030, MAP-040, MAP-050 | L | 不要 |
| MAP-075 | `done` | 通常コードから使える最小の配列構築手段を追加する | MAP-030, MAP-060, MAP-070 | M | 不要 |
| MAP-080 | `done` | 汎用 `map` trait と配列用 impl を Rhodolite で実装する | MAP-050, MAP-075 | M | 不要 |
| MAP-090 | `done` | 正典 fixture、差分実行、決定性検証を追加する | MAP-080 | M | 不要 |
| MAP-100 | `done` | 仕様と利用者向け文書を更新する | MAP-090 | S | 不要 |

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

契約は MAP-Q6（Decisions）で確定済み。`trait Push<T> { fn push(&mut self, x: T) }`
を新設し、`impl<T> Push<T> for [T]` はコンパイラ組み込み実装として提供する。

完了条件:

- `Push<T>` trait を宣言でき、`impl<T> Push<T> for [T]` が型検査・要求推論・
  ownership 検査を通る（本体は `array_clone` / `array_drop` と同じ位置付けの
  コンパイラ組み込みで、通常の Rhodolite ソースでは書けない）
- `xs.push(y)` は `xs` を既存の暗黙 `&mut` 借用規約（`db.save(...)` と同じ）で
  可変借用し、`y` は通常の関数呼び出し引数と同じ move-once セマンティクスで
  評価される
- capacity 超過時は倍々成長する。capacity が 0 の配列への初回 push は
  capacity 1 を確保し、以降は現在の capacity の2倍を確保する
- push 中の realloc が OOM した場合、既存 allocator 規約（ADR-0011）どおり
  `unreachable` トラップする。push 固有の新しい失敗表現は追加しない
- インタプリタと Core Wasm の両方で push を実行でき、結果配列の内容・長さ・
  要素の所有権最終状態が一致する
- `len()` や添字アクセス（`xs[i]`）は今回のスコープに含めない

検証方法:

- typecheck / ownership の `Push<T>` 契約と `&mut` 借用の焦点テスト
- push の容量成長（0→1→2→4→...）と realloc 発生時のコピーを検証する
  snapshot / unit test
- push を使う小さなプログラムのインタプリタと Wasm の差分実行テスト

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

配列の `len()`（長さ取得）と添字アクセス `xs[i]` を追加する(IDX-000 〜 IDX-060)。
MAP-075 で意図的にスコープ外にした2機能を追加し、`push` に続く配列の基本読み取り
操作を揃える。

| タスクID | 状態 | タスク名 | 依存 | 工数 | 設計判断 |
|---|---|---|---|---|---|
| IDX-000 | `done` | `len()` と添字アクセスの観測可能な契約を決める | なし | S | 必要 |
| IDX-010 | `done` | 文法へ `len()` 呼び出しと `xs[i]` 読み取り・代入の添字構文を追加する | IDX-000 | M | 不要 |
| IDX-020 | `planned` | 添字アクセス・代入の型検査と ownership（借用）規則を追加する | IDX-010 | M | 不要 |
| IDX-030 | `planned` | `len()` と添字の読み取り・代入・範囲外アクセスの実行時契約を HIR インタプリタに実装する | IDX-020 | M | 不要 |
| IDX-040 | `planned` | `len()` と添字の読み取り・代入・範囲外アクセスの実行時契約を Core Wasm に実装する | IDX-020 | M | 不要 |
| IDX-050 | `planned` | 差分実行 fixture と決定性検証を追加する | IDX-030, IDX-040 | S | 不要 |
| IDX-060 | `planned` | 仕様と利用者向け文書を更新する | IDX-050 | S | 不要 |

### IDX-000 — `len()` と添字アクセスの契約

目的:

- MAP-075 の完了条件（「`len()` や添字アクセス `xs[i]` は今回のスコープに含めない」）を
  解除し、実装が構造を勝手に選ばないよう最小契約を先に固定する

完了条件:

- IDX-Q1 〜 IDX-Q5 がすべて Decisions へ移っている
- 呼び出し構文、戻り値の所有権（借用かコピーか）、範囲外アクセスの失敗表現、
  代入可能性（`xs[i] = v` を左辺値として許すか）の境界が Decisions に明記されている

検証方法:

- IDX-Q1 〜 IDX-Q5 が Open Questions に残っていないことをレビューする
- IDX-Q1 〜 IDX-Q5 の決定が Decisions にあることをレビューする

### IDX-010 — 文法

完了条件:

- IDX-000 の契約どおりに `len()` 呼び出し、読み取りの `xs[i]`、代入先としての
  `xs[i] = v` を parse できる
- 添字式の中に任意の式を書け、優先順位・結合規則が既存の method 呼び出し・
  配列リテラルと矛盾しない
- `xs[i] = v` は既存の `user.id = id` と同じ代入文の産出に載せ、代入以外の文脈で
  `xs[i]` は通常の式として使える
- 不正な添字構文（閉じ括弧欠落など）を source span 付きで拒否する
- 既存の非対象構文の AST dump と振る舞いが変わらない

検証方法:

- lexer / parser の焦点テストと dump snapshot
- 不正構文の CLI 診断テスト

### IDX-020 — 型検査と ownership

完了条件:

- `len()` は `int` を返し、レシーバの所有権を消費しない
- 式としての `xs[i]` は要素への `&T` を返す（IDX-Q2）。`i` に `int` 以外を渡す
  ケースを source span 付きで拒否する
- 返す `&T` は既存の `&T` 借用検査（生存期間・排他性）に載る。ADR-0010 が言う
  「動的添字はコンテナ全体と保守的に重なる」借用として扱う
- 代入先としての `xs[i] = v` は `xs` への暗黙 `&mut` 借用（`db.save(...)` と同じ
  規約）のもとで型検査し、`v` は通常の move-once 引数と同じ所有権規則で書き込む
- 範囲チェックは実行時契約（IDX-Q3/Q4）に委ね、型検査では `i` の値を検証しない

検証方法:

- typecheck / ownership の焦点テスト（読み取り借用・代入・借用競合）
- 型不一致・借用違反の CLI 診断テスト

### IDX-030 — HIR インタプリタ

完了条件:

- `len()` が配列の実際の要素数を返す
- 範囲内の `xs[i]` が正しい要素への借用を返す
- 範囲内の `xs[i] = v` が旧要素を drop してから `v` を move で書き込む
- 範囲外の読み取り・代入がどちらも IDX-Q3/Q4 の trap 契約どおりに振る舞う

検証方法:

- eval の `len()` / 範囲内読み取り / 範囲内代入 / 範囲外（読み取り・代入）の焦点テスト

### IDX-040 — Core Wasm 生成

完了条件:

- `len()`、範囲内の読み取り、範囲内の代入がインタプリタと同じ観測結果を返す
  Wasm を生成する
- 範囲外アクセス（読み取り・代入とも）の trap がインタプリタと一致する
- 新しい host import やランタイム構造を追加しない（既存 allocator 契約の範囲で実装する）

検証方法:

- Wasm の `len()` / 添字読み取り / 添字代入の snapshot test
- 独立 engine による差分実行 test

### IDX-050 — 差分 fixture と決定性

完了条件:

- 範囲内読み取り・範囲内代入・範囲外読み取り・範囲外代入・Copy/non-Copy を
  維持された fixture に含める
- 戻り値、実行時失敗、最終状態がインタプリタと Wasm で一致する
- 同じ source tree と option から byte 単位で同じ Wasm が生成される

検証方法:

- differential corpus に追加した fixture の実行
- 連続する2回の build の byte 比較

### IDX-060 — 仕様と利用者向け文書

完了条件:

- OpenSpec の delta specs が main specs へ sync されている
- README、overview、grammar が `len()` / `xs[i]`（読み取り・代入）の契約を
  同じ言葉で説明する
- Current State と Milestones が実装後の状態に更新されている

検証方法:

- 文書間の用語とリンクのレビュー
- `bunx @fission-ai/openspec validate --all --strict`

### IDX-010 〜 IDX-060 共通の完了条件

各実装タスクは IDX-000 で作る OpenSpec tasks の対応範囲を実装する。
次の条件をすべて満たしたときだけ `done` にできる。

- 対応する OpenSpec scenario に自動テストがある
- 新規テストと既存テストが通る
- `cargo fmt --check` と warning をエラーにした Clippy が通る
- インタプリタと Wasm の両方に関わる振る舞いは差分検証されている
- 同じ入力から生成する Wasm が byte 単位で決定的である
- 検証済みの安定状態が単独のスナップショットとしてコミットされている

捕捉付き無名関数(クロージャ)を追加する(CLO-000 〜 CLO-090)。名前付き
トップレベル関数値だけだった呼び出し可能値に、周囲のローカルを捕捉する
無名関数を加え、struct field や配列要素への格納(aggregate 格納)まで含める。
公開 ABI への露出はこの系列に含めない(Future Directions に残す)。

| タスクID | 状態 | タスク名 | 依存 | 工数 | 設計判断 |
|---|---|---|---|---|---|
| CLO-000 | `done` | 捕捉付き無名関数の観測可能な契約を決める | なし | M | 必要 |
| CLO-010 | `ready` | 無名関数リテラルの文法と closure 値の型表現を追加する | CLO-000 | M | 不要 |
| CLO-020 | `planned` | 自由変数の捕捉解決・closure 本体の型検査・aggregate 格納の型検査を追加する | CLO-010 | L | 不要 |
| CLO-030 | `planned` | 捕捉と aggregate 格納の ownership（move/コピー・環境の drop）を検査する | CLO-020 | L | 不要 |
| CLO-040 | `planned` | 捕捉環境を whole-program 特殊化キーに加える | CLO-020, CLO-030 | M | 不要 |
| CLO-050 | `planned` | 捕捉された closure の ambient 要求を推論する | CLO-040 | L | 不要 |
| CLO-060 | `planned` | closure の具体化を HIR インタプリタで実行する | CLO-030, CLO-040, CLO-050 | M | 不要 |
| CLO-070 | `planned` | closure の具体化を Core Wasm へ生成する | CLO-030, CLO-040, CLO-050 | L | 不要 |
| CLO-080 | `planned` | 差分実行 fixture と決定性検証を追加する | CLO-060, CLO-070 | M | 不要 |
| CLO-090 | `planned` | 仕様と利用者向け文書を更新する | CLO-080 | S | 不要 |

### CLO-000 — 捕捉付き無名関数の契約

目的:

- 実装が捕捉・所有権・単相化の構造を勝手に選ばないよう、最小の言語契約を先に固定する

完了条件:

- CLO-Q1 〜 CLO-Q5 がすべて Decisions へ移っている
- 構文、捕捉範囲、所有権モード、型表現、ambient 要求伝播、aggregate 格納と
  公開 ABI のスコープ境界が Decisions に明記されている

検証方法:

- CLO-Q1 〜 CLO-Q5 が Open Questions に残っていないことをレビューする
- CLO-Q1 〜 CLO-Q5 の決定が Decisions にあることをレビューする

### CLO-010 — 文法と型表現

完了条件:

- 名前を省いた `fn(params -> ret) { body }` を無名関数リテラルとして parse できる
- パラメータ型・戻り値型の注釈は named 関数と同じく必須とし、期待型からの推論は
  行わない
- closure 値は既存の named 関数値と同じ型 `fn(P1, P2 -> R)` として型検査へ渡る
- 既存の named 関数値・第二級ブロックの AST dump と振る舞いが変わらない

検証方法:

- lexer / parser の焦点テストと dump snapshot
- 不正構文（型注釈欠落など）の CLI 診断テスト

### CLO-020 — 捕捉解決・型検査・aggregate 格納

完了条件:

- closure 本体で参照する自由変数をローカルのみから一意に解決する。ambient は
  捕捉対象にしない（CLO-050 で呼び出し時に解決する）
- closure 値は `fn(P1, P2 -> R)` として named 関数値と同じ適合規則で
  引数・戻り値・`let` に渡せる
- closure 値を struct field の型、配列要素の型として宣言・型検査できる
  （公開 ABI 上の型としては扱わない）

検証方法:

- typecheck の自由変数解決・スコープ外参照（ambient 誤参照)・型適合の焦点テスト
- struct field / 配列要素に closure 値を持つ宣言の焦点テスト

### CLO-030 — ownership 境界

完了条件:

- 捕捉した Copy local は自動的にコピーされ、非 Copy local は本体中の
  `move x` で明示的に一度だけ消費される。借用捕捉（`&T` / `&mut T` を環境に
  格納すること）はこの系列で扱わない
- closure が呼ばれずに drop される経路でも捕捉環境が一度だけ drop される
- struct field / 配列要素に格納した closure は他の owned 値と同じ move・drop
  規則に従う
- 未解決の捕捉が ownership 以降へ漏れないことを検査する

検証方法:

- ownership の捕捉 move / 二重消費 / 環境 drop の焦点テスト
- aggregate に格納した closure の move / drop 焦点テスト

### CLO-040 — whole-program 特殊化

完了条件:

- 捕捉環境の型（またはキー）を specialization key に加える
- 同じ closure 定義でも捕捉内容が異なれば別 instance になる
- 到達しない closure instance は生成しない

検証方法:

- 特殊化キーの決定性・共有・到達性の snapshot test

### CLO-050 — ambient 要求推論

完了条件:

- closure もただの callable 値として扱い、MAP-050 が作った関数値・型引数ごとの
  要求推論をそのまま適用する
- closure 固有の特別な要求推論経路は追加しない
- 提供忘れは closure を経由する到達経路付きで診断する

検証方法:

- requirement / ambient ABI の closure 経由の組み合わせテスト（MAP-050 の
  既存テストを closure 値でも実行する）
- 提供忘れの CLI 診断テスト

### CLO-060 — HIR インタプリタ

完了条件:

- 捕捉環境を保持した closure 値を実行できる
- 同じ closure 定義を異なる捕捉内容で呼び分けられる
- struct field / 配列要素に格納した closure を呼び出せる
- 捕捉値の move・drop・ambient の振る舞いが検査済み計画と一致する

検証方法:

- eval の捕捉あり closure の生成・呼び出し・drop・aggregate 格納の焦点テスト

### CLO-070 — Core Wasm 生成

完了条件:

- 到達した closure instance だけを生成する
- 捕捉環境が異なる instance を決定的な直接呼び出しへ下ろす
- 新しい table、`funcref`、動的ディスパッチを追加しない
- インタプリタと戻り値・失敗分類・所有値の最終状態が一致する

検証方法:

- Wasm の closure instance・直接 call・aggregate 格納の snapshot test
- 独立 engine による差分実行 test

### CLO-080 — 差分 fixture と決定性

完了条件:

- 捕捉あり/なし、Copy/non-Copy 捕捉、ambient を使う closure、aggregate に
  格納した closure を維持された fixture に含める
- 戻り値、実行時失敗、最終状態がインタプリタと Wasm で一致する
- 同じ source tree と option から byte 単位で同じ Wasm が生成される

検証方法:

- differential corpus に追加した fixture の実行
- 連続する2回の build の byte 比較

### CLO-090 — 仕様と利用者向け文書

完了条件:

- OpenSpec の delta specs が main specs へ sync されている
- README、overview、grammar、CONTEXT.md の「第二級ブロック」記述が捕捉付き
  無名関数と aggregate 格納の現在の境界を同じ言葉で説明する
- 公開 ABI への露出が未実装と明示される
- Current State と Milestones が実装後の状態に更新されている

検証方法:

- 文書間の用語とリンクのレビュー
- `bunx @fission-ai/openspec validate --all --strict`

### CLO-010 〜 CLO-090 共通の完了条件

各実装タスクは CLO-000 で作る OpenSpec tasks の対応範囲を実装する。
次の条件をすべて満たしたときだけ `done` にできる。

- 対応する OpenSpec scenario に自動テストがある
- 新規テストと既存テストが通る
- `cargo fmt --check` と warning をエラーにした Clippy が通る
- インタプリタと Wasm の両方に関わる振る舞いは差分検証されている
- 同じ入力から生成する Wasm が byte 単位で決定的である
- 検証済みの安定状態が単独のスナップショットとしてコミットされている

callable 値（named 関数値・closure）を公開 ABI の引数・戻り値に出せるようにする
(CAB-000 〜 CAB-060)。現在は「公開 ABI に callable 値を出せない」という制約
（Current State）を解除する。ADR-0009 の import-free 制約と ADR-0011 の
内部アドレス非公開の原則は変えない前提で設計する。

| タスクID | 状態 | タスク名 | 依存 | 工数 | 設計判断 |
|---|---|---|---|---|---|
| CAB-000 | `needs-design` | callable 値の公開 ABI 露出の観測可能な契約を決める | なし | L | 必要 |
| CAB-010 | `planned` | 公開シグネチャへの callable 型の許可と ambient 要求ゼロ制約の型検査を追加する | CAB-000 | M | 不要 |
| CAB-020 | `planned` | callable 値の handle 表現とライフサイクル管理をランタイムに実装する | CAB-000 | L | 不要 |
| CAB-030 | `planned` | 汎用 invoke export と（必要なら）解放 export を Core Wasm へ実装する | CAB-010, CAB-020 | L | 不要 |
| CAB-040 | `planned` | ABI v1 メタデータに callable の型記述を追加する | CAB-010 | M | 不要 |
| CAB-050 | `planned` | インタプリタ直接呼び出しと Wasm ABI 越し呼び出しの差分 fixture を追加する | CAB-030, CAB-040 | M | 不要 |
| CAB-060 | `planned` | 仕様と利用者向け文書を更新する | CAB-050 | S | 不要 |

### CAB-000 — callable 値の公開 ABI 露出の契約

目的:

- 実装が handle 表現・ライフサイクル・invoke 規約を勝手に選ばないよう、
  最小の契約を先に固定する
- ADR-0009（import-free artifact）・ADR-0011（内部アドレス非公開）と矛盾しない
  設計にする

完了条件:

- CAB-Q1（方向性）は決定済み。CAB-Q2 〜 CAB-Q5 がすべて Decisions へ移っている
- handle 表現とライフサイクル、対象範囲、ambient 要求の扱い、
  メタデータと invoke 規約が Decisions に明記されている
- 既存 ADR と衝突する場合は、その旨と解決方法（ADR 更新の要否を含む）が
  Decisions に明記されている

検証方法:

- CAB-Q2 〜 CAB-Q5 が Open Questions に残っていないことをレビューする
- CAB-Q2 〜 CAB-Q5 の決定が Decisions にあり、ADR-0009 / ADR-0011 と矛盾しないことをレビューする

### CAB-010 — 型検査：公開シグネチャへの callable 許可

完了条件:

- CAB-Q3 で決めた範囲（named 関数値のみ、または closure も含む）の callable
  型を公開関数の引数・戻り値として宣言できる
- CAB-Q4 の ambient 要求ゼロ制約に反する callable 型を source span 付きで拒否する
- 対象外の callable（例えば CAB-Q3 で除外した種類）を公開シグネチャに書いた
  場合を拒否する

検証方法:

- typecheck の公開シグネチャ callable 許可・拒否の焦点テスト
- ambient 要求が残る callable を公開境界に出した場合の CLI 診断テスト

### CAB-020 — handle 表現とライフサイクル

完了条件:

- CAB-Q2 で決めた handle 表現（使い捨て or 明示解放）をランタイムに実装する
- handle は内部アドレスを含まない不透明 ID である
- 使い捨てでない場合、二重解放・未解放を診断または安全に無視する規約が定まる

検証方法:

- handle 生成・呼び出し・（該当すれば）解放の unit test
- 二重解放・不正 handle の安全性 test

### CAB-030 — Core Wasm の invoke / 解放 export

完了条件:

- CAB-Q5 で決めた汎用 invoke export のシグネチャで callable を呼び出せる
- CAB-Q1 の方向性どおり、host からの callable 注入経路は追加しない
- 新しい table や `funcref` を増やす場合も、host が触れるのは export された
  関数番号のみで、内部レイアウトを公開しない

検証方法:

- Wasm の invoke export の snapshot test
- 独立 engine（host 役）からの呼び出し test

### CAB-040 — ABI v1 メタデータ拡張

完了条件:

- `types` グラフに callable の型記述（引数型・戻り値型）を追加する
- 公開署名から到達した callable 型だけを載せる（既存の到達性原則を保つ）
- 既存の scalar / struct / enum / optional / array のメタデータ形式を変えない

検証方法:

- メタデータ snapshot test（callable を含む公開関数）

### CAB-050 — 差分 fixture

完了条件:

- callable を公開境界へ出す・受け取るプログラムをインタプリタ直接呼び出しと
  Wasm ABI 越し呼び出しの両方で実行する
- 戻り値、実行時失敗、handle ライフサイクルの振る舞いが両者で一致する

検証方法:

- differential corpus に追加した fixture の実行

### CAB-060 — 仕様と利用者向け文書

完了条件:

- OpenSpec の delta specs が main specs へ sync されている
- README、overview、ADR-0011 が callable 値の公開 ABI 露出の契約を同じ言葉で
  説明する（ADR-0011 の更新または新規 ADR の追加を CAB-000 の決定に従って行う）
- Current State の「公開 ABI に borrow や callable 値を出せない」という記述を
  実装後の境界に合わせて更新する

検証方法:

- 文書間の用語とリンクのレビュー
- `bunx @fission-ai/openspec validate --all --strict`

### CAB-010 〜 CAB-060 共通の完了条件

各実装タスクは CAB-000 で作る OpenSpec tasks の対応範囲を実装する。
次の条件をすべて満たしたときだけ `done` にできる。

- 対応する OpenSpec scenario に自動テストがある
- 新規テストと既存テストが通る
- `cargo fmt --check` と warning をエラーにした Clippy が通る
- インタプリタと Wasm の両方に関わる振る舞いは差分検証されている
- 同じ入力から生成する Wasm が byte 単位で決定的である
- 検証済みの安定状態が単独のスナップショットとしてコミットされている

host が実装する関数を `extern` で宣言し、effect handler の中から実際の I/O を
呼べるようにする(EXT-000 〜 EXT-060)。CAB が「wasm→host」の一方向だけを
扱うのに対し、EXT は逆方向(host→wasm への import)を扱う。[ADR-0009](./docs/adr/0009-core-wasm-is-the-compiler-artifact.md)
の「成果物は import を1つも要求しない」という決定1と正面から相容れないため、
EXT-000 の完了条件には新しい ADR による ADR-0009 の supersede を含める。

| タスクID | 状態 | タスク名 | 依存 | 工数 | 設計判断 |
|---|---|---|---|---|---|
| EXT-000 | `needs-design` | `extern` 宣言の観測可能な契約を決め、ADR-0009 を supersede する ADR を書く | なし | L | 必要 |
| EXT-010 | `planned` | `extern fn` 宣言の文法を追加する | EXT-000 | M | 不要 |
| EXT-020 | `planned` | extern 関数の型検査（シグネチャ制約・ambient 要求ゼロ）を追加する | EXT-010 | M | 不要 |
| EXT-030 | `planned` | Core Wasm の import section 生成と `rhodolite.abi` の import 記述を追加する | EXT-020 | L | 不要 |
| EXT-040 | `planned` | インタプリタで extern を実行するための host スタブ機構を追加する | EXT-020 | M | 不要 |
| EXT-050 | `planned` | host スタブを共有した差分実行 fixture を追加する | EXT-030, EXT-040 | M | 不要 |
| EXT-060 | `planned` | 仕様と利用者向け文書を更新する | EXT-050 | S | 不要 |

### EXT-000 — `extern` 宣言の契約

目的:

- host 依存部分の構文・型境界・失敗表現を先に固定する
- ADR-0009 決定1（import-free）を変更する以上、新しい ADR で経緯と差分を
  明文化し、0009 の status を superseded にする

完了条件:

- EXT-Q1 〜 EXT-Q6 がすべて Decisions へ移っている
- 宣言構文、型と ownership の境界、instantiate 契約とメタデータ、
  ambient/effect との統合、失敗の伝達、スコープ境界が Decisions に明記されている
- ADR-0009 を supersede する新しい ADR（`docs/adr/0012-*.md` 想定）の草稿ができ、
  0009 の frontmatter が `status: superseded by ADR-0012` に更新されている

検証方法:

- EXT-Q1 〜 EXT-Q6 が Open Questions に残っていないことをレビューする
- EXT-Q1 〜 EXT-Q6 の決定が Decisions にあることをレビューする
- 新 ADR のレビューと ADR-0009 の status 更新の確認

### EXT-010 — 文法

完了条件:

- EXT-000 の契約どおりに `extern fn name(params -> ret)`（本体なし）を parse できる
- extern 関数は通常の named 関数値と同じ型 `fn(P1, P2 -> R)` として扱われ、
  trait method の実装内から普通に呼び出せる
- 本体を持つ `extern fn` や、対象外の構文（例えば `extern impl`）を source span
  付きで拒否する

検証方法:

- lexer / parser の焦点テストと dump snapshot
- 不正構文の CLI 診断テスト

### EXT-020 — 型検査

完了条件:

- extern 関数のシグネチャは EXT-Q2 で決めた型集合に制限される
- extern 関数は ambient を要求できない（要求があれば source span 付きで拒否する）
- extern 関数を呼ぶ側は通常の関数呼び出しと同じ規則で型検査される

検証方法:

- typecheck の型集合制限・ambient 要求拒否の焦点テスト

### EXT-030 — Core Wasm の import と ABI メタデータ

完了条件:

- 到達した extern 関数だけを Wasm の import として生成する
- `rhodolite.abi` に host が満たすべき import シグネチャの一覧を追加する
- 未解決 import がある場合、instantiate が失敗することを文書化する
  （ADR-0009 決定1の変更点として EXT-000 の新 ADR に明記済みであること）

検証方法:

- Wasm の import section と `rhodolite.abi` の snapshot test
- host スタブを満たした instantiate の成功 test と、満たさない場合の失敗 test

### EXT-040 — インタプリタの host スタブ

完了条件:

- HIR インタプリタが extern 呼び出しを host スタブ実装へ委譲できる
- テストごとに host スタブを差し替えられる
- host スタブが無い extern 呼び出しは明確なエラーで止まる（サイレントに
  no-op しない）

検証方法:

- eval の host スタブ呼び出し・未設定時のエラー焦点テスト

### EXT-050 — 差分 fixture

完了条件:

- 同じ host スタブ実装をインタプリタと Wasm 側（テスト用ホストドライバ）の
  両方で使う fixture を追加する
- extern の戻り値・失敗がインタプリタと Wasm で一致する

検証方法:

- differential corpus に追加した fixture の実行

### EXT-060 — 仕様と利用者向け文書

完了条件:

- OpenSpec の delta specs が main specs へ sync されている
- README、overview、grammar が `extern fn` の契約を同じ言葉で説明する
- Current State の「公開 ABI に borrow や callable 値を出せない」等の記述を
  実装後の境界（extern 経由の host 連携が可能になったこと）に合わせて更新する
- ADR-0009 の status が superseded になっており、新 ADR が `docs/adr/` に
  存在する

検証方法:

- 文書間の用語とリンクのレビュー
- `bunx @fission-ai/openspec validate --all --strict`

### EXT-010 〜 EXT-060 共通の完了条件

各実装タスクは EXT-000 で作る OpenSpec tasks の対応範囲を実装する。
次の条件をすべて満たしたときだけ `done` にできる。

- 対応する OpenSpec scenario に自動テストがある
- 新規テストと既存テストが通る
- `cargo fmt --check` と warning をエラーにした Clippy が通る
- インタプリタと Wasm の両方に関わる振る舞いは差分検証されている
- 同じ入力から生成する Wasm が byte 単位で決定的である
- 検証済みの安定状態が単独のスナップショットとしてコミットされている


## Future Directions

まだ実施を約束していない長期案。

- 関数単位キャッシュと増分ビルド
- LSP と IDE 連携

## Non-goals

少なくとも現在は実装しないもの。

- 型に表れるエフェクト変数や effect row
- モジュール単独型検査と汎用バイナリの配布
- generic struct / enum
- 制約付き型パラメータと overload resolution

## Open Questions

まだ設計判断が終わっていない問題。

- **CAB-Q2 — handle のライフサイクル:** 公開境界を越えて host が持つ
  callable の handle は、一度呼んだら自動解放される使い捨てにするか、host が
  明示的に解放 export を呼ぶ複数回呼び出し可能な handle にするか。内部アドレスは
  [ADR-0011](./docs/adr/0011-owned-data-layout-and-abi-v1.md) の決定1により
  host へ公開できないため、handle は不透明な ID にする
- **CAB-Q3 — 対象範囲:** named 関数値だけを対象にするか、CLO で追加した
  捕捉付き closure（捕捉環境という owned data を伴う）も対象にするか
- **CAB-Q4 — ambient 要求の扱い:** 公開境界を越える callable が ambient を
  要求する場合をどう扱うか。要求ゼロの callable だけ許可し、要求が残るものは
  拒否するのが素直だが、それでよいか
- **CAB-Q5 — メタデータと invoke 規約:** 汎用 invoke export のシグネチャ、
  callable の引数・戻り値型を ADR-0011 の `types` グラフへどう記述するか。
  異なるシグネチャの callable を同じ汎用 invoke に混在させる場合の型安全性を
  どう担保するか
- **EXT-Q1 — 宣言構文:** host 実装の関数をどう宣言するか。`extern fn
  write_log(msg: str -> unit)` のような独立宣言にし、それを普通の関数値として
  trait method の実装（`impl`）内から呼ぶ形にするか、それとも `extern impl`
  のような専用構文で trait 実装ごと host 側に委ねる形にするか
- **EXT-Q2 — 型と ownership の境界:** extern 関数の引数・戻り値にどの型を
  許すか（scalar だけか、ADR-0011 の rich ABI 型まで含むか）。`&T` / `&mut T`
  を渡せるか。渡せる場合、公開 ABI と同じ符号化契約（encode/decode）を
  再利用できるか
- **EXT-Q3 — instantiate 契約とメタデータ:** host が用意すべき import 関数群を
  `rhodolite.abi` にどう記述するか（`imports` 欄の追加など）。ADR-0009 決定1の
  「instantiate は何も走らせない」という前提は崩れる（未解決 import があれば
  instantiate 自体が失敗する）が、それをどう文書化するか
- **EXT-Q4 — ambient / effect との統合:** extern 関数は ambient を要求できない
  （host に要求解決の概念が無い）という前提でよいか。effect の `impl` が
  内部で extern 関数を呼んで実際の I/O を行う、という用途を想定した設計にするか
- **EXT-Q5 — 失敗の伝達:** host 提供の extern 関数が失敗した場合、既存の
  trap 契約（ADR-0009 決定4）にそのまま乗せるか、それとも新しい失敗表現を
  extern 境界だけに導入するか
- **EXT-Q6 — スコープ境界:** extern は常にキャプチャの無い named 関数として
  だけ扱うか（closure 相当の概念を host 側に持ち込まない）。テスト時に
  インタプリタ側で extern をどう実行するか（host スタブの共有機構が要るか）

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
- **MAP-Q6 — 配列構築手段:** `trait Push<T> { fn push(&mut self, x: T) }` を
  新設し、`impl<T> Push<T> for [T]` はコンパイラ組み込み実装として提供する
  （生バッファ操作を要するため通常の Rhodolite ソースでは書けない。
  `array_clone` / `array_drop` と同じ位置付け）。`xs.push(y)` は既存の
  暗黙 `&mut` 借用規約（`db.save(...)` と同じ）で `xs` を可変借用し、`y` は
  通常の関数呼び出し引数と同じ move-once セマンティクスで評価される。
  capacity 超過時は倍々成長し、capacity 0 からの初回 push は capacity 1 を
  確保する。realloc が OOM した場合は既存 allocator 規約（ADR-0011）どおり
  `unreachable` トラップし、push 固有の失敗表現は追加しない。`len()` と
  添字アクセス（`xs[i]`）は今回のスコープに含めない（Future Directions）。
- **IDX-Q1 — `len()` の呼び出し形態:** method 呼び出し `xs.len()` とする。
  `Push<T>` と同様に trait method として宣言し、`xs.push(y)` / `db.save(u)` と
  同じ呼び出し規約に揃える。property 風の `xs.len` は導入しない。
- **IDX-Q2 — `xs[i]` の所有権的意味:** 式としての `xs[i]` は要素への共有借用
  `&T` を返す。ADR-0010 が「動的な配列添字はコンテナ全体と保守的に重なる借用」
  として既に想定している意味論をそのまま使う。値そのものが要る場合は呼び出し側が
  `.clone()`（non-Copy）または通常の読み取り（Copy）で得る。
- **IDX-Q3 — 範囲外アクセス時の失敗表現:** 範囲外の `i` は `unreachable` trap に
  する。ADR-0011 の allocator OOM と同じ trap 系に揃え、`xs[i]` の型は常に `T`
  のままとし `T?` にはしない。範囲チェックが要る呼び出し元は事前に `len()` で
  確認する。
- **IDX-Q4 — インデックスの型と異常値の扱い:** インデックスは言語に唯一の
  整数型 `int` のみを受け付ける。負数・`len()` 以上の値はどちらも
  `0 <= i < len()` の範囲外として扱い、IDX-Q3 と同じ trap 契約に従う
  （区分の異なる失敗表現は設けない）。
- **IDX-Q5 — `xs[i]` への代入:** `xs[i] = v` を左辺値としてこのマイルストーンに
  含める。既存の field 代入・`&mut` place 規約と同じく、代入は `xs` への暗黙
  `&mut` 借用のもとで行い、範囲外は IDX-Q3/Q4 と同じ trap、`v` は通常の
  move-once 引数と同じ所有権規則で書き込む（旧要素の drop を含む）。
- **CLO-Q1 — 構文とパラメータ注釈:** 無名関数リテラルは名前を省いた
  `fn(params -> ret) { body }`。CONTEXT.md の「第二級ブロック」が
  `fn(...) { ... }` を「環境構築を遅延する第一級の値」と位置づけている構文を
  そのまま流用する。パラメータ型・戻り値型の注釈は named 関数と同じく必須とし、
  期待型からの推論は今回追加しない。
- **CLO-Q2 — 捕捉範囲と既定モード:** 捕捉できるのはローカル変数だけとする。
  Copy local は自動的にコピーされ、非 Copy local は本体中の `move x` で
  明示的に一度だけ消費される。借用捕捉（`&T` / `&mut T` を closure の環境に
  格納すること）は今回扱わない（ADR-0010 が未対応の aggregate borrow に当たるため）。
  ambient は捕捉扱いにせず、CLO-Q4 のとおり呼び出し時に解決する。
- **CLO-Q3 — closure 値の型表現:** 捕捉のある closure も既存の named 関数値と
  同じ型 `fn(P1, P2 -> R)` として扱う。捕捉内容の違いは静的型ではなく
  CLO-040 の whole-program 特殊化キー側で区別し、`apply<T, U>` のような既存
  generic helper へ無改造で渡せるようにする。
- **CLO-Q4 — 捕捉された closure の ambient 要求伝播:** closure もただの
  callable 値として扱い、MAP-050 で作った「関数値・型引数ごとの要求推論」を
  そのまま適用する。生成時点の ambient を固定する特別な意味論は導入しない。
- **CLO-Q5 — スコープ境界:** closure 値を struct field・配列要素へ格納する
  aggregate 格納はこの系列に含める。公開 ABI の引数・戻り値へ closure 値を
  出すことは含めず、引き続き Future Direction とする。
- **CAB-Q1 — 方向性:** CAB は wasm→host の一方向（wasm 側が作った callable
  への opaque handle を host が invoke export 経由で呼ぶ）だけを扱う。
  host が定義した実際のロジックを wasm 側から呼ぶ経路は CAB の対象にせず、
  別系列 EXT（`extern` 宣言、ADR-0009 を supersede する新 ADR が必要）が
  引き受ける。したがって逆方向を Non-goals へ明記する必要はない
  ——CAB の範囲外なだけで、EXT で正式に扱う。
