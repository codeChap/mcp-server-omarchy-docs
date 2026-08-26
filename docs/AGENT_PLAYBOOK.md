# Escalation ladder — what to do when the KB has no answer

Follow in order. Stop as soon as you have a confident, sourced answer.

## L0 — Retry the local KB (free)

- Reformulate with Omarchy's exact tool/file names: `omarchy-debug`,
  `omarchy-menu`, `hyprland.lua`, `Super+Space`.
- Single distinctive nouns beat phrases; check `did_you_mean` in zero-result
  responses.
- Config questions → `omarchy_find_config`; version questions →
  `omarchy_whats_new` / `omarchy_diff_releases`.
- Brand-new features (Quickshell shell, plugin system, Lua configs) may exist
  **only** in v4.0.0 release notes or commit history, not yet in manual prose.

## L1 — Omarchy source code (seconds; usually the answer)

The repo is the documentation for internals.

- Installed system live checkout: `~/.local/share/omarchy` (or wherever your
  install puts it).
- `bin/` — every `omarchy-*` CLI command is a readable script.
- `config/`, `default/` — shipped defaults incl. the Lua Hyprland configs.
- Git history: `git -C ~/.local/share/omarchy log --oneline --grep=<term>` —
  design rationale lives in commit messages.
- Plugin development: search the local `plugin-dev` source first (indexed from
  omarchyplugins.com/develop.html and publish.html). Then read real plugins
  from `$OMARCHY_PATH/shell/plugins/` as executable documentation.

## L2 — Upstream docs (seconds–minutes; needs web)

- **Quickshell**: quickshell.org/docs — QML API for the v4 shell. Omarchy's
  own usage of it is documented only by its QML files (L1).
- **Hyprland**: wiki.hypr.land — underlying options and dispatchers. The Lua
  config layer is Omarchy-specific: the wiki documents options, the Omarchy
  repo documents the wrapper. Map between them.
- **Arch Wiki**: wiki.archlinux.org — pacman, kernels, drivers, limine,
  mkinitcpio, everything below Omarchy.

Curated starting points are attached to search results as `see_also`
(data/upstream_map.json).

## L3 — GitHub (minutes to read; days if posting)

- Search **closed issues first**: https://github.com/basecamp/omarchy/issues
  (`is:issue is:closed <term>`) — most breakage is already solved there.
- Recent commits/PRs for features newer than the KB snapshot (check freshness
  via `omarchy_kb_info`, refresh via scripts/fetch_sources.sh + build_kb.py).

## L4 — Humans (the agent cannot post — hand off to the user)

- **Discord** discord.gg/tXFUdasqhY — fastest human answers.
- **GitHub issues** — reproducible bugs only; not a support forum.

Hand the user a ready-to-post message containing: Omarchy version
(`omarchy-debug` prints it), what was already tried from L0–L3, the exact
error, and `omarchy-debug` output.

## Hard rule

Never invent Omarchy-specific behavior from general Arch/Hyprland knowledge.
Omarchy overrides upstream defaults heavily. If L0–L1 don't confirm something,
label it unverified and cite the upstream source you're extrapolating from.
