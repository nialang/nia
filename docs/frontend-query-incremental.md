# Frontend Query And Incremental Plan

Status: implementation plan for the 0.2.x frontend architecture

This document describes the planned frontend query and incremental compilation
boundary. It is intentionally narrower than a full IDE architecture. The goal is
to give source loading, lexing, parsing, import discovery, and early semantic
queries stable identities so later red/green trees and fine-grained invalidation
can be added without moving compiler semantics into the driver.

## Goals

- keep file identity independent from import resolution;
- make source text, tokens, parse trees, imports, definitions, and semantic
  tables addressable as typed queries;
- track dependencies between frontend queries;
- make source revisions explicit so future invalidation has a stable input;
- keep parser and AST semantics unchanged while the query boundary matures;
- avoid introducing a red/green tree until stable source and node identities are
  in place.

## Non-goals For The First Step

- no IDE-oriented lossless syntax tree yet;
- no partial reparsing yet;
- no persistent cache format;
- no cross-session incremental compilation;
- no semantic behavior changes.

## Source Identity

`nia-source` owns source identity:

- `SourcePath` identifies a file path;
- `SourceId` identifies a source file independently from its current text;
- `SourceRevision` identifies a specific version of that file;
- `SourceFile` carries id, path, revision, and text.

`nia-imports` may use source paths, but it must not own source identity. Import
resolution is a client of source identity, not its storage layer.

## Query Layers

The frontend should settle into these layers:

```text
source path/module map
  -> source file query
  -> token query
  -> parse query
  -> import query
  -> module graph query
  -> definition query
  -> name/type/body semantic queries
```

The loader currently owns the first parse/import/module graph queries. That is
acceptable while the compiler is still batch-oriented, but the query keys should
move toward source ids and source revisions rather than raw paths.

## Stable Node Identity

Current AST nodes are syntax values with byte spans. They should stay that way
until there is a dedicated syntax tree layer.

Future node identity should be derived from:

- `SourceId`;
- `SourceRevision`;
- syntax kind;
- span or green-tree child path.

Do not put semantic ids directly into AST nodes. Semantic ids belong to later
tables such as defs, locals, types, and body IR.

## Red/Green Tree Direction

A red/green tree is useful when Nia needs:

- lossless syntax preservation;
- partial reparsing;
- IDE-grade node identity;
- cheap subtree reuse after edits.

It should come after source identity and frontend query keys are stable. Adding a
green tree before that would create another syntax representation without a
clear invalidation model.

## Incremental Invalidation Direction

The existing query runtime records dependency edges. Later invalidation should
use those edges with explicit source revisions:

- when a source revision changes, invalidate source-dependent queries;
- token and parse queries depend on one source revision;
- import graph depends on import queries for reachable parsed modules;
- semantic queries depend on parsed modules, definitions, public surfaces, and
  earlier semantic tables.

The first implementation can stay in-memory and batch-oriented. The important
part is that inputs are shaped so invalidation can be added without changing
every pass signature again.
