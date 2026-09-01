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

| # | Branch | What it does | Landed | Upstream status |
|---|---|---|---|---|
| 1 | `feat/bench-session` | `BenchSession`: a public runner entry returning the report in-process, enabling sequential multi-phase runs | [#1](https://github.com/humb1t/rlt/pull/1) | cherry-pick of upstream [PR #104](https://github.com/wfxr/rlt/pull/104), still open there |
| 2 | `feat/load-model-open` | `--load-model open\|closed`: absolute-schedule dispatcher with `offered`/`dropped` counters | [#2](https://github.com/humb1t/rlt/pull/2) | not offered — the reason this fork exists |
| 3 | `feat/bencher-reporter` | `-o bencher` output for `benchmark-action/github-action-benchmark`, with an open-loop throughput guard | [#3](https://github.com/humb1t/rlt/pull/3) | answers upstream [issue #18](https://github.com/wfxr/rlt/issues/18); to be offered |
| 4 | `feat/tui-feature-gate` | `tui` feature so the interactive stack can be compiled out (197 → 119 crates) | [#4](https://github.com/humb1t/rlt/pull/4) | to be offered |
| 5 | `feat/pacing` | `Pacing` on `BenchOpts`/`BenchReport`: declare what set the rate instead of inferring it from `offered > 0` | [#5](https://github.com/humb1t/rlt/pull/5) | to be offered |
| 6 | `feat/public-stats` | public `histogram` / `stats` modules and a canonical `LatencyStats`, so a consumer can name the types its report is made of | [#6](https://github.com/humb1t/rlt/pull/6) | to be offered |
| 7 | `feat/throughput-observer` | `observer::Throughput`: a per-second series without the TUI, measured against the session `Clock` | [#7](https://github.com/humb1t/rlt/pull/7) | to be offered |
| 8 | `feat/run-report` | `run::RunReport<M>` plus JSON and bencher run reporters: several sessions recorded as one document | [#8](https://github.com/humb1t/rlt/pull/8) | not offered — see below |
| 9 | `feat/text-reporter-feature` | `text` feature so the table reporter can be compiled out (127 → 111 crates), taking `tabled` and `byte-unit` — and the two RUSTSEC findings behind them — with it | [#9](https://github.com/humb1t/rlt/pull/9) | to be offered |

Patches 3, 4, 5, 6, 7 and 9 are self-contained and worth sending upstream; 2 is
not, and 1 is already an open upstream PR.

Patch 8 is fork-only by decision, not by neglect. A multi-session run concept is
a large ask for a tool whose shape is one invocation, one `BenchCli`, one
session; upstream would have to commit to a public API it has no use for.
Patches 5–7 are the ones that would help any consumer.

Patches 5–8 all came out of the first real consumer
([`fleet-load`](https://github.com/exein-io/runtime-platform/tree/main/fleet-load)).
Each closes a seam that only opens when rlt is driven as a library rather than as
a CLI — which is what patch 1 made possible and what nothing upstream exercises.

Patch 9 is patch 4's argument applied to the other half of the reporting stack.
`tabled` is reached only from `reporter/text.rs`, and a consumer that collects a
`RunReport` of its own never renders a table; leaving it wired in meant every
such consumer inherited two subtrees it never calls: `tabled_derive` with the
unmaintained `proc-macro-error2` behind it ([RUSTSEC-2026-0173][rustsec-0173]),
and `byte-unit` with `rust_decimal` and a vulnerable `rkyv`
([RUSTSEC-2026-0235][rustsec-0235]). Both were `cargo audit` findings in a
downstream repository for code that repository does not compile.

Patches are offered to `wfxr/rlt` opportunistically. Nothing here is ever blocked
on upstream review.

[rustsec-0173]: https://rustsec.org/advisories/RUSTSEC-2026-0173
[rustsec-0235]: https://rustsec.org/advisories/RUSTSEC-2026-0235

## Conventions

- **`main` is never force-pushed.** The mirror syncs by merge; a rebase of `main`
  breaks every future sync.
- Tags are `exein-v<upstream version>.<n>` (e.g. `exein-v0.5.2`), deliberately not
  matching upstream's `v*.*.*` release trigger.
- The crate keeps upstream's name and version. Consumers pin a tag:
  `rlt = { git = "https://github.com/exein-io/rlt", tag = "exein-v0.5.2" }`.

## Syncing the mirror

`exein-io/rlt` carries a `sync-from-humb1t` workflow, but **its schedule is off and it
cannot push**: `GITHUB_TOKEN` refuses refs that create or update files under
`.github/workflows/`, which this repository's history does. Until that job gets a
credential of its own — narrowest being a deploy key on the mirror — sync by hand from a
clone with both remotes (`origin` = here, `mirror` = `exein-io/rlt`):

```sh
git switch mirror-main && git merge --no-ff origin/main
git push mirror mirror-main:main && git push mirror --tags
```

`mirror-main` exists because the mirror's `main` is this repository's `main` plus that one
workflow file. Never force-push either branch.

## Licensing

Unchanged from upstream: `MIT OR Apache-2.0`, see `LICENSE` and `LICENSE-APACHE`.
The table above is the record of what this fork modified.
