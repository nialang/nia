# Frontend Query And Incremental Plan

Status: implemented frontend architecture through the red/green syntax layer

This document describes the frontend query and incremental compilation
boundary. It is intentionally narrower than a full IDE architecture. The goal is
to keep source loading, lossless syntax, AST lowering, import discovery, and
early semantic queries addressable through stable identities without moving
compiler semantics into the driver.

## Goals

- keep file identity independent from import resolution;
- make source text, syntax trees, AST parse results, imports, definitions, and
  semantic tables addressable as typed queries;
- track dependencies between frontend queries;
- make source revisions explicit so future invalidation has a stable input;
- keep parser and AST semantics unchanged while syntax identity matures;
- keep the lossless syntax layer compiler-owned, not LSP-owned.

## Current Non-goals

- no persistent cache format;
- no cross-session incremental compilation;
- no LSP server architecture in compiler crates;
- no grammar-complete syntax tree requirement before the AST parser is retired;
- no semantic behavior changes from syntax representation changes.

## Historical First-step Non-goals

These were intentionally deferred until source identity, query keys,
invalidation, and node identity were in place:

- no IDE-oriented lossless syntax tree;
- no partial reparsing;
- no persistent cache format;
- no cross-session incremental compilation;
- no semantic behavior changes.

## Source Identity

`nia-source` owns source identity:

- `SourcePath` identifies a file path;
- `SourceId` identifies a source file independently from its current text;
- `SourceRevision` identifies a specific version of that file;
- `SourceFile` carries id, path, revision, and text;
- `SourceTable` assigns stable ids for paths inside one compiler session.

`SourceId` is session-local in the current design. It is stable while a compiler
session lives, but it is not a persistent cross-session fingerprint. Persistent
incremental compilation should add a separate source fingerprint or cache key
rather than reusing `SourceId` for that purpose.

`nia-imports` may use source paths, but it must not own source identity. Import
resolution is a client of source identity, not its storage layer.

## Query Layers

The frontend uses these layers:

```text
source path/module map
  -> source file query
  -> syntax query
  -> AST parse query
  -> import query
  -> module graph query
  -> definition query
  -> name/type/body semantic queries
```

The loader currently owns the first syntax/parse/import/module graph queries.
That is acceptable while the compiler is still batch-oriented. Query keys use
source versions for syntax, AST, and import queries so same-session invalidation
can reconnect after source edits.

The old tokenized module query is not part of the loader path. The lexer still
offers semantic tokens for CLI/debugging, but parser lowering consumes
`nia-syntax` tokens directly. The official lossless source representation is
`nia-syntax`.

## Stable Node Identity

Current AST nodes are still syntax values with byte spans. They stay
semantic-free and are lowered from `nia-syntax` for existing compiler passes.

Node identity is derived from:

- `SourceId`;
- `SourceRevision`;
- syntax kind;
- span or green-tree child path.

Do not put semantic ids directly into AST nodes. Semantic ids belong to later
tables such as defs, locals, types, and body IR.

## Red/Green Tree Direction

`nia-syntax` provides the official lossless syntax layer. The current green tree
preserves tokens and trivia and groups delimiter subtrees. Red nodes provide
source-versioned child-path identity through `nia-node-id`.

A red/green tree is useful when Nia needs:

- lossless syntax preservation;
- partial reparsing;
- IDE-grade node identity;
- cheap subtree reuse after edits.

Partial reparsing is exposed through a conservative `SyntaxTree::reparse` API.
Single-token or trivia edits that preserve token kind can reuse the existing
tree shape; wider edits fall back to full-tree parsing. That gives IDE and
future LSP callers a stable correctness-first entry point without pushing an LSP
architecture into compiler crates.

## Incremental Invalidation Direction

The query runtime records dependency edges and supports in-memory invalidation.
Those edges use explicit source revisions:

- when a source revision changes, invalidate source-dependent queries;
- syntax and parse queries depend on one source revision;
- import graph depends on import queries for reachable parsed modules;
- semantic queries depend on parsed modules, definitions, public surfaces, and
  earlier semantic tables.

The implementation remains in-memory and batch-friendly. Persistent caches,
cross-session fingerprints, scheduling priorities, and cancellation should be
added as separate layers rather than folded into source or syntax identity.
