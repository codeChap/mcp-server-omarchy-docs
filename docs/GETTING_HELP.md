# Where to find help with Omarchy

Ranked by usefulness-to-response-time. The escalation ladder for AI agents is
in [AGENT_PLAYBOOK.md](AGENT_PLAYBOOK.md); this page is for humans.

| # | Channel | Response time | Good for | Before you ask |
|---|---------|---------------|----------|----------------|
| 1 | Manual Troubleshooting chapter + local tools | Instant | Update breakage ("I broke my system with an update" → limine **snapshot rollback** at boot menu), diagnostics via **`omarchy-debug`**, nuclear option: *Setup > Reset Computer* factory reset | Run `omarchy-debug` and read its output first. It prints versions, logs, and common failure checks. |
| 2 | Arch Wiki + Hyprland wiki (wiki.hypr.land) | Instant | Anything below Omarchy: pacman, kernels, drivers, Hyprland option semantics | Omarchy overrides upstream defaults (Lua config layer!) — verify against shipped configs before applying wiki advice verbatim. |
| 3 | github.com/omacom/omarchy — closed issues | Instant (read) | "Has anyone hit this?" — densest archive of solved problems | Always search before posting anywhere. |
| 4 | Omarchy Discord — discord.gg/tXFUdasqhY | Minutes–hours | Quick questions, config help, plugin/theme dev, "is this known-broken in 4.0.0?" | Search channel history; state version + attach `omarchy-debug` output in the first message. |
| 5 | GitHub issues (posting) | Days | Reproducible bugs, regressions, feature requests | Reproduce on default configs if possible; include version, `omarchy-debug` output, exact steps. Not a support forum. |
| 6 | omarchy.org/themes + omarchyplugins.com | Instant | Theme catalog; plugin marketplace plus develop/publish guides (indexed in this KB as `plugin-dev`) | Read a shipped plugin's source before asking how plugins work. |

## The one command to know

```bash
omarchy-debug
```

Collects system info, versions, service state, and logs into a form you can
paste into Discord or an issue. Run it before asking anyone anything.
