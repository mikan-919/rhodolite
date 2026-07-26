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
    clock(system_clock): {     // ← 提供
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
      └─ analyze  構文木 → 要求と経路        src/requirement.rs
                                            ↑ いまここまで
      ✗ 評価（インタプリタ）                 まだ無い
```

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

## 4. ファイルの地図

| ファイル | 役割 | 行 |
|---|---|---|
| `src/lex.rs` | 字句解析 + 行継続 | 390 |
| `src/ast.rs` | 構文木の型定義。ここを読めば言語の形が分かる | 142 |
| `src/parse.rs` | 再帰下降パーサ | 812 |
| `src/requirement.rs` | **要求推論(中核)** | 710 |
| `src/main.rs` | 繋ぐだけ | 80 |

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
| 0004 | 中核は手書き(手順4〜6は写経に変更) |

文法は `docs/grammar.md`、用語は `CONTEXT.md`、プロジェクトの目的は `README.md`。

## 6. 次の一歩

**インタプリタ。** 構文木を評価して `test` ブロックの `assert` を実行する。
そこまで行くと「差し替えが動く」が絵ではなく実物になり、v1 完了線に到達する。

いま無いのは評価だけ。推論も検査もエラー表示も、もう動いている。
