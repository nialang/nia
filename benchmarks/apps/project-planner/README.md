# Project Planner

Project Planner is a small command-line application used as Nia's first real
medium-project performance workload. It reads a dependency plan, validates
task names and edges, rejects cycles, computes a topological schedule and the
critical path, and prints a deterministic summary.

Build and run it from this directory:

```sh
nia build
./.nia-build/project-planner data/reference.plan
```

The input grammar is intentionally plain text. Each record has the form
`task <name> <positive-duration> <dependency>... ;`; `#` starts a comment.
Task declarations may appear in any order.

`data/missing-dependency.plan` and `data/cycle.plan` are stable invalid inputs.
They exit with status 5 (parse/reference validation) and 6 (graph analysis),
respectively.
