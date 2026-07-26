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
           fn read(self -> Int)\n\
         }\n\
         struct Store {}\n\
         impl StoreApi for Store {\n\
           fn read(self -> Int) { 1 }\n\
         }\n\
         effect db: StoreApi\n\
         fn read() { db.read() }\n",
    );
    project.write(
        "sides/right.rd",
        "trait StoreApi {\n\
           fn read(self -> Int)\n\
         }\n\
         struct Store {}\n\
         impl StoreApi for Store {\n\
           fn read(self -> Int) { 2 }\n\
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
         \x20 fn read(self -> Int)\n\
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
         \x20 fn read(self -> Int)\n\
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
         fn helper(services: Int) { services::users::run() }\n\
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
