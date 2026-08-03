# 全体像

迷ったらここに戻る。詳細は他の文書にあるので、ここには**地図だけ**置く。

## 1. この言語は何か

> **関数呼び出しで切れない束縛を持つ言語。**

変数は関数を跨ぐと消える。消えない変数を1個足したら、DIコンテナが要らなくなった。
それだけ。他の全部はこれを成立させるための細部。

## 2. 見るべきコードは1つ

```rhodolite
effect clock: Clock            // ← 宣言

fn stamp(u: &mut User) {
    u.at = clock.now()         // ← 使用
}
fn promote(u: &mut User) {
    stamp(u)                   // ← 何も書いていない
}

fn main() {
    let mut user = User { at = 0 }
    with clock(system_clock) { // ← 提供
        promote(&mut user)
    }
}
```

**`promote` が1文字も書いていないのに、`stamp` の要求が `main` に届く。** これが全部。

`examples/canonical.rd` がこれの完全版で、**v1 の到達目標**。

## 3. いま動くもの

```
  ソース (.rd)
      │
      ├─ lex      文字 → トークン            src/lex.rs
      ├─ join     行継続の改行を消す          src/lex.rs
      ├─ parse    トークン → 構文木           src/parse.rs → src/ast.rs
      ├─ load     use を辿って名前解決         src/module.rs
      ├─ check    構文木 → 型付き HIR         src/typecheck.rs → src/hir.rs
      ├─ ownership HIR → CheckedProgram        src/ownership.rs
      ├─ analyze  CheckedProgram → 要求と経路  src/requirement.rs
      └─ eval     CheckedProgram を走らせる    src/eval.rs
                                             ↑ 全部つながっている

  `check` と ownership 検査を通った後は構文木を見ない。CheckedProgram の HIR は全ての式の具体型と、
  呼び出し先・フィールド・variant・局所束縛・提供する実装をプログラム内 ID で
  持っているので、要求解析も評価も名前で引き直さない

  診断は全段が `Diag` を返し、CLI が抜粋付きで描く  src/diag.rs / src/render.rs
```

**v1 の到達目標は達成済み。**`cargo run` で `examples/canonical.rd` の
test が緑になる。

`cargo run` で確認できる出力:

```
推論された要求:
  stamp / clock
  promote / clock, db      ← 誰も書いていない
  main / (要求なし)
```

```
$ cargo run examples/missing_handler.rd

  × missing_handler::main: `missing_handler::clock` が提供されていません
    ╭─[examples/missing_handler.rd:17:12]
 16 │ fn stamp(u: &mut User) {
 17 │     u.at = clock.now()
    ·            ────┬────
    ·                ╰── `missing_handler::clock` がここで要る
 18 │ }
    ╰────
  help: missing_handler::clock が要る ← missing_handler::stamp ← missing_handler::promote ← missing_handler::handle ← missing_handler::main
  ├─▶   × `missing_handler::stamp` を呼んでいます
  │       ╭─[examples/missing_handler.rd:21:5]
  │    20 │ fn promote(u: &mut User) {
  │    21 │     stamp(u)
  │       ·     ────┬───
  │       ·         ╰── ここが要求を運ぶ
  │    22 │ }
  │       ╰────
  ├─▶   × `missing_handler::promote` を呼んでいます
  │       ╭─[examples/missing_handler.rd:25:5]
  │    24 │ fn handle(u: &mut User) {
  │    25 │     promote(u)
  │       ·     ─────┬────
  │       ·          ╰── ここが要求を運ぶ
  │    26 │ }
  │       ╰────
  ╰─▶   × `missing_handler::handle` を呼んでいます
          ╭─[examples/missing_handler.rd:30:5]
       29 │     let mut user = User { at = 0 }
       30 │     handle(&mut user)
          ·     ────────┬────────
          ·             ╰── ここが要求を運ぶ
       31 │ }
          ╰────
```

```
テスト:
  ok   昇格すると Gold になり時刻が刻まれる

1 件中 1 件成功
```

## 4. ファイルの地図

| ファイル | 役割 | 行 |
|---|---|---|
| `src/lex.rs` | 字句解析 + 行継続 | 499 |
| `src/ast.rs` | 構文木の型定義。ここを読めば言語の形が分かる | 313 |
| `src/parse.rs` | 再帰下降パーサ | 1774 |
| `src/module.rs` | `use` を辿るモジュール読み込みと名前解決 | 2009 |
| `src/hir.rs` | 型付き・参照解決済みの中間表現。ownership pass の入力 | 1195 |
| `src/ownership.rs` | borrow / move / drop を検査・計画し `CheckedProgram` を作る | 5823 |
| `src/ambient_abi.rs` | ambient を単相化で消す計画。Wasm 生成の入力 | 1376 |
| `src/wasm.rs` | 計画から Core Wasm を生成。対応範囲の検査もここ | 1200 |
| `src/wasm_abi.rs` | Rhodolite Wasm ABI v0。公開署名と埋め込みメタデータ | 200 |
| `src/typecheck.rs` | 型検査と HIR への下ろし(同じ1回の走査) | 5913 |
| `src/requirement.rs` | **要求推論(中核)**。入力は `ownership::CheckedProgram` | 1690 |
| `src/eval.rs` | **所有権検査済み HIR を評価**。compound value は store、borrow は検査済み place | 1932 |
| `src/diag.rs` | 診断の値。位置・ラベル・help・従属診断。全段が返す | 75 |
| `src/render.rs` | 診断をソース抜粋付きで描く。miette を知る唯一の場所 | 107 |
| `src/main.rs` | 繋ぐだけ | 164 |

`requirement.rs` の中は6段。手順1〜3が mikan の手書き、4〜6は代筆。

| 手順 | 関数 | やること |
|---|---|---|
| 1 | `hir::Program::slots` | `effect db: Database` は下ろしの時点で `SlotId` になっている |
| 2〜4 | `scan` | 本体を1回歩いて、直接使用・呼び出し辺・**`provided` による打ち消し**を同時に集める |
| 5 | `analyze` | 変化がなくなるまで回して要求を伝播させる |
| 6 | `unsatisfied` | 残った要求を到達経路付きで報告 |

関数ごとの詳細は `docs/requirement-map.md`。

## 5. 決まっていること(理由は `docs/adr/`)

| | |
|---|---|
| 0001 | v1 はエフェクト1本から始める（所有権の棚上げは [0010](./adr/0010-owned-values-and-inferred-borrows.md) で後続実装） |
| 0002 | `effect` は**スロット宣言**。契約は trait。記述は4箇所だけ |
| 0003 | whole-program 単相化でエフェクト変数を型から消す |
| 0004 | 構造の追加は mikan が決める(申告制)。それ以外は代筆 |
| 0005 | ブロック形から `:` を外し、ambient 提供を `with` で書く |
| 0006 | ファイル/ディレクトリをモジュールとし、読み込みを `use` に一本化 |
| 0007 | 診断の描画に `miette` を1つだけ依存に足す |
| 0008 | ambient の実行時契約は特殊化計画。vtable 無し、型提供は消える |
| 0009 | 成果物は Core Wasm + ABI v0。Component/WIT は下流のアダプタ |
| 0010 | 値は単独所有。借用領域と return provenance は全プログラムから推論 |

文法は `docs/grammar.md`、用語は `CONTEXT.md`、プロジェクトの目的は `README.md`。
**まだ決まっていない設計は `docs/design-notes/`**（測った結果・却下案・未決の問い）。
インタプリタを参照実装として残し、型付き HIR から Wasm 生成へ進む順序と各段階の完了線は
[`docs/compiler-roadmap.md`](./compiler-roadmap.md)。

## 6. 所有権の契約

型検査のあとに ownership pass が走る。ここを通った `CheckedProgram` だけが要求解析、
インタプリタ、ambient 計画、Wasm build へ渡る。したがって、後段が未検査 HIR を
実行する通常経路はない。

非 Copy 値は一つの owner を持つ。`&T` の read-only call は自動借用だが、変更は
`&mut`、既存 local の所有引数・consuming receiver への引き渡しは `move`、独立した
値の作成は `clone()` と書く。借用の最終使用と borrowed return の起点は読み込んだ
プログラム全体で推論し、move 後の使用、共有中の変更、重なる可変借用、owner を越える
borrow は実行前に診断する。

```rhodolite
fn read(user: &User -> int) { user.id }
fn change(user: &mut User) { user.id = 2 }

let mut user = User { id = 1 }
read(user)
change(&mut user)
```

波括弧が常に local scope を作るわけではない。Rhodolite の second-class block は外側の
local scope を再利用する。match arm、loop variable、`with` の body、callable/test body
は隔離される。ownership の scope と drop もこの既存の境界に従うので、借用を短くする
ためだけの裸 block は意味を変えない。

owned local は scope exit で逆宣言順に drop され、move 済みの source は drop しない。
`return`・分岐・loop exit も cleanup を通る。runtime failure は language-level
unwinding をしない。`indirect` は有限な再帰所有 edge を明示する構文で、shared ownership、
aggregate に格納した borrow、GC、runtime borrow check、raw pointer、`unsafe` はまだない。
理由と境界は [ADR-0010](./adr/0010-owned-values-and-inferred-borrows.md)。

現在の interpreter はこの契約を実行する参照実装である。一方、Core Wasm v0 は到達した
scalar (`unit` / `bool` / `int`) だけを生成し、non-scalar data、borrow、public borrowed
signature は source span 付きで build 前に拒否する。owned data の layout は次段まで
意図的に未実装である。

## 7. 検査がいま保証すること

v1 完了線は越えた。未定義の直接関数呼び出し、重複スロットの検査、
ADR-0006 のモジュール分割、struct の形の検査、enum の宣言、
関数の署名の検査、基本式の型検査、配列の型検査、メソッドと関連関数の
呼び出しの型検査、fieldless enum の限定参照と `match`、データを持つ variant の
構築と pattern 束縛、`with` の提供の契約検査、実行前の診断への span の付与、
そして**型検査の全域化**は完了した。

`src/typecheck.rs` を通ったプログラムでは、**値を産む式はすべて具体的な型を持ち、
すべての呼び出しは一意の宣言へ解決されている**。「型が分からないので実行時へ委ねる」
経路は残っていない。分類できない式が1つでもあれば、その式を指して実行前に落ちる
(呼ばれない宣言の中でも同じ)。これは型付き HIR と Wasm バックエンドへ検査結果を
そのまま引き渡すための契約で、`docs/compiler-roadmap.md` の次の一歩の前提になる。

形の保証はそのまま残る。struct リテラルは宣言済み struct を指し、宣言フィールドを
過不足なく一度ずつ持つ。ローカルに隠されていない裸の struct 名はフィールド0個。
フィールドの読みは宣言済みフィールドを指し、算術と単項 `-` は `int` を取り、`==` は
両辺が同じ型で、条件・`assert`・`match` の guard は `bool`。struct リテラルの
フィールド値・フィールドへの代入・ローカルへの再代入・引数・明示 `return`・
最後の式は、宛先の型と適合する。

値を産まない式は型ではなく**制御の脱出**として言い分ける。`return`、全枝が
`return` する条件式、全 arm が抜ける `match`、空 enum を 0 arm で網羅した `match`
はこれに当たり、どんな期待型の位置にも収まる。`let`・代入・`assert`・ループ・
空ブロック・`else` の無い条件式は `unit` を産む。`with` と第二級ブロックは
本体の最後の式の値をそのまま産む。

組み込みのスカラー型は `int` / `bool` / `str` / `unit` の4つで、これらは
予約されていてユーザー宣言が名乗れない。関数・trait メンバー・実装メソッドは
必ず1つの実効戻り値型を持ち、注釈があればそれ、無ければ `unit`。本体からは
推論しない(再帰と相互再帰に制約解決が要るため)ので、注釈を省略した宣言が
`unit` 以外の値で終わるのはエラー。

`nil` は期待される `T?` の文脈だけで適合し、`T? ?? T` は `T` になる。`??` の右辺が
制御を抜けるなら、その枝は値を産まずに終了できる。裸の `let x = nil` は nominal 型を
決めず、後の代入から遡って推論もしないので、**`let x: T? = nil` と注釈する**。
局所束縛の型注釈は、初期化子だけでは型が決まらない `nil` と空配列へ期待型を与える
唯一の口で、引数・フィールド・戻り値と同じ型文法とモジュール解決規則を使う。
注釈があれば束縛の型はそれで固定され、後の参照と再代入もその型で照合される。
`nil == nil` は両辺のどちらからも nominal 型を決められないのでエラー。
`S?.?field` は receiver が `nil` なら `nil`、
値があれば field を読み、宣言型 `T` / `T?` のどちらからも結果 `T?` を作る。
期待型が決まる引数・戻り値・field・代入では、同名の `T` を `T?` へ一方向に注入できる。
これは式の推論型を変えず、逆向きの `T? -> T` や `T == T?` は許さない。

型注釈の `[T]` は要素型 `T` の配列で、後置 `?` は直前の完成した型に付くので `[T]?` と
`[T?]` は別物。期待型が `[T]` の位置では配列リテラルの各要素を `T` と照合し、空の `[]`
はそこに収まる。期待型が無ければ、型の分かる全要素が同じ `T` のときだけ `[T]` と推論し、
食い違えば診断する。空配列や、型の分かる要素が一つも無い配列は独立には型を持たないので、
後の代入から遡らず、その場で要素型を要求する(`let xs: [T] = []`)。配列**値**の適合は
要素型について不変(`[T]` は `[T?]` へ渡せない)で、外側の `[T] -> [T]?` だけが他の型と
同じく注入できる。`for x in xs` は `xs` が非 optional な配列であることを要求し、
`x` を要素型に束縛する。反復対象の型が決まらなければ実行前に落ちる。

型注釈の `fn(P1, P2 -> R)` は名前付きトップレベル関数の値型。トップレベル関数の
名前を値位置に書くとその callable 値になり、宣言署名が期待する callable 型と
**完全一致**したときだけ適合する(引数の個数・型・所有モード・結果型のどれが
違っても落ちる)。callable 値は Copy で、局所束縛も ambient 提供もレシーバも
捕捉しない。置ける場所はこの版では不変 local の初期化子と関数呼び出しの引数だけで、
可変 local・フィールド・variant payload・配列要素・戻り値に置くと型検査が断る。
メソッド・関連関数・スロットの名前は値にならない。callable 型のローカルや引数は
通常の呼び出し構文で呼べて、その呼び先は呼び出し特殊化ごとに1つの名前付き関数へ
静的に決まる。

trait を実装する `impl` は宣言の時点で契約と突き合わされ、メソッドの過不足・
レシーバの形・引数型・戻り値型が一致する(引数名は実装側の局所名なので入らない)。
呼び出しは評価器と同じ順序で解決する。レシーバの具体型が分かるなら inherent と
trait 実装をまとめて名前で絞って一意を要求し、レシーバがスロット名なら
`effect` が指す契約だけを見る(同名のローカルはスロットを隠す)。`.` と `::` は
宣言された `self` の有無と一致しなければならず、解決した呼び出しの実効戻り値型は
そのまま後続の検査へ届く。レシーバの型が決まらない呼び出し、候補の無い呼び出し、
複数残って曖昧な呼び出しは、どれも実行前に落ちる。

payload を持たない variant は、裸の名前に加えて `Rank::Gold` と限定して参照できる。
限定参照は宣言を直接指すのでローカル束縛に隠されず、同名 variant を持つ enum が
複数あっても曖昧にならない。`match` は既知の非 optional な enum の値を variant
ごとに分岐し、選ばれた arm の値を式全体の値にする。arm は対象 enum の宣言 variant
を過不足なく一度ずつ持たなければならず、欠落・重複・別 enum の variant は実行前に
落ちる。ただし catch-all pattern `_` を最後の arm に一度だけ書けば、限定 arm が
拾わなかった variant を全部そこが受けるので網羅的になる。`_` の重複と後続 arm は
実行前に落ちる。実行時は一致する限定 arm が先で、無ければ `_` へ落ちる。arm の
結果型は期待型、無ければ最初に型の分かる arm を基準にして照合し、その型が後続の
検査へ流れる。抜ける arm は基準にならず、全 arm が抜ければ `match` 自体が抜ける。
値として使われる `match` の結果型が決まらなければ実行前に落ちる。
要求推論はどの arm も実行されうるものとして全 arm の要求を合流する。

限定 pattern には `if 条件` の guard を続けられる。guard は arm が選ばれるかだけを
決め、payload の束縛を本体と同じに見る。guard は実行前に `bool` であることを
要求され、型が決まらなければそこで落ちる。`_` に guard は
付けられない。guard は偽になりうるので、guard 付きの arm はその variant を網羅した
ことにならず、偽のときの行き先として最後の `_` が要る。重複の禁止は guard の有無に
関わらず効くので、同じ variant を条件違いで並べることはできない。実行時は限定 arm が
一致したら payload を束縛してから guard を一度だけ評価し、真のときだけその本体を
走らせる。偽なら本体を走らせずに `_` へ落ちる。要求推論は guard も本体と同じに
保守的へ合流する。

variant は0個以上の型付き positional payload を宣言できる。payload の型は他の型
注釈と同じ nominal 規則で正準化される。payload を持つ variant は
`Lookup::Found(user)` という限定 path の呼び出しでだけ作れ、裸の path は値でも
first-class な constructor でもない。構築は関数呼び出しと同じ規則で、引数の個数が
宣言 payload と一致し、各引数が対応する payload 型と適合することを実行前に要求する
(この照合は関連関数の解決より先に走る)。式の型は宣言した enum になる。

arm は宣言 payload と同じ個数の平坦な pattern 要素を並べる。要素は識別子か `_`
だけで、個数の不一致と同じ pattern 内の重複した名前は実行前に落ちる。識別子は
対応する宣言 payload の型を持ち、その arm 本体の間だけ同名の外側ローカル・宣言・
ambient スロットを隠す。その型は field の読み・呼び出しの引数・代入・戻り値という
既存の検査へそのまま流れ、隣の arm にも match の後にも漏れない。`_` は値を捨て、
名前を導入しないので何も隠さない。arm 全体の pattern としての `_` も同じで、受けた
variant の識別も payload も晒さず、本体からは外側の名前がそのまま見える。
enum 値の等値は enum・variant の同一性と payload
の対応ごとの構造的同値で、payload の比較には既存の値の等値規則が効く。

`with` の提供は、スロットの契約を実装した具体型だけが置ける。`with db<Postgres>`
は型名から、`with db(value)` は値の型から、どちらも実行前に契約と突き合わせる。
提供値の型が決まらなければそこで落ちるので、型の不確かさが実行時へ回ることはない。
提供された実装は下ろしの時点で `TraitImplId` に確定しているので、評価器に
同じ判定は残っていない。
型の同一性は形と後置 `?` の一致だけで、部分型も暗黙の optional 展開も無い。
上記の期待型境界に限り、present な値を optional 宛先へ注入する。

## 8. 診断の形

実行前の段(`lex` / `parse` / `load` / `check` / `analyze`)と評価器
(`eval`)は、すべて `diag::Diag` を返す。

```rust
struct Diag {
    msg: String,          // 文言。span の有無に関わらず必ず出る
    span: Option<Span>,   // 主原因の位置
    label: Option<String>,// span に添える短い語
    help: Option<String>, // 直し方 / 到達経路の1行表現
    related: Vec<Diag>,   // 別の位置(別ファイルもありうる)を指す従属診断
}
```

`Span` は `{ src, start, end }` で、`src` は読み込んだソースの識別子。
`module::LoadedProgram.sources` の添字になっているので、複数モジュールを
1つの `Program` に畳んだ後でも、span 単体から元のファイルとバイト範囲を引ける。
読み込みに失敗したときも、そこまでに読めたソースは診断と一緒に返る。

**span が保証されるもの**:

| 診断 | 指す場所 |
|---|---|
| 字句解析・構文解析 | 読めなかったトークンの位置 |
| `use` が指すモジュールを解決できない | その `use` 宣言 |
| import 名の衝突・メンバー不在 | その `use` 宣言 |
| 宣言名の重複・組み込み型名の宣言 | その宣言 |
| 型検査の式に関する診断 | その式(引数・戻り値・フィールド値は部分式そのもの) |
| ownership の move / borrow conflict | 問題の access。元の move / loan は `related` |
| `match` の重複・別 enum・未知 variant・pattern の個数と重複束縛 | その arm |
| `_` の重複 | 2つ目以降の `_` の arm |
| `_` が最後でない | 先頭の `_` の arm |
| `match` の variant 欠落 | `match` 式全体 |
| arm の guard の型違い | その guard 式 |
| `effect` の重複 | その `effect` 宣言 |
| 未定義の直接呼び出し | その呼び出し |
| 提供忘れ | スロットの使用地点。経路の各ホップは `related` |
| 実行時エラー | 失敗を最初に見た内側の式 |

**span を持たないもの**は、ソースへ届く前に失敗した読み込みだけ:
エントリーが `.rd` でない、親ディレクトリが無い、ファイル名が UTF-8 でない、
ファイルを読めない。これらは素のテキストのまま出る。評価器では、式を1つも
評価する前に失敗する経路(未知のエントリ名を直接呼ぶ)だけが位置を持たない。

実行時エラーは `Flow::Error(Diag)` で運ぶ。span を入れるのは `CheckedInterp::eval` の
1か所だけで、まだ位置を持たない失敗にいま評価中の式の span を入れる。再帰も
呼び出し先の本体もこの境界を通るので、最初に失敗を見た内側の式が埋め、外側
(ブロック・呼び出し元・別モジュール)は上書きしない。`return` は制御フローで
あって診断ではないので、`Flow::Return` のまま関数の境界で受け止まる。

描画は `src/render.rs` の1ファイルに閉じている。`Diag` は miette を知らない
素の構造体で、CLI 境界でだけ `miette::Report` へ変換する。依存を外すか
差し替える判断がこの1ファイルで済む(ADR-0007)。

## 9. 次の一歩

ambient の低水準契約まで決まった。`src/ambient_abi.rs` は、`main` と全 test を根に
到達した本体を**実装の組み合わせごとに単相化する計画**を作る。

| 決めたこと | 形 |
|---|---|
| 隠し ambient 引数 | 値要求だけを欄に持つ不変な record。値要求が無ければ引数そのものが無い |
| 値提供 | 具体 provider への access を1欄。`&` / `&mut` / `move` / temporary は ownership pass が検査 |
| 型提供 | instance の鍵と呼び先を変えるだけ。実行時には残らない |
| スロット呼び出し | 提供の `TraitImplId` から実装本体への直接呼び出し。vtable は無い |
| 内側の `with` | 外側の record を書き換えず、写した文脈を置き換える |
| 再帰・相互再帰 | 鍵を歩く前に確保するので同じ instance を共有して閉じる |
| callback | instance の鍵は本体・選ばれた callback の束縛・provider の3つ組。同じ helper でも callback が違えば別 instance |

正典プログラム全体がこの計画へ落ちることは決定的なスナップショットで固定してある。
判断の理由は [ADR-0008](./adr/0008-ambient-abi-is-a-specialization-plan.md)。

この計画を入力にした Core Wasm 生成 (`emit-core-wasm-programs`) は動いている。
`rhodolite build <entry.rd> --target wasm` が、`main` と `pub use` で明示選択した
公開関数を根に、到達した instance ごとに1つの Wasm 関数を出す。呼び先は計画が持つ
`InstanceId` を関数番号へ引き直すだけで、名前解決をやり直さない。ホスト面の
取り決めは [ADR-0009](./adr/0009-core-wasm-is-the-compiler-artifact.md)。

次は、owned data の Wasm 表現(`compile-wasm-owned-data-values`)。順序と完了線は
[`compiler-roadmap.md`](./compiler-roadmap.md)。
