# Project Planner

Project Planner is a small command-line application and the medium-size
`project_planner_build` workload of the competitive performance baseline (see
[`maintain/performance.md`](../../../maintain/performance.md)). It reads a
dependency plan, validates task names and edges, rejects cycles, computes a
topological schedule and the critical path, and prints a deterministic
summary.

Build and run it from this directory:

```sh
nia build
./build/out/project-planner data/reference.plan
```

Without an argument it reads `data/reference.plan`.

The input grammar is plain text. Each record has the form
`task <name> <positive-duration> <dependency>... ;`, and `#` starts a comment.
Task declarations may appear in any order.

`data/missing-dependency.plan` and `data/cycle.plan` are stable invalid
inputs. They exit with status 5 (parse or reference validation) and 6 (graph
analysis), respectively. An input file that cannot be read exits with
status 4.
