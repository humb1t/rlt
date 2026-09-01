# This is a fork

`humb1t/rlt` is Exein's development line for [wfxr/rlt](https://github.com/wfxr/rlt),
a universal load-testing library for Rust. `exein-io/rlt` is a mirror that syncs
from this repository daily and is the one Exein projects depend on.

The fork exists because the Exein platform's load driver needs an **open-loop**
load model — an unmet target rate must be reported as a counted shortfall
(`offered` vs `dropped`), never as a lower, apparently-healthy rate. Upstream rlt
is closed-loop by construction and has had no commit to `main` since 2026-04-03.
The full evaluation, including why k6, goose and balter were rejected, is in
[ADR-0023](https://github.com/exein-io/runtime-platform/blob/main/docs/adr/0023-fleet-load-engine-rlt-fork.md).

## Patch set

Every patch is developed on its own branch so it stays individually
cherry-pickable for an upstream pull request.

| # | Branch | What it does | Upstream status |
|---|---|---|---|
| 1 | `feat/bench-session` | `BenchSession`: a public runner entry returning the report in-process, enabling sequential multi-phase runs | cherry-pick of upstream [PR #104](https://github.com/wfxr/rlt/pull/104) (open) |
| 2 | `feat/load-model-open` | `--load-model open\|closed`: absolute-schedule dispatcher with `offered`/`dropped` counters | not offered — the reason this fork exists |
| 3 | `feat/bencher-reporter` | `-o bencher` output for `benchmark-action/github-action-benchmark`, with an open-loop throughput guard | closes upstream [issue #18](https://github.com/wfxr/rlt/issues/18) if accepted |
| 4 | `feat/tui-feature-gate` | `tui` feature so the interactive stack can be compiled out | to be offered |

Patches are offered to `wfxr/rlt` opportunistically. Nothing here is ever blocked
on upstream review.

## Conventions

- **`main` is never force-pushed.** The mirror syncs by merge; a rebase of `main`
  breaks every future sync.
- Tags are `exein-v<upstream version>.<n>` (e.g. `exein-v0.5.1`), deliberately not
  matching upstream's `v*.*.*` release trigger.
- The crate keeps upstream's name and version. Consumers pin a tag:
  `rlt = { git = "https://github.com/exein-io/rlt", tag = "exein-v0.5.1" }`.

## Licensing

Unchanged from upstream: `MIT OR Apache-2.0`, see `LICENSE` and `LICENSE-APACHE`.
The table above is the record of what this fork modified.
