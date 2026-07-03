# Divband Migration Toolchain

The single entry point is:

```shell
python3 agent-coordinator/divband-codex/run.py --force
```

Default paths:

- Source vanilla clone: `agent-coordinator/divband-codex/codex`
- Output migrated repo: `agent-coordinator/divband-codex-output`
- Per-commit patches: `agent-coordinator/divband-codex/patches`
- Migration logs: `agent-coordinator/divband-codex-output/.divband-migration/logs`
- Final report: `agent-coordinator/divband-codex-output/.divband-migration/REPORT.md`
- Shared Cargo cache: `agent-coordinator/divband-codex/.cache/cargo-target`
- Built binaries copied to: `agent-coordinator/divband-codex-output/.divband-migration/bin`

What the runner does:

1. Reads the feature, implementation, rebase, testing, inventory, and diffstat
   documents from this directory.
2. Copies the vanilla Codex clone to `divband-codex-output`.
3. Creates or resets branch `divband-migrated` in the output repo.
4. Applies the Divband overlay with `git am --3way`.
5. Copies the migration docs into `.divband-migration/source-docs`.
6. Runs Cursor and Codex review agents when available.
7. Builds the key Divband binaries.
8. Runs the focused validation matrix.
9. Writes `manifest.json` and `REPORT.md`.

## Resource-Friendly Run

```shell
python3 agent-coordinator/divband-codex/run.py \
  --force \
  --jobs 1 \
  --agents available \
  --test-profile focused
```

The runner uses a shared `CARGO_TARGET_DIR` by default, so deleting and
recreating `divband-codex-output` does not throw away Rust build artifacts. To
force Cargo to write `target/` inside the output repository instead, add:

```shell
--no-shared-target-dir
```

## Fast Patch-Only Run

```shell
python3 agent-coordinator/divband-codex/run.py \
  --force \
  --agents off \
  --skip-build \
  --test-profile none
```

## Rerun Build/Tests On Existing Output

After the overlay has already been applied once:

```shell
python3 agent-coordinator/divband-codex/run.py \
  --skip-apply \
  --agents off \
  --jobs 1 \
  --test-profile focused
```

This avoids copying the upstream clone and reapplying patches.

## Expanded Validation

```shell
python3 agent-coordinator/divband-codex/run.py \
  --force \
  --jobs 2 \
  --test-profile expanded \
  --run-fix
```

## Agent Integration

Agent mode is controlled by `--agents`:

- `off`: skip Cursor and Codex review agents.
- `available`: run them when commands and credentials are present, otherwise
  record a skipped step.
- `required`: fail the migration if either agent is unavailable or fails.

Cursor defaults mirror the `cursor-session` MCP tool:

- `CURSOR_SESSION_AGENT_COMMAND`, default `cursor-agent`
- `CURSOR_SESSION_HOME`, default `/cursor-home`
- `CURSOR_SESSION_MODE`, default `ask`
- `CURSOR_SESSION_MODEL`, default `auto`

The runner requires Cursor auth at:

```text
$CURSOR_SESSION_HOME/.config/cursor/auth.json
```

Codex review defaults to:

```shell
codex --best exec --sandbox read-only
```

when managed profiles are available. The runner checks:

- `CODEX_PROFILES_DIR`, if set
- otherwise `~/.config/codex-switch`
- at least one `homes/*/auth.json`

If that check fails, default `auto` mode falls back to:

```shell
codex exec --sandbox read-only
```

Control this explicitly with:

```shell
--codex-review-auth auto   # use --best when profiles are present, otherwise plain
--codex-review-auth best   # require managed profiles for the Codex review agent
--codex-review-auth plain  # never use --best for the Codex review agent
```

An explicit command still wins over that selection:

```shell
export DIVBAND_CODEX_REVIEW_COMMAND='codex exec --sandbox read-only'
```

Both agent prompts are written into:

```text
agent-coordinator/divband-codex-output/.divband-migration/prompts/
```

## Notes

The output repository is ignored by the parent repo through
`agent-coordinator/.gitignore`. The local upstream clone under
`divband-codex/codex` is also ignored.
