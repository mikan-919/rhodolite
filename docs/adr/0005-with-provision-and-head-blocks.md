---
status: accepted
date: 2026-07-26
---

# ブロック形の Head からコロンを外し、ambient 提供を `with` で書く

`if condition: { ... }` は `:` と `{` が本体の開始を二重に示している。Head の表面構文を
一つに揃えることより、頻出するブロック形を短くすることを優先し、ブロックの前では
コロンを外す。同時に、関数呼び出しと同じ形だったambient提供へ `with` を付ける。

このADRは、ADR-0002の提供構文と、スロット名がローカル名と衝突した場合の扱いを
置き換える。traitを契約、`effect`をスロット宣言とする判断は変えない。

## Decision

### ブロック形と一行形

ブロック自身が本体の境界を示すため、ブロック形では `:` を書かない。一行形では
Headと本体を分けるために `:` を残す。

```rhodolite
if ready {
    start()
}

if ready: start()

for user in users {
    promote(user)
}

while running {
    tick()
}
```

`elif`と`else`も同じ規則に従う。

```rhodolite
if primary {
    one()
}
elif fallback {
    two()
}
else {
    three()
}
```

### ambient 提供

ambient提供は `with` で始める。`with` があるため、通常の関数呼び出しや将来の
trailing closureと構文上衝突しない。

```rhodolite
with db(store), clock(frozen) {
    handle(id)
}

with db(store): handle(id)
```

提供値は、提供されたスロットが見えるようになる**前**の環境で評価する。

```rhodolite
let db = Postgres::new(url)

with db(db) {
    db.save(user)
}
```

`with db(db)` の左の `db` は提供先のスロット、右の `db` は外側のローカル値。
本体へ入った時点では提供されたスロットが最も内側の束縛になる。本体内でさらに
`let db = ...` と書けば、そのローカルがスロットを隠す。

### 型の提供と実体の提供

スロットは名前付きの実装選択である。実装型だけを提供する段階と、実体まで提供する
段階を分ける。

```rhodolite
with db<Postgres> {
    let db = db::new(url)

    with db(db) {
        handle()
    }
}
```

| 提供 | 意味 | 使用できる射影 |
|---|---|---|
| `with db<Postgres>` | `Database`の実装型として`Postgres`を選ぶ | `db::new()` |
| `with db(value)` | 値の具体型を選び、その実体も置く | `db::new()`と`db.save()` |

`<>`は型、`()`は値という既存の読み方に合わせる。実体を直接提供する形では具体型を
値から推論するため、`db<Postgres>(value)`とは書かない。

実体を常に遅延生成する方式は採らない。初回に一度か使用ごとか、初期化引数をどこから
得るか、失敗をいつ報告するか、状態を共有するか、というライフサイクルまで暗黙に
決めてしまうため。初期化は通常のコードとして明示し、必要なスコープ内で行う。

### 名前解決

スロット使用にはシジルを付けず、通常のメソッド呼び出しとして書く。

```rhodolite
db.save(user)
```

スロットとローカルは同じ値名前空間に置き、常に最も内側の束縛を選ぶ。構文位置や
メソッド名によって解決先を変えない。

```rhodolite
effect db: Database

fn inspect(db: LocalDb) {
    db.save(user)        // ローカル。slot要求ではない

    with db(db) {
        db.save(user)    // withが提供したslot
    }

    db.save(user)        // 再びローカル
}
```

同じ関数でローカル値とスロットの両方を同時に使うなら、ローカルを別名にする。
slotを隠すローカル束縛への警告は将来追加してよいが、名前解決規則には含めない。

### structリテラルとの曖昧性

`if condition { ... }` では、条件末尾の `{` がstructリテラルか本体か判別できない場合が
ある。Headの条件では裸のstructリテラルを読まず、必要なら括弧で囲む。

```rhodolite
if user == User { id = 1 } { ... }    // 書けない
if user == (User { id = 1 }) { ... }  // 書ける
```

頻度の低い条件内structリテラルへ括弧を課すことで、すべての `if` にコロンを課すことを
避ける。

## Implementation Structure

ADR-0004に従い、この判断を実装するために次の構造を追加する。

- `Tok::With` — `with`を予約語として字句解析する
- `Provision` — 型提供と実体提供をAST上で区別する
  - `Type { slot, type_name }`
  - `Value { slot, value }`
- `AmbientBinding` — 実行時に型だけの束縛と実体を持つ束縛を区別する
- `SlotLevel` — 要求と提供の強さを `Type` / `Value` の2段階で表す
- `Requirement` — 要求した `SlotLevel` と到達経路を一緒に持つ
- `Binding` とスコープ化した `Env` — ローカルとスロットのどちらが最も内側かを
  実行時にも一つの規則で決め、`with` を出たら外側の束縛へ戻す
- 名前解決の不変条件 — 通常は最も内側のローカルが勝ち、`with`の本体では
  そこで提供したスロットが最も内側になる

要求推論は `db::new()` と `db.save()` の両方をスロット要求として扱う。型提供は
型射影の要求だけを満たし、実体提供は型射影と値射影の両方を満たす。要求の結合は
`要求なし < Type < Value` の最大値を取る。これにより、同じ関数が `db::new()` と
`db.save()` の両方を使えば、外へ伝わる要求は `Value` になる。

## Considered Options

- **すべてを `Head ':' expr` に揃える。**意味論は一様だが、頻出するブロック形で
  `:`と`{}`が重複するため不採用。Headという統一はASTと意味論に残す。
- **`db(value) { ... }` と書く。**関数呼び出しやtrailing closureと区別できないため不採用。
- **`with db = value` と書く。**束縛であることは明確だが、型提供との対が弱い。
  `db<Type>` / `db(value)`の方が型と値の違いを直接表すため不採用。
- **`@db.save(user)` と書く。**ambient使用は見えるが、使用地点すべてにシジルを課す。
  経由する関数を無記述にすることが主眼であり、通常の名前解決で曖昧性を消せるため
  採用しない。
- **スロット名をローカル名として禁止する。**`db`や`clock`は頻出するローカル名なので
  名前空間への負担が大きい。通常のシャドーイングで扱う。

## Consequences

- 表面文法ではHeadごとに綴りが異なるが、ASTではすべて「環境を構築し、続く一つを
  評価する」Headのまま扱える。
- `with`を見ればambient環境の変更だと分かり、提供と通常の呼び出しをパーサが
  名前解決なしで区別できる。
- 実装型の選択と初期化を分離できるため、内側で得た設定値を使って実体を作れる。
- 名前解決器と要求推論はローカル束縛を追跡する必要がある。従来の「スロット名なら
  常にslot」という走査では不十分になる。
- 要求推論はスロット名だけでなく型射影か値射影かを伝播する。提供による打ち消しも
  同じ2段階を比較する。
- 既存の `.rd` ソースは提供構文とブロック形Headの書き換えが必要になる。
