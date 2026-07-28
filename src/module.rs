//! ADR-0006 のモジュール読み込みと名前解決。
//!
//! パーサが作る各ファイル内の AST を集め、宣言と参照を宣言元の完全修飾名へ
//! 書き換えてから、既存の要求推論と評価器へ渡す。

use crate::ast::*;
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

#[derive(Debug)]
pub struct LoadedProgram {
    pub program: Program,
    pub entry: String,
}

pub fn load(entry_file: &Path) -> Result<LoadedProgram, Vec<String>> {
    let Some(root) = entry_file.parent() else {
        return Err(vec![
            "エントリーファイルの親ディレクトリがありません".to_string(),
        ]);
    };
    let root = if root.as_os_str().is_empty() {
        Path::new(".")
    } else {
        root
    };
    let Some(stem) = entry_file.file_stem().and_then(|s| s.to_str()) else {
        return Err(vec![
            "エントリーファイル名を UTF-8 として読めません".to_string(),
        ]);
    };
    if entry_file.extension().and_then(|s| s.to_str()) != Some("rd") {
        return Err(vec![
            "エントリーファイルは `.rd` でなければなりません".to_string(),
        ]);
    }
    if !valid_ident(stem) {
        return Err(vec![format!(
            "モジュール名 `{stem}` は有効な識別子ではありません"
        )]);
    }

    let entry_path = ModulePath(vec![stem.to_string()]);
    let mut modules = BTreeMap::new();
    let mut directories = BTreeSet::new();
    let mut diagnostics = Vec::new();
    load_module(
        root,
        &mut modules,
        &mut directories,
        &mut diagnostics,
        &entry_path,
    );
    if !diagnostics.is_empty() {
        return Err(diagnostics);
    }
    resolve(modules, directories, &entry_path)
}

fn load_module(
    root: &Path,
    modules: &mut BTreeMap<ModulePath, ParsedModule>,
    directories: &mut BTreeSet<ModulePath>,
    diagnostics: &mut Vec<String>,
    path: &ModulePath,
) {
    if modules.contains_key(path) || directories.contains(path) {
        return;
    }
    for part in &path.0 {
        if !valid_ident(part) {
            diagnostics.push(format!(
                "モジュールパス `{path}` の `{part}` は有効な識別子ではありません"
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

        if (has_leaf && !exact_spelling(&leaf)) || (has_directory && !exact_spelling(&directory)) {
            diagnostics.push(format!(
                "モジュール `{prefix}` のパスはファイルシステム上の綴りと完全一致しません"
            ));
            return;
        }
        if has_leaf && has_directory {
            diagnostics.push(format!(
                "モジュール `{prefix}` に `{}` と `{}` の両方があります",
                leaf.display(),
                directory.display()
            ));
            return;
        }
        if length < path.0.len() && has_leaf {
            diagnostics.push(format!(
                "リーフモジュール `{prefix}` は子モジュールを持てません"
            ));
            return;
        }
        if length < path.0.len() && !has_directory {
            diagnostics.push(format!("モジュール `{prefix}` が見つかりません"));
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
        diagnostics.push(format!("モジュール `{path}` が見つかりません"));
        return;
    }
    if has_directory {
        directories.insert(path.clone());
        return;
    }

    let source = match std::fs::read_to_string(&leaf) {
        Ok(source) => source,
        Err(error) => {
            diagnostics.push(format!("{} を読めません: {error}", leaf.display()));
            return;
        }
    };
    let tokens = match lex::lex(&source) {
        Ok(tokens) => lex::join(tokens),
        Err(error) => {
            diagnostics.push(format!("{}: 字句解析エラー: {error}", leaf.display()));
            return;
        }
    };
    let program = match parse::parse(&tokens) {
        Ok(program) => program,
        Err(error) => {
            diagnostics.push(format!("{}: {error}", leaf.display()));
            return;
        }
    };
    let uses = program.uses.clone();
    modules.insert(
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
        load_module(root, modules, directories, diagnostics, &target);
        if use_decl.members.is_some() && directories.contains(&target) {
            for member in use_decl.members.unwrap_or_default() {
                let child = target.child(&member.name);
                if module_exists(root, &child) {
                    load_module(root, modules, directories, diagnostics, &child);
                }
            }
        }
    }

    // ディレクトリモジュールは、修飾参照に現れた子だけを辿る。
    // ディレクトリ全体を走査しないため、未参照ファイルは読まれない。
    let Some(module) = modules.get(path) else {
        return;
    };
    let references = module_references(&module.items);
    let mut directory_imports = Vec::new();
    for use_decl in &module.uses {
        let target = ModulePath::from_parts(&use_decl.path);
        if let Some(members) = &use_decl.members {
            for member in members {
                let child = target.child(&member.name);
                if directories.contains(&child) {
                    directory_imports.push((
                        member.alias.clone().unwrap_or_else(|| member.name.clone()),
                        child,
                    ));
                }
            }
        } else if directories.contains(&target) {
            directory_imports.push((
                use_decl
                    .alias
                    .clone()
                    .unwrap_or_else(|| target.name().to_string()),
                target,
            ));
        }
    }

    for reference in references {
        let Some((_, mut current)) = directory_imports
            .iter()
            .find(|(alias, _)| reference.first() == Some(alias))
            .cloned()
        else {
            continue;
        };
        for part in reference.iter().skip(1) {
            let child = current.child(part);
            if !module_exists(root, &child) {
                break;
            }
            load_module(root, modules, directories, diagnostics, &child);
            current = child;
        }
    }
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

fn resolve(
    modules: BTreeMap<ModulePath, ParsedModule>,
    directories: BTreeSet<ModulePath>,
    entry_path: &ModulePath,
) -> Result<LoadedProgram, Vec<String>> {
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
                    if !seen.insert(variant.as_str()) {
                        diagnostics.push(format!(
                            "enum `{name}`: variant `{variant}` が重複して宣言されています"
                        ));
                    }
                }
            }
            for name in item_names(item) {
                if names
                    .insert(name.to_string(), module.path.qualified(name))
                    .is_some()
                {
                    diagnostics.push(format!(
                        "モジュール `{}` で名前 `{name}` が重複しています",
                        module.path
                    ));
                }
            }
        }
        declarations.insert(module.path.clone(), names);
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
                    let child = target.child(&member.name);
                    if local.contains_key(alias) {
                        diagnostics
                            .push(format!("import名 `{alias}` がローカル宣言と衝突しています"));
                    } else if imported_declarations.contains_key(alias)
                        || imported_modules.contains_key(alias)
                    {
                        diagnostics.push(format!("import名 `{alias}` が衝突しています"));
                    } else if declarations.contains_key(&child) {
                        imported_modules.insert(alias.to_string(), child);
                    } else if let Some(name) = declarations
                        .get(&target)
                        .and_then(|names| names.get(&member.name))
                    {
                        imported_declarations.insert(alias.to_string(), name.clone());
                    } else {
                        diagnostics.push(format!(
                            "モジュール `{target}` にメンバー `{}` がありません",
                            member.name
                        ));
                    }
                }
            } else if declarations.contains_key(&target) {
                let alias = use_decl.alias.as_deref().unwrap_or_else(|| target.name());
                if local.contains_key(alias) {
                    diagnostics.push(format!("import名 `{alias}` がローカル宣言と衝突しています"));
                } else if imported_declarations.contains_key(alias)
                    || imported_modules.contains_key(alias)
                {
                    diagnostics.push(format!("import名 `{alias}` が衝突しています"));
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
                diagnostics.push(format!("モジュール名 `{first}` は `use` されていません"));
            }
        }
    }

    if !diagnostics.is_empty() {
        diagnostics.sort();
        diagnostics.dedup();
        return Err(diagnostics);
    }

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

    diagnostics.sort();
    diagnostics.dedup();
    if !diagnostics.is_empty() {
        return Err(diagnostics);
    }

    Ok(LoadedProgram {
        program: Program {
            uses: Vec::new(),
            items,
        },
        entry: entry_path.qualified("main"),
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
                if seen.insert(variant.as_str()) {
                    names.push(variant);
                }
            }
            names
        }
        Item::Effect { slot, .. } => vec![slot],
        Item::Fn { sig, .. } => vec![&sig.name],
        Item::Impl { .. } | Item::Test { .. } => Vec::new(),
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
    diagnostics: &mut Vec<String>,
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
            for (_, ty) in fields {
                resolve_type(
                    ty,
                    local,
                    imported_declarations,
                    imported_modules,
                    declarations,
                );
            }
        }
        Item::Enum { name, variants, .. } => {
            *name = local[name].clone();
            for variant in variants {
                *variant = local[variant].clone();
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
                if sig.has_self {
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
    diagnostics: &mut Vec<String>,
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
    diagnostics: &mut Vec<String>,
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
                diagnostics.push(format!(
                    "モジュール名 `{}` は `use` されていません",
                    original[0]
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
        ExprKind::Let { name, value } => {
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
        ExprKind::Unary(_, inner) | ExprKind::Assert(inner) => resolve_expr(
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
        // arm は束縛を導入しないが、本体の `let` を他の arm や後続へ漏らさない
        // ために、それぞれ外側 locals の複製で解決する(design.md 決定4)
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
                if !arm
                    .enum_name
                    .split("::")
                    .next()
                    .is_some_and(|first| locals.contains(first))
                {
                    arm.enum_name = resolve_name(
                        &arm.enum_name,
                        local,
                        imported_declarations,
                        imported_modules,
                        declarations,
                    );
                }
                resolve_expr(
                    &mut arm.body,
                    &mut locals.clone(),
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
                    if sig.has_self {
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
                for (_, ty) in fields {
                    collect_type_paths(ty, &mut paths);
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
            // enum は型も値も参照しない
            Item::Enum { .. } | Item::Test { .. } => {}
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
            | ExprKind::Assert(recv) => {
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
            ExprKind::Let { name, value } => {
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
                    if !arm
                        .enum_name
                        .split("::")
                        .next()
                        .is_some_and(|first| locals.contains(first))
                    {
                        collect_name_path(&arm.enum_name, paths);
                    }
                    collect_expr_paths(std::slice::from_ref(&arm.body), &mut locals.clone(), paths);
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

#[cfg(test)]
mod tests {
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
                let (variant_module, _) =
                    variant.rsplit_once("::").expect("variant は正準名を持つ");
                assert_eq!(&format!("{variant_module}::{enum_short}"), name);
                checked += 1;
            }
        }
        assert_eq!(checked, 2, "正典の `Rank` は variant を2つ持つ");
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
        assert_eq!(arms[0].enum_name, "main::Rank");
        assert_eq!(arms[0].variant, "Bronze");
        assert_eq!(arms[1].enum_name, "dep::Grade");
        assert_eq!(arms[1].variant, "Low");
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
        for (_, ty) in fields.iter_mut() {
            resolve_type(
                ty,
                &local,
                &BTreeMap::new(),
                &BTreeMap::new(),
                &BTreeMap::new(),
            );
        }

        let TypeKind::Array(outer) = &fields[0].1.kind else {
            panic!("配列ではない: {:?}", fields[0].1)
        };
        let TypeKind::Array(inner) = &outer.kind else {
            panic!("入れ子の配列ではない: {outer:?}")
        };
        assert_eq!(inner.name(), Some("main::User"));
        assert!(outer.optional, "要素の後置 `?` は解決で失われない");
        assert_eq!(fields[1].1.name(), Some("main::User"));
    }
}
