## Context

Rhodolite の enum は現在 `Item::Enum { variants: Vec<String> }` と `Value::Enum { enum_name, variant }` だけを持ち、`match` arm は限定 variant と本体だけを表す。loader、type checker、requirement analyzer、evaluator はすでに enum 宣言・限定 variant・全 arm の走査を通るため、この変更はその経路へ payload の型・値・束縛を一段ずつ通す。

言語は nominal 型、positional な関数引数、第二級の arm 本体をすでに持つ。新しい一般 pattern 言語や型 join を作らず、既存の型適合・字句スコープ・網羅性を再利用する。ADR-0004 に従い、この変更で追加する構造と不変条件を実装前にここへ申告する。

## Goals / Non-Goals

**Goals:**

- variant ごとに0個以上の型付き payload を宣言する。
- payload variant を限定 path と呼び出し構文で構築し、個数・型を静的に検査する。
- match arm で payload を名前または `_` に分解し、名前を arm 本体だけへ型付きで導入する。
- fieldless enum、variant 単位の網羅性、値ベースの match、ambient 要求の合流を維持する。
- enum 値の比較と表示に payload を含める。

**Non-Goals:**

- guard、variant 全体の catch-all、入れ子・OR・literal pattern。
- 名前付き payload、payload field access、enum メソッド、first-class な variant constructor。
- payload 型からの enum 型推論、一般的な型 join、所有権規則の追加。
- 裸の payload variant による構築。

## Decisions

### 1. 宣言と構築は positional な既存記法を組み合わせる

```rhodolite
enum Lookup {
    Found(User)
    Missing(str)
    Skipped
}

let result = Lookup::Found(user)
```

variant 宣言は `Name(Type, Type, ...)`、構築は `Enum::Name(expr, expr, ...)` とする。0 payload の variant は従来どおり括弧なしで宣言・参照する。関数呼び出しと同じ positional 規則を使うため、新しい field 記法や名前対応規則を作らずに済む。

payload variant の裸の path は値でも first-class constructor でもない。型検査は「payload が必要」と診断し、構築には限定 path の call を要求する。裸の constructor 呼び出しまで許す案は、variant を通常関数と同じ呼び出し表へ入れ、module loader と要求推論の direct-call 判定を広げるため、この段では採らない。

### 2. 宣言・pattern・実行時値へ payload を明示する

追加・変更する構造は次の三つ。

- enum 宣言の `variants` を `Vec<EnumVariant>` にし、`EnumVariant { name, payload: Vec<Type> }` を追加する。
- `MatchArm` に `bindings: Vec<PatternBinding>` を足し、`PatternBinding` は `Bind(String)` と `Discard` の二形だけを持つ。
- `Value::Enum` に `payload: Vec<Value>` を足す。

不変条件は、静的検査を通ったプログラムでは宣言 payload 型、constructor 引数、arm binding、実行時 payload の長さがすべて等しいこと。fieldless variant は三者とも長さ0として同じ表現に載せ、別の値 variant を増やさない。

`EnumVariant` を fieldless / payload の別 enum に分ける案は、どの層でも結局0要素と1要素以上を同じ処理で走査するため採らない。payload を struct に包む案も、匿名 nominal 型と field access という別機能を暗黙に増やすため採らない。

### 3. arm pattern は variant と同じ arity の平坦な binding 列に限る

```rhodolite
let label = match result {
    Lookup::Found(user): user.name
    Lookup::Missing(reason) {
        audit(reason)
        reason
    }
    Lookup::Skipped: "skipped"
}
```

各 pattern 要素は識別子または `_` だけとする。識別子は対応する宣言 payload の型を持ち、外側の同名ローカルを arm 本体の間だけ隠す。同じ pattern 内の重複名、payload arity の不一致、fieldless variant への括弧付き binding は診断する。`_` は値を捨て、local 名として登録しない。

arm ごとに variant が一度だけ現れる既存規則を維持するため、guard は入れない。guard を許すと同じ variant の複数 arm、guard が全て偽の場合の fallback、網羅性への算入規則を同時に決める必要がある。

### 4. constructor は既存の call より先に限定 variant として解決する

type checker と evaluator は `Call(Path([enum, variant]), args)` を見たとき、宣言済み payload variant との一致を最初に調べる。一致すれば constructor として引数を左から一度ずつ検査・評価し、`Value::Enum` を作る。一致しなければ既存の関連関数・ambient 型射影の解決へ進む。

module loader は enum 名と payload 内の型名を既存の正準名へ解決する。pattern の enum 名も従来どおり正準化する。constructor 専用の `ExprKind` を parser で作る案は、parser が宣言表なしで path call の意味を決められないため採らない。

### 5. pattern binding は各解析の既存 local scope を拡張する

type checker は宣言 payload 型から `Binding::Value` を作り、arm ごとの cloned locals へ binding を追加してから本体を検査する。evaluator も一致 arm の cloned / arm-local environment に実 payload を束縛して本体を評価する。requirement analyzer と module traversal は binding 名が arm 内で同名 slot や宣言を隠すことを認識し、隣の arm と match 後へ漏らさない。

要求推論は従来どおり subject と全 arm body を保守的に走査する。payload binding 自体は ambient 要求を作らない。

### 6. enum の同値性は identity と payload の構造的同値を使う

二つの enum 値は enum 名、variant 名、payload 数が一致し、対応する payload が既存の `Value::eq_at` ですべて等しい場合だけ等しい。表示は `Enum.Variant(value, ...)` の形で payload を含める。payload を比較しない案は、`Found(1) == Found(2)` を真にして値の違いを失うため採らない。

## Risks / Trade-offs

- [Risk] `Call(Path(...))` が variant constructor と関連関数の二つの候補を持つ。 → 宣言済み enum membership を先に照合し、同じ優先順を type checker と evaluator のテストで固定する。
- [Risk] `Vec` の index 対応は宣言・pattern・runtime 間でずれると誤束縛になる。 → arity を静的に検査し、複数 payload の順序を unit / CLI テストで固定する。
- [Risk] payload に参照型の struct / array が入ると enum の clone も参照を共有する。 → 既存 `Value::Clone` と複合値の共有規則をそのまま適用し、この変更で新しいコピー意味論を作らない。
- [Trade-off] guard と catch-all が無いため、値条件の分岐は arm 本体の `if` で書く。 → variant の構築・分解が往復する最小段を先に完成させ、網羅性モデルを変える pattern 拡張は利用例とともに別 change で決める。

## Migration Plan

1. AST と parser に payload 宣言・binding pattern を追加し、fieldless 構文の互換テストを維持する。
2. loader で payload 型と pattern path を正準化する。
3. type checker に constructor、pattern arity、binding 型と scope の検査を追加する。
4. evaluator、requirement analyzer、表示・比較を接続する。
5. CLI 結合テストと文書を更新し、全 Rust テストと strict OpenSpec validation を通す。

既存ソースの移行は不要。実装途中で戻す場合は payload を使う例と追加構造を除けば、fieldless variant は長さ0の既存意味へ戻せる。

## Open Questions

なし。追加する構造と不変条件は上記のとおりで、guard・catch-all・入れ子 pattern は別 change とする。
