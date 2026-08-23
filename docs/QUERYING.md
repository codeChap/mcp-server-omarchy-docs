# Querying effectively

This is SQLite FTS5 keyword search, **not** semantic search. Consequences:

1. **Use Omarchy's exact vocabulary.** "wallpaper" not "background image",
   "keybind" not "keyboard shortcut", "Super+Space" not "the launcher",
   "snapshot" not "restore point", "quickshell" not "desktop shell".
   A built-in synonym list covers common swaps (shortcut→keybind,
   background→wallpaper, sleep→suspend), but exact terms always rank better.
2. **Fewer, more specific terms beat sentences.** Query `factory reset`,
   not `how do I reset omarchy back to defaults`.
3. **If 0 results:** drop to a single distinctive noun, then try a synonym.
   Zero/thin results come back with `did_you_mean` suggestions and an
   `escalation` block — read them.
4. **Config questions → `omarchy_find_config` first.** It matches file paths
   *and* contents; plain search may miss config internals.
5. **"When did X change" → `omarchy_whats_new` / `omarchy_diff_releases`,**
   never search. Release notes are tier 3 and rank below the manual on purpose.

## Tool selection

| You want to… | Use |
|---|---|
| Answer any Omarchy question | `omarchy_search_docs` |
| Read a full chapter you've found | `omarchy_get_page(title\|url\|path)` |
| Know what changed between versions | `omarchy_diff_releases(from, to)` |
| Latest release / notes for one version | `omarchy_whats_new(version?\|since?)` |
| Find where a setting/default lives | `omarchy_find_config(app)` |
| Check KB freshness before trusting it | `omarchy_kb_info()` |
