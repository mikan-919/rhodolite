# Rhodolite

> **関数呼び出しで切れない束縛を持つ言語。**

変数の束縛は関数を跨ぐと切れる。切れない束縛を言語に入れると、DIコンテナが不要になる。

```rhodolite
effect db: Database                       // スロット宣言
effect clock: Clock

fn stamp(u: &mut User) {                  // 使用
    u.promoted_at = clock.now()
}

fn promote(id: int -> bool) {             // 経由するだけ = 無記述
    let mut u = db.find(id) ?? return false
    stamp(&mut u)
    db.save(move u)
    true
}

fn main() {
    with db(Postgres::new(url)), clock(system_clock) {   // 提供
        promote(user_id)
    }
}
```

`promote` は1文字も書いていないのに、`stamp` の要求が `main` まで届く。
`stamp` に新しい要求が増えても、触るのは `stamp` だけ。

用語の定義は [CONTEXT.md](./CONTEXT.md)、設計判断の理由は [docs/adr/](./docs/adr/)。
v1 の到達目標は [examples/canonical.rd](./examples/canonical.rd)。

## このプロジェクトの成功条件

1. **進学ポートフォリオで語れること** — 設計判断の理由を自分の言葉で説明できる
2. **自分でなるべく作ること** — 前作 iris で「設計者としても実装者としても中途半端」
   になった反省への回答

採用数・エコシステム・実用性は評価軸に**入れない**。判断に迷ったら
「一言で語れるか」と「自分の手で終わらせられるか」の2つで採点する。

## v1 の完了線

[examples/canonical.rd](./examples/canonical.rd) に対して、次の3つが動いたら v1 は終わり。

1. **差し替え** — 本番実装と in-memory 実装を、呼ばれる側を変更せずに入れ替えられる
2. **可視化** — 各関数が要求する trait を推論して表示できる
3. **経路エラー** — 提供忘れを到達経路付きで報告できる(`clock ← stamp ← promote ← main`)

3つとも、呼び出しグラフを SCC 縮約して逆位相順に1パス舐める処理の副産物になる。

**機能の底**: 正典プログラム1本が動く分だけ実装する。1つも先回りしない。
足りないと判明した時点で1個だけ足す。

## 開発の規則

**中核は手で書く。**

| | 担当 |
|---|---|
| エフェクト推論 / ハンドラ解決 / 到達経路付きエラー生成 | **mikan が手書き** |
| lexer / parser / CLI / テストの定型 | AI 可 |
| 設計相談 / レビュー / 調査 | AI |

語れる部分と手を動かす部分を一致させるための規則。実装言語は Rust、
まずインタプリタで意味を固めてからバックエンドへ進む。

## 現状

v1 の到達目標は達成済み。lexer、parser、要求推論、インタプリタ、CLIがつながり、
`cargo run`で正典プログラムのテストが完走する。現在の地図と次の作業は
[docs/overview.md](./docs/overview.md)。

処理系の境界は ownership 検査済み HIR になった。パイプラインは
`load AST → check/lower HIR → ownership check → CheckedProgram → analyze / eval / Wasm`。
型検査・所有権検査を通った後は構文木を見ない。HIR は全ての式の具体型と、呼び出し先・
フィールド・variant・局所束縛・提供する実装をプログラム内の ID で持つので、要求解析も
評価器も名前で引き直さない(`src/hir.rs`)。

型検査は全域化した。**値を産む式はすべて具体的な型を持ち、すべての呼び出しは
一意の宣言へ解決される**。分類できない式が1つでもあれば、呼ばれない宣言の中でも
実行前に落ちる。組み込みのスカラー型は `int` / `bool` / `str` / `unit` の4つで
予約されており、リテラル・フィールドの読み・演算子・条件・代入は実行前に照合される。
戻り値型の注釈を省略した宣言は `unit` を返す宣言と同じ意味で、本体からは推論しない。
`nil` は `T?` の文脈だけで使え、`T? ?? T` は `T` になる。単独では型が決まらない
`nil` と空配列には `let x: T? = nil` / `let xs: [T] = []` と局所注釈を書く。
`value.?field` は optional な struct から field を安全に読み、結果を optional にする。
引数・戻り値・field・代入・局所注釈の期待型が `T?` なら同名の `T` を渡せるが、
式自体は `T` のままで、`T` と `T?` の等価比較はできない。
enum の variant は `Rank::Gold` と限定して参照でき、`match` で variant ごとに
分岐できる。arm は宣言 variant を過不足なく一度ずつ持つことを実行前に要求され、
選ばれた arm の値がそのまま式の値になる。variant は型付きの positional payload を
宣言でき、`Lookup::Found(user)` で構築して arm の `Lookup::Found(found)` で
取り出す。個数と型は実行前に検査され、束縛はその arm の中だけで見える。
`_` で個々の payload を捨てられる。arm 全体の pattern としての `_` は最後に一度だけ
書けて、限定 arm が拾わなかった variant を全部受ける(名前は導入しない)。
限定 pattern には `if 条件` の guard を続けられる。guard は payload を見られる
`bool` 式(実行前に確定する)で、偽なら本体を走らせずに `_` へ落ちるので、その
variant を網羅したことにはならない。入れ子 pattern と、同じ variant を条件違いで
並べる arm は無い。
メソッドと関連関数の呼び出しも静的に解決される。trait `impl` は宣言時に契約と
突き合わされ、呼び出しはレシーバの具体型かスロットの trait から一意の宣言を選び、
引数と戻り値がそのまま後続の検査へ流れる。`with` の提供値がスロットの trait を
実装しているかも実行前に確定する。

ambient の低水準契約も決まった。到達した本体を**実装の組み合わせごとに単相化する
計画**が処理系の中にあり、スロット呼び出しは実装本体への直接呼び出し、値提供だけが
不変な record の欄になる。型提供は実行時から消え、値要求が無い関数は隠し引数を持たない。
正典プログラム全体がこの計画へ機械的に落ちることをスナップショットで固定している
(`src/ambient_abi.rs`、[ADR-0008](./docs/adr/0008-ambient-abi-is-a-specialization-plan.md))。
この計画を入力にした **Core WebAssembly 生成**が動くようになった。
`rhodolite build app.rd --target wasm` は、`main` と `pub use` で明示選択した
公開関数を根に、到達した instance だけを決定的な `.wasm` へ落とす。成果物は
import も start section も持たず、`__rhodolite_main` と公開名を export し、
インタフェース記述を `rhodolite.abi` custom section に埋め込む。`str`、owned struct、
payload enum、optional、配列と、それらの field 操作・`clone()`・`??`・`match`・`for` は、
組み込み allocator と ownership の drop plan を使って線形メモリへ生成される。scalar
だけの公開面は既存の ABI v0 のまま、owned data が1つでも公開署名にあればモジュール
全体が ABI v1 を選び、正準 bytes を export memory 経由で受け渡す。内部 layout と
公開 wire format は分離している
([ADR-0009](./docs/adr/0009-core-wasm-is-the-compiler-artifact.md)、
[ADR-0011](./docs/adr/0011-owned-data-layout-and-abi-v1.md))。インタプリタは参照実装として残る。

所有権検査も型付き HIR の次の境界として動いている。非 Copy 値は単独所有、`&T` の
読み取り呼び出しだけは自動借用、変更は `&mut`、既存 local の移譲は `move`、複製は
`clone()` と明示する。寿命名は書かず、全モジュールグラフの使用から借用領域と
borrowed return の出どころを推論する。

```rhodolite
fn stamp(user: &mut User) { user.promoted_at = 1000 }
fn persist(user: User) { let ignored = user }

let mut user = User { id = 1, rank = Bronze, promoted_at = 0 }
stamp(&mut user)
let backup = user.clone()
persist(move user)
```

インタプリタは所有 compound value を store で実行し、Wasm backend は同じ検査済み
アクセスモードと drop plan を消費する。公開署名の borrow と aggregate に格納する borrow
は引き続き拒否する。到達した inherent method、trait implementation、slot、`with`、値・型
ambient record は specialization plan の直接 call と hidden handle へ下ろされ、提供の所有権と
cleanup も保持する。`src/differential.rs` の維持された fixture 群が、同じ checked HIR を
インタプリタと独立した Wasm engine に通し、結果、実行時失敗、公開 probe、宣言 test、生成 bytes
を継続照合する。fixture を増やすときは、そこで名前付き source tree と期待する失敗分類を登録し、
代表的な artifact 形の変更は意図して snapshot を更新する。構文と診断の詳細は [docs/grammar.md](./docs/grammar.md)、設計判断は
[ADR-0010](./docs/adr/0010-owned-values-and-inferred-borrows.md) と
[ADR-0011](./docs/adr/0011-owned-data-layout-and-abi-v1.md)、順序は
[docs/compiler-roadmap.md](./docs/compiler-roadmap.md)。
