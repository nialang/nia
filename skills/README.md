# Skills

Instructions for AI coding assistants that write Nia. They follow the
[Agent Skills](https://agentskills.io) layout: each skill is a directory
whose `SKILL.md` starts with a name and a description of when it applies.

| Skill | Use it for |
| --- | --- |
| [writing-nia](writing-nia/SKILL.md) | Writing, editing, or reviewing Nia code: what to read first, the workflow, and the habits from Rust and C that do not carry over |

## Installing

Copy a skill's directory to where your assistant looks for skills. For
Claude Code that is `~/.claude/skills/` for every project, or `.claude/skills/`
inside one project:

```sh
cp -r skills/writing-nia ~/.claude/skills/
```

An assistant that loads skills uses one when its description matches the
task. One that does not can be given `SKILL.md` as part of its instructions.
Copy the skill again after updating Nia, so that it matches the toolchain's
documentation.

## Maintaining

A skill holds no rules of its own. The rules of the language and the style
belong to the [documentation](../docs/README.md), above all the guide
[Writing Idiomatic Nia](../docs/guides/writing-nia.md); a skill says what to
read, how to work, and which mistakes to look for, and links to the section
that owns each rule. Change the documentation first, then the skill's
checklist when a habit it lists changes.
