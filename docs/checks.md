# Development checks

The [Checks workflow](../.github/workflows/ci.yml) runs on pull requests and pushes to
`main`, independently of image publishing. It also supports manual runs in GitHub
Actions. The workflow and local hook use the same checked-in command:

```sh
scripts/check
```

To run only selected groups, pass their names, for example:

```sh
scripts/check valinor flutter
```

| Group | Checks |
| --- | --- |
| `airmail`, `crates`, `poi_service`, `topo_pipeline`, `wof_service`, `valinor` | Rustfmt check and locked Cargo workspace tests (including doctests) |
| `flutter` | Authored Dart formatting, package tests, and host Rust bridge tests |
| `python` | Publish pytest suite and deployment/transit unittest suites |
| `go` | gofmt for authored pipeline code and `go test ./...` |
| `java` | Spotless formatting and Maven tests for Planetiler |

Install Rust 1.98 with rustfmt, Flutter 3.47.9, Python 3.12,
`uv`, Go (the version in `data_pipeline/go.mod`), and JDK 25. Native library and
system package dependencies for Linux are listed in the workflow. Python currently
has no repository-wide formatter configured. Device integration tests, large-data
oracles, and ignored Rust tests are not part of these checks.

## Install the pre-commit hook

From the repository root, run:

```sh
git config --local core.hooksPath .githooks
```

Git will then use the executable [pre-commit hook](../.githooks/pre-commit) directly
from the checkout, so there is no copy to keep in sync. The setting is local to this
clone. If you already use a custom hooks path, integrate `scripts/check` into your
existing hook instead.

The hook checks the **working tree**, not an isolated copy of the staged files. Keep
that in mind when committing partially staged changes. It runs every group, so the
first run may take some time to fetch dependencies and build native libraries.
If a check fails, the commit is rejected; formatting checks do not reformat files.
For an exceptional commit, `git commit --no-verify` bypasses the local hook but not CI.
To uninstall it, run `git config --local --unset core.hooksPath`.
