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
6種類とする。`needs-design` は mikan の判断待ち、`planned` は依存タスクの
完了待ちを表す。

| タスクID | 状態 | タスク名 | 依存 | 工数 | 設計判断 |
|---|---|---|---|---|---|
| MAP-000 | `needs-design` | 汎用 `map` の観測可能な契約を決める | なし | M | 必要 |
| MAP-010 | `planned` | 関数・trait・impl の型パラメータ構文と型表現を追加する | MAP-000 | L | 不要 |
| MAP-020 | `planned` | 汎用関数の型検査と呼び出し時の具体化を追加する | MAP-010 | L | 不要 |
| MAP-025 | `planned` | 汎用 trait / impl の契約検査と method resolution を追加する | MAP-010, MAP-020 | L | 不要 |
| MAP-030 | `planned` | 具体化された型を ownership 検査へ渡す | MAP-020, MAP-025 | M | 不要 |
| MAP-040 | `planned` | 型引数と trait impl を whole-program 特殊化キーに加える | MAP-020, MAP-025 | L | 不要 |
| MAP-050 | `planned` | 関数値と型引数ごとに ambient 要求を推論する | MAP-040 | L | 不要 |
| MAP-060 | `planned` | 汎用関数と trait method の具体化を HIR インタプリタで実行する | MAP-030, MAP-040 | M | 不要 |
| MAP-070 | `planned` | 汎用関数と trait method の具体化を Core Wasm へ生成する | MAP-030, MAP-040 | L | 不要 |
| MAP-075 | `planned` | 通常コードから使える最小の配列構築手段を追加する | MAP-030, MAP-060, MAP-070 | M | 不要 |
| MAP-080 | `planned` | 汎用 `map` trait と配列用 impl を Rhodolite で実装する | MAP-050, MAP-075 | M | 不要 |
| MAP-090 | `planned` | 正典 fixture、差分実行、決定性検証を追加する | MAP-080 | M | 不要 |
| MAP-100 | `planned` | 仕様と利用者向け文書を更新する | MAP-090 | S | 不要 |

### MAP-000 — 汎用 `map` の契約

目的:

- 実装が新しい構造を勝手に選ばないよう、最小の言語契約を先に固定する

完了条件:

- MAP-000 の Open Questions がすべて Decisions へ移っている
- 正常系、提供忘れ、型不一致を示す正典プログラムが決まっている
- OpenSpec の proposal / design / specs / tasks が揃い、strict validation を通る
- 実装中に追加する新しい構造と不変条件を列挙し、mikan が承認している

検証方法:

- `bunx @fission-ai/openspec validate --all --strict`
- 正典プログラムの各構文が spec の scenario と対応していることをレビューする

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

- **MAP-Q3A:** `map` trait の名前、trait と method への型パラメータの配置、
  および配列以外の実装を許す契約の広さをどうするか
- **MAP-Q4:** `map` が入力配列と各要素を所有・共有借用・明示選択のどれで受け取るか
- **MAP-Q5:** 型置換と単相化をパイプラインのどの境界で行い、再帰をどう有限化するか

## Decisions

- **MAP-Q1 — 型パラメータの宣言構文:** 宣言名の後ろに `<T, U>` を書く。
  例: `fn map<T, U>(...)`、`trait Map<T> { ... }`、`impl<T> Map<T> for [T] { ... }`。
  今回は generic function / trait / impl を対象とし、generic struct / enum には広げない。
- **MAP-Q2 — 型引数の指定:** 型引数は具体的な呼び出し引数からすべて推論する。
  明示的な型引数指定、部分的な明示指定、推論できない型パラメータは今回扱わない。
- **MAP-Q3 — `map` の提供形態:** `map` は generic trait の method として宣言し、
  配列用の generic impl を通常の Rhodolite コードで実装する。`xs.map(f)` は
  コンパイル時に trait impl へ解決し、prototype chain や実行時のメソッド書き換えは導入しない。
