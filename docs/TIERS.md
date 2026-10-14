# Trust model: result tiers

Search results interleave four tiers, always in this priority order — a manual
hit outranks a config hit regardless of raw text score.

## Tier 1 — `core` (manual, repo-docs, plugin-dev)

Authoritative for **v4.0.3 behavior as shipped**. The manual is generated from
the same markdown that renders omarchy.org/manual; repo-docs are the
maintainers' internal design docs (theming engine, CLI router, update process).
`plugin-dev` is the community marketplace authoring tutorial
([develop.html](https://omarchyplugins.com/develop.html) and
[publish.html](https://omarchyplugins.com/publish.html)): clone, manifest
kinds, QML bar/panel examples, validate/run, and listing a plugin. Trust it
for *how to build a third-party plugin*; the official shell contract still
lives in the repo (`shell/README.md`, `manual/32-shell-plugins.md`).

## Tier 2 — `configs`

The **shipped default** config files (hypr/*.lua, foot.ini, tmux.conf, /etc
overrides). Authoritative for defaults only — the user's `~/.config` copies may
have been modified since install. Say so when answering from this tier:
"the default is X; your local copy may differ."

## Tier 3 — `releases`

Historically accurate **for the version they describe**, but superseded by
later releases. A v2.x note describing bash-based Hyprland configs describes a
world that no longer exists in v4.0.0 (Lua now). Never answer from a release
note alone without checking the manual; use `omarchy_whats_new` /
`omarchy_diff_releases` for version questions instead of search.

## Tier 4 — `history` (git commits)

One row per non-merge commit from basecamp/omarchy — DHH's commit messages are
often the only documentation of *why* something works the way it does, and of
features newer than the last release tag. Treat as evidence of intent and
recency, not user-facing documentation.
