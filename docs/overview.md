# 全体像

迷ったらここに戻る。詳細は他の文書にあるので、ここには**地図だけ**置く。

## 1. この言語は何か

> **関数呼び出しで切れない束縛を持つ言語。**

変数は関数を跨ぐと消える。消えない変数を1個足したら、DIコンテナが要らなくなった。
それだけ。他の全部はこれを成立させるための細部。

## 2. 見るべきコードは1つ

```rhodolite
effect clock: Clock            // ← 宣言

fn stamp(u: User) {
    u.at = clock.now()         // ← 使用
}
fn promote(id: UserId) {
    stamp(id)                  // ← 何も書いていない
}

fn main() {
    with clock(system_clock) { // ← 提供
        promote(1)
    }
}
```

**`promote` が1文字も書いていないのに、`stamp` の要求が `main` に届く。** これが全部。

`examples/canonical.rd` がこれの完全版で、**v1 の到達目標**。

## 3. いま動くもの

```
  ソース (.rd)
      │
      ├─ lex      文字 → トークン           src/lex.rs
      ├─ join     行継続の改行を消す         src/lex.rs
      ├─ parse    トークン → 構文木          src/parse.rs → src/ast.rs
      ├─ load     use を辿って名前解決        src/module.rs
      ├─ check    struct の形と分かる型を照合  src/typecheck.rs
      ├─ analyze  構文木 → 要求と経路        src/requirement.rs
      └─ eval     構文木を走らせる            src/eval.rs
                                            ↑ 全部つながっている
```

**v1 の到達目標は達成済み。**`cargo run` で `examples/canonical.rd` の
test が緑になる。

`cargo run` で確認できる出力:

```
推論された要求:
  stamp / clock, db
  promote / clock, db      ← 誰も書いていない
  main / (要求なし)
```

```
$ cargo run examples/missing_handler.rd
main: `clock` が提供されていません
  clock が要る ← handle ← promote ← stamp
```

```
テスト:
  ok   昇格すると Gold になり時刻が刻まれる

1 件中 1 件成功
```

## 4. ファイルの地図

| ファイル | 役割 | 行 |
|---|---|---|
| `src/lex.rs` | 字句解析 + 行継続 | 448 |
| `src/ast.rs` | 構文木の型定義。ここを読めば言語の形が分かる | 204 |
| `src/parse.rs` | 再帰下降パーサ | 1218 |
| `src/module.rs` | `use` を辿るモジュール読み込みと名前解決 | 1244 |
| `src/typecheck.rs` | struct の形と、分かる範囲の型の検査 | 1054 |
| `src/requirement.rs` | **要求推論(中核)** | 1097 |
| `src/eval.rs` | **評価。`Env` は切れて `Ambient` は切れない** | 1546 |
| `src/main.rs` | 繋ぐだけ | 147 |

`requirement.rs` の中は6段。手順1〜3が mikan の手書き、4〜6は代筆。

| 手順 | 関数 | やること |
|---|---|---|
| 1 | `collect_slots` | `effect db: Database` を表にする |
| 2〜4 | `scan` | 本体を1回歩いて、直接使用・呼び出し辺・**`provided` による打ち消し**を同時に集める |
| 5 | `analyze` | 変化がなくなるまで回して要求を伝播させる |
| 6 | `unsatisfied` | 残った要求を到達経路付きで報告 |

関数ごとの詳細は `docs/requirement-map.md`。

## 5. 決まっていること(理由は `docs/adr/`)

| | |
|---|---|
| 0001 | v1 はエフェクト1本。所有権と `'a` は棚上げ |
| 0002 | `effect` は**スロット宣言**。契約は trait。記述は4箇所だけ |
| 0003 | whole-program 単相化でエフェクト変数を型から消す |
| 0004 | 構造の追加は mikan が決める(申告制)。それ以外は代筆 |
| 0005 | ブロック形から `:` を外し、ambient 提供を `with` で書く |
| 0006 | ファイル/ディレクトリをモジュールとし、読み込みを `use` に一本化 |

文法は `docs/grammar.md`、用語は `CONTEXT.md`、プロジェクトの目的は `README.md`。
**まだ決まっていない設計は `docs/design-notes/`**（測った結果・却下案・未決の問い）。

## 6. 次の一歩

v1 完了線は越えた。未定義の直接関数呼び出し、重複スロットの検査、
ADR-0006 のモジュール分割、struct の形の検査、データを持たない enum、
関数の署名の検査は完了した。

`src/typecheck.rs` が保証するのは**形**と、**分かる範囲の型**だけ。
ここを通ったプログラムでは、struct リテラルは宣言済み struct を指し、宣言
フィールドを過不足なく一度ずつ持つ。ローカルに隠されていない裸の struct 名は
フィールド0個。非 optional の enum 型フィールドには、**両辺の型が分かる
限り**別の enum の値が入っていない。そしてトップレベル関数の直接呼び出しは
宣言どおりの個数の引数を取り、**型の分かる**引数・明示 `return`・最後の式は
宣言された型と一致する。

型が分かる入口は5つだけ — 型注釈付き引数、`self`、struct リテラル、enum
variant、戻り値型を宣言したトップレベル関数の直接呼び出し、およびそれらを
直接束縛・参照する式。演算・`nil`・`??`・配列の要素・フィールドの読み・
メソッドと関連関数の呼び出しは「分からない」に落ちるので、そこには診断が出ない。
型の同一性は名前と後置 `?` の一致だけで、部分型も optional の自動展開も無い。

残っている穴埋めの優先順は次の通り。

1. **型検査が名前の一致しか見ていない。** 演算、optional の展開、配列の要素型、
   メソッド候補の絞り込み、trait 宣言と impl の突き合わせはまだ見ていない。
   ADR-0003 の whole-program 単相化はこの続き
2. **enum は最小のまま。** データ付き variant、`match`、網羅性検査、
   限定参照(`Rank::Gold`)は無い
3. エラーに span が付いていない(`miette` を入れるならここ)
