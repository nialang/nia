# Testing

Tests in Nia are ordinary programs. A test suite is an executable that runs
test functions and exits with a failure status when one fails; `nia build`
builds and runs it. There is no hidden test harness: the standard library's
`std::test` module provides assertions and a small runner, and the suite's
`main` decides what to run and how to report it.

## A First Suite

The package template from [Getting Started](getting-started.md) already
declares a suite in `nia.toml`:

```toml
[[test]]
name = "unit"
path = "tests/unit.nia"
```

A test is a function that returns `test::Error!()`. Assertions return the same
type, so `.?` stops the test at the first failed check:

```nia
using std::process;
using std::test;

fun addsNumbers() test::Error!() {
    test::expectEqual(2 + 2, 4).?;
    !()
}

fun measuresText() test::Error!() {
    let text = "nia";
    test::expect(text.len() == 3).?;
    test::expectNotEqual(text[0], 'x').?;
    !()
}

pub fun main(init: process::Init) process::ExitCode!() {
    _ = init;
    let cases = [
        test::Case::init("adds numbers", addsNumbers),
        test::Case::init("measures text", measuresText),
    ];
    if not test::Runner::init(cases).run().isSuccessful() {
        return process::ExitCode(1)!;
    }
    !()
}
```

| Assertion | Fails when |
| --- | --- |
| `test::expect(condition)` | `condition` is false |
| `test::expectEqual(actual, expected)` | the values differ; the type needs `Eq` |
| `test::expectNotEqual(actual, expected)` | the values are equal |
| `test::fail()` | always |

A `Case` pairs a name with a test function, and a `Runner` runs a table of
cases and returns a `RunSummary` with `total()`, `passed()`, `failed()`, and
`isSuccessful()`. `.withFailFast()` makes the runner skip the remaining cases
after the first failure.

## Running Tests

| Command | Effect |
| --- | --- |
| `nia build test` | builds and runs every suite |
| `nia build unit` | runs one suite by name |
| `nia build test --keep-going` | runs every suite even after one fails |
| `nia build` | builds everything and runs every suite |

A suite that passed is not run again until its code or inputs change, so
`nia build test` stays fast on a large package. A suite runs with the package
root as its working directory.

## Reporting Failures

`run()` reports nothing itself; the suite's exit status is all `nia build`
sees. To see which case failed and where, pass a reporter to `runWith`. It
receives each case's `CaseResult`, whose `error()` holds the failure, and a
`test::Error` formats as the source location of the failed assertion:

```nia
using std::io;
using std::process;
using std::test;

fun countWords(text: &[char]) usize {
    let mut count: usize = 0;
    let mut inWord = false;
    for &ch in text {
        if ch == ' ' {
            inWord = false;
        } else if not inWord {
            inWord = true;
            count += 1;
        }
    }
    count
}

fun countsWords() test::Error!() {
    test::expectEqual(countWords("one two three"), 3).?;
    !()
}

fun ignoresExtraSpaces() test::Error!() {
    test::expectEqual(countWords("  a  b "), 2).?;
    !()
}

pub fun main(init: process::Init) process::ExitCode!() {
    _ = init;
    let cases = [
        test::Case::init("counts words", countsWords),
        test::Case::init("ignores extra spaces", ignoresExtraSpaces),
    ];
    let mut report = \result: test::CaseResult -> {
        if result.error() is ?failure {
            _ = io::debugPrint("FAIL {}: {}\n", [result.name(), failure]);
        }
    };
    let summary = test::Runner::init(cases).runWith(report);
    _ = io::debugPrint("{} passed, {} failed\n", [summary.passed(), summary.failed()]);
    if not summary.isSuccessful() {
        return process::ExitCode(1)!;
    }
    !()
}
```

```text
2 passed, 0 failed
```

When a case fails, its line names the test and the assertion:

```text
FAIL counts nothing: build-package:root:/tests/words.nia:15:5: values are not equal
1 passed, 1 failed
```

`nia build` then reports the suite as failed, with its exit status.

## Organizing Tests

A suite is one program, so it can import any number of test modules. Keep the
runner in the suite's root file and the tests in child modules beside it:

```text
tests/
├── unit.nia      the suite: builds the case table and runs it
├── words.nia     test functions for one area
└── parsing.nia   test functions for another
```

```nia ignore
// tests/unit.nia
module words;
module parsing;

using std::process;
using std::test;

pub fun main(init: process::Init) process::ExitCode!() {
    _ = init;
    let cases = [
        test::Case::init("counts words", words::countsWords),
        test::Case::init("parses numbers", parsing::parsesNumbers),
    ];
    if not test::Runner::init(cases).run().isSuccessful() {
        return process::ExitCode(1)!;
    }
    !()
}
```

```nia ignore
// tests/words.nia
using hello;
using std::test;

pub fun countsWords() test::Error!() {
    test::expectEqual(hello::wordCount("one two three"), 3).?;
    !()
}
```

Test functions in child modules are `pub` so the suite's root can name them.
A package may also declare several suites, each its own `[[test]]` entry, for
example to keep slow integration tests apart from fast unit tests.

## Testing The Library

A suite reaches the package's library by the package name, like any user of
the library, so it sees the library's `pub` items. Code that should exist only
for tests, such as a hook into a private helper, is marked `@[test]`: it is
compiled when the library is built for a test suite and left out of every
other build.

```nia ignore
// lib/root.nia
fun countWords(text: &[char]) usize {
    // ...
}

pub fun wordCount(text: &[char]) usize {
    countWords(text)
}

// Only in test builds: exposes the private helper to the suites.
@[test]
pub fun testCountWords(text: &[char]) usize {
    countWords(text)
}
```

`@[test]` works on any item or statement. Dependencies that only tests need go
in `[dev-dependencies]`, which only test suites can import; see
[Packages](../guides/packages.md#who-can-use-a-dependency).

## Summary

- A suite is an executable declared with `[[test]]`; it passes when it exits
  with status 0.
- Test functions return `test::Error!()` and use `expect`, `expectEqual`,
  `expectNotEqual`, and `fail` with `.?`.
- `Runner::run` reports only through its summary; `runWith` with a reporter
  prints which cases failed and where.
- `nia build test` runs every suite and skips suites whose code has not
  changed.
- `@[test]` code and `[dev-dependencies]` exist only in test builds.

Build scripts can declare suites programmatically with `addTestSuite`; see
[Build scripts](../guides/build-scripts.md#tests). This is the last chapter of
[Learning Nia](README.md). The [guides](../README.md#guides) cover larger
tasks, and the [reference](../reference/language.md) has every rule.
