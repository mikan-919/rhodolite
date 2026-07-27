## 1. 構文とAST

- [x] 1.1 `enum` キーワード、フィールド無しenum、空enum、複数variantを固定する lexer・parser テストを追加する
- [x] 1.2 `Item::Enum { name, variants, span }` を追加し、データを持たないenum宣言をパースする
- [x] 1.3 重複variantを決定的な診断にする単体テストと検査を追加する

## 2. モジュール名前解決

- [x] 2.1 同一モジュールの裸variant、member import、別名、宣言衝突、字句シャドーイングをモジュールテストにする
- [x] 2.2 enum名とvariant名を既存の宣言名前空間へ登録し、両方を正準名へ解決する
- [x] 2.3 variantの正準名から所属enumの正準名を再構成できるロード後の不変条件をテストする

## 3. 最小enum型検査

- [x] 3.1 enum・variant索引と、既知ローカル型を保持する検査環境の境界を単体テストで固定する
- [x] 3.2 struct生成のenum型フィールドについて、同じenumのvariantを受理し異なるenumを診断する
- [x] 3.3 型が分かるstructレシーバへのフィールド代入について、同じenumのvariantを受理し異なるenumを診断する
- [x] 3.4 未知型・optional・配列・呼び出し結果を今回の保証外として受理する回帰テストを追加する

## 4. enum値の評価

- [x] 4.1 variantの評価、同値・非同値比較、ローカルによるshadowing、不正なフィールドアクセスを評価器テストにする
- [x] 4.2 enum名とvariant名を保持する不変な実行値を追加し、裸variant参照と表示・等値比較を実装する

## 5. 正典と文書

- [x] 5.1 `examples/canonical.rd` の代用structを `enum Rank { Bronze Gold }` へ置き換え、既存の昇格テストを通す
- [x] 5.2 `docs/grammar.md`、`docs/overview.md`、関連するソースの保証範囲とponytailを最小enumに合わせて更新する
- [x] 5.3 ADR-0004で申告した新構造と不変条件について、mikan自身の1〜3行の説明を設計文書またはADRへ記録する

## 6. 統合検証

- [x] 6.1 有効なenumプログラムとenum型不一致をCLI結合テストで検証する
- [x] 6.2 `cargo fmt --check`、`cargo clippy -- -D warnings`、全テスト、正典プログラムを実行する
- [x] 6.3 `bunx @fission-ai/openspec validate add-minimal-enum --strict` を実行する
