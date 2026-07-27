# Rhodolite 文法

`examples/canonical.rd` をパースできるところまでの確定分。
用語は [CONTEXT.md](../CONTEXT.md)、設計判断の理由は [adr/](./adr/)。

## モジュールと `use`

エントリーファイルの親ディレクトリをソースルートとし、`.rd` ファイルと
ディレクトリをモジュール木に対応させる。依存先の読み込みと名前導入は、モジュール
先頭の接頭部に置く `use` が同時に行う。

```ebnf
use_decl   ::= 'use' module_path ('as' ident)?
             | 'use' module_path '::' '{' use_member (',' use_member)* ','? '}'
use_member ::= ident ('as' ident)?
module_path ::= ident ('::' ident)*
```

```rhodolite
use data::database
use services::{
    users,
    billing as payments,
}
```

波括弧なしはモジュール自身、波括弧ありはそのメンバーを導入する。パスは常に
ソースルート基準で、相対パス・glob・外部パッケージはない。詳細な読み込み規則、
名前衝突、循環 `use` は
[ADR-0006](./adr/0006-modules-are-loaded-by-use.md)による。

## 産出は4つ。例外なし

```
expr ::= 単純式
       | '{' expr* '}'
       | Head ':' 単純式          # Head と同一物理行に限る
       | Head '{' expr* '}'
```

**値ベース。**文と式の区別を持たない。すべての形が値を産む。

`Head` は環境を構築して続く1つを実行するもの:

| Head | 導入するもの |
|---|---|
| `if c` / `elif c` / `else` | 分岐(実行回数 0か1) |
| `if let Ok(x) = e` | 束縛 + 分岐 |
| `for x in xs` / `while c` | 束縛 + 反復 |
| `with db(pg)` / `with db(pg), clock(sys)` | **ambient 束縛**(関数呼び出しで切れない) |
| `with db<Postgres>` | ambient な実装型 |
| `ar` (将来) | ambient なアロケータ |

ブロック自身が本体の境界を示すため、ブロック形では `:` を書かない。
一行形だけは `:` でHeadと本体を分ける。

- `elif` はキーワード(else-if 特例を持たないため)
- `;` は言語に存在しない。1行1文
- Head の本体は単純式かブロックのみ、ワンライナーは同一行限定。
  これで dangling else と goto-fail が両方**構文レベルで**死ぬ

## 行継続

> **行末が式を終えられない、または次行の行頭が式を始められないなら、継続。**

特例リストなし。この1本で `.` のメソッド連鎖も `else`/`elif` の改行も、
二項演算子の前置き・後置きも全部カバーする。

```rhodolite
let total = price +        // 行末 `+` が終えられない → 継続
            tax

let total = price
          + tax            // 行頭 `+` が始められない → 継続

let x = if c {
    foo()
}
else: bar()                // 行頭 `else` が始められない → 継続

users.filter(active)
     .map(name)            // 行頭 `.` が始められない → 継続
```

意図と食い違う場合(`a = b` 改行 `-c` など)は曖昧ではなく決定的に切れる。
拾うのは「**値が捨てられている**」の警告 — 値ベースなので全行が値を持ち、
この警告はどのみち必要。文位置の式を呼び出し/代入に制限する案は**採らない**
(ブロックの最後の式が戻り値である以上、制限が掛けられない)。

## 値

- **ブロックの値は最後の式**。それ以外の行の値は捨てる(unit でなければ警告)
- 早期離脱に `return` も使える(`db.find(id) ?? return false`)
- `else` の要否は**型規則**であって文法規則ではない。値として使われた `if` に
  `else` がなければ型が合わない、というだけ

## struct 生成

```rhodolite
Circle { r = 1.0 }
```

`=` は「束縛」で `let x = 1` と同じ意味。`:` は Head 専用に保つ。

Head の条件では裸のstructリテラルを読まない。条件に置く場合だけ括弧で囲む。

```rhodolite
if value == (Circle { r = 1.0 }) {
    same()
}
```

頻度の低い条件内structリテラルへ括弧を課す代わりに、すべてのブロック形Headから
コロンを外した（→ ADR-0005）。

## enum

データを持たない有限個の値。variant に payload も明示値も書けない。

```ebnf
enum_decl ::= 'enum' ident '{' ident* '}'
```

```rhodolite
enum Rank {
    Bronze
    Gold
}
```

variant は宣言モジュールの**普通の宣言**で、`Gold` という裸の名前で参照する。
`Rank::Gold` のような限定参照は無い。名前の解決・衝突・`use` での導入・ローカル
束縛によるシャドーイングは、struct や fn と同じ規則がそのまま効く。

variant は struct ではない。フィールドアクセスもメソッド解決もできず、
等しいのは**同じ enum の同じ variant** のときだけ。

`match`、網羅性検査、データ付き variant、enum のメソッドは無い。

## ambient は4箇所にしか現れない

```rhodolite
trait Database { fn save(self, u: User -> unit) }  // 契約
effect db: Database                            // スロット宣言
with db(Postgres::new(url)) { handle(id) }     // 実体の提供
with db<Postgres> { db::new(url) }              // 実装型の提供
db.save(u)                                     // 使用
```

経由するだけの関数は**無記述**。要求は推論する(→ ADR-0002)。

`with db(value)` の値は提供前の外側で評価する。したがって `with db(db)` の右側は
外側のローカル、ブロック内の `db` は提供されたスロットになる。

## メソッドと関連関数

シグネチャの第一引数が `self` かどうかだけが両者の区別。暗黙にしない。

```rhodolite
impl Database for Postgres {
    fn save(self, u: User -> unit) { ... }   // メソッド。`pg.save(u)` で呼ぶ
    fn new(url: Str -> Postgres) { ... }     // 関連関数。`Postgres::new(url)` で呼ぶ
}
```

`self` を暗黙にすると `new` にもレシーバがあることになり、トップレベルの `fn` と
trait の中の `fn` が同じ見た目で違う意味になる。宣言に出す方を採った。
`self` は ambient と違って**関数呼び出しで切れる**普通の束縛。

## まだ決めていない

- `??` の正確な意味論(`Option` の unwrap-or-else? 型は?)
- パターンマッチ(`match`)を v1 に入れるか — 正典には出てこない。
  enum が入ったので、必要になったときに限定参照(`Rank::Gold`)と一緒に決める
- `elif`/`else` を `}` と同じ行に置くか次行かは**フォーマッタ規約**
  (行継続規則がどちらも受けるため文法の問題ではない)
- 非局所制御フローの細則(break のラベル、ネストした Head からの early return)
