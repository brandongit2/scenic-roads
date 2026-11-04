# Sourced by Claude Code (zsh) before each Bash command (CLAUDE_ENV_FILE in .claude/settings.local.json;
# tools/dev/testing.md): puts tools/dev/bin first on the PATH, so `cargo` in an agent worktree builds
# into this Mac's shared build folder, one at a time (tools/dev/bin/cargo, #915). Elsewhere that
# cargo passes straight through. Its folder from $CLAUDE_ENV_FILE: Claude Code runs this file's text,
# where %x names no file (it gave <the command's folder>/bin: #937's agent shell had no shim).
_scenic_bin=${${CLAUDE_ENV_FILE:-${(%):-%x}}:A:h}/bin
[[ :$PATH: == *:$_scenic_bin:* ]] || PATH=$_scenic_bin:$PATH
unset _scenic_bin
