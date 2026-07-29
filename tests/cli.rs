use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT_PROJECT: AtomicUsize = AtomicUsize::new(0);

struct Project {
    root: PathBuf,
}

impl Project {
    fn new() -> Self {
        let number = NEXT_PROJECT.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "rhodolite-module-test-{}-{number}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        Self { root }
    }

    fn write(&self, relative: impl AsRef<Path>, source: &str) {
        let path = self.root.join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, source).unwrap();
    }

    fn run(&self, entry: &str) -> Output {
        Command::new(env!("CARGO_BIN_EXE_rhodolite"))
            .arg(self.root.join(entry))
            .output()
            .unwrap()
    }

    fn run_relative(&self, entry: &str) -> Output {
        Command::new(env!("CARGO_BIN_EXE_rhodolite"))
            .current_dir(&self.root)
            .arg(entry)
            .output()
            .unwrap()
    }
}

impl Drop for Project {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.root).unwrap();
    }
}

fn output_text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[test]
fn 同名スロットを持つ二つのモジュールを別々に提供して実行できる() {
    let project = Project::new();
    project.write(
        "main.rd",
        "use sides::{left as primary, right as replica}\n\
         \n\
         fn main() {\n\
           with primary::db(primary::Store {}), replica::db(replica::Store {}) {\n\
             primary::read() + replica::read()\n\
           }\n\
         }\n",
    );
    project.write(
        "sides/left.rd",
        "trait StoreApi {\n\
           fn read(self -> int)\n\
         }\n\
         struct Store {}\n\
         impl StoreApi for Store {\n\
           fn read(self -> int) { 1 }\n\
         }\n\
         effect db: StoreApi\n\
         fn read() { db.read() }\n",
    );
    project.write(
        "sides/right.rd",
        "trait StoreApi {\n\
           fn read(self -> int)\n\
         }\n\
         struct Store {}\n\
         impl StoreApi for Store {\n\
           fn read(self -> int) { 2 }\n\
         }\n\
         effect db: StoreApi\n\
         fn read() { db.read() }\n",
    );

    let output = project.run("main.rd");
    let text = output_text(&output);
    assert!(output.status.success(), "{text}");
    assert!(text.contains("main -> 3"), "{text}");
    assert!(text.contains("sides::left::db"), "{text}");
    assert!(text.contains("sides::right::db"), "{text}");
}

#[test]
fn import名とローカル宣言の衝突を報告する() {
    let project = Project::new();
    project.write(
        "main.rd",
        "use dep::{run}\n\
         fn run() { 1 }\n\
         fn main() { run() }\n",
    );
    project.write("dep.rd", "fn run() { 2 }\n");

    let output = project.run("main.rd");
    let text = output_text(&output);
    assert!(!output.status.success(), "{text}");
    assert!(
        text.contains("import名 `run` がローカル宣言と衝突"),
        "{text}"
    );
}

#[test]
fn useしていないモジュール参照を報告する() {
    let project = Project::new();
    project.write("main.rd", "fn main() { services::users::run() }\n");
    project.write("services/users.rd", "fn run() { 1 }\n");

    let output = project.run("main.rd");
    let text = output_text(&output);
    assert!(!output.status.success(), "{text}");
    assert!(
        text.contains("モジュール名 `services` は `use` されていません"),
        "{text}"
    );
}

#[test]
fn ディレクトリuseは参照した子モジュールだけを読み込む() {
    let project = Project::new();
    project.write(
        "main.rd",
        "use services\n\
         fn main() { services::users::run() }\n",
    );
    project.write("services/users.rd", "fn run() { 7 }\n");
    project.write("services/billing.rd", "これは読まれてはいけない\n");

    let output = project.run("main.rd");
    let text = output_text(&output);
    assert!(output.status.success(), "{text}");
    assert!(text.contains("main -> 7"), "{text}");
    assert!(!text.contains("billing"), "{text}");
}

#[test]
fn 存在しない選択メンバーを報告する() {
    let project = Project::new();
    project.write(
        "main.rd",
        "use services::{missing}\n\
         fn main() { 0 }\n",
    );
    std::fs::create_dir_all(project.root.join("services")).unwrap();

    let output = project.run("main.rd");
    let text = output_text(&output);
    assert!(!output.status.success(), "{text}");
    assert!(
        text.contains("モジュール `services` にメンバー `missing` がありません"),
        "{text}"
    );
}

#[test]
fn 循環useを解決できる() {
    let project = Project::new();
    project.write(
        "main.rd",
        "use a::{through_b}\n\
         fn main() { through_b() }\n",
    );
    project.write(
        "a.rd",
        "use b::{value as b_value}\n\
         fn value() { 1 }\n\
         fn through_b() { b_value() }\n",
    );
    project.write(
        "b.rd",
        "use a::{value as a_value}\n\
         fn value() { a_value() + 1 }\n",
    );

    let output = project.run("main.rd");
    let text = output_text(&output);
    assert!(output.status.success(), "{text}");
    assert!(text.contains("main -> 2"), "{text}");
}

#[test]
fn 存在しないモジュールを報告する() {
    let project = Project::new();
    project.write(
        "main.rd",
        "use missing\n\
         fn main() { 0 }\n",
    );

    let output = project.run("main.rd");
    let text = output_text(&output);
    assert!(!output.status.success(), "{text}");
    assert!(
        text.contains("モジュール `missing` が見つかりません"),
        "{text}"
    );
}

#[test]
fn リーフとディレクトリの衝突を報告する() {
    let project = Project::new();
    project.write(
        "main.rd",
        "use conflict\n\
         fn main() { 0 }\n",
    );
    project.write("conflict.rd", "fn value() { 1 }\n");
    project.write("conflict/child.rd", "fn value() { 2 }\n");

    let output = project.run("main.rd");
    let text = output_text(&output);
    assert!(!output.status.success(), "{text}");
    assert!(text.contains("モジュール `conflict`"), "{text}");
    assert!(text.contains("両方があります"), "{text}");
}

#[test]
fn import同士の衝突を報告する() {
    let project = Project::new();
    project.write(
        "main.rd",
        "use left::{run}\n\
         use right::{run}\n\
         fn main() { run() }\n",
    );
    project.write("left.rd", "fn run() { 1 }\n");
    project.write("right.rd", "fn run() { 2 }\n");

    let output = project.run("main.rd");
    let text = output_text(&output);
    assert!(!output.status.success(), "{text}");
    assert!(text.contains("import名 `run` が衝突"), "{text}");
}

#[test]
fn リーフのメンバーを複数行で選択して別名導入できる() {
    let project = Project::new();
    project.write(
        "main.rd",
        "use values::{\n\
           answer as first,\n\
           other as second,\n\
         }\n\
         fn main() { first() + second() }\n",
    );
    project.write(
        "values.rd",
        "fn answer() { 20 }\n\
         fn other() { 22 }\n",
    );

    let output = project.run("main.rd");
    let text = output_text(&output);
    assert!(output.status.success(), "{text}");
    assert!(text.contains("main -> 42"), "{text}");
}

#[test]
fn 単一ファイルの既存動作を維持する() {
    let project = Project::new();
    project.write("main.rd", "fn main() { 42 }\n");

    let output = project.run("main.rd");
    let text = output_text(&output);
    assert!(output.status.success(), "{text}");
    assert!(text.contains("main -> 42"), "{text}");
}

#[test]
fn 選択導入したディレクトリから参照した子だけを読み込む() {
    let project = Project::new();
    project.write(
        "main.rd",
        "use services::{admin as backoffice}\n\
         fn main() { backoffice::users::run() }\n",
    );
    project.write("services/admin/users.rd", "fn run() { 9 }\n");
    project.write("services/admin/broken.rd", "読まない\n");

    let output = project.run("main.rd");
    let text = output_text(&output);
    assert!(output.status.success(), "{text}");
    assert!(text.contains("main -> 9"), "{text}");
    assert!(!text.contains("broken"), "{text}");
}

#[test]
fn 深いパスでも親のリーフとディレクトリの衝突を報告する() {
    let project = Project::new();
    project.write(
        "main.rd",
        "use conflict::child\n\
         fn main() { child::value() }\n",
    );
    project.write("conflict.rd", "fn value() { 1 }\n");
    project.write("conflict/child.rd", "fn value() { 2 }\n");

    let output = project.run("main.rd");
    let text = output_text(&output);
    assert!(!output.status.success(), "{text}");
    assert!(text.contains("モジュール `conflict`"), "{text}");
    assert!(text.contains("両方があります"), "{text}");
}

#[test]
fn 宣言位置の修飾参照から子モジュールを読み込む() {
    let project = Project::new();
    project.write(
        "main.rd",
        "use services\n\
         effect db: services::users::StoreApi\n\
         fn main() { 0 }\n",
    );
    project.write(
        "services/users.rd",
        "trait StoreApi {\n\
         \x20 fn read(self -> int)\n\
         }\n",
    );

    let output = project.run("main.rd");
    let text = output_text(&output);
    assert!(output.status.success(), "{text}");
    assert!(
        text.contains("main::db: services::users::StoreApi"),
        "{text}"
    );
    assert!(!text.contains("use` されていません"), "{text}");
}

#[test]
fn useしていない宣言位置のモジュール参照を報告する() {
    let project = Project::new();
    project.write(
        "main.rd",
        "effect db: services::users::StoreApi\n\
         fn main() { 0 }\n",
    );
    project.write(
        "services/users.rd",
        "trait StoreApi {\n\
         \x20 fn read(self -> int)\n\
         }\n",
    );

    let output = project.run("main.rd");
    let text = output_text(&output);
    assert!(!output.status.success(), "{text}");
    assert!(
        text.contains("モジュール名 `services` は `use` されていません"),
        "{text}"
    );
}

#[test]
fn ローカル束縛は修飾参照でもモジュールimportを隠す() {
    let project = Project::new();
    project.write(
        "main.rd",
        "use services\n\
         fn helper(services: int) { services::users::run() }\n\
         fn main() { 0 }\n",
    );
    project.write("services/users.rd", "this must not be read\n");

    let output = project.run("main.rd");
    let text = output_text(&output);
    assert!(output.status.success(), "{text}");
    assert!(text.contains("main -> 0"), "{text}");
}

#[test]
fn 依存モジュールのtestは今回の実行対象にしない() {
    let project = Project::new();
    project.write(
        "main.rd",
        "use dep\n\
         fn main() { dep::value() }\n",
    );
    project.write(
        "dep.rd",
        "fn value() { 5 }\n\
         test \"dependency test\" { assert false }\n",
    );

    let output = project.run("main.rd");
    let text = output_text(&output);
    assert!(output.status.success(), "{text}");
    assert!(text.contains("main -> 5"), "{text}");
    assert!(!text.contains("dependency test"), "{text}");
}

#[test]
fn 裸の相対エントリ名を読み込める() {
    let project = Project::new();
    project.write("main.rd", "fn main() { 6 }\n");

    let output = project.run_relative("main.rd");
    let text = output_text(&output);
    assert!(output.status.success(), "{text}");
    assert!(text.contains("main -> 6"), "{text}");
}

// ---- struct の形の検査 (src/typecheck.rs) ----

#[test]
fn 呼ばれない関数の不正なstruct生成でも実行前に失敗する() {
    let project = Project::new();
    project.write(
        "main.rd",
        "struct User { id: int }\n\
         fn unused() { User { id = 1, nope = 2 } }\n\
         fn main() { 1 }\n",
    );

    let output = project.run("main.rd");
    let text = output_text(&output);
    assert!(!output.status.success(), "{text}");
    assert!(text.contains("`nope`"), "{text}");
    assert!(!text.contains("main -> 1"), "{text}");
}

#[test]
fn モジュールを跨ぐstruct生成を正準名で報告する() {
    let project = Project::new();
    project.write(
        "main.rd",
        "use dep::{User}\n\
         fn main() { User { id = 1 } }\n",
    );
    project.write("dep.rd", "struct User { id: int\nrank: int }\n");

    let output = project.run("main.rd");
    let text = output_text(&output);
    assert!(!output.status.success(), "{text}");
    assert!(text.contains("dep::User"), "{text}");
    assert!(text.contains("`rank`"), "{text}");
}

#[test]
fn 宣言どおりのstruct生成はモジュールを跨いでも実行される() {
    let project = Project::new();
    project.write(
        "main.rd",
        "use dep::{User}\n\
         fn main() {\n\
           let u = User { rank = 2, id = 1 }\n\
           u.rank\n\
         }\n",
    );
    project.write("dep.rd", "struct User { id: int\nrank: int }\n");

    let output = project.run("main.rd");
    let text = output_text(&output);
    assert!(output.status.success(), "{text}");
    assert!(text.contains("main -> 2"), "{text}");
}

// ---- enum の宣言と名前解決 (src/module.rs) ----

#[test]
fn 同一モジュールの裸variantは宣言元へ解決される() {
    let project = Project::new();
    project.write(
        "main.rd",
        "enum Rank { Bronze Gold }\n\
         fn main() { Gold == Gold }\n",
    );

    let output = project.run("main.rd");
    let text = output_text(&output);
    assert!(output.status.success(), "{text}");
    assert!(text.contains("enum   main::Rank (2 variant)"), "{text}");
    assert!(text.contains("main -> true"), "{text}");
}

#[test]
fn 別モジュールのvariantをmember_importで導入できる() {
    let project = Project::new();
    project.write(
        "main.rd",
        "use dep::{Gold, Bronze}\n\
         fn main() { Gold == Bronze }\n",
    );
    project.write("dep.rd", "enum Rank { Bronze Gold }\n");

    let output = project.run("main.rd");
    let text = output_text(&output);
    assert!(output.status.success(), "{text}");
    assert!(text.contains("main -> false"), "{text}");
}

#[test]
fn importしたvariantに別名を付けられる() {
    let project = Project::new();
    project.write(
        "main.rd",
        "use dep::{Gold as Best}\n\
         fn main() { Best }\n",
    );
    project.write("dep.rd", "enum Rank { Bronze Gold }\n");

    let output = project.run("main.rd");
    let text = output_text(&output);
    assert!(output.status.success(), "{text}");
    // 別名は参照側の綴りでしかない。値は宣言元の正準名を持つ
    assert!(text.contains("main -> dep::Rank.dep::Gold"), "{text}");
}

#[test]
fn variantと同名の宣言を衝突として報告する() {
    let project = Project::new();
    project.write(
        "main.rd",
        "enum Rank { Bronze Gold }\n\
         struct Gold {}\n\
         fn main() { 1 }\n",
    );

    let output = project.run("main.rd");
    let text = output_text(&output);
    assert!(!output.status.success(), "{text}");
    assert!(
        text.contains("モジュール `main` で名前 `Gold` が重複しています"),
        "{text}"
    );
}

#[test]
fn 同じvariantを二度並べたenumを報告する() {
    let project = Project::new();
    project.write(
        "main.rd",
        "enum Rank { Gold Gold }\n\
         fn main() { 1 }\n",
    );

    let output = project.run("main.rd");
    let text = output_text(&output);
    assert!(!output.status.success(), "{text}");
    assert!(
        text.contains("enum `Rank`: variant `Gold` が重複して宣言されています"),
        "{text}"
    );
}

#[test]
fn ローカル束縛はvariantを隠す() {
    let project = Project::new();
    project.write(
        "main.rd",
        "enum Rank { Bronze Gold }\n\
         fn main() {\n\
           let Gold = 7\n\
           Gold\n\
         }\n",
    );

    let output = project.run("main.rd");
    let text = output_text(&output);
    assert!(output.status.success(), "{text}");
    assert!(text.contains("main -> 7"), "{text}");
}

// ---- 限定した variant と match (src/parse.rs, src/typecheck.rs, src/eval.rs) ----

#[test]
fn 限定したvariantのmatchはarmの値を産んで実行される() {
    let project = Project::new();
    project.write(
        "main.rd",
        "use dep::{Grade, High}\n\
         enum Rank { Bronze Gold }\n\
         fn label(r: Rank -> str) {\n\
         \x20 match r {\n\
         \x20   Rank::Bronze: \"bronze\"\n\
         \x20   Rank::Gold {\n\
         \x20     \"gold\"\n\
         \x20   }\n\
         \x20 }\n\
         }\n\
         fn score(g: Grade -> int) {\n\
         \x20 match g {\n\
         \x20   Grade::Low: 1\n\
         \x20   Grade::High: 2\n\
         \x20 }\n\
         }\n\
         fn main(-> str) {\n\
         \x20 assert score(High) == 2\n\
         \x20 label(Rank::Gold)\n\
         }\n",
    );
    project.write("dep.rd", "enum Grade { Low High }\n");

    let output = project.run("main.rd");
    let text = output_text(&output);
    assert!(output.status.success(), "{text}");
    assert!(text.contains("main -> \"gold\""), "{text}");
}

#[test]
fn 裸のarmパスは実行前に失敗する() {
    実行前に失敗する(
        "enum Rank { Bronze Gold }\n\
         fn main(r: Rank -> int) { match r { Bronze: 1\nGold: 2 } }\n",
        "`Enum::Variant`",
    );
}

#[test]
fn enumでない対象のmatchは実行前に失敗する() {
    実行前に失敗する(
        "enum Rank { Bronze Gold }\n\
         fn unused(n: int) { match n { Rank::Bronze: 1\nRank::Gold: 2 } }\n\
         fn main() { 1 }\n",
        "`match` の対象は非 optional な enum",
    );
}

#[test]
fn 網羅していないmatchは実行前に失敗する() {
    実行前に失敗する(
        "enum Rank { Bronze Gold }\n\
         fn unused(r: Rank -> int) { match r { Rank::Gold: 2 } }\n\
         fn main() { 1 }\n",
        "`main::Rank` の variant `Bronze` を扱っていません",
    );
}

#[test]
fn 別のenumのarmは正準名で報告される() {
    let project = Project::new();
    project.write(
        "main.rd",
        "use dep::{Low}\n\
         enum Rank { Bronze Gold }\n\
         fn unused(r: Rank -> int) {\n\
         \x20 match r {\n\
         \x20   Rank::Bronze: 1\n\
         \x20   Rank::Gold: 2\n\
         \x20   dep::Grade::Low: 3\n\
         \x20 }\n\
         }\n\
         fn main() { 1 }\n",
    );
    project.write("dep.rd", "enum Grade { Low High }\n");

    let output = project.run("main.rd");
    let text = output_text(&output);
    assert!(!output.status.success(), "{text}");
    assert!(
        text.contains("arm `dep::Grade::Low` は `main::Rank` の variant ではありません"),
        "{text}"
    );
    assert!(!text.contains("main ->"), "{text}");
}

#[test]
fn 型の違うarmの値は実行前に失敗する() {
    実行前に失敗する(
        "enum Rank { Bronze Gold }\n\
         fn unused(r: Rank -> str) { match r { Rank::Bronze: \"b\"\nRank::Gold: 2 } }\n\
         fn main() { 1 }\n",
        "arm `main::Rank::Gold` の値は `str` ですが、`int` です",
    );
}

/// arm の中の ambient 使用も、他の枝と同じく到達経路付きで届く
#[test]
fn armの中の提供忘れは到達経路付きで失敗する() {
    実行前に失敗する(
        "trait Clock { fn now(self -> int) }\n\
         effect clock: Clock\n\
         enum Rank { Bronze Gold }\n\
         fn stamp(-> int) { clock.now() }\n\
         fn main(r: Rank -> int) {\n\
         \x20 match r {\n\
         \x20   Rank::Bronze: 0\n\
         \x20   Rank::Gold: stamp()\n\
         \x20 }\n\
         }\n",
        "main::clock が要る ← main::stamp ← main::main",
    );
}

// ---- payload を持つ variant (src/parse.rs, src/typecheck.rs, src/eval.rs) ----

/// 構築と分解が往復する縦切り。payload の順序・`_`・fieldless の共存・
/// モジュールを跨ぐ payload 型まで、1本のプログラムで通す
#[test]
fn payloadの構築と分解が往復して実行される() {
    let project = Project::new();
    project.write(
        "main.rd",
        "use dep::{User, make}\n\
         enum Lookup {\n\
         \x20 Found(User, int)\n\
         \x20 Missing(str)\n\
         \x20 Skipped\n\
         }\n\
         fn describe(l: Lookup -> str) {\n\
         \x20 match l {\n\
         \x20   Lookup::Found(found, _) {\n\
         \x20     assert found.id == 7\n\
         \x20     \"found\"\n\
         \x20   }\n\
         \x20   Lookup::Missing(reason): reason\n\
         \x20   Lookup::Skipped: \"skipped\"\n\
         \x20 }\n\
         }\n\
         fn rank(l: Lookup -> int) {\n\
         \x20 match l {\n\
         \x20   Lookup::Found(_, n): n\n\
         \x20   Lookup::Missing(_): 0\n\
         \x20   Lookup::Skipped: 0\n\
         \x20 }\n\
         }\n\
         fn main(-> str) {\n\
         \x20 let u = make()\n\
         \x20 assert rank(Lookup::Found(u, 5)) == 5\n\
         \x20 assert describe(Lookup::Missing(\"gone\")) == \"gone\"\n\
         \x20 assert describe(Skipped) == \"skipped\"\n\
         \x20 assert Lookup::Missing(\"a\") == Lookup::Missing(\"a\")\n\
         \x20 assert (Lookup::Missing(\"a\") == Lookup::Missing(\"b\")) == false\n\
         \x20 describe(Lookup::Found(u, 5))\n\
         }\n",
    );
    project.write(
        "dep.rd",
        "struct User { id: int }\n\
         fn make(-> User) { User { id = 7 } }\n",
    );

    let output = project.run("main.rd");
    let text = output_text(&output);
    assert!(output.status.success(), "{text}");
    assert!(text.contains("main -> \"found\""), "{text}");
}

/// payload の型がモジュール参照なら、他の型注釈と同じく `use` を要求する
#[test]
fn use_していないモジュールのpayload型を報告する() {
    実行前に失敗する(
        "enum Lookup { Found(dep::User) }\n\
         fn main() { 1 }\n",
        "モジュール名 `dep` は `use` されていません",
    );
}

#[test]
fn 構築の個数違いは実行前に失敗する() {
    実行前に失敗する(
        "struct User { id: int }\n\
         enum Lookup { Found(User, int) }\n\
         fn main() { Lookup::Found(User { id = 1 }) }\n",
        "`main::Lookup::Found` は引数を 2 個取りますが、1 個渡しています",
    );
}

#[test]
fn 構築の引数の型違いは実行前に失敗する() {
    実行前に失敗する(
        "struct User { id: int }\n\
         enum Lookup { Found(User) }\n\
         fn main() { Lookup::Found(1) }\n",
        "`main::Lookup::Found` の第 1 引数は `main::User` ですが、`int` を渡しています",
    );
}

#[test]
fn 構築しないpayload_variantは実行前に失敗する() {
    実行前に失敗する(
        "struct User { id: int }\n\
         enum Lookup { Found(User) }\n\
         fn main() { Lookup::Found }\n",
        "`main::Lookup::Found` は payload を 1 個取ります",
    );
}

#[test]
fn patternの個数違いは実行前に失敗する() {
    実行前に失敗する(
        "enum Lookup { Found(str) }\n\
         fn unused(l: Lookup -> int) { match l { Lookup::Found(a, b): 1 } }\n\
         fn main() { 1 }\n",
        "arm `main::Lookup::Found` は payload を 2 個束縛しますが、\
         `main::Lookup::Found` の payload は 1 個です",
    );
}

#[test]
fn 同じ名前を二度束縛するpatternは実行前に失敗する() {
    実行前に失敗する(
        "enum Lookup { Found(str, str) }\n\
         fn unused(l: Lookup -> str) { match l { Lookup::Found(x, x): x } }\n\
         fn main() { 1 }\n",
        "arm `main::Lookup::Found` の pattern が `x` を二度束縛しています",
    );
}

#[test]
fn 束縛の型は後続の検査へ流れる() {
    実行前に失敗する(
        "enum Lookup { Found(str) }\n\
         fn take(n: int) { n }\n\
         fn unused(l: Lookup -> int) { match l { Lookup::Found(reason): take(reason) } }\n\
         fn main() { 1 }\n",
        "`main::take` の第 1 引数は `int` ですが、`str` を渡しています",
    );
}

/// payload の名前が ambient スロットを隠すのは、その arm の本体の間だけ。
/// 隠した arm は要求を作らず、隠していない arm の要求は残る
#[test]
fn payload束縛に隠されたスロットは要求にならない() {
    let project = Project::new();
    project.write(
        "main.rd",
        "trait Clock { fn now(self -> int) }\n\
         struct SystemClock {}\n\
         impl Clock for SystemClock { fn now(self -> int) { 42 } }\n\
         effect clock: Clock\n\
         enum Lookup { Found(SystemClock) Skipped }\n\
         fn read(l: Lookup -> int) {\n\
         \x20 match l {\n\
         \x20   Lookup::Found(clock): clock.now()\n\
         \x20   Lookup::Skipped: 0\n\
         \x20 }\n\
         }\n\
         fn main(-> int) { read(Lookup::Found(SystemClock {})) }\n",
    );

    let output = project.run("main.rd");
    let text = output_text(&output);
    // `with` が1つも無くても、要求が無いので走る
    assert!(output.status.success(), "{text}");
    assert!(text.contains("main::read / (要求なし)"), "{text}");
    assert!(text.contains("main -> 42"), "{text}");
}

// ---- enum 型の検査 (src/typecheck.rs) ----

#[test]
fn enum型フィールドに同じenumのvariantを入れたプログラムは走る() {
    let project = Project::new();
    project.write(
        "main.rd",
        "enum Rank { Bronze Gold }\n\
         struct User { rank: Rank }\n\
         fn main() {\n\
           let u = User { rank = Bronze }\n\
           u.rank = Gold\n\
           u.rank == Gold\n\
         }\n",
    );

    let output = project.run("main.rd");
    let text = output_text(&output);
    assert!(output.status.success(), "{text}");
    assert!(text.contains("main -> true"), "{text}");
}

#[test]
fn 別のenumのvariantをenum型フィールドへ与えると実行前に失敗する() {
    let project = Project::new();
    project.write(
        "main.rd",
        "use dep::{Grade, Low}\n\
         enum Rank { Bronze Gold }\n\
         struct User { rank: Rank }\n\
         fn main() { User { rank = Low } }\n",
    );
    project.write("dep.rd", "enum Grade { Low High }\n");

    let output = project.run("main.rd");
    let text = output_text(&output);
    assert!(!output.status.success(), "{text}");
    assert!(text.contains("`main::Rank`"), "{text}");
    assert!(text.contains("`dep::Grade`"), "{text}");
    assert!(!text.contains("main ->"), "{text}");
}

// ---- 関数の署名の検査 (src/typecheck.rs) ----

#[test]
fn 引数の個数が合わない呼び出しは実行前に失敗する() {
    let project = Project::new();
    project.write(
        "main.rd",
        "use dep::{add}\n\
         fn main() { add(1) }\n",
    );
    project.write("dep.rd", "fn add(a: int, b: int -> int) { a }\n");

    let output = project.run("main.rd");
    let text = output_text(&output);
    assert!(!output.status.success(), "{text}");
    assert!(text.contains("`dep::add`"), "{text}");
    assert!(text.contains("2 個取ります"), "{text}");
    assert!(!text.contains("main ->"), "{text}");
}

#[test]
fn 型の違う引数を渡す呼び出しは実行前に失敗する() {
    let project = Project::new();
    project.write(
        "main.rd",
        "use dep::{Grade, Low}\n\
         enum Rank { Bronze Gold }\n\
         fn rank(r: Rank -> Rank) { r }\n\
         fn main() { rank(Low) }\n",
    );
    project.write("dep.rd", "enum Grade { Low High }\n");

    let output = project.run("main.rd");
    let text = output_text(&output);
    assert!(!output.status.success(), "{text}");
    assert!(text.contains("第 1 引数"), "{text}");
    assert!(text.contains("`dep::Grade`"), "{text}");
    assert!(!text.contains("main ->"), "{text}");
}

#[test]
fn 宣言と違う型を返す関数は実行前に失敗する() {
    let project = Project::new();
    project.write(
        "main.rd",
        "use dep::{Grade}\n\
         enum Rank { Bronze Gold }\n\
         fn pick(-> Grade) { Gold }\n\
         fn main() { pick() }\n",
    );
    project.write("dep.rd", "enum Grade { Low High }\n");

    let output = project.run("main.rd");
    let text = output_text(&output);
    assert!(!output.status.success(), "{text}");
    assert!(text.contains("main::pick: 戻り値"), "{text}");
    assert!(text.contains("`dep::Grade`"), "{text}");
    assert!(!text.contains("main ->"), "{text}");
}

#[test]
fn 宣言どおりの呼び出しと戻り値はモジュールを跨いでも実行される() {
    let project = Project::new();
    project.write(
        "main.rd",
        "use dep::{Rank, Gold, pick}\n\
         fn keep(r: Rank -> Rank) { r }\n\
         fn main() { keep(pick()) == Gold }\n",
    );
    project.write(
        "dep.rd",
        "enum Rank { Bronze Gold }\n\
         fn pick(-> Rank) { Gold }\n",
    );

    let output = project.run("main.rd");
    let text = output_text(&output);
    assert!(output.status.success(), "{text}");
    assert!(text.contains("main -> true"), "{text}");
}

// ---- 基本の式の型の検査 (src/typecheck.rs) ----

/// 検査を通らないプログラムは1行も走らない、を各規則で1本ずつ固定する。
/// 診断の文面そのものは typecheck.rs の単体テストが見ている
fn 実行前に失敗する(source: &str, expected: &str) {
    let project = Project::new();
    project.write("main.rd", source);

    let output = project.run("main.rd");
    let text = output_text(&output);
    assert!(!output.status.success(), "{text}");
    assert!(text.contains(expected), "{text}");
    assert!(!text.contains("main ->"), "{text}");
}

#[test]
fn 組み込み型名の宣言は実行前に失敗する() {
    実行前に失敗する(
        "struct int {}\nfn main() { 1 }\n",
        "`int` は組み込み型の名前なので宣言できません",
    );
}

#[test]
fn 宣言に無いフィールドの読みは実行前に失敗する() {
    実行前に失敗する(
        "struct User { id: int }\n\
         fn f(u: User -> int) { u.nope }\n\
         fn main() { 1 }\n",
        "`nope`",
    );
}

#[test]
fn 整数でない被演算子は実行前に失敗する() {
    実行前に失敗する("fn main() { \"a\" + 1 }\n", "`+` の左辺");
}

#[test]
fn boolでない条件は実行前に失敗する() {
    実行前に失敗する("fn main() { if 1 { 2 } }\n", "条件は `bool`");
}

#[test]
fn 型の違うフィールド値は実行前に失敗する() {
    実行前に失敗する(
        "struct Card { n: int }\nfn main() { Card { n = \"x\" } }\n",
        "`str` を与えています",
    );
}

#[test]
fn nilを非optionalへ渡すと実行前に失敗する() {
    実行前に失敗する(
        "fn take(n: int) { n }\n\
         fn unused() { take(nil) }\n\
         fn main() { 1 }\n",
        "`nil`",
    );
}

#[test]
fn 非optionalの左辺へfallbackを使うと実行前に失敗する() {
    実行前に失敗する(
        "fn unused(n: int) { n ?? 0 }\nfn main() { 1 }\n",
        "`??` の左辺",
    );
}

#[test]
fn fallbackの結果型は実行前の戻り値検査へ届く() {
    実行前に失敗する(
        "enum Rank { Bronze Gold }\n\
         enum Grade { Low High }\n\
         fn unused(r: Rank? -> Grade) { r ?? Gold }\n\
         fn main() { 1 }\n",
        "戻り値は `main::Grade` ですが、`main::Rank`",
    );
}

#[test]
fn optional_field_accessは値を読みnilを伝播して実行できる() {
    let project = Project::new();
    project.write(
        "main.rd",
        "struct User { id: int }\n\
         fn some(-> User?) { User { id = 7 } }\n\
         fn none(-> User?) { nil }\n\
         fn main() {\n\
         \x20 assert (some().?id ?? 0) == 7\n\
         \x20 none().?id == nil\n\
         }\n",
    );

    let output = project.run("main.rd");
    let text = output_text(&output);
    assert!(output.status.success(), "{text}");
    assert!(text.contains("main -> true"), "{text}");
}

#[test]
fn optional_field_accessのreceiver形式を実行前に検査する() {
    実行前に失敗する(
        "struct User { id: int }\n\
         fn unused(u: User) { u.?id }\n\
         fn main() { 1 }\n",
        "レシーバは optional",
    );
    実行前に失敗する(
        "struct User { id: int }\n\
         fn unused(u: User?) { u.id }\n\
         fn main() { 1 }\n",
        "`.?id`",
    );
}

#[test]
fn optional_fieldの結果型は実行前の引数検査へ届く() {
    実行前に失敗する(
        "struct User { name: str }\n\
         fn take(n: int?) { n }\n\
         fn unused(u: User?) { take(u.?name) }\n\
         fn main() { 1 }\n",
        "`str?`",
    );
}

/// `examples/` は誰も走らせないと腐る(実際 `missing_handler.rd` は
/// メソッド呼び出しの検査が入った時点で型検査に落ちていた)。
#[test]
fn 提供忘れの例は到達経路付きで失敗する() {
    let output = Command::new(env!("CARGO_BIN_EXE_rhodolite"))
        .arg("examples/missing_handler.rd")
        .output()
        .unwrap();
    let text = output_text(&output);

    assert!(!output.status.success(), "{text}");
    // 使っている関数から呼び出し元へさかのぼる向き(README と同じ形)
    assert!(
        text.contains(
            "missing_handler::clock が要る \
             ← missing_handler::stamp \
             ← missing_handler::promote \
             ← missing_handler::handle \
             ← missing_handler::main"
        ),
        "{text}"
    );
}

#[test]
fn 契約を満たさない提供は実行前に失敗する() {
    let contract = "trait Clock { fn now(self -> int) }\n\
                    effect clock: Clock\n\
                    struct NotAClock {}\n";
    // 値の形。`with` の中は実行されなくても、宣言の時点で落ちる
    実行前に失敗する(
        &format!("{contract}fn main() {{ with clock(NotAClock {{}}) {{ 1 }} }}\n"),
        "`main::NotAClock` は `main::Clock` を実装していないので `main::clock` に提供できません",
    );
    // 型の形
    実行前に失敗する(
        &format!("{contract}fn main() {{ with clock<NotAClock> {{ 1 }} }}\n"),
        "実装していないので `main::clock` に提供できません",
    );
    // struct ですらない型
    実行前に失敗する(
        &format!("{contract}fn main() {{ with clock(1) {{ 1 }} }}\n"),
        "`int` は `main::Clock` を実装していない",
    );
}

#[test]
fn 型の違う再代入は実行前に失敗する() {
    実行前に失敗する(
        "fn f(n: int) { n = \"x\" }\nfn main() { 1 }\n",
        "`n` への代入",
    );
}

// ---- 診断の描画 (src/render.rs) ----

/// span を持つ診断は、ファイル・行・桁と原因行の抜粋を伴って出る。
#[test]
fn 位置を持つ診断はソース抜粋付きで出る() {
    let project = Project::new();
    project.write(
        "main.rd",
        "struct Card { n: int }\nfn main() { Card { n = \"x\" } }\n",
    );

    let output = project.run("main.rd");
    let text = output_text(&output);
    assert!(!output.status.success(), "{text}");
    assert!(text.contains("main.rd:2:24"), "{text}");
    assert!(text.contains("fn main() { Card { n = \"x\" } }"), "{text}");
}

/// ソースへ届く前に失った診断は、抜粋を持たない素のテキストのまま出る。
#[test]
fn 位置を持たない診断は抜粋なしで出る() {
    let project = Project::new();
    project.write("main.txt", "fn main() { 1 }\n");

    let output = project.run("main.txt");
    let text = output_text(&output);
    assert!(!output.status.success(), "{text}");
    assert_eq!(
        text.trim(),
        "エントリーファイルは `.rd` でなければなりません"
    );
}

/// 実行時エラーは span を持たない。従来どおりの1行のまま。
#[test]
fn 実行時エラーは従来どおりの見た目で出る() {
    let project = Project::new();
    project.write("main.rd", "fn main(-> int) { 1 / 0 }\n");

    let output = project.run("main.rd");
    let text = output_text(&output);
    assert!(!output.status.success(), "{text}");
    assert!(text.contains("  main で失敗: "), "{text}");
    assert!(!text.contains("╭─"), "{text}");
}

/// 推論された要求の一覧は診断ではない。span が入っても書式は変わらない。
#[test]
fn 推論された要求の一覧は従来どおり出る() {
    let project = Project::new();
    project.write(
        "main.rd",
        "trait Clock { fn now(self -> int) }\n\
         effect clock: Clock\n\
         struct SystemClock {}\n\
         impl Clock for SystemClock { fn now(self -> int) { 7 } }\n\
         fn main(-> int) { with clock(SystemClock {}) { clock.now() } }\n",
    );

    let output = project.run("main.rd");
    let text = output_text(&output);
    assert!(output.status.success(), "{text}");
    assert!(text.contains("\n推論された要求:\n"), "{text}");
    assert!(text.contains("  main::main / (要求なし)\n"), "{text}");
}

/// 到達経路がモジュールを跨いでも、各ホップは自分のファイルの上に描かれる。
#[test]
fn 到達経路のホップは宣言元のファイルで描かれる() {
    let project = Project::new();
    project.write(
        "main.rd",
        "use dep\n\
         trait Clock { fn now(self -> int) }\n\
         effect clock: Clock\n\
         fn main(-> int) { dep::relay() }\n",
    );
    project.write(
        "dep.rd",
        "use main\n\
         fn relay(-> int) { main::clock.now() }\n",
    );

    let output = project.run("main.rd");
    let text = output_text(&output);
    assert!(!output.status.success(), "{text}");
    // 使用地点は dep.rd、要求を運ぶ呼び出しは main.rd にある
    assert!(text.contains("dep.rd:2:20"), "{text}");
    assert!(text.contains("main.rd:4:19"), "{text}");
    assert!(
        text.contains("main::clock が要る ← dep::relay ← main::main"),
        "{text}"
    );
}
