# `src/requirement.rs` の関数地図

シグネチャと呼び出し関係。中身の説明はソースのドキュメントコメントにある。

## 呼び出し関係

```mermaid
flowchart TD
    main["main.rs / eval.rs"]

    subgraph entry["入口"]
        analyze["analyze(&Program) -> Analysis"]
        render["Analysis::render(&self) -> String"]
        unsat["Analysis::unsatisfied(&self) -> Vec&lt;String&gt;"]
    end

    subgraph body["本体の走査"]
        collect["collect_slots(&Program) -> Slots"]
        scan_body["scan_body(&[Expr], &Slots) -> BodyFacts"]
        scan["scan(&Expr, &Slots, &BTreeSet&lt;String&gt;, &mut BodyFacts)"]
        psn["provided_slot_name(&Expr) -> Option&lt;String&gt;"]
        test_key["test_key(&str) -> String"]
    end

    subgraph slots_api["Slots の API"]
        is_slot["Slots::is_slot(&self, &str) -> bool"]
        trait_of["Slots::trait_of(&self, &str) -> Option&lt;&str&gt;"]
        names["Slots::names(&self) -> BTreeSet&lt;&str&gt;"]
    end

    main --> analyze
    main --> render
    main --> unsat
    main --> names
    main --> trait_of

    analyze --> collect
    analyze --> scan_body
    analyze --> test_key

    scan_body --> scan
    scan --> scan
    scan --> psn
    scan --> is_slot
```

`scan` の自己ループが全部。手順2（スロット使用）・手順3（呼び出し辺）・
手順4（提供による打ち消し）を、本体1回の再帰で同時に集めている。

## データの流れ

```mermaid
flowchart LR
    P["Program (AST)"]
    S["Slots<br/>slot -> trait 名"]
    BF["BodyFacts<br/>escaping: BTreeSet&lt;String&gt;<br/>calls: Vec&lt;CallSite&gt;"]
    CS["CallSite<br/>callee: String<br/>provided: BTreeSet&lt;String&gt;"]
    R["Reqs<br/>= BTreeMap&lt;String, Vec&lt;String&gt;&gt;<br/>slot -> 到達経路"]
    A["Analysis<br/>slots / reqs / order"]
    OUT["文字列出力"]

    P -->|collect_slots| S
    P -->|scan_body 各関数1回| BF
    BF -.含む.-> CS
    S -->|参照| BF
    BF -->|analyze の不動点反復| R
    R --> A
    S --> A
    A -->|render / unsatisfied| OUT
```

## 手順との対応

| 手順 | 関数 | 書いた人 |
|---|---|---|
| 1 | `collect_slots` | 手書き |
| 2 | `scan` の `Field` 腕 | 代筆（元は手書きの `direct_uses`） |
| 3 | `scan` の `Call` 腕 | 代筆（元は手書きの `calls`） |
| 4 | `scan` の `Head::Ambient` 腕 | 代筆 |
| 5 | `analyze` の `loop` | 代筆 |
| 6 | `Analysis::unsatisfied` | 代筆 |

手順2・3は最初 `direct_uses` / `calls` として別々に手書きしたが、
手順4で `provided` を持ち回る必要が出た時点で `scan` に畳んだ。
実装が2本あると片方だけ直して食い違うので、実装は1本にしてある。
検査は残っていて、`手順2_*` / `手順3_*` のテストは `scan_body` の
`escaping` と `calls` を見ている。
