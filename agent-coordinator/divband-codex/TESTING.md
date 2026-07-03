# Testing Notes

This file records the focused validation performed for the snapshot and the
recommended checks for future rebases.

## Tests Run For This Snapshot

The following commands were run with low Cargo parallelism where relevant:

```shell
just fmt
CARGO_BUILD_JOBS=1 just test -p codex-cli best_profile
CARGO_BUILD_JOBS=1 just test -p codex-cli resume_best
CARGO_BUILD_JOBS=1 just test -p codex-app-server thread_profile_refresh
CARGO_BUILD_JOBS=1 just test -p codex-core usage_limit_switches_profile_and_retries_turn
CARGO_BUILD_JOBS=1 just fix -p codex-core
git diff --check
```

The core integration test was also rerun outside the sandbox because the local
Codex sandbox sets `CODEX_SANDBOX_NETWORK_DISABLED`, and the test helper skips
network-bound tests in that environment.

Passing results observed:

- CLI `best_profile` filter: 7 passed.
- CLI `resume_best` filter: 2 passed.
- App-server `thread_profile_refresh` filter: 1 passed.
- Core `usage_limit_switches_profile_and_retries_turn` filter: 1 passed.
- `just fix -p codex-core`: passed.
- `git diff --check`: passed.

## Tests Not Run In This Session

These were not run during the final validation because of machine resource
constraints or because they were outside the last focused patch:

- Full workspace `just test`.
- Full `codex-cli` test package.
- Full `codex-app-server` test package.
- Full `codex-core` test package.
- `codex-mcp-server` cursor-session test filters.
- Snapshot review for unrelated TUI output.

Run broader tests in CI or on a larger machine before cutting a production
release.

## Recommended Future Rebase Test Matrix

Minimum local matrix:

```shell
python3 agent-coordinator/divband-codex/run.py \
  --skip-apply \
  --agents off \
  --jobs 1 \
  --test-profile focused \
  --codex-review-auth plain
```

Expanded local matrix when resources allow:

```shell
cd codex-rs
CARGO_BUILD_JOBS=2 just test -p codex-cli
CARGO_BUILD_JOBS=2 just test -p codex-core
CARGO_BUILD_JOBS=2 just test -p codex-app-server
CARGO_BUILD_JOBS=2 just test -p codex-mcp-server
```

CI/final matrix:

```shell
cd codex-rs
just test
```

Use `CARGO_BUILD_JOBS=1` or `CARGO_BUILD_JOBS=2` on machines that hang during
Rust compilation. This lowers RAM and disk pressure at the cost of longer
builds.

## Entrypoint Runtime Notes

The focused entrypoint run completed with these observed timings on a warm
shared Cargo target cache:

- Binary build: 1.3 seconds.
- `just fmt`: 6.8 seconds.
- CLI `best_profile`: 7 passed in 1.5 seconds.
- CLI `resume_best`: 2 passed in 1.3 seconds.
- App-server `thread_profile_refresh`: 1 passed in 1.8 seconds.
- Core profile failover: 1 passed in 98.5 seconds.
- MCP cursor-session: 10 passed in 245.4 seconds.

The binary build completing does not mean test compilation is over. Cargo uses
separate test artifacts for `nextest`, and the core/MCP test filters can still
compile large parts of the workspace.

Use the runner's selective step flags for reruns:

```shell
python3 agent-coordinator/divband-codex/run.py \
  --skip-apply \
  --agents off \
  --skip-copy-artifacts \
  --only-step test-core-profile-failover
```

Use `--test-profile focused-no-mcp` when MCP code has not changed and the
cursor-session compile cost is not needed for the current validation.

By default the runner points tests at an isolated managed-profile root under
`.divband-migration/profiles-empty`. This prevents app-server profile refresh
tests from seeing a real local profile pool. Pass `--use-real-profiles` or
`--profiles-dir` only when the run is intentionally validating real managed
profiles.

The shared Cargo target cache trades disk for speed. During validation it grew
to tens of gigabytes. The copied debug binaries can also be several gigabytes;
use `--skip-copy-artifacts` for test-only reruns.

## Feature-Specific Assertions

Profile manager:

- `codex profiles list` can find managed profiles.
- `codex profiles limits --refresh` writes/updates limit cache files.
- `codex profiles best --refresh` picks the least constrained usable profile.
- `codex --best` prepares a project home and failover candidates.

Runtime failover:

- A usage-limit response switches to the next profile.
- The active profile is marked limited.
- Auth is reloaded before retry.
- The model-client session is reset before retry.
- No retry occurs when candidates are exhausted.

Manual refresh:

- `/refresh-profile` sends `Op::RefreshProfileAuth`.
- `thread/profile/refresh` returns an empty response and emits a warning.
- The operation does not emit `turn/started`.

AvalAI:

- `--avalai` adds provider/model/base-url/env-key overrides.
- `--avalai` conflicts with OSS/local provider modes.
- Exec resume accepts the global flag after the subcommand.

Project homes:

- Project ids are validated.
- Markers prevent reusing the same project id for another root.
- Auth copies from the base home to the project home.

Cursor MCP:

- `tools/list` exposes `cursor-session`.
- Missing arguments return a structured error.
- With a fake `cursor-agent`, the tool runs from the configured workspace and
  returns structured output.
