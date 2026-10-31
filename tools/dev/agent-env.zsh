# Sourced by Claude Code (zsh) before each Bash command (CLAUDE_ENV_FILE in .claude/settings.local.json;
# tools/dev/testing.md): puts tools/dev/bin first on the PATH, so `cargo` in an agent worktree builds
# into this Mac's shared build folder, one at a time (tools/dev/bin/cargo, #915). Elsewhere that
# cargo passes straight through.
[[ :$PATH: == *:${${(%):-%x}:A:h}/bin:* ]] || PATH=${${(%):-%x}:A:h}/bin:$PATH
