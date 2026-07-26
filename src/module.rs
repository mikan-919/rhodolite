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
    directory: bool,
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
    let mut loader = Loader {
        root: root.to_path_buf(),
        modules: BTreeMap::new(),
        diagnostics: Vec::new(),
    };
    loader.load_module(&entry_path);
    if !loader.diagnostics.is_empty() {
        return Err(loader.diagnostics);
    }
    resolve(loader.modules, &entry_path)
}

struct Loader {
    root: PathBuf,
    modules: BTreeMap<ModulePath, ParsedModule>,
    diagnostics: Vec<String>,
}

impl Loader {
    fn load_module(&mut self, path: &ModulePath) {
        if self.modules.contains_key(path) {
            return;
        }
        for part in &path.0 {
            if !valid_ident(part) {
                self.diagnostics.push(format!(
                    "モジュールパス `{path}` の `{part}` は有効な識別子ではありません"
                ));
                return;
            }
        }

        let relative = path.0.iter().fold(PathBuf::new(), |p, part| p.join(part));
        let leaf = self.root.join(&relative).with_extension("rd");
        let directory = self.root.join(&relative);
        let has_leaf = leaf.is_file();
        let has_directory = directory.is_dir();

        if has_leaf && has_directory {
            self.diagnostics.push(format!(
                "モジュール `{path}` に `{}` と `{}` の両方があります",
                leaf.display(),
                directory.display()
            ));
            return;
        }
        if !has_leaf && !has_directory {
            self.diagnostics
                .push(format!("モジュール `{path}` が見つかりません"));
            return;
        }
        if has_directory {
            self.modules.insert(
                path.clone(),
                ParsedModule {
                    path: path.clone(),
                    directory: true,
                    uses: Vec::new(),
                    items: Vec::new(),
                },
            );
            return;
        }

        let source = match std::fs::read_to_string(&leaf) {
            Ok(source) => source,
            Err(error) => {
                self.diagnostics
                    .push(format!("{} を読めません: {error}", leaf.display()));
                return;
            }
        };
        let tokens = match lex::lex(&source) {
            Ok(tokens) => lex::join(tokens),
            Err(error) => {
                self.diagnostics
                    .push(format!("{}: 字句解析エラー: {error}", leaf.display()));
                return;
            }
        };
        let program = match parse::parse(&tokens) {
            Ok(program) => program,
            Err(error) => {
                self.diagnostics
                    .push(format!("{}: {error}", leaf.display()));
                return;
            }
        };
        let uses = program.uses.clone();
        self.modules.insert(
            path.clone(),
            ParsedModule {
                path: path.clone(),
                directory: false,
                uses: program.uses,
                items: program.items,
            },
        );

        // 先に現在のモジュールを登録するため、循環 use はここで自然に止まる。
        for use_decl in uses {
            let target = ModulePath::from_parts(&use_decl.path);
            self.load_module(&target);
            if use_decl.members.is_some() {
                let target_is_directory = self
                    .modules
                    .get(&target)
                    .is_some_and(|module| module.directory);
                if target_is_directory {
                    for member in use_decl.members.unwrap_or_default() {
                        let child = target.child(&member.name);
                        if self.module_exists(&child) {
                            self.load_module(&child);
                        }
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
                    if self
                        .modules
                        .get(&child)
                        .is_some_and(|module| module.directory)
                    {
                        directory_imports.push((
                            member.alias.clone().unwrap_or_else(|| member.name.clone()),
                            child,
                        ));
                    }
                }
            } else if self
                .modules
                .get(&target)
                .is_some_and(|module| module.directory)
            {
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
                if !self.module_exists(&child) {
                    break;
                }
                self.load_module(&child);
                current = child;
            }
        }
    }

    fn module_exists(&self, path: &ModulePath) -> bool {
        let relative = path.0.iter().fold(PathBuf::new(), |p, part| p.join(part));
        self.root.join(&relative).with_extension("rd").is_file()
            || self.root.join(relative).is_dir()
    }
}

fn resolve(
    modules: BTreeMap<ModulePath, ParsedModule>,
    entry_path: &ModulePath,
) -> Result<LoadedProgram, Vec<String>> {
    let mut diagnostics = Vec::new();
    let mut declarations: BTreeMap<ModulePath, BTreeMap<String, String>> = BTreeMap::new();

    for module in modules.values() {
        let mut names = BTreeMap::new();
        for item in &module.items {
            let Some(name) = item_name(item) else {
                continue;
            };
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
                    } else if modules.contains_key(&child) {
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
            } else if modules.contains_key(&target) {
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

    if !diagnostics.is_empty() {
        return Err(diagnostics);
    }

    let mut items = Vec::new();
    for (path, mut module) in modules {
        if module.directory {
            continue;
        }
        let (local, imported_declarations, imported_modules) = &scopes[&path];
        for mut item in module.items.drain(..) {
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

fn item_name(item: &Item) -> Option<&str> {
    match item {
        Item::Trait { name, .. } | Item::Struct { name, .. } => Some(name),
        Item::Effect { slot, .. } => Some(slot),
        Item::Fn { sig, .. } => Some(&sig.name),
        Item::Impl { .. } | Item::Test { .. } => None,
    }
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
    ty.name = resolve_name(
        &ty.name,
        local,
        imported_declarations,
        imported_modules,
        declarations,
    );
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
        ExprKind::Field(recv, _) => resolve_expr(
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
            *name = resolve_name(
                name,
                local,
                imported_declarations,
                imported_modules,
                declarations,
            );
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
                                *type_name = resolve_name(
                                    type_name,
                                    local,
                                    imported_declarations,
                                    imported_modules,
                                    declarations,
                                );
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
    let mut paths = Vec::new();
    for item in items {
        match item {
            Item::Impl { methods, .. } => {
                for (_, body) in methods {
                    collect_expr_paths(body, &mut paths);
                }
            }
            Item::Fn { body, .. } | Item::Test { body, .. } => {
                collect_expr_paths(body, &mut paths);
            }
            _ => {}
        }
    }
    paths
}

fn collect_expr_paths(body: &[Expr], paths: &mut Vec<Vec<String>>) {
    for expr in body {
        match &expr.kind {
            ExprKind::Path(parts) => paths.push(parts.clone()),
            ExprKind::Field(recv, _) | ExprKind::Unary(_, recv) | ExprKind::Assert(recv) => {
                collect_expr_paths(std::slice::from_ref(recv), paths);
            }
            ExprKind::Call(callee, args) => {
                collect_expr_paths(std::slice::from_ref(callee), paths);
                collect_expr_paths(args, paths);
            }
            ExprKind::Array(items) | ExprKind::Block(items) => collect_expr_paths(items, paths),
            ExprKind::StructLit { name, fields } => {
                let parts: Vec<String> = name.split("::").map(str::to_string).collect();
                if parts.len() > 1 {
                    paths.push(parts);
                }
                for (_, value) in fields {
                    collect_expr_paths(std::slice::from_ref(value), paths);
                }
            }
            ExprKind::Let { value, .. } => {
                collect_expr_paths(std::slice::from_ref(value), paths);
            }
            ExprKind::Assign { target, value }
            | ExprKind::Binary {
                lhs: target,
                rhs: value,
                ..
            } => {
                collect_expr_paths(std::slice::from_ref(target), paths);
                collect_expr_paths(std::slice::from_ref(value), paths);
            }
            ExprKind::Return(Some(value)) => {
                collect_expr_paths(std::slice::from_ref(value), paths);
            }
            ExprKind::Head { head, body, orelse } => {
                match head {
                    Head::If(condition) | Head::Elif(condition) | Head::While(condition) => {
                        collect_expr_paths(std::slice::from_ref(condition), paths);
                    }
                    Head::For { iter, .. } => {
                        collect_expr_paths(std::slice::from_ref(iter), paths);
                    }
                    Head::Ambient(provisions) => {
                        for provision in provisions {
                            match provision {
                                Provision::Type { slot, type_name } => {
                                    for name in [slot, type_name] {
                                        let parts: Vec<String> =
                                            name.split("::").map(str::to_string).collect();
                                        if parts.len() > 1 {
                                            paths.push(parts);
                                        }
                                    }
                                }
                                Provision::Value { slot, value } => {
                                    let parts: Vec<String> =
                                        slot.split("::").map(str::to_string).collect();
                                    if parts.len() > 1 {
                                        paths.push(parts);
                                    }
                                    collect_expr_paths(std::slice::from_ref(value), paths);
                                }
                            }
                        }
                    }
                    Head::Else => {}
                }
                collect_expr_paths(std::slice::from_ref(body), paths);
                if let Some(orelse) = orelse {
                    collect_expr_paths(std::slice::from_ref(orelse), paths);
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
}
