# Emela 言語調査

調査日: 2026-08-01（Asia/Tokyo）

調査対象: `emela-lang/emela` の `main`（調査時 HEAD [`68b479f`](https://github.com/emela-lang/emela/commit/68b479f0ceca5e9629e79170e59371c5e09be24a)）と公式仕様 `emela-lang/specification`

## 要約

Emela は、WebAssembly を第一ターゲット、JavaScript を第二ターゲットとする、pre-1.0 の実験的な関数型言語である。設計者自身の要約は「Gleam の書き味 × WASM ファースト × effect を型で追う」。最大の特徴は、関数型の `uses` effect row を静的に検査し、派生 effect を解決した最終的な leaf row を、WASM モジュールがホストへ要求する import／capability manifest に対応させる点にある。つまり、型検査と WASM サンドボックスの双方で副作用の権限を制約することが言語の中心思想である。([compiler README](https://github.com/emela-lang/emela/blob/main/README.md), [design principles](https://github.com/emela-lang/specification/blob/main/specs/0000-language-design-principles.md), [current effects reference](https://github.com/emela-lang/specification/blob/main/reference/effects.md))

表面は、immutable な `let`、式指向の block / `if` / `match`、代数的データ型、パターンマッチ、第一級関数、推論されるジェネリクス、trait などを持つ。一方、effects は Koka のような限定継続を伴う実行時 handler ではなく、capability の追跡と、静的 handler の単相化・消去によるコンパイル時 DI である。WASM では wasm-gc に依存せず、非循環 immutable heap と決定的 ARC を採る。([functions](https://github.com/emela-lang/specification/blob/main/reference/functions.md), [data types](https://github.com/emela-lang/specification/blob/main/reference/data-types.md), [traits](https://github.com/emela-lang/specification/blob/main/reference/traits.md), [effects](https://github.com/emela-lang/specification/blob/main/reference/effects.md), [memory model](https://github.com/emela-lang/specification/blob/main/specs/0024-memory-model.md), [ARC](https://github.com/emela-lang/specification/blob/main/specs/0048-arc-wasm.md))

実装は短期間にかなり広い機能を積み上げているが、公式に「experimental」「pre-1.0 and moving quickly」とされ、`0.y.z` の minor release に破壊的変更が入りうる。調査時の最新安定版は 2026-07-26 公開の `v0.10.0` で、リポジトリ自体は 2026-06-26 作成と非常に若い。学習・実験・言語設計の参照には面白いが、互換性と運用実績が必要な本番採用は時期尚早と判断する。([README](https://github.com/emela-lang/emela/blob/main/README.md), [v0.10.0 release](https://github.com/emela-lang/emela/releases/tag/v0.10.0), [GitHub repository API](https://api.github.com/repos/emela-lang/emela))

## 1. 目的と設計思想

公式仕様が掲げる goals は、(1) ゼロから実装できる程度に小さい core、(2) Native/WASM 間で移植できる明示的な実行時挙動、(3) 型付き関数型セマンティクスと明示的 effect tracking、(4) 実装戦略より先に観測可能な挙動を仕様化すること、である。巨大な標準ライブラリ、早期の高度な最適化、ターゲットごとに意味が変わる挙動は初期の non-goals とされる。([specification README](https://github.com/emela-lang/specification/blob/main/README.md))

より具体的な設計原則は次のとおりである。([design principles](https://github.com/emela-lang/specification/blob/main/specs/0000-language-design-principles.md))

- `uses` は静的契約で、型検査後に消去される。effect 自体に実行時表現や実行時コストを持たせない。
- 観測可能な意味は backend 間で同じにし、backend ごとの差は platform 関数／intrinsic の「供給集合」に限定する。コンパイラは「要求 ⊆ 供給」を検査する。
- 純粋演算の意味は stdlib の trait + intrinsic、副作用は Runtime の platform 関数という二つの境界へ置き、コンパイラの核を小さく保つ。
- private 関数では effect を推論し、public API の境界で明示を要求する progressive disclosure を採る。
- 最も制約の厳しい WAMR を設計上の強制関数とし、wasm-gc 非依存、AOT 向きの単相化・静的解決、小フットプリント、決定的挙動を優先する。

明示的な non-goals は、表面言語の ownership / borrowing / move、Koka 型の限定継続つき実行時 effect handler、async、macro、HKT / higher-rank polymorphism / GADT / dependent types、trait object / runtime dispatch、wasm-gc 依存である。ただし、静的 effect handler は後から採用されている。([design principles](https://github.com/emela-lang/specification/blob/main/specs/0000-language-design-principles.md))

## 2. 言語モデル

### 2.1 評価と制御フロー

Emela は strict、expression-oriented な関数型 core である。`let` 束縛は immutable で再代入できず、block の最終式が値になる。文と式の区別や明示的 `return` はない。引数は左から右へ一度ずつ評価される。`if` は `else` 必須の式で、両 branch の型が一致する必要がある。`&&` / `||` は短絡評価する。([functions](https://github.com/emela-lang/specification/blob/main/reference/functions.md), [expressions](https://github.com/emela-lang/specification/blob/main/reference/expressions.md))

ループ構文はなく、反復は再帰で書く。トップレベル関数から自分自身への直接の末尾呼び出しは、スタックを消費しないことが仕様上保証される。ただし相互再帰、関数値経由、trait method 経由、非末尾再帰は保証外である。([functions: self tail calls](https://github.com/emela-lang/specification/blob/main/reference/functions.md#5-%E8%87%AA%E5%B7%B1%E6%9C%AB%E5%B0%BE%E5%91%BC%E3%81%B3%E5%87%BA%E3%81%97))

### 2.2 値と型

主な型は `Unit`, `Bool`, `Int`, `Float`, `Char`, `String`, `Bytes`, `Array<T>`, record, enum, function, `Never` である。`null` はなく、不在は Core Prelude の通常の enum である `Option<T>` で表す。`Array` と `String` は immutable、`Int` は signed i32、加減乗算は modulo 2^32 で wrap、`Float` は IEEE-754 binary64、`String` の利用者向け添字・長さは UTF-8 byte ではなく Unicode scalar value 単位である。([values & types](https://github.com/emela-lang/specification/blob/main/reference/values.md), [data types](https://github.com/emela-lang/specification/blob/main/reference/data-types.md))

enum は payload を持て、generic / recursive にできる。`match` は exhaustive でなければならず、到達不能 arm はエラーになる。pattern guard もある。record は nominal な名前付きフィールド型であり、組み込みの構造的等価を持たず、`==` には `impl Eq` が必要である。variant は `Color::Red` のように `::`、field / receiver / module は `.` を使う。([data types](https://github.com/emela-lang/specification/blob/main/reference/data-types.md), [README examples](https://github.com/emela-lang/emela/blob/main/README.md#syntax-by-example))

関数は第一級で、lambda と lexical closure を持つ。カリー化・部分適用はない。pipeline `lhs |> f(a)` は `f(lhs, a)`、`lhs |> f` は `f(lhs)` への純粋な糖衣である。([functions](https://github.com/emela-lang/specification/blob/main/reference/functions.md), [expressions: pipeline](https://github.com/emela-lang/specification/blob/main/reference/expressions.md#6-pipeline-))

### 2.3 ジェネリクスと trait

名前付き関数、enum、record は型パラメータを持てる。関数呼び出しの型引数は実引数から推論され、lowering 時に単相化される。明示型引数構文は未導入で、型引数を決められない generic function を第一級値にすることもできない。データ型の型引数は payload / field の実引数、または期待型から推論する。([generics](https://github.com/emela-lang/specification/blob/main/reference/generics.md), [generic data types](https://github.com/emela-lang/specification/blob/main/reference/data-types.md#4-%E3%82%B8%E3%82%A7%E3%83%8D%E3%83%AA%E3%83%83%E3%82%AF%E3%83%87%E3%83%BC%E3%82%BF%E5%9E%8B))

`trait` / `impl` は境界つき generic code と operator overloading を担う。呼び出しは単相化時に静的解決され、辞書や vtable は実行時に残らない。孤児規則により、impl は trait か対象型の定義モジュールにしか置けず、(trait, type) の実装を大域的に一意にする。`+`, `-`, `*`, `/`, `%`, `++`, `==`, `<` はそれぞれ Core Prelude の trait method に desugar されるが、`!`, `&&`, `||` は Bool 専用の組み込みである。trait object と runtime dispatch はない。([traits](https://github.com/emela-lang/specification/blob/main/reference/traits.md), [expressions](https://github.com/emela-lang/specification/blob/main/reference/expressions.md))

### 2.4 effect / capability

完全な関数型は概念的に `(A, B) -> T throws E uses F` である。`uses` は capability の集合（effect row）で、関数型では省略すると pure な `uses {}`、関数定義では省略すると本体から最小 row を推論する。`pub fn` は effect row の明示が必須で、注釈は推論 row の上界である。([effects](https://github.com/emela-lang/specification/blob/main/reference/effects.md))

effect operation は `Io.print(...)` のように effect 名で修飾して呼び、囲む関数の `uses` にその effect が含まれなければならない。`fn map<T, U, e>(xs: Array<T>, f: (T) -> U uses e) -> Array<U> uses e` のような effect-row polymorphism があり、小文字の row parameter を実引数の関数値から推論して型検査後に消去する。([effects: operations and gates](https://github.com/emela-lang/specification/blob/main/reference/effects.md#4-effect-%E6%93%8D%E4%BD%9C%E3%81%AE%E5%91%BC%E3%81%B3%E5%87%BA%E3%81%97%E3%81%A8-uses-%E3%82%B2%E3%83%BC%E3%83%88), [effects: row polymorphism](https://github.com/emela-lang/specification/blob/main/reference/effects.md#7-effect-row-%E5%A4%9A%E7%9B%B8), [v0.10.0 release](https://github.com/emela-lang/emela/releases/tag/v0.10.0))

effect は二種類ある。`extern fn` を持つ primitive effect は runtime が platform function を供給する。`extern fn` を持たない derived effect は静的 handler が実装し、依存 effect に discharge される。handler は第一級値ではなく、静的に解決・単相化・消去される。ここは「実行時に継続を capture / resume する algebraic effect handler」ではない。([effects: handler/discharge](https://github.com/emela-lang/specification/blob/main/reference/effects.md#8-handlerdischargeerasure), [runtime boundary](https://github.com/emela-lang/specification/blob/main/reference/runtime-boundary.md))

ただし重要な実装差がある。`specification/reference` は static `handler for ...` を現在の規範として記述する一方、調査時のコンパイラ `main` の AST/parser/typechecker には handler 宣言が見当たらない。したがって、handler 構文まで含む仕様全体ではなく、README がいう「current core subset」が実装対象だと解釈すべきである。これはソースからの推論である。([effects reference](https://github.com/emela-lang/specification/blob/main/reference/effects.md#8-handlerdischargeerasure), [compiler AST](https://github.com/emela-lang/emela/blob/main/crates/emela/src/ast.rs), [parser](https://github.com/emela-lang/emela/blob/main/crates/emela/src/parser.rs), [README](https://github.com/emela-lang/emela/blob/main/README.md))

### 2.5 エラー

失敗は三分類される。回復可能な失敗は関数型の `throws E`、不在は `Option<T>`、回復不能は `panic` である。組み込み `Result<T, E>` はない。`throws` は単一 error 型だけを持つ独立 channel で、複数の error は enum に束ねる。throwing call は `?` で同型の error を伝播するか、式である `try` / exhaustive `catch` で処理する。`?` は `Option` には使えず、`main` は error を外へ伝播できない。([errors](https://github.com/emela-lang/specification/blob/main/reference/errors.md), [error example](https://github.com/emela-lang/emela/blob/main/examples/error_handling.emel))

`throws` と `uses` は直交する。error の送出や `?` 自体は effect を発生させず、host/platform function の通常の失敗も `throws` channel に接続される。([errors](https://github.com/emela-lang/specification/blob/main/reference/errors.md), [runtime boundary](https://github.com/emela-lang/specification/blob/main/reference/runtime-boundary.md))

### 2.6 メモリモデル

表面言語に ownership / borrowing / move はない。heap value は immutable で、再帰 `let`、mutable cell、自己参照 closure などを持たないため、参照は「後に作られた値から先に作られた値」へしか張れず、heap graph を非循環に保つ。この不変条件により、WASM backend は wasm-gc に依存しない linear memory 上の決定的 ARC だけで cycle collector なしに完全回収できる、という設計である。JS backend は host GC を使える。([values](https://github.com/emela-lang/specification/blob/main/reference/values.md), [memory model](https://github.com/emela-lang/specification/blob/main/specs/0024-memory-model.md), [ARC](https://github.com/emela-lang/specification/blob/main/specs/0048-arc-wasm.md))

回収は観測不能で finalizer / destructor / weak reference はない。WASM では最後の参照が消えた点で決定的に回収し、作業集合が有界な末尾再帰 loop は反復回数によらず有界メモリになることを仕様化している。なお source-level resource cleanup については、`defer` の構文・型検査・lowering が `main` に入った直後で、std の error path における file descriptor leak 修正は調査時点で未 merge の PR である。([ARC specification](https://github.com/emela-lang/specification/blob/main/specs/0048-arc-wasm.md), [`defer` commit](https://github.com/emela-lang/emela/commit/68b479f0ceca5e9629e79170e59371c5e09be24a), [open PR #112](https://github.com/emela-lang/emela/pull/112))

## 3. 構文例

現行機能を短く組み合わせると次のようになる。各要素は公式 README / example / reference に基づく。([README syntax](https://github.com/emela-lang/emela/blob/main/README.md#syntax-by-example), [hello example](https://github.com/emela-lang/emela/blob/main/examples/hello.emel), [effect row reference](https://github.com/emela-lang/specification/blob/main/reference/effects.md#7-effect-row-%E5%A4%9A%E7%9B%B8))

```emela
import std.io

enum List<T> {
  Nil
  Cons(T, List<T>)
}

fn each<T, e>(xs: List<T>, f: (T) -> Unit uses e) -> Unit uses e {
  match xs {
    Nil -> ()
    Cons(head, tail) -> {
      f(head)
      each(tail, f)
    }
  }
}

fn main() -> Unit uses { Io } {
  let values: List<String> =
    List::Cons("hello", List::Cons("Emela", List::Nil))

  each(values, fn (value: String) -> Unit uses { Io } {
    Io.print(value ++ "\n")
  })
}
```

ここでは `List<T>` が recursive generic enum、`match` が exhaustive expression、`each` の `e` が effect-row parameter、lambda が `{ Io }` を要求するため `main` まで `Io` が伝播する。実際の standard list API は embedded `std.list` として提供されている。([stdlib list source](https://github.com/emela-lang/emela/blob/main/crates/emela/src/std/list.emel), [effects reference](https://github.com/emela-lang/specification/blob/main/reference/effects.md))

## 4. モジュール、標準ライブラリ、Pome

各 `.emel` file は module unit で、`pub` を持つ関数・enum・record・effect が外部公開される。import は関数単位ではなく module 単位で、import 先の関数は通常 module 修飾して呼ぶ。Core Prelude は全 compilation unit に暗黙 import され、演算子 trait と純粋 intrinsic を供給する。([modules](https://github.com/emela-lang/specification/blob/main/reference/modules.md), [runtime boundary: Core Prelude](https://github.com/emela-lang/specification/blob/main/reference/runtime-boundary.md#5-core-prelude), [core source](https://github.com/emela-lang/emela/blob/main/crates/emela/src/std/core.emel))

`std.io`, `std.fs`, `std.http`, `std.socket`, `std.random`, `std.clock`, `std.string`, `std.bytes`, `std.float` などは compiler に source として埋め込まれている。副作用を持つ操作は effect の背後の platform function へ落ちる。backend は提供可能な platform function / intrinsic の集合を宣言し、compiler が coverage を検査する。WASM executable は計算済み要求を `emela:capabilities` custom section に決定的 JSON として格納する仕様である。([embedded std source tree](https://github.com/emela-lang/emela/tree/main/crates/emela/src/std), [runtime boundary](https://github.com/emela-lang/specification/blob/main/reference/runtime-boundary.md))

配布単位は **Pome**。中央 registry はなく、`github.com/acme/pkg` のような source path の Git repository を `v` prefix の semver tag で versioning し、commit と content hash を `Pome.lock` に pin する。`emela pome add` は依存と transitive dependency が要求する capability 集合を source から計算し、書き込み前に表示する。複数 Pome の workspace は `Bushel.toml` を使う。([README: Pomes](https://github.com/emela-lang/emela/blob/main/README.md#pomes-distribution-and-dependencies), [packaging specification](https://github.com/emela-lang/specification/blob/main/specs/0032-packaging.md), [Pome implementation](https://github.com/emela-lang/emela/tree/main/crates/emela/src/pome))

## 5. コンパイラと実装構成

実装は Rust 2024 edition の Cargo workspace で、主な構成は次のとおり。([workspace manifest](https://github.com/emela-lang/emela/blob/main/Cargo.toml), [architecture](https://github.com/emela-lang/emela/blob/main/docs/ARCHITECTURE.md))

| crate | 役割 |
|---|---|
| `emela` | CLI、lexer、recursive-descent parser、import/name resolution、type checker、lowering、LSP、formatter/linter、Pome、組み込み `wasmi` runner |
| `emela-codegen` | 完全型付き IR、型、backend trait / registry、intrinsic / platform 契約、external backend plugin protocol |
| `emela-backend-wasm` | WASI Preview 1 core WebAssembly backend。Tier 1 |
| `emela-backend-wasm-wasip2` | WASI 0.2 component-model backend。Tier 1 |
| `emela-backend-js` | Node.js 向け JavaScript source backend。Tier 2 |
| `emela-wasm` | browser playground 用 wasm-bindgen binding。default build 対象外 |

pipeline は概ね `lex -> parse -> import resolution -> Core Prelude merge -> typecheck -> lowering/monomorphization -> typed IR -> backend`。frontend は declaration / stage をまたいで複数 error を収集し、すべて span つき diagnostic にする。IR は完全型付きで Serde serialization 可能なため、外部 process backend に JSON で渡せる。([architecture](https://github.com/emela-lang/emela/blob/main/docs/ARCHITECTURE.md), [IR source](https://github.com/emela-lang/emela/blob/main/crates/emela-codegen/src/ir.rs), [plugin protocol](https://github.com/emela-lang/emela/blob/main/crates/emela-codegen/src/plugin.rs))

WASM backend は WAT を生成して `wat` crate で binary 化し、`wasmparser` で検証する。`Int/Bool/Unit -> i32`, `Float -> f64`, heap / function value -> linear-memory pointer という表現で、closure は environment pointer と function table index を使う。参照保持・解放は typed IR へ挿入する。WASI 0.2 backend は core module emitter を再利用して component に wrap する。([architecture](https://github.com/emela-lang/emela/blob/main/docs/ARCHITECTURE.md), [WASM backend](https://github.com/emela-lang/emela/blob/main/crates/emela-backend-wasm/src/lib.rs), [WASI 0.2 backend](https://github.com/emela-lang/emela/blob/main/crates/emela-backend-wasm-wasip2/src/lib.rs))

## 6. 導入と実行

### release binary

公式 installer は macOS Apple Silicon と Linux x86_64 の stable binary を `$HOME/.emela/bin` に入れる。`EMELA_VERSION` で version pin、`EMELA_CHANNEL=nightly` で dev prerelease を選べる。Nix flake `emela-lang/emela.nix` も公式に案内される。([installation](https://github.com/emela-lang/emela/blob/main/README.md#install), [v0.10.0 assets](https://github.com/emela-lang/emela/releases/tag/v0.10.0), [installer](https://github.com/emela-lang/emela/blob/main/install.sh))

```sh
curl -fsSL https://raw.githubusercontent.com/emela-lang/emela/main/install.sh | sh
emela --version
```

### quick start

`emela run` は `wasm-wasi` backend で build し、pure-Rust の `wasmi` runtime で process 内実行するため、外部 WASI runtime は不要。生成した WASI Preview 1 module は `wasmtime` や WAMR `iwasm` でも実行できる。JavaScript output には Node.js が必要である。([quick start](https://github.com/emela-lang/emela/blob/main/README.md#quick-start), [requirements](https://github.com/emela-lang/emela/blob/main/README.md#requirements))

```sh
emela new hello
cd hello
emela run src/main.emel

# JS source を生成して実行
emela build --backend js-node src/main.emel | node

# WASI module を生成して外部 runtime で実行
emela build --backend wasm-wasi -o app.wasm src/main.emel
wasmtime app.wasm
```

source build には Rust 1.85+ / Cargo（edition 2024）が必要で、`cargo build`, `cargo test`, `cargo fmt` を使う。外部 wasm tool は不要。([building](https://github.com/emela-lang/emela/blob/main/README.md#building), [workspace manifest](https://github.com/emela-lang/emela/blob/main/Cargo.toml))

### CLI / tooling

current source の CLI は `check`, `build`, `ir`, `run`, `test`, `backends`, `lsp`, `fmt`, `lint`, `new`, `pome` を持つ。README の CLI 一覧は `test` / `fmt` / `lint` と `wasm-wasip2` をまだ反映していないため、実際の build の `emela --help` / `emela backends` を優先すべきである。([CLI source](https://github.com/emela-lang/emela/blob/main/crates/emela/src/driver.rs), [README CLI](https://github.com/emela-lang/emela/blob/main/README.md#cli))

LSP は stdio で diagnostic、context-aware completion、hover、match arm / impl stub の code action を提供する。VS Code extension、Vim/Neovim syntax + built-in LSP 設定、Sublime/syntect syntax が同梱されるが、Tree-sitter grammar は未提供である。([LSP docs](https://github.com/emela-lang/emela/blob/main/docs/lsp.md), [syntax highlighting docs](https://github.com/emela-lang/emela/blob/main/docs/syntax-highlight.md), [v0.10.0 release](https://github.com/emela-lang/emela/releases/tag/v0.10.0))

## 7. 開発状況と成熟度

2026-08-01 時点の観測は以下のとおり。

- repository は 2026-06-26 作成、調査時点で 15 stars / 3 forks / open issue+PR 5、Apache-2.0。最終 push は 2026-07-30。数値は変動する。([GitHub repository API](https://api.github.com/repos/emela-lang/emela), [LICENSE](https://github.com/emela-lang/emela/blob/main/LICENSE))
- 最新 stable は `v0.10.0`（2026-07-26）。0.1.0 から 0.10.0 まで約3週間で進み、0.10.0 だけでも effect-row polymorphism、Fs capability、LSP hover/code action、function value の effect/throws soundness fix が入った。活発さの証拠であると同時に、変更速度と互換性リスクの大きさも示す。([releases](https://github.com/emela-lang/emela/releases), [v0.10.0](https://github.com/emela-lang/emela/releases/tag/v0.10.0), [CHANGELOG](https://github.com/emela-lang/emela/blob/main/CHANGELOG.md))
- 調査時 `main` は次の `v0.11.0` release PR が開いており、unary minus、IR cleanup node、`defer` frontend/lowering を含む。([release PR #108](https://github.com/emela-lang/emela/pull/108), [`main` HEAD](https://github.com/emela-lang/emela/commit/68b479f0ceca5e9629e79170e59371c5e09be24a))
- CI は Ubuntu/macOS の workspace test、`cargo fmt`, Clippy `-D warnings`、Emela 自身による source formatting check を merge gate として定義する。調査 snapshot の `crates/emela/tests/*.rs` には `#[test]` が約401件あった（ローカル source count であり、全 test 数や成功を保証する数字ではない）。([CI workflow](https://github.com/emela-lang/emela/blob/main/.github/workflows/ci.yml), [integration tests](https://github.com/emela-lang/emela/tree/main/crates/emela/tests))
- compiler、仕様、example、LSP、formatter/linter、package manager、release binaries、CI は揃っており、単なる parser prototype より進んでいる。しかし project 自身が experimental / pre-1.0 と明記し、production-ready の宣言、互換性保証、長期運用実績は確認できない。([README](https://github.com/emela-lang/emela/blob/main/README.md))

## 8. 既知の制約と注意点

### 言語仕様上の制約

- explicit type arguments、generic function value、union/error-row polymorphism、native backend は未導入。error は単一型を enum で束ねる。([README limitations](https://github.com/emela-lang/emela/blob/main/README.md#language-features), [generics](https://github.com/emela-lang/specification/blob/main/reference/generics.md), [errors](https://github.com/emela-lang/specification/blob/main/reference/errors.md#%E6%9C%AA%E8%A7%A3%E6%B1%BA%E4%BA%8B%E9%A0%85))
- ownership / borrowing / move、async、macro、HKT / higher-rank / GADT / dependent types、trait objects / runtime dispatch、Koka 型 runtime handlers は設計上の non-goals。([design principles](https://github.com/emela-lang/specification/blob/main/specs/0000-language-design-principles.md))
- mutable binding/cell と loop 構文がなく、Array も固定長・immutable。tail recursion の保証は自己直接呼び出しに限定される。([values](https://github.com/emela-lang/specification/blob/main/reference/values.md), [functions](https://github.com/emela-lang/specification/blob/main/reference/functions.md))
- `Option` に `?` は使えず、traditional FFI のように任意 host function を app が宣言することもできない。platform registry / host interface と capability coverage の契約に従う。([errors](https://github.com/emela-lang/specification/blob/main/reference/errors.md), [runtime boundary](https://github.com/emela-lang/specification/blob/main/reference/runtime-boundary.md))

### 現行実装・toolchain の制約

- prebuilt binary は macOS arm64 / Linux x86_64 のみ。他 target は Nix source build または Cargo build が必要。生成 JS は Node.js、生成 WASI artifact の直接実行は別 runtime が必要。([installation](https://github.com/emela-lang/emela/blob/main/README.md#install), [requirements](https://github.com/emela-lang/emela/blob/main/README.md#requirements))
- native backend はなく、browser playground binding は workspace の default build から外される。([README](https://github.com/emela-lang/emela/blob/main/README.md), [Cargo workspace](https://github.com/emela-lang/emela/blob/main/Cargo.toml))
- unused imported code を IR から除く dead-code elimination は open issue。生成 binary / JS が実処理より肥大化する、と maintainer が記録している。([issue #23](https://github.com/emela-lang/emela/issues/23))
- `defer` 導入途中のため、調査時 `main` では std.fs / HTTP server の一部 error path に resource leak が残るとする修正 PR が開いている。([PR #112](https://github.com/emela-lang/emela/pull/112))
- Tree-sitter grammar はない。LSP はあるが、成熟した ecosystem / package registry / debugger までは確認できない。Pome は意図的に decentralized で中央 registry を持たない。([syntax docs](https://github.com/emela-lang/emela/blob/main/docs/syntax-highlight.md), [Pomes](https://github.com/emela-lang/emela/blob/main/README.md#pomes-distribution-and-dependencies))

### ドキュメントの整合性

公式仕様は `reference/` を「現在の規範」、番号つき `specs/` を design history / RFC として読むよう指示している。ところが compiler は「current core subset」であり、仕様の全機能を実装しているとは限らない。さらに compiler README の “Not yet implemented” は effect-row polymorphism を未実装と書く一方、`v0.10.0` release と current type checker / reference は実装済みとしている。backend / CLI 一覧にも同様の更新遅れがある。この言語を評価・利用するときは、`reference/`、release notes、実際の compiler source / `--help` の順に突き合わせる必要がある。([specification README](https://github.com/emela-lang/specification/blob/main/README.md), [compiler README](https://github.com/emela-lang/emela/blob/main/README.md), [v0.10.0](https://github.com/emela-lang/emela/releases/tag/v0.10.0), [CLI source](https://github.com/emela-lang/emela/blob/main/crates/emela/src/driver.rs))

## 9. 類似言語との差異

以下は各言語の公式資料との比較である。評価的な表現は資料から導く推論であり、厳密な機能同値を主張しない。

| 比較対象 | 共通点 | Emela の相違点 |
|---|---|---|
| **Gleam** | immutable、typed functional style、small/consistent language、generic custom types、pattern matching、pipeline、複数 backend という書き味が近い。Emela 自身が Gleam を直接の位置づけに挙げる。([Emela principles](https://github.com/emela-lang/specification/blob/main/specs/0000-language-design-principles.md), [Gleam tour](https://tour.gleam.run/everything/), [Gleam official article](https://gleam.run/news/gleams-new-interactive-language-tour/)) | Gleam は Erlang VM / JavaScript target で、target-specific external implementations を持てる。Emela は WASM/WAMR first で、backend を source から名指しせず、同じ観測可能意味と供給 coverage を要求する。さらに `uses` effect row を WASM capability manifest へ結びつける。([Gleam tour: externals](https://tour.gleam.run/everything/), [Emela runtime boundary](https://github.com/emela-lang/specification/blob/main/reference/runtime-boundary.md)) |
| **Koka** | typed functional language、effect types / effect-row polymorphism、reference counting、specialization を重視する点が近い。([Koka official book](https://koka-lang.github.io/koka/doc/book.html), [Emela effects](https://github.com/emela-lang/specification/blob/main/reference/effects.md), [Emela ARC](https://github.com/emela-lang/specification/blob/main/specs/0048-arc-wasm.md)) | Koka は user-defined runtime effect handlers と control operation の resume を主要機能にする。Emela は限定継続つき runtime handler を明示的に採らず、handler は compile-time DI として静的解決・単相化・消去する。Emela の effect は特に host capability / WASM import 権限の追跡へ焦点を置く。([Koka handlers](https://koka-lang.github.io/koka/doc/book.html#sec-effect-handlers), [Emela principles](https://github.com/emela-lang/specification/blob/main/specs/0000-language-design-principles.md), [Emela effects](https://github.com/emela-lang/specification/blob/main/reference/effects.md)) |
| **Elm** | immutable functional languageで、式・代数的データ型・message-oriented な純粋性を重視し、JavaScript に compile する点が一部重なる。([Elm official guide](https://guide.elm-lang.org/)) | Elm は Web application に特化し The Elm Architecture を中心にし、traditional JS FFI を意図的に制限する。Emela は general-purpose / WASM sandbox を志向し、WASI、typed `uses`/`throws`、trait、Pome、複数 backend を持つ。([Elm guide](https://guide.elm-lang.org/), [Elm interop limits](https://guide.elm-lang.org/interop/limits), [Emela README](https://github.com/emela-lang/emela/blob/main/README.md)) |
| **Rust** | wasm-gc に頼らず predictable な memory behavior を狙い、WASM / systems boundary を意識する点は近い。([Rust Book](https://doc.rust-lang.org/book/), [Emela memory model](https://github.com/emela-lang/specification/blob/main/specs/0024-memory-model.md)) | Rust は ownership / borrowing / lifetime を表面型システムに持ち mutation と zero-cost resource control を許す。Emela はそれらを non-goal とし、immutable acyclic heap + runtime-managed ARC で source 上の memory management を隠す。一方、Emela は host capability を `uses` に型として表す。([Rust Book: ownership](https://doc.rust-lang.org/book/ch04-00-understanding-ownership.html), [Emela values](https://github.com/emela-lang/specification/blob/main/reference/values.md), [Emela principles](https://github.com/emela-lang/specification/blob/main/specs/0000-language-design-principles.md)) |

要するに、表面の親戚は Gleam、effect typing の比較対象は Koka、Web/純粋関数型の近隣は Elm、WASM上の資源戦略との対照は Rust である。ただし Emela 固有の組み合わせは「小さい immutable functional core + static effect/capability rows + backend coverage + WASM manifest + wasm-gc 非依存 ARC」である。これは公式 design principles からの要約・推論である。([design principles](https://github.com/emela-lang/specification/blob/main/specs/0000-language-design-principles.md), [runtime boundary](https://github.com/emela-lang/specification/blob/main/reference/runtime-boundary.md), [ARC](https://github.com/emela-lang/specification/blob/main/specs/0048-arc-wasm.md))

## 10. 評価

Emela の最も新規性が高い部分は effect syntax 単体ではなく、source の effect intent を静的に leaf capability へ discharge し、compiler coverage check と配布 artifact の capability manifest、WASM runtime import restrictionまで一本につなぐ設計である。この軸は、plugin / sandbox / edge / embedded runtime のように「コードが何を要求するか」を事前監査したい用途と相性がよい。([effects](https://github.com/emela-lang/specification/blob/main/reference/effects.md), [runtime boundary](https://github.com/emela-lang/specification/blob/main/reference/runtime-boundary.md), [capability manifest spec](https://github.com/emela-lang/specification/blob/main/specs/0025-capability-manifest.md))

一方、現状は仕様と実装が高速に共進化しており、README の陳腐化、release 間の破壊的変更、未実装の仕様面、open な resource leak / code size 課題がある。したがって、現時点では production platform の即時採用候補というより、WASM-first capability language、effect-aware package audit、静的 DI、決定的 ARC を研究・prototype するための実働リファレンス実装と見るのが妥当である。これは上記一次資料に基づく評価である。([README stability warning](https://github.com/emela-lang/emela/blob/main/README.md), [CHANGELOG](https://github.com/emela-lang/emela/blob/main/CHANGELOG.md), [issue #23](https://github.com/emela-lang/emela/issues/23), [PR #112](https://github.com/emela-lang/emela/pull/112))

## 一次資料一覧

- [Compiler / CLI repository](https://github.com/emela-lang/emela)
- [Compiler README](https://github.com/emela-lang/emela/blob/main/README.md)
- [Compiler architecture](https://github.com/emela-lang/emela/blob/main/docs/ARCHITECTURE.md)
- [Changelog](https://github.com/emela-lang/emela/blob/main/CHANGELOG.md)
- [Releases](https://github.com/emela-lang/emela/releases)
- [Language specification repository](https://github.com/emela-lang/specification)
- [Current language reference](https://github.com/emela-lang/specification/tree/main/reference)
- [Language design principles](https://github.com/emela-lang/specification/blob/main/specs/0000-language-design-principles.md)
- [CI workflow](https://github.com/emela-lang/emela/blob/main/.github/workflows/ci.yml)
- [GitHub repository API](https://api.github.com/repos/emela-lang/emela)
