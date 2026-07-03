# Divband Codex Overlay

This directory is a migration package for carrying the Divband Codex changes on
top of future upstream `openai/codex` releases.

Snapshot metadata:

- Generated on: 2026-07-03
- Local upstream at generation time: `origin/main` = `da4c8ca5`
- Divband branch tip covered by the patch set: `73ef8a6b`
- Merge base used for the replay diff: `0ccb676d`
- Branch commits covered: `775880c9`, `278d94d8`, `2b00b288`,
  `83985d68`, `12986f5e`, `759f7720`, `73ef8a6b`

Important distinction: this package documents and exports the overlay from
`0ccb676d..73ef8a6b`. The `divband-codex` migration package itself was added
after that snapshot so the replay patch does not recursively include this
directory.

## Artifacts

- `divband-codex.patch`: single binary-safe replay patch for the overlay.
- `patches/*.patch`: same changes split by commit for easier conflict handling.
- `diffstat.txt`: high-level size and file footprint.
- `file-inventory.txt`: changed file list with add/modify status.
- `FEATURES.md`: comprehensive feature list and behavior explanation.
- `IMPLEMENTATION.md`: code ownership map by crate/module.
- `REBASE_PLAYBOOK.md`: how to apply the overlay to a new upstream release.
- `TESTING.md`: tests already run and tests to rerun after future rebases.

## Research Provenance

This audit used local Git history and source inspection. A live Cursor subagent
was not mounted in this chat session, so it was not used as evidence for this
documentation.

The branch does add a Cursor-backed MCP tool named `cursor-session`. That tool
is documented as part of the overlay and can be used during future migration
research when a Cursor login profile is mounted and `cursor-agent` is available.

## Quick Reapply

From a fresh upstream checkout, copy this directory somewhere outside the new
worktree or keep a path to the old worktree, then prefer the per-commit patches:

```shell
git fetch origin main
git switch -c divband-codex origin/main
git am /path/to/divband-codex/patches/*.patch
```

If preserving individual commits is less important than getting a conflictable
working tree:

```shell
git apply --3way /path/to/divband-codex/divband-codex.patch
git add -A
git commit -m "feat: apply divband codex overlay"
```

After applying, follow `REBASE_PLAYBOOK.md` and `TESTING.md`.

## Running The Entrypoint

The single entrypoint is:

```shell
python3 agent-coordinator/divband-codex/run.py
```

For an already-applied output repository, the deterministic validation command
used for this package is:

```shell
python3 agent-coordinator/divband-codex/run.py \
  --skip-apply \
  --agents off \
  --jobs 1 \
  --test-profile focused \
  --codex-review-auth plain
```

Operational defaults:

- The runner uses a shared Cargo target directory at
  `agent-coordinator/divband-codex/.cache/cargo-target` so repeated runs do not
  rebuild the whole workspace.
- Test and review commands use an isolated managed-profile root at
  `.divband-migration/profiles-empty` unless `CODEX_PROFILES_DIR`,
  `--profiles-dir`, or `--use-real-profiles` is provided.
- `--jobs 1` keeps Rust memory pressure lower. It is slower on cold caches but
  is less likely to hang a small machine.
- `--agents off` skips Cursor and Codex review agents. Use this for production
  migration validation when the agent credentials are not part of the test.
- `--codex-review-auth best --use-real-profiles` is only appropriate when a
  valid managed profile pool is mounted and the run is intended to exercise
  Codex review through `--best`.

Useful faster reruns:

```shell
# Rerun only the core failover test after a failure.
python3 agent-coordinator/divband-codex/run.py \
  --skip-apply \
  --agents off \
  --skip-copy-artifacts \
  --only-step test-core-profile-failover

# Rerun the final two focused tests.
python3 agent-coordinator/divband-codex/run.py \
  --skip-apply \
  --agents off \
  --skip-copy-artifacts \
  --only-step test-core-profile-failover \
  --only-step test-mcp-cursor-session

# Skip the cursor-session MCP test when MCP code was not touched.
python3 agent-coordinator/divband-codex/run.py \
  --skip-apply \
  --agents off \
  --skip-copy-artifacts \
  --test-profile focused-no-mcp
```

Artifacts and reports:

- Final report: `.divband-migration/REPORT.md`.
- Command logs: `.divband-migration/logs/`.
- Copied debug binaries: `.divband-migration/bin/`.

The copied binaries are unstripped debug artifacts and can be several
gigabytes. Use `--skip-copy-artifacts` for validation runs where the already
built binaries do not need to be copied into the report directory.
