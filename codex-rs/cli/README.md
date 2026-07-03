# Codex CLI crate notes

## Managed account profiles and `--best`

This repository includes a local `codex-profiles` helper for managing multiple
Codex account homes. A profile is just a named `CODEX_HOME` containing its own
`auth.json`; `codex --best` only selects from profiles that already exist in
that managed profile root.

`codex --best` does not create or log in to a new profile when the active
account reaches a usage limit. Before launching the interactive TUI it refreshes
ChatGPT usage for the existing managed profiles, picks the usable profile with
the lowest maximum usage across the 5-hour and weekly windows, copies that
profile's `auth.json` into the project home, and configures runtime failover
across the remaining usable candidates.

Runtime failover can only switch to another candidate from that same launch. If
the selected profile later returns a usage-limit error, Codex writes a limited
cache entry for it, copies the next candidate's `auth.json` into the project
home, reloads auth, and retries the turn. If there is no next candidate, the
usage-limit error is returned to the user.

While running a TUI session launched with `codex --best`, use
`/refresh-profile` to manually switch to the next managed profile without
starting a new turn. This keeps the current session and conversation context in
place; it only swaps the project home's `auth.json` and reloads auth.

Typical setup:

```shell
codex-profiles add main
codex-profiles add backup
codex --best
```

Useful inspection commands:

```shell
codex-profiles list
codex-profiles limits --refresh
codex-profiles best --refresh
codex-profiles home backup
```

If `codex --best` still reports a usage limit, check that more than one managed
profile exists and that at least one of them has remaining quota. To add another
candidate, create or import a profile first; `--best` will not invent one from
another ChatGPT account under the hood.
