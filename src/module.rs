//! ADR-0006 のモジュール読み込みと名前解決。
//!
//! パーサが作る各ファイル内の AST を集め、宣言と参照を宣言元の完全修飾名へ
//! 書き換えてから、既存の要求推論と評価器へ渡す。

use crate::ast::*;
use crate::diag::Diag;
use crate::lex::Span;
use crate::{lex, parse};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ModulePath(Vec<String>);

impl ModulePath {
    fn from_parts(parts: &[String]) -> Self {
        Self(parts.to_vec())
    }

    fn child(&self, name: &str) -> Self {
        let mut parts = self.0.clone();
        parts.push(name.to_string());
        Self(parts)
    }

    fn name(&self) -> &str {
        self.0.last().map(String::as_str).unwrap_or("")
    }

    fn qualified(&self, name: &str) -> String {
        format!("{}::{name}", self)
    }
}

impl std::fmt::Display for ModulePath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0.join("::"))
    }
}

#[derive(Debug)]
struct ParsedModule {
    path: ModulePath,
    uses: Vec<UseDecl>,
    items: Vec<Item>,
}

/// 読み込んだモジュール1つ分のソース。診断の描画まで本文を保つために持つ。
/// `lex::SourceId` はこの表の添字。
#[derive(Debug)]
pub struct SourceFile {
    pub path: PathBuf,
    pub text: String,
}

/// 読み込みが失敗したときの診断と、そこまでに読めたソース。
/// span 付きの診断を描くには本文が要るので、失敗時も一緒に返す。
#[derive(Debug)]
pub struct LoadError {
    pub diagnostics: Vec<Diag>,
    pub sources: Vec<SourceFile>,
}

#[derive(Debug)]
pub struct LoadedProgram {
    pub program: Program,
    pub entry: String,
    pub sources: Vec<SourceFile>,
    /// エントリーモジュールが `pub use path::{...}` で明示選択した公開束縛を
    /// 公開名の昇順で持つ。ホストから呼べる関数の候補はここからしか出ない。
    ///
    /// モジュール形の `pub use path` は名前空間だけを公開するので、
    /// 配下の関数は入らない(public-reexports spec)
    pub public_exports: Vec<PublicExport>,
}

/// エントリーモジュールの公開名1つと、それが指す宣言の正準名。
///
/// 関数とは限らない。struct や enum を明示選択したときも公開名前空間には
/// 載るので、ホスト関数への絞り込みは HIR 側(CallableId が引けるか)で行う
#[derive(Clone, Debug)]
pub struct PublicExport {
    pub name: String,
    pub canonical: String,
    pub span: Span,
}

/// モジュールが公開しているメンバー1つ。`pub use` だけがここへ名前を載せる。
#[derive(Clone, Debug)]
enum PublicMember {
    /// 宣言の正準名。別名や中継の再エクスポートを経ても宣言の同一性は変わらない
    Decl(String),
    Module(ModulePath),
}

pub fn load(entry_file: &Path) -> Result<LoadedProgram, LoadError> {
    let Some(root) = entry_file.parent() else {
        return Err(LoadError::single(Diag::msg(
            "エントリーファイルの親ディレクトリがありません",
        )));
    };
    let root = if root.as_os_str().is_empty() {
        Path::new(".")
    } else {
        root
    };
    let Some(stem) = entry_file.file_stem().and_then(|s| s.to_str()) else {
        return Err(LoadError::single(Diag::msg(
            "エントリーファイル名を UTF-8 として読めません",
        )));
    };
    if entry_file.extension().and_then(|s| s.to_str()) != Some("rd") {
        return Err(LoadError::single(Diag::msg(
            "エントリーファイルは `.rd` でなければなりません",
        )));
    }
    if !valid_ident(stem) {
        return Err(LoadError::single(Diag::msg(format!(
            "モジュール名 `{stem}` は有効な識別子ではありません"
        ))));
    }

    let entry_path = ModulePath(vec![stem.to_string()]);
    let mut loader = Loader {
        root,
        modules: BTreeMap::new(),
        directories: BTreeSet::new(),
        diagnostics: Vec::new(),
        sources: Vec::new(),
    };
    loader.load_module(&entry_path, None);
    if !loader.diagnostics.is_empty() {
        return Err(LoadError {
            diagnostics: loader.diagnostics,
            sources: loader.sources,
        });
    }
    resolve(
        loader.modules,
        loader.directories,
        loader.sources,
        &entry_path,
    )
}

/// 読み込み中の状態。再帰の引数を増やさずに、読んだソースと診断を溜める。
struct Loader<'a> {
    root: &'a Path,
    modules: BTreeMap<ModulePath, ParsedModule>,
    directories: BTreeSet<ModulePath>,
    diagnostics: Vec<Diag>,
    sources: Vec<SourceFile>,
}

impl Loader<'_> {
    /// `origin` は、このモジュールの読み込みを要求した `use` 宣言の位置。
    /// 見つからない・読めないといった診断はそこを指す。エントリーだけ `None`。
    fn load_module(&mut self, path: &ModulePath, origin: Option<Span>) {
        let root = self.root;
        if self.modules.contains_key(path) || self.directories.contains(path) {
            return;
        }
        for part in &path.0 {
            if !valid_ident(part) {
                self.diagnostics.push(Diag::from_span(
                    origin,
                    format!("モジュールパス `{path}` の `{part}` は有効な識別子ではありません"),
                ));
                return;
            }
        }

        for length in 1..=path.0.len() {
            let prefix = ModulePath(path.0[..length].to_vec());
            let relative = prefix
                .0
                .iter()
                .fold(PathBuf::new(), |built, part| built.join(part));
            let leaf = root.join(&relative).with_extension("rd");
            let directory = root.join(&relative);
            let has_leaf = leaf.is_file();
            let has_directory = directory.is_dir();

            if (has_leaf && !exact_spelling(&leaf))
                || (has_directory && !exact_spelling(&directory))
            {
                self.diagnostics.push(Diag::from_span(
                    origin,
                    format!(
                        "モジュール `{prefix}` のパスはファイルシステム上の綴りと完全一致しません"
                    ),
                ));
                return;
            }
            if has_leaf && has_directory {
                self.diagnostics.push(Diag::from_span(
                    origin,
                    format!(
                        "モジュール `{prefix}` に `{}` と `{}` の両方があります",
                        leaf.display(),
                        directory.display()
                    ),
                ));
                return;
            }
            if length < path.0.len() && has_leaf {
                self.diagnostics.push(Diag::from_span(
                    origin,
                    format!("リーフモジュール `{prefix}` は子モジュールを持てません"),
                ));
                return;
            }
            if length < path.0.len() && !has_directory {
                self.diagnostics.push(Diag::from_span(
                    origin,
                    format!("モジュール `{prefix}` が見つかりません"),
                ));
                return;
            }
        }

        let relative = path
            .0
            .iter()
            .fold(PathBuf::new(), |built, part| built.join(part));
        let leaf = root.join(&relative).with_extension("rd");
        let directory = root.join(relative);
        let has_leaf = leaf.is_file();
        let has_directory = directory.is_dir();
        if !has_leaf && !has_directory {
            self.diagnostics.push(Diag::from_span(
                origin,
                format!("モジュール `{path}` が見つかりません"),
            ));
            return;
        }
        if has_directory {
            self.directories.insert(path.clone());
            return;
        }

        let text = match std::fs::read_to_string(&leaf) {
            Ok(text) => text,
            Err(error) => {
                self.diagnostics.push(Diag::msg(format!(
                    "{} を読めません: {error}",
                    leaf.display()
                )));
                return;
            }
        };
        // 診断の描画まで本文を保つ。span はこの添字で自分のファイルを引く
        let id = self.sources.len() as lex::SourceId;
        self.sources.push(SourceFile {
            path: leaf.clone(),
            text,
        });
        let source = &self.sources[id as usize].text;
        let tokens = match lex::lex_source(source, id) {
            Ok(tokens) => lex::join(tokens),
            Err(error) => {
                self.diagnostics.push(Diag {
                    msg: format!("字句解析エラー: {error}"),
                    ..error
                });
                return;
            }
        };
        let program = match parse::parse(&tokens) {
            Ok(program) => program,
            Err(error) => {
                self.diagnostics.push(error);
                return;
            }
        };
        let uses = program.uses.clone();
        self.modules.insert(
            path.clone(),
            ParsedModule {
                path: path.clone(),
                uses: program.uses,
                items: program.items,
            },
        );

        // 先に現在のモジュールを登録するため、循環 use はここで自然に止まる。
        for use_decl in uses {
            let target = ModulePath::from_parts(&use_decl.path);
            self.load_module(&target, Some(use_decl.span));
            if use_decl.members.is_some() && self.directories.contains(&target) {
                for member in use_decl.members.unwrap_or_default() {
                    let child = target.child(&member.name);
                    if module_exists(root, &child) {
                        self.load_module(&child, Some(use_decl.span));
                    }
                }
            }
        }

        // ディレクトリモジュールは、修飾参照に現れた子だけを辿る。
        // ディレクトリ全体を走査しないため、未参照ファイルは読まれない。
        let Some(module) = self.modules.get(path) else {
            return;
        };
        let references = module_references(&module.items);
        let mut directory_imports = Vec::new();
        for use_decl in &module.uses {
            let target = ModulePath::from_parts(&use_decl.path);
            if let Some(members) = &use_decl.members {
                for member in members {
                    let child = target.child(&member.name);
                    if self.directories.contains(&child) {
                        directory_imports.push((
                            member.alias.clone().unwrap_or_else(|| member.name.clone()),
                            child,
                            use_decl.span,
                        ));
                    }
                }
            } else if self.directories.contains(&target) {
                directory_imports.push((
                    use_decl
                        .alias
                        .clone()
                        .unwrap_or_else(|| target.name().to_string()),
                    target,
                    use_decl.span,
                ));
            }
        }

        for reference in references {
            let Some((_, mut current, origin)) = directory_imports
                .iter()
                .find(|(alias, _, _)| reference.first() == Some(alias))
                .cloned()
            else {
                continue;
            };
            for part in reference.iter().skip(1) {
                let child = current.child(part);
                if !module_exists(root, &child) {
                    break;
                }
                self.load_module(&child, Some(origin));
                current = child;
            }
        }
    }
}

impl LoadError {
    fn single(diagnostic: Diag) -> Self {
        Self {
            diagnostics: vec![diagnostic],
            sources: Vec::new(),
        }
    }
}

/// 文言で整列して重複を畳む(span が入る前と同じ集合・同じ順序)。
/// 同じ文言が両方出たときは、位置を持つ方を残す。
fn sort_diagnostics(diagnostics: &mut Vec<Diag>) {
    diagnostics.sort_by(|a, b| (&a.msg, a.span.is_none()).cmp(&(&b.msg, b.span.is_none())));
    diagnostics.dedup_by(|a, b| a.msg == b.msg);
}

fn module_exists(root: &Path, path: &ModulePath) -> bool {
    let relative = path
        .0
        .iter()
        .fold(PathBuf::new(), |built, part| built.join(part));
    root.join(&relative).with_extension("rd").is_file() || root.join(relative).is_dir()
}

fn exact_spelling(path: &Path) -> bool {
    let Some(parent) = path.parent() else {
        return false;
    };
    let Some(file_name) = path.file_name() else {
        return false;
    };
    std::fs::read_dir(parent).is_ok_and(|entries| {
        entries
            .filter_map(Result::ok)
            .any(|entry| entry.file_name() == file_name)
    })
}

/// `use target::{name}` が指す先を1つ引く。
///
/// 子モジュール → `target` の宣言 → `target` の公開再エクスポート、の順。
/// 最後の一段だけが今回の追加で、先の二段は従来の解決順そのまま
fn lookup_member(
    target: &ModulePath,
    name: &str,
    declarations: &BTreeMap<ModulePath, BTreeMap<String, String>>,
    publics: &BTreeMap<ModulePath, BTreeMap<String, PublicMember>>,
) -> Option<PublicMember> {
    let child = target.child(name);
    if declarations.contains_key(&child) {
        return Some(PublicMember::Module(child));
    }
    if let Some(canonical) = declarations.get(target).and_then(|names| names.get(name)) {
        return Some(PublicMember::Decl(canonical.clone()));
    }
    publics
        .get(target)
        .and_then(|table| table.get(name))
        .cloned()
}

fn resolve(
    modules: BTreeMap<ModulePath, ParsedModule>,
    directories: BTreeSet<ModulePath>,
    sources: Vec<SourceFile>,
    entry_path: &ModulePath,
) -> Result<LoadedProgram, LoadError> {
    let mut diagnostics = Vec::new();
    let mut declarations: BTreeMap<ModulePath, BTreeMap<String, String>> = BTreeMap::new();
    for directory in directories {
        declarations.insert(directory, BTreeMap::new());
    }

    for module in modules.values() {
        let mut names = BTreeMap::new();
        for item in &module.items {
            // 同じ enum が同じ variant を二度並べたときだけは、衝突した名前ではなく
            // どの enum の話かを報告する。宣言名前空間の衝突判定は下の1本のまま
            if let Item::Enum { name, variants, .. } = item {
                let mut seen = BTreeSet::new();
                for variant in variants {
                    if !seen.insert(variant.name.as_str()) {
                        diagnostics.push(Diag::at(
                            item.span(),
                            format!(
                                "enum `{name}`: variant `{}` が重複して宣言されています",
                                variant.name
                            ),
                        ));
                    }
                }
            }
            for name in item_names(item) {
                if names
                    .insert(name.to_string(), module.path.qualified(name))
                    .is_some()
                {
                    diagnostics.push(Diag::at(
                        item.span(),
                        format!(
                            "モジュール `{}` で名前 `{name}` が重複しています",
                            module.path
                        ),
                    ));
                }
            }
        }
        declarations.insert(module.path.clone(), names);
    }

    // 公開再エクスポートの表。再エクスポートは再エクスポートを選べるので、
    // 増えなくなるまで回す。名前は足すだけなので循環 use があっても必ず止まる
    let mut publics: BTreeMap<ModulePath, BTreeMap<String, PublicMember>> = BTreeMap::new();
    loop {
        let mut additions = Vec::new();
        for module in modules.values() {
            for use_decl in module.uses.iter().filter(|use_decl| use_decl.public) {
                let target = ModulePath::from_parts(&use_decl.path);
                match &use_decl.members {
                    Some(members) => {
                        for member in members {
                            let alias = member.alias.as_deref().unwrap_or(&member.name);
                            if let Some(found) =
                                lookup_member(&target, &member.name, &declarations, &publics)
                            {
                                additions.push((module.path.clone(), alias.to_string(), found));
                            }
                        }
                    }
                    None if declarations.contains_key(&target) => {
                        let alias = use_decl.alias.as_deref().unwrap_or_else(|| target.name());
                        additions.push((
                            module.path.clone(),
                            alias.to_string(),
                            PublicMember::Module(target),
                        ));
                    }
                    None => {}
                }
            }
        }
        let mut changed = false;
        for (owner, name, member) in additions {
            let table = publics.entry(owner).or_default();
            if let std::collections::btree_map::Entry::Vacant(entry) = table.entry(name) {
                entry.insert(member);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }

    let mut scopes = BTreeMap::new();
    for module in modules.values() {
        let local = declarations.get(&module.path).cloned().unwrap_or_default();
        let mut imported_declarations = BTreeMap::new();
        let mut imported_modules = BTreeMap::new();

        for use_decl in &module.uses {
            let target = ModulePath::from_parts(&use_decl.path);
            if let Some(members) = &use_decl.members {
                for member in members {
                    let alias = member.alias.as_deref().unwrap_or(&member.name);
                    if local.contains_key(alias) {
                        diagnostics.push(Diag::at(
                            use_decl.span,
                            format!("import名 `{alias}` がローカル宣言と衝突しています"),
                        ));
                    } else if imported_declarations.contains_key(alias)
                        || imported_modules.contains_key(alias)
                    {
                        diagnostics.push(Diag::at(
                            use_decl.span,
                            format!("import名 `{alias}` が衝突しています"),
                        ));
                    } else {
                        match lookup_member(&target, &member.name, &declarations, &publics) {
                            Some(PublicMember::Module(child)) => {
                                imported_modules.insert(alias.to_string(), child);
                            }
                            Some(PublicMember::Decl(name)) => {
                                imported_declarations.insert(alias.to_string(), name);
                            }
                            // 再エクスポートの循環がどのモジュールにも行き着かない
                            // ときもここへ落ちる。公開宣言を捏造せず読み込みを止める
                            None => diagnostics.push(Diag::at(
                                use_decl.span,
                                format!(
                                    "モジュール `{target}` にメンバー `{}` がありません",
                                    member.name
                                ),
                            )),
                        }
                    }
                }
            } else if declarations.contains_key(&target) {
                let alias = use_decl.alias.as_deref().unwrap_or_else(|| target.name());
                if local.contains_key(alias) {
                    diagnostics.push(Diag::at(
                        use_decl.span,
                        format!("import名 `{alias}` がローカル宣言と衝突しています"),
                    ));
                } else if imported_declarations.contains_key(alias)
                    || imported_modules.contains_key(alias)
                {
                    diagnostics.push(Diag::at(
                        use_decl.span,
                        format!("import名 `{alias}` が衝突しています"),
                    ));
                } else {
                    imported_modules.insert(alias.to_string(), target);
                }
            }
        }
        scopes.insert(
            module.path.clone(),
            (local, imported_declarations, imported_modules),
        );
    }

    for module in modules.values() {
        let (local, imported_declarations, imported_modules) = &scopes[&module.path];
        for reference in declaration_references(&module.items) {
            let Some(first) = reference.first() else {
                continue;
            };
            if reference.len() >= 2
                && !local.contains_key(first)
                && !imported_declarations.contains_key(first)
                && !imported_modules.contains_key(first)
            {
                // 型注釈の参照には span が無い。同じ文言を式側が span 付きで
                // 出したときは、下の整列がそちらを残す
                diagnostics.push(Diag::msg(format!(
                    "モジュール名 `{first}` は `use` されていません"
                )));
            }
        }
    }

    if !diagnostics.is_empty() {
        sort_diagnostics(&mut diagnostics);
        return Err(LoadError {
            diagnostics,
            sources,
        });
    }

    // ホストへ出る候補はエントリーモジュールの明示選択だけ。ここで正準名まで
    // 落としておけば、後段は宣言の同一性だけを見ればよくなる
    let mut public_exports = Vec::new();
    if let Some(entry_module) = modules.get(entry_path) {
        for use_decl in entry_module.uses.iter().filter(|use_decl| use_decl.public) {
            let Some(members) = &use_decl.members else {
                continue;
            };
            let target = ModulePath::from_parts(&use_decl.path);
            for member in members {
                let alias = member.alias.as_deref().unwrap_or(&member.name);
                if let Some(PublicMember::Decl(canonical)) =
                    lookup_member(&target, &member.name, &declarations, &publics)
                {
                    public_exports.push(PublicExport {
                        name: alias.to_string(),
                        canonical,
                        span: use_decl.span,
                    });
                }
            }
        }
    }
    public_exports.sort_by(|a, b| a.name.cmp(&b.name));

    let mut items = Vec::new();
    for (path, mut module) in modules {
        let (local, imported_declarations, imported_modules) = &scopes[&path];
        for mut item in module.items.drain(..) {
            // ADR-0006 は複数モジュールの test 実行を決めない。エントリーの既存 test
            // だけを従来どおり実行し、依存モジュールの test は今回の Program に入れない。
            if path != *entry_path && matches!(item, Item::Test { .. }) {
                continue;
            }
            resolve_item(
                &mut item,
                local,
                imported_declarations,
                imported_modules,
                &declarations,
                &mut diagnostics,
            );
            items.push(item);
        }
    }

    sort_diagnostics(&mut diagnostics);
    if !diagnostics.is_empty() {
        return Err(LoadError {
            diagnostics,
            sources,
        });
    }

    Ok(LoadedProgram {
        program: Program {
            uses: Vec::new(),
            items,
        },
        entry: entry_path.qualified("main"),
        sources,
        public_exports,
    })
}

/// その item がモジュールの宣言名前空間へ出す名前。
///
/// enum だけが複数出す。enum 名と各 variant 名が同じ表に並ぶので、
/// `Gold` は struct や fn と同じ規則で解決・衝突・import される(design.md 決定2)。
/// 同じ variant の重複は呼び出し側が別に報告するため、ここでは畳んでおく。
pub fn item_names(item: &Item) -> Vec<&str> {
    match item {
        Item::Trait { name, .. } | Item::Struct { name, .. } => vec![name],
        Item::Enum { name, variants, .. } => {
            let mut names = vec![name.as_str()];
            let mut seen = BTreeSet::new();
            for variant in variants {
                if seen.insert(variant.name.as_str()) {
                    names.push(variant.name.as_str());
                }
            }
            names
        }
        Item::Effect { slot, .. } => vec![slot],
        Item::Fn { sig, .. } => vec![&sig.name],
        Item::Impl { .. } | Item::Test { .. } => Vec::new(),
    }
}

/// arm pattern が本体へ導入する名前。`_` は名前を作らないので何も足さない。
/// 名前を locals へ入れることで、同名の宣言を隠す規則が既存の走査に乗る。
fn bind_pattern(bindings: &[PatternBinding], locals: &mut BTreeSet<String>) {
    for binding in bindings {
        if let PatternBinding::Bind(name) = binding {
            locals.insert(name.clone());
        }
    }
}

/// 正準名の末尾。`main::Gold` → `Gold`。
///
/// variant は所属 enum と同じモジュールで宣言されるので、enum の正準名と短い
/// variant 名の組だけで variant を一意に指せる。限定参照 `Rank::Gold` の照合は
/// この形で行い、所属の表を別に持たない(下のテストが不変条件を固定している)。
pub fn short_name(name: &str) -> &str {
    name.rsplit_once("::").map_or(name, |(_, short)| short)
}

fn resolve_item(
    item: &mut Item,
    local: &BTreeMap<String, String>,
    imported_declarations: &BTreeMap<String, String>,
    imported_modules: &BTreeMap<String, ModulePath>,
    declarations: &BTreeMap<ModulePath, BTreeMap<String, String>>,
    diagnostics: &mut Vec<Diag>,
) {
    match item {
        Item::Trait { name, methods, .. } => {
            *name = local[name].clone();
            for sig in methods {
                resolve_sig_types(
                    sig,
                    local,
                    imported_declarations,
                    imported_modules,
                    declarations,
                );
            }
        }
        Item::Struct { name, fields, .. } => {
            *name = local[name].clone();
            for field in fields {
                resolve_type(
                    &mut field.ty,
                    local,
                    imported_declarations,
                    imported_modules,
                    declarations,
                );
            }
        }
        // payload の型は struct フィールドや関数引数と同じ正準化を通す
        // (design.md 決定4)
        Item::Enum { name, variants, .. } => {
            *name = local[name].clone();
            for variant in variants {
                variant.name = local[&variant.name].clone();
                for payload in &mut variant.payload {
                    resolve_type(
                        &mut payload.ty,
                        local,
                        imported_declarations,
                        imported_modules,
                        declarations,
                    );
                }
            }
        }
        Item::Impl {
            trait_name,
            type_name,
            methods,
            ..
        } => {
            if let Some(name) = trait_name {
                *name = resolve_name(
                    name,
                    local,
                    imported_declarations,
                    imported_modules,
                    declarations,
                );
            }
            *type_name = resolve_name(
                type_name,
                local,
                imported_declarations,
                imported_modules,
                declarations,
            );
            for (sig, body) in methods {
                resolve_sig_types(
                    sig,
                    local,
                    imported_declarations,
                    imported_modules,
                    declarations,
                );
                let mut locals: BTreeSet<String> =
                    sig.params.iter().map(|p| p.name.clone()).collect();
                if sig.has_self() {
                    locals.insert("self".to_string());
                }
                resolve_exprs(
                    body,
                    &mut locals,
                    local,
                    imported_declarations,
                    imported_modules,
                    declarations,
                    diagnostics,
                );
            }
        }
        Item::Effect {
            slot, trait_name, ..
        } => {
            *trait_name = resolve_name(
                trait_name,
                local,
                imported_declarations,
                imported_modules,
                declarations,
            );
            *slot = local[slot].clone();
        }
        Item::Fn { sig, body, .. } => {
            let original_name = sig.name.clone();
            resolve_sig_types(
                sig,
                local,
                imported_declarations,
                imported_modules,
                declarations,
            );
            sig.name = local[&original_name].clone();
            let mut locals = sig.params.iter().map(|p| p.name.clone()).collect();
            resolve_exprs(
                body,
                &mut locals,
                local,
                imported_declarations,
                imported_modules,
                declarations,
                diagnostics,
            );
        }
        Item::Test { body, .. } => {
            resolve_exprs(
                body,
                &mut BTreeSet::new(),
                local,
                imported_declarations,
                imported_modules,
                declarations,
                diagnostics,
            );
        }
    }
}

fn resolve_sig_types(
    sig: &mut Sig,
    local: &BTreeMap<String, String>,
    imported_declarations: &BTreeMap<String, String>,
    imported_modules: &BTreeMap<String, ModulePath>,
    declarations: &BTreeMap<ModulePath, BTreeMap<String, String>>,
) {
    for param in &mut sig.params {
        resolve_type(
            &mut param.ty,
            local,
            imported_declarations,
            imported_modules,
            declarations,
        );
    }
    if let Some(ret) = &mut sig.ret {
        resolve_type(
            ret,
            local,
            imported_declarations,
            imported_modules,
            declarations,
        );
    }
}

fn resolve_type(
    ty: &mut Type,
    local: &BTreeMap<String, String>,
    imported_declarations: &BTreeMap<String, String>,
    imported_modules: &BTreeMap<String, ModulePath>,
    declarations: &BTreeMap<ModulePath, BTreeMap<String, String>>,
) {
    // 正準化するのは名前の葉だけ。角括弧と後置 `?` は解決の対象ではない
    // (design.md 決定6)
    match &mut ty.kind {
        TypeKind::Named(name) => {
            *name = resolve_name(
                name,
                local,
                imported_declarations,
                imported_modules,
                declarations,
            );
        }
        TypeKind::Array(element) => resolve_type(
            element,
            local,
            imported_declarations,
            imported_modules,
            declarations,
        ),
    }
}

#[allow(clippy::too_many_arguments)]
fn resolve_exprs(
    body: &mut [Expr],
    locals: &mut BTreeSet<String>,
    local: &BTreeMap<String, String>,
    imported_declarations: &BTreeMap<String, String>,
    imported_modules: &BTreeMap<String, ModulePath>,
    declarations: &BTreeMap<ModulePath, BTreeMap<String, String>>,
    diagnostics: &mut Vec<Diag>,
) {
    for expr in body {
        resolve_expr(
            expr,
            locals,
            local,
            imported_declarations,
            imported_modules,
            declarations,
            diagnostics,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn resolve_expr(
    expr: &mut Expr,
    locals: &mut BTreeSet<String>,
    local: &BTreeMap<String, String>,
    imported_declarations: &BTreeMap<String, String>,
    imported_modules: &BTreeMap<String, ModulePath>,
    declarations: &BTreeMap<ModulePath, BTreeMap<String, String>>,
    diagnostics: &mut Vec<Diag>,
) {
    match &mut expr.kind {
        ExprKind::Ident(name) if !locals.contains(name) => {
            *name = resolve_name(
                name,
                local,
                imported_declarations,
                imported_modules,
                declarations,
            );
        }
        ExprKind::Path(parts) => {
            if parts.first().is_some_and(|first| locals.contains(first)) {
                return;
            }
            let original = parts.clone();
            let resolved = resolve_parts(
                parts,
                local,
                imported_declarations,
                imported_modules,
                declarations,
            );
            if resolved == original
                && original.len() >= 2
                && !locals.contains(&original[0])
                && !local.contains_key(&original[0])
                && !imported_declarations.contains_key(&original[0])
                && !imported_modules.contains_key(&original[0])
            {
                diagnostics.push(Diag::at(
                    expr.span,
                    format!("モジュール名 `{}` は `use` されていません", original[0]),
                ));
            }
            expr.kind = if resolved.len() == 1 {
                ExprKind::Ident(resolved[0].clone())
            } else {
                ExprKind::Path(resolved)
            };
        }
        ExprKind::Field(recv, _) | ExprKind::OptionalField(recv, _) => resolve_expr(
            recv,
            locals,
            local,
            imported_declarations,
            imported_modules,
            declarations,
            diagnostics,
        ),
        ExprKind::Call(callee, args) => {
            resolve_expr(
                callee,
                locals,
                local,
                imported_declarations,
                imported_modules,
                declarations,
                diagnostics,
            );
            resolve_exprs(
                args,
                locals,
                local,
                imported_declarations,
                imported_modules,
                declarations,
                diagnostics,
            );
        }
        ExprKind::Array(items) | ExprKind::Block(items) => resolve_exprs(
            items,
            locals,
            local,
            imported_declarations,
            imported_modules,
            declarations,
            diagnostics,
        ),
        ExprKind::StructLit { name, fields } => {
            if !name
                .split("::")
                .next()
                .is_some_and(|first| locals.contains(first))
            {
                *name = resolve_name(
                    name,
                    local,
                    imported_declarations,
                    imported_modules,
                    declarations,
                );
            }
            for (_, value) in fields {
                resolve_expr(
                    value,
                    locals,
                    local,
                    imported_declarations,
                    imported_modules,
                    declarations,
                    diagnostics,
                );
            }
        }
        ExprKind::Let {
            name,
            annotation,
            value,
            ..
        } => {
            // 注釈の名前の葉は引数・フィールド・戻り値と同じ規則で正準化する
            if let Some(annotation) = annotation {
                resolve_type(
                    annotation,
                    local,
                    imported_declarations,
                    imported_modules,
                    declarations,
                );
            }
            resolve_expr(
                value,
                locals,
                local,
                imported_declarations,
                imported_modules,
                declarations,
                diagnostics,
            );
            locals.insert(name.clone());
        }
        ExprKind::Assign { target, value }
        | ExprKind::Binary {
            lhs: target,
            rhs: value,
            ..
        } => {
            resolve_expr(
                target,
                locals,
                local,
                imported_declarations,
                imported_modules,
                declarations,
                diagnostics,
            );
            resolve_expr(
                value,
                locals,
                local,
                imported_declarations,
                imported_modules,
                declarations,
                diagnostics,
            );
        }
        ExprKind::Unary(_, inner)
        | ExprKind::Assert(inner)
        | ExprKind::Access { place: inner, .. } => resolve_expr(
            inner,
            locals,
            local,
            imported_declarations,
            imported_modules,
            declarations,
            diagnostics,
        ),
        ExprKind::Return(Some(value)) => resolve_expr(
            value,
            locals,
            local,
            imported_declarations,
            imported_modules,
            declarations,
            diagnostics,
        ),
        // payload の束縛も本体の `let` も他の arm や後続へ漏らさないために、
        // それぞれ外側 locals の複製で解決する(design.md 決定4・5)
        ExprKind::Match { subject, arms } => {
            resolve_expr(
                subject,
                locals,
                local,
                imported_declarations,
                imported_modules,
                declarations,
                diagnostics,
            );
            for arm in arms {
                let mut arm_locals = locals.clone();
                // 正準化と payload 束縛は限定 variant にだけある。`_` は素通り
                if let MatchPattern::Variant {
                    enum_name,
                    bindings,
                    ..
                } = &mut arm.pattern
                {
                    if !enum_name
                        .split("::")
                        .next()
                        .is_some_and(|first| locals.contains(first))
                    {
                        *enum_name = resolve_name(
                            enum_name,
                            local,
                            imported_declarations,
                            imported_modules,
                            declarations,
                        );
                    }
                    bind_pattern(bindings, &mut arm_locals);
                }
                // guard も本体も同じ payload スコープで見るが、片方で増えた
                // 束縛をもう片方へ漏らさないよう複製で入る(design.md 決定3)
                if let Some(guard) = &mut arm.guard {
                    resolve_expr(
                        guard,
                        &mut arm_locals.clone(),
                        local,
                        imported_declarations,
                        imported_modules,
                        declarations,
                        diagnostics,
                    );
                }
                resolve_expr(
                    &mut arm.body,
                    &mut arm_locals,
                    local,
                    imported_declarations,
                    imported_modules,
                    declarations,
                    diagnostics,
                );
            }
        }
        ExprKind::Head { head, body, orelse } => {
            match head {
                Head::Ambient(provisions) => {
                    for provision in provisions.iter_mut() {
                        if let Provision::Value { value, .. } = provision {
                            resolve_expr(
                                value,
                                locals,
                                local,
                                imported_declarations,
                                imported_modules,
                                declarations,
                                diagnostics,
                            );
                        }
                    }
                    let mut inner_locals = locals.clone();
                    for provision in provisions {
                        let original_slot = provision.slot().to_string();
                        match provision {
                            Provision::Type { slot, type_name } => {
                                if !type_name
                                    .split("::")
                                    .next()
                                    .is_some_and(|first| locals.contains(first))
                                {
                                    *type_name = resolve_name(
                                        type_name,
                                        local,
                                        imported_declarations,
                                        imported_modules,
                                        declarations,
                                    );
                                }
                                *slot = resolve_name(
                                    slot,
                                    local,
                                    imported_declarations,
                                    imported_modules,
                                    declarations,
                                );
                            }
                            Provision::Value { slot, .. } => {
                                *slot = resolve_name(
                                    slot,
                                    local,
                                    imported_declarations,
                                    imported_modules,
                                    declarations,
                                );
                            }
                        }
                        inner_locals.remove(&original_slot);
                    }
                    resolve_expr(
                        body,
                        &mut inner_locals,
                        local,
                        imported_declarations,
                        imported_modules,
                        declarations,
                        diagnostics,
                    );
                }
                Head::If(condition) | Head::Elif(condition) | Head::While(condition) => {
                    resolve_expr(
                        condition,
                        locals,
                        local,
                        imported_declarations,
                        imported_modules,
                        declarations,
                        diagnostics,
                    );
                    resolve_expr(
                        body,
                        &mut locals.clone(),
                        local,
                        imported_declarations,
                        imported_modules,
                        declarations,
                        diagnostics,
                    );
                }
                Head::For { var, iter } => {
                    resolve_expr(
                        iter,
                        locals,
                        local,
                        imported_declarations,
                        imported_modules,
                        declarations,
                        diagnostics,
                    );
                    let mut inner_locals = locals.clone();
                    inner_locals.insert(var.clone());
                    resolve_expr(
                        body,
                        &mut inner_locals,
                        local,
                        imported_declarations,
                        imported_modules,
                        declarations,
                        diagnostics,
                    );
                }
                Head::Else => resolve_expr(
                    body,
                    &mut locals.clone(),
                    local,
                    imported_declarations,
                    imported_modules,
                    declarations,
                    diagnostics,
                ),
            }
            if let Some(orelse) = orelse {
                resolve_expr(
                    orelse,
                    &mut locals.clone(),
                    local,
                    imported_declarations,
                    imported_modules,
                    declarations,
                    diagnostics,
                );
            }
        }
        ExprKind::Int(_)
        | ExprKind::Str(_)
        | ExprKind::Bool(_)
        | ExprKind::Nil
        | ExprKind::Ident(_)
        | ExprKind::Return(None) => {}
    }
}

fn resolve_name(
    name: &str,
    local: &BTreeMap<String, String>,
    imported_declarations: &BTreeMap<String, String>,
    imported_modules: &BTreeMap<String, ModulePath>,
    declarations: &BTreeMap<ModulePath, BTreeMap<String, String>>,
) -> String {
    let parts: Vec<String> = name.split("::").map(str::to_string).collect();
    let resolved = resolve_parts(
        &parts,
        local,
        imported_declarations,
        imported_modules,
        declarations,
    );
    resolved.join("::")
}

fn resolve_parts(
    parts: &[String],
    local: &BTreeMap<String, String>,
    imported_declarations: &BTreeMap<String, String>,
    imported_modules: &BTreeMap<String, ModulePath>,
    declarations: &BTreeMap<ModulePath, BTreeMap<String, String>>,
) -> Vec<String> {
    let Some(first) = parts.first() else {
        return Vec::new();
    };
    if let Some(name) = local
        .get(first)
        .or_else(|| imported_declarations.get(first))
    {
        let mut resolved = vec![name.clone()];
        resolved.extend(parts.iter().skip(1).cloned());
        return resolved;
    }
    let Some(mut module) = imported_modules.get(first).cloned() else {
        return parts.to_vec();
    };

    let mut index = 1;
    while index < parts.len() {
        let child = module.child(&parts[index]);
        if declarations.contains_key(&child) {
            module = child;
            index += 1;
        } else {
            break;
        }
    }
    if let Some(member) = parts.get(index)
        && let Some(name) = declarations
            .get(&module)
            .and_then(|names| names.get(member))
    {
        let mut resolved = vec![name.clone()];
        resolved.extend(parts.iter().skip(index + 1).cloned());
        return resolved;
    }
    parts.to_vec()
}

fn valid_ident(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first.is_ascii_alphabetic() || first == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn module_references(items: &[Item]) -> Vec<Vec<String>> {
    let mut paths = declaration_references(items);
    for item in items {
        match item {
            Item::Impl { methods, .. } => {
                for (sig, body) in methods {
                    let mut locals: BTreeSet<String> =
                        sig.params.iter().map(|param| param.name.clone()).collect();
                    if sig.has_self() {
                        locals.insert("self".to_string());
                    }
                    collect_expr_paths(body, &mut locals, &mut paths);
                }
            }
            Item::Fn { sig, body, .. } => {
                let mut locals = sig.params.iter().map(|param| param.name.clone()).collect();
                collect_expr_paths(body, &mut locals, &mut paths);
            }
            Item::Test { body, .. } => {
                collect_expr_paths(body, &mut BTreeSet::new(), &mut paths);
            }
            Item::Trait { .. } | Item::Struct { .. } | Item::Enum { .. } | Item::Effect { .. } => {}
        }
    }
    paths
}

fn declaration_references(items: &[Item]) -> Vec<Vec<String>> {
    let mut paths = Vec::new();
    for item in items {
        match item {
            Item::Trait { methods, .. } => {
                for sig in methods {
                    collect_sig_paths(sig, &mut paths);
                }
            }
            Item::Struct { fields, .. } => {
                for field in fields {
                    collect_type_paths(&field.ty, &mut paths);
                }
            }
            Item::Impl {
                trait_name,
                type_name,
                methods,
                ..
            } => {
                if let Some(trait_name) = trait_name {
                    collect_name_path(trait_name, &mut paths);
                }
                collect_name_path(type_name, &mut paths);
                for (sig, _) in methods {
                    collect_sig_paths(sig, &mut paths);
                }
            }
            Item::Effect { trait_name, .. } => collect_name_path(trait_name, &mut paths),
            Item::Fn { sig, .. } => collect_sig_paths(sig, &mut paths),
            // enum が参照するのは variant payload の型だけ
            Item::Enum { variants, .. } => {
                for variant in variants {
                    for payload in &variant.payload {
                        collect_type_paths(&payload.ty, &mut paths);
                    }
                }
            }
            Item::Test { .. } => {}
        }
    }
    paths
}

fn collect_sig_paths(sig: &Sig, paths: &mut Vec<Vec<String>>) {
    for param in &sig.params {
        collect_type_paths(&param.ty, paths);
    }
    if let Some(ret) = &sig.ret {
        collect_type_paths(ret, paths);
    }
}

/// 型木の葉にある名前だけがモジュール参照になりうる。
fn collect_type_paths(ty: &Type, paths: &mut Vec<Vec<String>>) {
    match &ty.kind {
        TypeKind::Named(name) => collect_name_path(name, paths),
        TypeKind::Array(element) => collect_type_paths(element, paths),
    }
}

fn collect_name_path(name: &str, paths: &mut Vec<Vec<String>>) {
    let parts: Vec<String> = name.split("::").map(str::to_string).collect();
    if parts.len() > 1 {
        paths.push(parts);
    }
}

fn collect_expr_paths(body: &[Expr], locals: &mut BTreeSet<String>, paths: &mut Vec<Vec<String>>) {
    for expr in body {
        match &expr.kind {
            ExprKind::Path(parts) if !parts.first().is_some_and(|first| locals.contains(first)) => {
                paths.push(parts.clone());
            }
            ExprKind::Field(recv, _)
            | ExprKind::OptionalField(recv, _)
            | ExprKind::Unary(_, recv)
            | ExprKind::Assert(recv)
            | ExprKind::Access { place: recv, .. } => {
                collect_expr_paths(std::slice::from_ref(recv), locals, paths);
            }
            ExprKind::Call(callee, args) => {
                collect_expr_paths(std::slice::from_ref(callee), locals, paths);
                collect_expr_paths(args, locals, paths);
            }
            ExprKind::Array(items) | ExprKind::Block(items) => {
                collect_expr_paths(items, locals, paths);
            }
            ExprKind::StructLit { name, fields } => {
                if !name
                    .split("::")
                    .next()
                    .is_some_and(|first| locals.contains(first))
                {
                    collect_name_path(name, paths);
                }
                for (_, value) in fields {
                    collect_expr_paths(std::slice::from_ref(value), locals, paths);
                }
            }
            ExprKind::Let {
                name,
                annotation,
                value,
                ..
            } => {
                // 注釈が他モジュールの型を名乗るなら、その参照も収集する
                if let Some(annotation) = annotation {
                    collect_type_paths(annotation, paths);
                }
                collect_expr_paths(std::slice::from_ref(value), locals, paths);
                locals.insert(name.clone());
            }
            ExprKind::Assign { target, value }
            | ExprKind::Binary {
                lhs: target,
                rhs: value,
                ..
            } => {
                collect_expr_paths(std::slice::from_ref(target), locals, paths);
                collect_expr_paths(std::slice::from_ref(value), locals, paths);
            }
            ExprKind::Return(Some(value)) => {
                collect_expr_paths(std::slice::from_ref(value), locals, paths);
            }
            ExprKind::Match { subject, arms } => {
                collect_expr_paths(std::slice::from_ref(subject), locals, paths);
                for arm in arms {
                    let mut arm_locals = locals.clone();
                    if let MatchPattern::Variant {
                        enum_name,
                        bindings,
                        ..
                    } = &arm.pattern
                    {
                        if !enum_name
                            .split("::")
                            .next()
                            .is_some_and(|first| locals.contains(first))
                        {
                            collect_name_path(enum_name, paths);
                        }
                        bind_pattern(bindings, &mut arm_locals);
                    }
                    if let Some(guard) = &arm.guard {
                        collect_expr_paths(
                            std::slice::from_ref(&**guard),
                            &mut arm_locals.clone(),
                            paths,
                        );
                    }
                    collect_expr_paths(std::slice::from_ref(&arm.body), &mut arm_locals, paths);
                }
            }
            ExprKind::Head { head, body, orelse } => {
                match head {
                    Head::If(condition) | Head::Elif(condition) | Head::While(condition) => {
                        collect_expr_paths(std::slice::from_ref(condition), locals, paths);
                        collect_expr_paths(std::slice::from_ref(body), &mut locals.clone(), paths);
                    }
                    Head::For { var, iter } => {
                        collect_expr_paths(std::slice::from_ref(iter), locals, paths);
                        let mut inner_locals = locals.clone();
                        inner_locals.insert(var.clone());
                        collect_expr_paths(std::slice::from_ref(body), &mut inner_locals, paths);
                    }
                    Head::Ambient(provisions) => {
                        for provision in provisions {
                            match provision {
                                Provision::Type { slot, type_name } => {
                                    collect_name_path(slot, paths);
                                    if !type_name
                                        .split("::")
                                        .next()
                                        .is_some_and(|first| locals.contains(first))
                                    {
                                        collect_name_path(type_name, paths);
                                    }
                                }
                                Provision::Value { slot, value } => {
                                    collect_name_path(slot, paths);
                                    collect_expr_paths(std::slice::from_ref(value), locals, paths);
                                }
                            }
                        }
                        let mut inner_locals = locals.clone();
                        for provision in provisions {
                            inner_locals.remove(provision.slot());
                        }
                        collect_expr_paths(std::slice::from_ref(body), &mut inner_locals, paths);
                    }
                    Head::Else => {
                        collect_expr_paths(std::slice::from_ref(body), &mut locals.clone(), paths)
                    }
                }
                if let Some(orelse) = orelse {
                    collect_expr_paths(std::slice::from_ref(orelse), &mut locals.clone(), paths);
                }
            }
            ExprKind::Int(_)
            | ExprKind::Str(_)
            | ExprKind::Bool(_)
            | ExprKind::Nil
            | ExprKind::Ident(_)
            | ExprKind::Path(_)
            | ExprKind::Return(None) => {}
        }
    }
}

/// 一時ディレクトリに書いて `main.rd` を読み込み、後片付けまでやる。
/// 複数モジュールを畳んだ `Program` を要るテストはどの段からもここを使う。
#[cfg(test)]
pub fn load_files(files: &[(&str, &str)]) -> Result<LoadedProgram, LoadError> {
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let number = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!(
        "rhodolite-module-unit-{}-{number}",
        std::process::id()
    ));
    std::fs::create_dir_all(&root).unwrap();
    for (name, source) in files {
        let path = root.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, source).unwrap();
    }
    let loaded = load(&root.join("main.rd"));
    std::fs::remove_dir_all(&root).unwrap();
    loaded
}

#[cfg(test)]
mod tests {
    use super::load_files;
    use super::*;
    use crate::lex::{join, lex};
    use crate::parse;

    /// ロード後の不変条件(design.md 決定2)。
    ///
    /// variant は所属 enum と同じモジュールで宣言されるので、variant の正準名の
    /// モジュール接頭辞に enum の短い名前を繋げば、enum の正準名がそのまま出る。
    /// 所属を別の表に持たなくても、名前だけから辿れる状態を固定する。
    #[test]
    fn variantの正準名から所属enumの正準名を再構成できる() {
        let loaded = load(Path::new("examples/canonical.rd")).expect("正典はロードできる");

        let mut checked = 0;
        for item in &loaded.program.items {
            let Item::Enum { name, variants, .. } = item else {
                continue;
            };
            let (_, enum_short) = name.rsplit_once("::").expect("enum は正準名を持つ");
            for variant in variants {
                let (variant_module, _) = variant
                    .name
                    .rsplit_once("::")
                    .expect("variant は正準名を持つ");
                assert_eq!(&format!("{variant_module}::{enum_short}"), name);
                checked += 1;
            }
        }
        assert_eq!(checked, 2, "正典の `Rank` は variant を2つ持つ");
    }

    /// 局所注釈の名前の葉も、引数や戻り値と同じ規則で正準名になる。
    /// import した型を注釈に書けないと `let u: User? = nil` が他モジュールの
    /// 型に使えない
    #[test]
    fn letの型注釈も正準名へ解決する() {
        let loaded = load_files(&[
            (
                "main.rd",
                "use dep::{User}\nfn main() {\n  let u: User? = nil\n  let us: [User] = []\n}\n",
            ),
            ("dep.rd", "struct User { name: str }\n"),
        ])
        .expect("ロードできる");

        let annotations: Vec<String> = loaded
            .program
            .items
            .iter()
            .filter_map(|item| match item {
                Item::Fn { sig, body, .. } if short_name(&sig.name) == "main" => Some(body),
                _ => None,
            })
            .flatten()
            .filter_map(|e| match &e.kind {
                ExprKind::Let { annotation, .. } => annotation.as_ref(),
                _ => None,
            })
            .map(|ty| format!("{:?}", ty.kind))
            .collect();

        // どちらの注釈も `dep::User` を指す。配列の内側も同じ
        assert_eq!(annotations.len(), 2, "{annotations:?}");
        for shown in &annotations {
            assert!(shown.contains("dep::User"), "{shown}");
        }
    }

    /// 畳んだ後の span 単体から元のファイルへ戻れること(design.md 決定1・2)。
    /// バイト範囲は他のファイルとぶつかるので、`src` が無いと引けない。
    #[test]
    fn 畳んだ後もspanは元のファイルへ解決する() {
        let loaded = load_files(&[
            ("main.rd", "use dep\nfn main() { dep::value() }\n"),
            ("dep.rd", "fn value() { 1 }\n"),
        ])
        .expect("ロードできる");

        let mut seen = BTreeMap::new();
        for item in &loaded.program.items {
            let span = item.span();
            let source = &loaded.sources[span.src as usize];
            let text = &source.text[span.start as usize..span.end as usize];
            seen.insert(
                source
                    .path
                    .file_name()
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .to_string(),
                text.to_string(),
            );
        }
        assert_eq!(seen["main.rd"], "fn main() { dep::value() }");
        assert_eq!(seen["dep.rd"], "fn value() { 1 }");
    }

    /// 実行時の失敗も同じ表で引ける。呼び出し元は `main.rd` にいるが、指すのは
    /// 失敗した式を持つ `dep.rd` のほう。
    #[test]
    fn 呼び出し先の実行時エラーは失敗した式のモジュールを指す() {
        let loaded = load_files(&[
            ("main.rd", "use dep\nfn main() { dep::boom() }\n"),
            ("dep.rd", "fn boom() { assert false }\n"),
        ])
        .expect("ロードできる");

        let hir = crate::typecheck::check_and_lower(&loaded.program).expect("型検査を通る");
        let checked = crate::ownership::check(hir).expect("所有権検査を通る");
        let Err(crate::eval::Flow::Error(diagnostic)) =
            crate::eval::Interp::new_checked(&checked).run(&loaded.entry)
        else {
            panic!("`assert false` は失敗するはず");
        };
        let span = diagnostic.span.expect("失敗した式を指す");
        let source = &loaded.sources[span.src as usize];
        assert_eq!(source.path.file_name().unwrap(), "dep.rd");
        assert_eq!(
            &source.text[span.start as usize..span.end as usize],
            "assert false"
        );
    }

    #[test]
    fn 見つからないモジュールはuse宣言を指す() {
        let source = "use missing\nfn main() { 1 }\n";
        let failure = load_files(&[("main.rd", source)]).expect_err("`missing` は見つからない");
        let diagnostic = &failure.diagnostics[0];
        assert_eq!(diagnostic.msg, "モジュール `missing` が見つかりません");
        let span = diagnostic.span.expect("use 宣言を指す");
        assert_eq!(
            &source[span.start as usize..span.end as usize],
            "use missing"
        );
    }

    /// ソースへ届く前に失敗した読み込みは指すものが無い。
    #[test]
    fn エントリーが読めないときはspanを持たない() {
        let failure = load(Path::new("no-such-directory/main.rd")).expect_err("読めない");
        assert!(
            failure.diagnostics.iter().all(|d| d.span.is_none()),
            "{failure:?}"
        );
    }

    #[test]
    fn optional_fieldのレシーバも正準名へ解決する() {
        let mut program =
            parse::parse(&join(lex("fn main() { dep::make().?value }\n").unwrap())).unwrap();
        let Item::Fn { body, .. } = &mut program.items[0] else {
            panic!()
        };

        let module = ModulePath(vec!["dep".to_string()]);
        let imported_modules = BTreeMap::from([("dep".to_string(), module.clone())]);
        let declarations = BTreeMap::from([(
            module,
            BTreeMap::from([("make".to_string(), "dep::make".to_string())]),
        )]);
        let mut diagnostics = Vec::new();
        resolve_expr(
            &mut body[0],
            &mut BTreeSet::new(),
            &BTreeMap::new(),
            &BTreeMap::new(),
            &imported_modules,
            &declarations,
            &mut diagnostics,
        );

        let ExprKind::OptionalField(recv, field) = &body[0].kind else {
            panic!("optional field ではない: {:?}", body[0].kind)
        };
        let ExprKind::Call(callee, _) = &recv.kind else {
            panic!("receiver が call ではない: {:?}", recv.kind)
        };
        assert!(matches!(&callee.kind, ExprKind::Ident(name) if name == "dep::make"));
        assert_eq!(field, "value");
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
    }

    /// match の arm の enum も、他の名前と同じ規則で正準名になる。
    /// variant は短いままで、`short_name` と組で宣言を一意に指す
    #[test]
    fn matchのarmと本体を正準名へ解決する() {
        let mut program = parse::parse(&join(
            lex("fn main(r: Rank) {\n\
                 \x20 match r {\n\
                 \x20   Rank::Bronze: local()\n\
                 \x20   dep::Grade::Low { let shadowed = 1\nshadowed }\n\
                 \x20 }\n\
                 \x20 shadowed\n\
                 }\n")
            .unwrap(),
        ))
        .unwrap();
        let Item::Fn { body, .. } = &mut program.items[0] else {
            panic!()
        };

        let module = ModulePath(vec!["dep".to_string()]);
        let local = BTreeMap::from([
            ("Rank".to_string(), "main::Rank".to_string()),
            ("local".to_string(), "main::local".to_string()),
        ]);
        let imported_modules = BTreeMap::from([("dep".to_string(), module.clone())]);
        let declarations = BTreeMap::from([(
            module,
            BTreeMap::from([("Grade".to_string(), "dep::Grade".to_string())]),
        )]);
        let mut diagnostics = Vec::new();
        let mut locals = BTreeSet::from(["r".to_string()]);
        resolve_exprs(
            body,
            &mut locals,
            &local,
            &BTreeMap::new(),
            &imported_modules,
            &declarations,
            &mut diagnostics,
        );

        let ExprKind::Match { subject, arms } = &body[0].kind else {
            panic!("match ではない: {:?}", body[0].kind)
        };
        assert!(matches!(&subject.kind, ExprKind::Ident(name) if name == "r"));
        assert_eq!(arms[0].pattern.label(), "main::Rank::Bronze");
        assert_eq!(arms[1].pattern.label(), "dep::Grade::Low");
        assert!(diagnostics.is_empty(), "{diagnostics:?}");

        let ExprKind::Call(callee, _) = &arms[0].body.kind else {
            panic!("arm 本体が呼び出しではない: {:?}", arms[0].body.kind)
        };
        assert!(matches!(&callee.kind, ExprKind::Ident(name) if name == "main::local"));

        // arm 内の束縛は後続へ漏れないので、`shadowed` は宣言として解決を試みる
        assert!(
            matches!(&body[1].kind, ExprKind::Ident(name) if name == "shadowed"),
            "{:?}",
            body[1].kind
        );
        assert!(!locals.contains("shadowed"));
    }

    /// payload の名前は他の名前を隠すが、その arm の本体の間だけ。
    /// 隣の arm にも match の後にも漏らさない(design.md 決定5)
    #[test]
    fn armのpayload束縛はその本体の間だけ宣言を隠す() {
        let mut program = parse::parse(&join(
            lex("fn main(l: Lookup) {\n\
                 \x20 match l {\n\
                 \x20   Lookup::Found(local): local()\n\
                 \x20   Lookup::Missing(_): local()\n\
                 \x20 }\n\
                 \x20 local\n\
                 }\n")
            .unwrap(),
        ))
        .unwrap();
        let Item::Fn { body, .. } = &mut program.items[0] else {
            panic!()
        };

        let local = BTreeMap::from([
            ("Lookup".to_string(), "main::Lookup".to_string()),
            ("local".to_string(), "main::local".to_string()),
        ]);
        let mut diagnostics = Vec::new();
        let mut locals = BTreeSet::from(["l".to_string()]);
        resolve_exprs(
            body,
            &mut locals,
            &local,
            &BTreeMap::new(),
            &BTreeMap::new(),
            &BTreeMap::new(),
            &mut diagnostics,
        );

        let ExprKind::Match { arms, .. } = &body[0].kind else {
            panic!("match ではない: {:?}", body[0].kind)
        };
        // 束縛した arm では payload が勝ち、`_` の arm では宣言のままになる
        let callee = |arm: &MatchArm| match &arm.body.kind {
            ExprKind::Call(callee, _) => match &callee.kind {
                ExprKind::Ident(name) => name.clone(),
                other => panic!("呼び出し先が裸の名前ではない: {other:?}"),
            },
            other => panic!("arm 本体が呼び出しではない: {other:?}"),
        };
        assert_eq!(callee(&arms[0]), "local");
        assert_eq!(callee(&arms[1]), "main::local");
        // match の後にも漏れない
        assert!(matches!(&body[1].kind, ExprKind::Ident(name) if name == "main::local"));
        assert!(!locals.contains("local"));
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
    }

    /// guard は本体と同じ payload スコープで解決される。payload に隠されない
    /// 名前は輸入宣言として正準名になる(design.md 決定3)
    #[test]
    fn armのguardも本体と同じスコープで解決する() {
        let mut program = parse::parse(&join(
            lex("fn main(l: Lookup) {\n\
                 \x20 match l {\n\
                 \x20   Lookup::Found(local) if local == ready: 1\n\
                 \x20   Lookup::Missing(_) if local == ready: 2\n\
                 \x20   _: 0\n\
                 \x20 }\n\
                 }\n")
            .unwrap(),
        ))
        .unwrap();
        let Item::Fn { body, .. } = &mut program.items[0] else {
            panic!()
        };

        let local = BTreeMap::from([
            ("Lookup".to_string(), "main::Lookup".to_string()),
            ("local".to_string(), "main::local".to_string()),
            ("ready".to_string(), "main::ready".to_string()),
        ]);
        let mut diagnostics = Vec::new();
        let mut locals = BTreeSet::from(["l".to_string()]);
        resolve_exprs(
            body,
            &mut locals,
            &local,
            &BTreeMap::new(),
            &BTreeMap::new(),
            &BTreeMap::new(),
            &mut diagnostics,
        );

        let ExprKind::Match { arms, .. } = &body[0].kind else {
            panic!("match ではない: {:?}", body[0].kind)
        };
        let lhs = |arm: &MatchArm| {
            let ExprKind::Binary { lhs, rhs, .. } = &arm.guard.as_ref().unwrap().kind else {
                panic!("guard が二項式ではない")
            };
            let (ExprKind::Ident(l), ExprKind::Ident(r)) = (&lhs.kind, &rhs.kind) else {
                panic!("guard の両辺が裸の名前ではない")
            };
            // 隠されない `ready` はどちらの arm でも宣言として解決される
            assert_eq!(r, "main::ready");
            l.clone()
        };
        // payload が同名の宣言を隠すのは束縛した arm の guard の中だけ
        assert_eq!(lhs(&arms[0]), "local");
        assert_eq!(lhs(&arms[1]), "main::local");
        assert!(!locals.contains("local"));
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
    }

    /// `_` の arm は正準化する enum パスを持たないが、本体は他の arm と同じに
    /// 辿る。輸入モジュールの名前も本体の中で解決される
    #[test]
    fn catch_all_armの本体も正準名へ解決する() {
        let mut program = parse::parse(&join(
            lex("fn main(r: Rank) {\n\
                 \x20 match r {\n\
                 \x20   Rank::Bronze: local()\n\
                 \x20   _: dep::audit()\n\
                 \x20 }\n\
                 }\n")
            .unwrap(),
        ))
        .unwrap();
        let Item::Fn { body, .. } = &mut program.items[0] else {
            panic!()
        };

        let module = ModulePath(vec!["dep".to_string()]);
        let local = BTreeMap::from([
            ("Rank".to_string(), "main::Rank".to_string()),
            ("local".to_string(), "main::local".to_string()),
        ]);
        let imported_modules = BTreeMap::from([("dep".to_string(), module.clone())]);
        let declarations = BTreeMap::from([(
            module,
            BTreeMap::from([("audit".to_string(), "dep::audit".to_string())]),
        )]);
        let mut diagnostics = Vec::new();
        resolve_exprs(
            body,
            &mut BTreeSet::from(["r".to_string()]),
            &local,
            &BTreeMap::new(),
            &imported_modules,
            &declarations,
            &mut diagnostics,
        );

        let ExprKind::Match { arms, .. } = &body[0].kind else {
            panic!("match ではない: {:?}", body[0].kind)
        };
        assert_eq!(arms[0].pattern.label(), "main::Rank::Bronze");
        assert_eq!(arms[1].pattern.label(), "_");
        let ExprKind::Call(callee, _) = &arms[1].body.kind else {
            panic!("arm 本体が呼び出しではない: {:?}", arms[1].body.kind)
        };
        assert!(matches!(&callee.kind, ExprKind::Ident(name) if name == "dep::audit"));
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
    }

    /// payload の型は struct フィールドと同じ規則で正準名になる
    #[test]
    fn payloadの型も正準名へ解決する() {
        let mut program = parse::parse(&join(
            lex("enum Lookup { Found(User, [dep::Row?]?) Skipped }\n").unwrap(),
        ))
        .unwrap();

        let module = ModulePath(vec!["dep".to_string()]);
        let local = BTreeMap::from([
            ("Lookup".to_string(), "main::Lookup".to_string()),
            ("Found".to_string(), "main::Found".to_string()),
            ("Skipped".to_string(), "main::Skipped".to_string()),
            ("User".to_string(), "main::User".to_string()),
        ]);
        let imported_modules = BTreeMap::from([("dep".to_string(), module.clone())]);
        let declarations = BTreeMap::from([(
            module,
            BTreeMap::from([("Row".to_string(), "dep::Row".to_string())]),
        )]);
        let mut diagnostics = Vec::new();
        resolve_item(
            &mut program.items[0],
            &local,
            &BTreeMap::new(),
            &imported_modules,
            &declarations,
            &mut diagnostics,
        );

        let Item::Enum { name, variants, .. } = &program.items[0] else {
            panic!()
        };
        assert_eq!(name, "main::Lookup");
        assert_eq!(variants[0].name, "main::Found");
        // ローカル宣言・import 経由・角括弧と後置 `?` の内側まで同じ規則で通る
        assert_eq!(variants[0].payload[0].ty.name(), Some("main::User"));
        let TypeKind::Array(element) = &variants[0].payload[1].ty.kind else {
            panic!("配列ではない: {:?}", variants[0].payload[1])
        };
        assert!(variants[0].payload[1].ty.optional);
        assert_eq!(element.name(), Some("dep::Row"));
        assert!(element.optional);
        // fieldless は payload 無しのまま
        assert_eq!(variants[1].name, "main::Skipped");
        assert!(variants[1].payload.is_empty());
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
    }

    /// 角括弧の内側の名前も、裸の名前と同じ規則で正準名になる
    #[test]
    fn 配列の要素型も正準名へ解決する() {
        let mut program = parse::parse(&join(
            lex("struct Store { users: [[User]?]\nowner: User }\n").unwrap(),
        ))
        .unwrap();
        let Item::Struct { fields, .. } = &mut program.items[0] else {
            panic!()
        };

        let local = BTreeMap::from([("User".to_string(), "main::User".to_string())]);
        for field in fields.iter_mut() {
            resolve_type(
                &mut field.ty,
                &local,
                &BTreeMap::new(),
                &BTreeMap::new(),
                &BTreeMap::new(),
            );
        }

        let TypeKind::Array(outer) = &fields[0].ty.kind else {
            panic!("配列ではない: {:?}", fields[0].ty)
        };
        let TypeKind::Array(inner) = &outer.kind else {
            panic!("入れ子の配列ではない: {outer:?}")
        };
        assert_eq!(inner.name(), Some("main::User"));
        assert!(outer.optional, "要素の後置 `?` は解決で失われない");
        assert_eq!(fields[1].ty.name(), Some("main::User"));
    }

    // -----------------------------------------------------------------------
    // 公開再エクスポート (`pub use`)
    // -----------------------------------------------------------------------

    /// 公開名の一覧。`pub use` が実際に何をホストへ出したかを1本で見る
    fn exported(loaded: &LoadedProgram) -> Vec<(&str, &str)> {
        loaded
            .public_exports
            .iter()
            .map(|export| (export.name.as_str(), export.canonical.as_str()))
            .collect()
    }

    fn load_err(files: &[(&str, &str)]) -> String {
        load_files(files)
            .expect_err("読み込みは失敗するはず")
            .diagnostics
            .into_iter()
            .map(|d| d.msg)
            .collect::<Vec<_>>()
            .join(" / ")
    }

    /// 普通の `use` は導入したモジュールの中だけ。下流からは選べない
    #[test]
    fn 普通のuseは再エクスポートしない() {
        let message = load_err(&[
            ("main.rd", "use mid::{find}\nfn main(-> int) { find() }\n"),
            ("mid.rd", "use users::{find}\n"),
            ("users.rd", "fn find(-> int) { 1 }\n"),
        ]);
        assert!(
            message.contains("メンバー `find` がありません"),
            "{message}"
        );
    }

    /// `pub use` を挟むと同じ選択が通る。違いは公開フラグ1つだけ
    #[test]
    fn pub_useした関数は下流から選べる() {
        let loaded = load_files(&[
            ("main.rd", "use mid::{find}\nfn main(-> int) { find() }\n"),
            ("mid.rd", "pub use users::{find}\n"),
            ("users.rd", "fn find(-> int) { 1 }\n"),
        ])
        .expect("読み込めるはず");
        // 再エクスポートを経ても宣言の同一性は変わらない
        assert!(
            loaded.program.items.iter().any(|item| matches!(
                item,
                Item::Fn { sig, .. } if sig.name == "users::find"
            )),
            "元の宣言のまま残る"
        );
    }

    /// 中継が何段あっても指すのは元の宣言。エントリーの公開名だけが変わる
    #[test]
    fn 再エクスポートは何段でも元の宣言を指す() {
        let loaded = load_files(&[
            (
                "main.rd",
                "pub use mid::{find as find_user}\nfn main(-> int) { find_user() }\n",
            ),
            ("mid.rd", "pub use users::{find}\n"),
            ("users.rd", "fn find(-> int) { 1 }\n"),
        ])
        .expect("読み込めるはず");
        assert_eq!(exported(&loaded), [("find_user", "users::find")]);
    }

    /// 別名は局所名と公開名の両方を変える。元の綴りは再エクスポートされない
    #[test]
    fn 別名が公開名になり元の名前は出ない() {
        let message = load_err(&[
            ("main.rd", "use mid::{find}\nfn main(-> int) { find() }\n"),
            ("mid.rd", "pub use users::{find as lookup}\n"),
            ("users.rd", "fn find(-> int) { 1 }\n"),
        ]);
        assert!(
            message.contains("メンバー `find` がありません"),
            "{message}"
        );
    }

    /// 型も再エクスポートできる。ただし公開名前空間に載るだけで関数にはならない
    #[test]
    fn 型の再エクスポートは公開名前空間だけに載る() {
        let loaded = load_files(&[
            (
                "main.rd",
                "pub use users::{User}\nfn main() { let u: User? = nil }\n",
            ),
            ("users.rd", "struct User { name: str }\n"),
        ])
        .expect("読み込めるはず");
        assert_eq!(exported(&loaded), [("User", "users::User")]);
    }

    /// モジュール形は名前空間を1つ足すだけ。配下の関数は公開面へ降りてこない
    #[test]
    fn モジュール形の再エクスポートは配下の関数を公開面に出さない() {
        let loaded = load_files(&[
            (
                "main.rd",
                "pub use users\nfn main(-> int) { users::find() }\n",
            ),
            ("users.rd", "fn find(-> int) { 1 }\n"),
        ])
        .expect("読み込めるはず");
        assert!(exported(&loaded).is_empty(), "{:?}", exported(&loaded));
    }

    /// 再エクスポートしたモジュールは下流から名前空間として選べる
    #[test]
    fn 再エクスポートしたモジュールは下流から選べる() {
        load_files(&[
            (
                "main.rd",
                "use mid::{users}\nfn main(-> int) { users::find() }\n",
            ),
            ("mid.rd", "pub use users\n"),
            ("users.rd", "fn find(-> int) { 1 }\n"),
        ])
        .expect("読み込めるはず");
    }

    /// 公開名も既存の1つの名前空間に入る。上書きではなく拒否する
    #[test]
    fn 公開名の衝突は拒否する() {
        let message = load_err(&[
            (
                "main.rd",
                "pub use left::{find}\npub use right::{find}\nfn main(-> int) { find() }\n",
            ),
            ("left.rd", "fn find(-> int) { 1 }\n"),
            ("right.rd", "fn find(-> int) { 2 }\n"),
        ]);
        assert!(message.contains("`find` が衝突しています"), "{message}");
    }

    #[test]
    fn 公開名とローカル宣言の衝突も拒否する() {
        let message = load_err(&[
            (
                "main.rd",
                "pub use users::{find}\nfn find(-> int) { 1 }\nfn main(-> int) { find() }\n",
            ),
            ("users.rd", "fn find(-> int) { 2 }\n"),
        ]);
        assert!(
            message.contains("`find` がローカル宣言と衝突しています"),
            "{message}"
        );
    }

    /// 別名で分ければ両方が別の公開メンバーとして通る
    #[test]
    fn 別名にすれば衝突しない() {
        let loaded = load_files(&[
            (
                "main.rd",
                "pub use left::{find as left_find}\n\
                 pub use right::{find as right_find}\n\
                 fn main(-> int) { left_find() + right_find() }\n",
            ),
            ("left.rd", "fn find(-> int) { 1 }\n"),
            ("right.rd", "fn find(-> int) { 2 }\n"),
        ])
        .expect("読み込めるはず");
        assert_eq!(
            exported(&loaded),
            [("left_find", "left::find"), ("right_find", "right::find")]
        );
    }

    /// 互いを指すだけの循環にはどこにも元の宣言が無い。公開宣言を捏造せず止める
    #[test]
    fn 元の宣言が無い再エクスポートの循環は失敗する() {
        let message = load_err(&[
            ("main.rd", "use a::{find}\nfn main(-> int) { find() }\n"),
            ("a.rd", "pub use b::{find}\n"),
            ("b.rd", "pub use a::{find}\n"),
        ]);
        assert!(
            message.contains("メンバー `find` がありません"),
            "{message}"
        );
    }

    /// 循環 use そのものは従来どおり通る。元の宣言が1つでもあれば解決する
    #[test]
    fn 循環していても元の宣言があれば解決する() {
        let loaded = load_files(&[
            ("main.rd", "pub use a::{find}\nfn main(-> int) { find() }\n"),
            ("a.rd", "pub use b::{find}\n"),
            ("b.rd", "use a\nfn find(-> int) { 1 }\n"),
        ])
        .expect("読み込めるはず");
        assert_eq!(exported(&loaded), [("find", "b::find")]);
    }

    /// 公開名の並びは宣言順ではなく公開名の昇順。決定的な ABI 順の土台になる
    #[test]
    fn 公開名は昇順で並ぶ() {
        let loaded = load_files(&[
            (
                "main.rd",
                "pub use users::{zebra, alpha}\nfn main(-> int) { zebra() + alpha() }\n",
            ),
            (
                "users.rd",
                "fn zebra(-> int) { 1 }\nfn alpha(-> int) { 2 }\n",
            ),
        ])
        .expect("読み込めるはず");
        assert_eq!(
            exported(&loaded),
            [("alpha", "users::alpha"), ("zebra", "users::zebra")]
        );
    }

    /// エントリーの明示選択だけがホスト面。依存モジュールの `pub use` は入らない
    #[test]
    fn ホスト面に入るのはエントリーの明示選択だけ() {
        let loaded = load_files(&[
            ("main.rd", "use mid::{find}\nfn main(-> int) { find() }\n"),
            ("mid.rd", "pub use users::{find}\n"),
            ("users.rd", "fn find(-> int) { 1 }\n"),
        ])
        .expect("読み込めるはず");
        assert!(exported(&loaded).is_empty(), "{:?}", exported(&loaded));
    }

    /// 公開名は型検査後の HIR の宣言へそのまま引ける。ここが ABI 層の入口になる
    #[test]
    fn 公開名から型検査後のcallableを引ける() {
        let loaded = load_files(&[
            (
                "main.rd",
                "pub use users::{find as find_user}\nfn main(-> int) { find_user() }\n",
            ),
            ("users.rd", "fn find(-> int) { 1 }\n"),
        ])
        .expect("読み込めるはず");
        let checked = crate::typecheck::check_and_lower(&loaded.program).expect("型検査を通る");
        let export = &loaded.public_exports[0];
        assert_eq!(export.name, "find_user");
        assert!(checked.free_callable(&export.canonical).is_some());
    }
}
