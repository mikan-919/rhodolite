## Why

型付き HIR は slot 使用と `with` の具体的な `TraitImplId` を保持できるようになったが、
要求推論の結果をコンパイル可能な関数引数と直接呼び出しへ変換する契約はまだない。
C 生成へ進む前に、ambient を whole-program 単相化で消す境界を実行可能な内部表現として
固定する。

## What Changes

- 要求解析の意味情報を、表示名ではなく `BodyId` / `SlotId` で後段から利用できるようにする。
- `main` と各 test を根に、到達した callable を ambient 実装の組み合わせごとに
  需要駆動で単相化する plan を構築する。
- slot trait 呼び出しを、選択された `TraitImplId` が持つ具体的な `CallableId` への
  直接呼び出しとして計画する。
- 型提供を特殊化キーだけに残して実行時表現から消し、値提供だけを具体型の安定 handle として
  最小 ambient record に載せる。
- ambient record を不変な値とし、`with` を外側で提供式を評価してから record を
  射影・置換する操作として表す。
- 再帰・相互再帰のインスタンスを有限に収束させ、plan と record layout の決定的な
  テキスト表現を追加する。
- C の擬似表現と async を閉じない lifetime 不変条件を ADR に記録する。
- ソース言語、型検査、要求診断、HIR インタプリタ、CLI の観測可能な振る舞いは変更しない。

## Capabilities

### New Capabilities

なし。このchangeは既存言語の内部コンパイル表現を追加する。

### Modified Capabilities

なし。既存の言語要件と利用者向け動作は変更しない。

## Impact

- 要求解析に ID ベースの後段向け結果を追加する。
- 新しい内部モジュールで specialization instance、provider context、ambient record
  layout、解決済み call edge を表現・生成する。
- `src/hir.rs` の既存 ID と provision 情報を再利用し、AST や文字列名へ戻らない。
- 正典、型提供、値提供、nested `with`、複数 provider、再帰、相互再帰、重複排除を
  plan snapshot で検証する。
- C emitter、実データ layout、async、動的 provider、vtable、外部 ABI、CLI オプション、
  外部依存は追加しない。
