## Context

Rhodolite の enum は `Item::Enum { name, variants }` と `Value::Enum { enum_name, variant }` だけを持つ fieldless な有限集合である。variant は宣言モジュールの通常の値名前空間へ裸の名前で公開され、型検査は variant から所属 enum を引ける。一方、`ExprKind::Path` は現在 `Type::associated_fn` と ambient の型射影にだけ使われ、値にはならない。

言語は値ベースで、ブロックは第二級だが最後の式の値を産む。したがって `match` も文専用の制御構文ではなく、選ばれた arm の値を返す式として設計する。要求推論は実行時に選ばれうる全 arm を保守的に走査する必要がある。

## Goals / Non-Goals

**Goals:**

- 既存の fieldless enum variant を `Rank::Gold` と限定して値として参照できる。
- enum 値を一度だけ評価し、variant ごとに第二級ブロックまたは単純式を実行できる。
- 既知の enum に対して欠落・重複・別 enum の arm を実行前に診断する。
- arm の結果型を既存の型検査へ接続し、`match` を引数・代入・戻り値などで使える。
- match 内の呼び出しから生じる ambient 要求を既存の要求推論へ伝える。

**Non-Goals:**

- payload を持つ variant、パターン束縛、分解、ワイルドカード、guard。
- optional な enum の暗黙展開、型不明の対象に対する推測、enum メソッド。
- 裸の variant 宣言・参照の廃止。
- match 専用の末尾離脱型や一般的な型 join の導入。

## Decisions

### 1. match arm は限定 variant path と Head 形式の本体を持つ

構文は次の形に限定する。

```rhodolite
let label = match rank {
    Rank::Bronze: "bronze"
    Rank::Gold {
        audit()
        "gold"
    }
}
```

`match` の対象は通常の式、各 arm は `Enum::Variant` と、既存 Head と同じ `: 単純式` または `{ ... }` の本体を持つ。arm の区切りは既存ブロックと同じ改行で、カンマは導入しない。限定 path を必須にすることで arm 単体から所属 enum が分かり、同名 variant がある場合も曖昧にならない。

`case` キーワードと `=>` を足す案は、同じ「条件と続く第二級ブロック」を既存 Head と異なる記号で表すため採らない。裸の arm pattern を許す案は、限定参照を導入する目的である衝突回避を match 内だけ失うため採らない。

### 2. AST には match と arm を明示し、variant 値は既存 Path を使う

`ExprKind::Match { subject, arms }` と `MatchArm { enum_name, variant, body, span }` を追加する。parser は arm path の最後を variant、その手前を enum path として保持し、module loader が enum 名を正準化し、variant がその enum に属することを照合できる形にする。

式位置の `Rank::Gold` は既存 `ExprKind::Path` のまま保つ。loader、type checker、evaluator は path が宣言済み enum とその variant の組なら enum 値として扱い、それ以外は従来どおり関連関数・ambient 型射影の候補として扱う。専用の variant 式を追加する案は、parser が宣言表なしで同じ path を分類できず、既存 Path との二段階変換が必要になるため採らない。

### 3. match は既知の非 optional enum にだけ許し、集合の完全一致を検査する

型検査は subject の型が既知の非 optional な enum であることを要求する。その enum の宣言 variant 集合と arm の集合を比較し、次を診断する。

- subject と異なる enum の arm
- 同じ variant の重複
- 宣言されていない variant
- 一つ以上の variant の欠落

宣言 variant が空なら arm がゼロの match を網羅的とみなす。型不明の対象を runtime に委ねる案は、arm の所属・網羅性・結果型の基準を決められず、この capability の保証を失うため採らない。optional enum は `??` などで明示的に非 optional にしてから match する。

### 4. subject は一度だけ評価し、全 arm の要求は静的に合流する

evaluator は subject を一度評価し、得た `Value::Enum` の enum identity と variant identity に一致する arm の本体だけを評価する。静的検査を通らず evaluator を直接使うテストに備え、非 enum、enum 不一致、一致 arm 不在は runtime error とする。

module、typecheck、requirement の各走査は subject と全 arm body を訪れる。arm は束縛を導入しないため、各 body は外側 locals / provided 集合の clone で検査し、arm 内の束縛を他 arm や後続へ漏らさない。要求推論はどの arm も実行されうるため、全 arm の要求と呼び出し辺を合流する。

### 5. arm の型は既存の期待型と適合規則を再利用する

match に外側から期待型が渡る場合、全 arm body をその期待型で検査する。期待型がない場合は、配列リテラルと同様に最初の推論可能な arm 型を基準に残りの既知 arm を照合し、その型を match の推論型とする。全 arm が型不明、または arm がゼロなら match 自身の型も不明のままにする。

これにより新しい union、never、一般 join を導入せず、既存の nominal 同一性と `T -> T?` 注入だけを使える。早期 `return` を特別な bottom 型として扱うことはこの変更に含めない。

## Risks / Trade-offs

- [最初の推論可能な arm を基準にすると、文脈なしの optional 注入が arm 順に影響されうる] → 配列と同じ既存規則を再利用し、この変更で一般的な型 join を先回りしない。期待型のある引数・代入・戻り値では順序に依存しない。
- [Path が関連関数と variant 値の二つの意味を持つ] → 宣言済み enum membership を先に一意に照合し、call callee では従来の関連関数解決を維持する。衝突と誤用を parser / typecheck / evaluator のテストで固定する。
- [網羅性診断が span を十分に示せない] → 現在の CLI 診断形式で enum・variant 名を決定的に示し、span 付き診断は roadmap の別変更に残す。
- [全 arm の要求を合流すると runtime では通らない arm の要求も表示される] → 静的な呼び出し可能性を表す既存の `if` と同じ保守的意味論として文書化する。

## Migration Plan

1. `match` keyword、AST、parser と module traversal を追加し、限定 variant path を正準名へ解決する。
2. 型検査へ subject、arm 集合、arm 結果の検査と推論を追加する。
3. evaluator と requirement traversal を接続し、単体・CLI 結合テストを追加する。
4. 文法・overview を更新し、全テストと OpenSpec validation を実行する。

既存構文は変更しないためデータ移行はない。実装途中で戻す場合は新構文を使う例とテストを外し、追加した AST arm を除けば既存の裸 variant 動作へ戻せる。

## Open Questions

なし。payload、wildcard、guard、一般的な型 join は利用例が必要になった時点で別 change として決める。
