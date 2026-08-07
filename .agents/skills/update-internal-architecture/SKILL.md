---
name: update-internal-architecture
description: Update and verify Rhodolite's standalone interactive compiler architecture viewer at docs/internal-architecture.html. Use when implementation, roadmap status, compiler stages, Rust modules, production functions, supported language capabilities, or known limitations change; also use when asked to refresh, audit, or repair the internal architecture atlas.
---

# Update the Internal Architecture Atlas

Keep `docs/internal-architecture.html` accurate to the current repository. Treat source and project documents as authoritative; never infer implementation status from the existing viewer alone.

## Workflow

1. Read `AGENTS.md` and inspect `git status --short --branch`. Preserve unrelated work.
2. Read the current viewer before editing it.
3. Rebuild the current-state model from these sources:
   - `ROADMAP.md` for task state and explicitly deferred work.
   - `docs/overview.md` and `docs/compiler-roadmap.md` for pipeline boundaries and supported scope.
   - `src/main.rs` for the CLI orchestration path.
   - `src/*.rs` for actual module responsibilities and production function names.
   - Current OpenSpec specs only when a capability boundary is ambiguous. Use `bunx @fission-ai/openspec` for OpenSpec commands.
4. Update only facts that changed. Keep the viewer standalone: inline CSS and JavaScript, no network assets, build step, or framework.
5. Preserve its four views unless the architecture demands a structural change:
   - Processing flow: end-to-end stages and interpreter/Wasm branch.
   - Function index: production modules, real function names, roles, search, filters, and source links.
   - Coverage: distinguish front end acceptance, HIR interpreter execution, and Core Wasm execution.
   - Unimplemented areas: distinguish partial implementation, deliberate non-goals, and not-yet-started work.

## Accuracy Rules

- Call a capability complete only when the relevant source path and repository documentation agree.
- Describe split delivery precisely. Example: closure syntax and AST may be implemented while type checking, capture, and execution remain unsupported.
- Do not count test-only helpers as production architecture functions.
- Keep every function entry attached to its actual `src/*.rs` file. Prefer stable file links; displayed line numbers are informational and must be refreshed when touched.
- Do not present the visual completion bar as a calculated percentage. It is an orientation device; the textual status is authoritative.
- Keep the snapshot date current when any factual content changes.
- Write UI copy in Japanese and keep Rust identifiers unchanged.

## Verification

Run all checks before finishing.

Validate that embedded JavaScript parses:

```sh
bun -e 'const s=await Bun.file("docs/internal-architecture.html").text();const a=s.indexOf("<script>")+8,b=s.lastIndexOf("</script>");if(a<8||b<a)throw Error("script missing");new Function(s.slice(a,b));console.log("script syntax: ok")'
```

Validate that every indexed function exists in its declared source file:

```sh
bun -e 'const s=await Bun.file("docs/internal-architecture.html").text();const a=s.indexOf("const modules = ")+16,b=s.indexOf(";\nconst layerNames",a);const ms=new Function("return "+s.slice(a,b))();let bad=0;for(const m of ms){const src=await Bun.file(m.file).text();for(const f of m.f){const n=f[0].split("::").pop();if(!src.includes("fn "+n+"(")){console.error(m.id+": "+f[0]+" missing");bad++}}}if(bad)process.exit(1);console.log("function index: ok")'
```

Then run:

```sh
git diff --check
cargo test
```

Visually inspect desktop and narrow layouts when browser tooling is available. Check navigation, search, layer filters, stage dialogs, keyboard activation, source links, overflow behavior, and reduced-motion handling.

## Completion

Summarize factual changes to the atlas and verification results. When the repository instructions require stable snapshots, commit only the atlas and skill changes that belong to this task after all checks pass.
