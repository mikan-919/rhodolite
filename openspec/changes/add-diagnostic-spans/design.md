## Context

`lex::Span { start, end }` はバイト位置で、AST の `Item` / `Expr` / `UseDecl` は既にこれを持っている。にもかかわらず実行前の診断はすべて `String` で、span を組み立ての時点で捨てている。段ごとの現状は次の通り。

| 段 | 戻り値 | span |
|---|---|---|
| `lex::lex` | `Result<Vec<Token>, String>` | 無い(位置を知っているのに落とす) |
| `parse::parse` | `Result<Program, ParseError>` | `line: u32` だけ |
| `module::load` | `Result<LoadedProgram, Vec<String>>` | 無い。ソース本文も読み捨てる |
| `typecheck::check` | `Vec<String>` | 無い(42 箇所の `out.push`) |
| `requirement::Analysis::errors_for` | `Vec<String>` | 無い(4 箇所 + 提供忘れ) |
| `eval` | `Result<_, String>` | 無い(この change の対象外) |

制約が3つある。第一に、`module::load` は複数ファイルを1つの `Program` に畳むので、畳んだ後の span 単体ではどのファイルのバイト位置か分からない。第二に、`load_module` は読んだソースをローカル変数に持つだけで、パース後に捨てている。miette は抜粋を描くのにソース本文を要求する。第三に、README の「到達経路付きエラー生成」は mikan の手書き担当であり、`requirement::CallSite` にも `Requirement.path` にも span が無いため、経路のホップを位置で指す土台がまだ無い。

`tests/cli.rs` を含む既存テストは診断の**文言の部分一致**で検査している。文言を変えなければテストは生き残る。

## Goals / Non-Goals

**Goals:**

- 実行前に出る診断が、原因のバイト範囲と、それがどのファイルのものかを持つ。
- CLI がソース抜粋と下線付きで診断を描画する。
- 提供忘れの到達経路の各ホップを、その呼び出し地点の位置で示せる。
- `match` の網羅性・重複・別 enum の診断が、`match` 式と該当 arm の位置を指す。
- 既存の診断文言と、診断が出る条件・順序・終了コードを変えない。

**Non-Goals:**

- `eval` の実行時エラーに span を付けること。
- 診断ごとの型(エラーコード付きの `enum Diagnostic`)へ全面的に作り替えること。
- 新しい検査を足すこと、既存の検査を緩めること。
- 複数診断の集約・抑制・重複除去、警告レベルの導入。
- LSP / エディタ連携、JSON 出力。

## Decisions

### 決定1: `Span` にソース識別子を持たせる

`Span` を `{ src: SourceId, start: u32, end: u32 }` にし、`lex::lex(src, id)` が受け取った id をすべてのトークンに刻む。`module::load_module` はファイルを読むときに id を採番する。

**代案A: 診断側にモジュールを持たせる。** 型検査の診断は `ctx`(`canonical::stamp` のような修飾名)を持っているので、モジュール接頭辞から逆引きできる。文字列からの逆引きに依存するうえ、修飾名を持たない診断で破れる。却下。

**代案B: 畳むときに span を大域オフセットへ書き換える。** 全ソースを連結した1本の座標系にすれば `src` は要らない。AST 全体を歩いて span を書き換えるパスが増え、`Item` を跨ぐ span の再計算が全産出に染み出す。却下。

id を span 側に置けば、診断は「span を持てば描画に必要な情報を全部持つ」で閉じる。`Span` は `Copy` のままなので、既存の受け渡しは変わらない。

### 決定2: ソース本文は `LoadedProgram` が保持する

`load_module` が読んだ本文とパスを `sources: Vec<SourceFile { path, text }>` に積み、`SourceId` はその添字にする。`LoadedProgram` に `sources` を足して CLI まで運ぶ。読み込み失敗で本文が無いモジュールは、そもそも span を持つ診断を出さない。

### 決定3: 診断は1つの構造体に統一し、文言は `format!` のまま残す

```rust
pub struct Diag {
    msg: String,              // 既存の文言をそのまま
    span: Option<Span>,       // 主原因
    label: Option<String>,    // span に添える短い語
    help: Option<String>,     // 直し方 / 到達経路
    related: Vec<Diag>,       // 経路のホップなど、別の位置を指す従属診断
}
```

63 箇所の `push(format!(...))` は `push(Diag::at(span, format!(...)))` になるだけで、文言は動かない。

**代案: 診断ごとに variant を持つ `enum` を作り `#[derive(Diagnostic)]` する。** エラーコードと構造化された help が最初から付く。ただし型検査だけで 42 種あり、variant 定義とフィールドの受け渡しでこの change が数倍になる。診断の**位置**を出すという目的に対して過剰。将来コードや `--explain` が要るときに、`Diag` を分解して variant へ移せばよい。

`span: Option<Span>` にするのは、ソースを指せない診断が実在するため — エントリーファイルが `.rd` でない、親ディレクトリが無い、ファイルを読めない、の3種。ただし `use` が要求したモジュールが見つからない場合は `UseDecl.span` を指すので、span 有りにできる。

### 決定4: miette は CLI 境界でだけ使う

`Diag` は miette を知らない素の構造体にしておき、`main.rs` で `Diag` + `sources` を miette の `Report` へ変換して描く。`#[derive(Diagnostic)]` を付けた薄い変換用の型を1つ置き、`#[source_code]` に該当ファイルの `NamedSource` を、`#[label]` に主 span を、`#[related]` にホップを渡す。

依存の刻み方は `miette = { version = "7", features = ["fancy"] }`。`fancy` はカラー・罫線・折り返しのために推移依存を数個引き込むが、この change の目的そのものなので払う。`fancy` を外すと素のテキスト描画になるので、依存量が問題になった時点で外せる逃げ道は残る。

### 決定5: 到達経路は `related` で描く

`requirement::CallSite` に呼び出し地点の `Span` を足し、`Requirement.path` を「関数名 + その呼び出しの span」の列にする。提供忘れの診断は、使用地点(`clock.now()`)を主 span、経路の各ホップを `related` として持つ。

**代案: 1つの診断に複数ラベルを付ける。** 見た目は1枚の枠にまとまって読みやすいが、miette の複数ラベルは同一 `source_code` 前提なので、経路がファイルを跨ぐと描けない。ADR-0006 のモジュール分割がある以上、経路は跨ぐ。`related` なら各ホップが自分のファイルを持てる。

既存の `clock ← stamp ← promote ← main` の1行表現は `help` に残す。位置が要らない読み方(CLI の一覧、テストの部分一致)がそのまま生き続ける。

### 決定6: `match` の arm 位置は既存の span をそのまま使う

`MatchArm` は既に `span` を持つ。重複・別 enum の arm はその arm の span を、欠落 variant は `match` 式全体の span を指す。欠落は「無いもの」なので指すべき arm が存在せず、式全体が唯一正しい位置になる。

### 決定7: 段の入口を先に、`typecheck` の 42 箇所は後に

`Span` の形と `Diag` を先に入れ、`lex` / `parse` / `module` を通してから `typecheck` / `requirement` を移す。途中の段階でも `Diag::at(None, ...)` で既存文言をそのまま出せるので、段ごとにビルドとテストが通る状態を保てる。

## Risks / Trade-offs

- **[63 箇所の機械的な書き換えで、span の取り違えが混ざる]** → 各段の移行で「その診断が指すべき式」をテストで固定する。文言は変えないので、既存の部分一致テストが回帰の網として同時に効く。
- **[`fancy` が推移依存を引き込み、ゼロ依存だった木が太る]** → 依存は `miette` 1つに閉じ、`Diag` 側は miette を知らない素の構造体にしておく。外すか差し替える判断が CLI の変換関数1つに閉じる。
- **[`Span` に `src` が増え、既存の span 生成箇所とテストが全部通らなくなる]** → 生成は `lex.rs` の数箇所に閉じている。テストは `lex(src)` を直接呼ぶものが多いので、テスト用に id 0 を入れる薄い入口を残す。
- **[`Requirement.path` の型が変わり、要求の可視化出力が壊れる]** → `render()` は名前しか使わないので、ホップから名前を取り出す形に直すだけで出力は同一に保てる。可視化の既存テストで固定する。
- **[経路を `related` で描くと、1つの提供忘れに対して枠が複数出て冗長になる]** → 主診断の `help` に1行表現を残すので、枠を読まなくても経路は分かる。ホップ数が多いときの省略は、実際に長い経路が出てから決める。
- **[複数ファイルのソースを常に保持するのでメモリが増える]** → 診断のために読んだ本文を捨てないだけで、whole-program 前提(ADR-0003)ではどのみち全ソースを読む。実害なし。

## Migration Plan

1. `Span` に `SourceId` を足し、`lex` / `parse` / `module` を新しい形へ通す(この時点では診断の文言・出力は変わらない)。
2. `Diag` を導入し、`module` / `lex` / `parse` の診断を span 付きにする。CLI は素のテキストのまま。
3. `miette` を足し、CLI の描画を差し替える。
4. `typecheck` の 42 箇所、`requirement` の宣言診断を span 付きへ移す。
5. `CallSite` / `Requirement.path` に span を足し、到達経路を `related` にする。
6. `docs/overview.md` と `docs/requirement-map.md` を更新する。

途中で問題が出た場合、3 より前で止めれば CLI の出力は現状と同一なので、いつでも打ち切れる。

## Open Questions

- 経路のホップが多いとき(5階層以上)に `related` を省略するか。実際に長い経路を持つプログラムが出てから決める。
- `eval` の実行時エラーに span を付けるかは別の change。付けるなら評価器の呼び出し規約に span の受け渡しが入るため、この change では触れない。
