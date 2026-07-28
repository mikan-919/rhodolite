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

## 組み込みのスカラー型

型名は4つだけ予約されている。ユーザーの宣言(struct / enum / variant / trait /
effect / fn)はこの名前を名乗れない。

| 型 | 値 |
|---|---|
| `int` | 整数リテラル、算術と単項 `-` の結果 |
| `bool` | `true` / `false`、`==` の結果 |
| `str` | 文字列リテラル |
| `unit` | 値を返さない式の結果 |

新しい構文は無い。綴りは小文字で固定で、別名も昇格も無い。

- `+` `-` `*` `/` は `int × int -> int`。文字列連結ではない
- 単項 `-` は `int -> int`
- `==` は両辺が同じ型のときだけ書け、結果は `bool`
- `if` / `elif` / `while` の条件と `assert` の対象は `bool`。整数や文字列からの
  暗黙の真偽値変換は無い

## optional

型名の後置 `?` は値が `nil` になりうることを表す。`nil` は単独の nominal 型ではなく、
期待型のある位置で `T?` にだけ適合する。したがって `fn f(x: int?) { f(nil) }` は通るが、
`fn f(x: int) { f(nil) }` は型エラーになる。裸の `let x = nil` から `T` は推論しない。

引数・戻り値・struct field・field 代入・型の決まった local 代入では、期待型が `T?`
なら同名の非 optional 値 `T` も渡せる。この注入は一方向で、`T?` を `T` の期待位置には
渡せない。また式の推論型は `T` のままなので、`T == T?` は型エラーになる。

fallback の型規則は1本:

```text
T? ?? T -> T
```

左辺が `nil` のときだけ右辺を評価する。右辺の直接 `return` は値を産まずに枝を終えてよい
ので、`value ?? return fallback` と書ける。`nil == value` は value が optional のときだけ
比較でき、結果は他の `==` と同じ `bool`。

optional な struct の field は `.?` で読む:

```rhodolite
user.?profile.?name
```

receiver が `nil` なら field を読まずに `nil` を返す。値があれば通常の field と同じく
宣言された値を読む。型規則は `S?.?field -> T?` で、field の宣言型が `T?` でも結果は
`T?` のまま(一段に平坦化)。通常の `.` は optional receiver を暗黙に展開しないため、
`user.profile` は `user: User?` なら型エラーになる。

`.?` は読み取り専用。`user.?name = value` と `user.?method()` は書けない。

## 配列

型注釈の `[T]` は要素型 `T` の配列。名前が書ける型位置ならどこでも書け、要素は
入れ子にも optional にもできる。後置 `?` は直前の**完成した**型に付くので、
`[T]?`(optional な配列)と `[T?]`(optional な要素の配列)は別物。

```rhodolite
struct Store {
    users: [User]     // User の配列
    notes: [Rank?]    // optional な要素の配列
    grid: [[int]]     // 配列の配列
}
```

配列リテラルの型付けは期待型の有無で決まる:

- 期待型が `[T]` の位置(引数・戻り値・struct field・field 代入・型の決まった
  local 代入)では、各要素を `T` と既存の適合規則で照合する。`nil` も非 optional
  から optional への注入も、他の期待型のある位置と同じ規則が効く。空の `[]` は
  この経路で任意の期待配列型に収まる
- 期待型が無ければ、型の分かる全要素が同じ `T` のときだけ `[T]` と推論する。
  食い違えば型エラー。空配列や、型の分からない要素・`nil` しか無い配列は型を持たず、
  後の代入から遡って型を得ることもない

既に型の分かっている配列**値**の適合は要素型について不変で、`[T]` は `[T?]` へ
渡せない(配列は参照として共有されるため)。外側の optional への注入
`[T] -> [T]?` だけは他の型と同じく一方向に通る。

`for x in xs` の `xs` は非 optional な配列でなければならない。`x` はその要素型に
束縛され、ループ本体の field の読みや代入は他の型の分かる値と同じに検査される。
`xs` が optional な配列なら先に `??` で展開する。`xs` の型が分からないときは
従来どおり `x` も型を持たない。

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

variant は宣言モジュールの**普通の宣言**で、`Gold` という裸の名前で参照できる。
名前の解決・衝突・`use` での導入・ローカル束縛によるシャドーイングは、struct や
fn と同じ規則がそのまま効く。

`Rank::Gold` と enum 名で限定した参照も同じ値になる。限定参照は宣言を直接
指すので、ローカル束縛には隠されず、同名 variant を持つ enum が複数あっても
曖昧にならない。`Enum` が宣言済み enum でなければ限定参照にはならず、
従来どおり関連関数や ambient の型射影として扱う。

```rhodolite
let a = Gold          // 裸の参照
let b = Rank::Gold    // 限定した参照。a == b
```

variant は struct ではない。フィールドアクセスもメソッド解決もできず、
等しいのは**同じ enum の同じ variant** のときだけ。

### match

enum の値を variant ごとに分岐する。選ばれた arm の値が式全体の値になる。

```ebnf
match_expr ::= 'match' expr '{' match_arm* '}'
match_arm  ::= module_path '::' ident (':' 単純式 | '{' expr* '}')
```

```rhodolite
let label = match rank {
    Rank::Bronze: "bronze"
    Rank::Gold {
        audit()
        "gold"
    }
}
```

arm の本体は Head と同じ2つの形だけで、区切りはブロックと同じ改行(カンマは
無い)。arm の pattern は限定参照に固定してあるので、arm 単体から所属 enum が
決まる。

- 対象は**既知の非 optional な enum**でなければならない。optional なら先に `??`
  で展開する。型が分からない対象は実行時へ回さず、その場で診断する
- arm は対象 enum の宣言 variant を**過不足なく一度ずつ**持つ。欠落・重複・
  別 enum の variant・宣言に無い variant はすべて実行前に落ちる。宣言 variant が
  0個の enum は arm ゼロで網羅的
- 期待型のある位置(引数・戻り値・field・代入)では、各 arm をその型と既存の
  適合規則で照合する。期待型が無ければ最初に型の分かる arm を結果型にして残りを
  照合し、その型が後続の検査へ流れる。全 arm が推論の外なら結果型も分からないまま
- arm は束縛を導入しない。本体は第二級ブロックで、そこで導入した `let` は隣の
  arm にも後続にも漏れない。`return` は既存どおり関数を抜ける
- 対象は一度だけ評価し、一致した arm だけを走らせる。要求推論はどの arm も
  実行されうるものとして**全 arm の要求を合流**する(`if` と同じ保守的な意味論)

データ付き variant、束縛を伴う pattern、ワイルドカード、guard、enum のメソッドは
無い。

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
    fn new(url: str -> Postgres) { ... }     // 関連関数。`Postgres::new(url)` で呼ぶ
}
```

`self` を暗黙にすると `new` にもレシーバがあることになり、トップレベルの `fn` と
trait の中の `fn` が同じ見た目で違う意味になる。宣言に出す方を採った。
`self` は ambient と違って**関数呼び出しで切れる**普通の束縛。

呼び出しは実行前に解決される。`trait` を実装する `impl` は宣言の時点で契約と
突き合わされ、メソッドの過不足・レシーバの形・引数型・戻り値型が合わなければ
そこで診断が出る(引数名は実装側の局所名なので契約に入らない)。呼び出し側では、
レシーバの具体型が分かるなら inherent と trait 実装をまとめて名前で絞り、候補が
複数残れば「どの trait のものか決まらない」として拒否する。レシーバがスロット名
(`db.find(id)` / `db::make()`)なら、実行時の具体型は意図的に変わるので、
`effect db: Database` が指す**契約だけ**を見る。同名のローカルはスロットを隠す。
解決した呼び出しは引数の個数と**型の分かる**引数を宣言と照合し、宣言された
戻り値型をそのまま後続の式へ渡す。型の分からないレシーバの呼び出しは保留する
(→ `src/typecheck.rs`)。

## まだ決めていない

- データ付き variant と、それに伴う pattern 束縛・ワイルドカード・guard —
  fieldless の `match` が入ったので、必要になった時点で1つずつ決める
- `elif`/`else` を `}` と同じ行に置くか次行かは**フォーマッタ規約**
  (行継続規則がどちらも受けるため文法の問題ではない)
- 非局所制御フローの細則(break のラベル、ネストした Head からの early return)
