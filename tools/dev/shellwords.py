"""A shell command line's commands, as the PreToolUse hooks read them (tools/dev/test-guard.py,
tools/dev/git-guard.py): commands, not text. A command starts at the start, after an unquoted
; & | ( ) or newline, or after a prefix (env, time, nice, command, xargs, exec, nohup). Quoted
text is looked into only where the hook asks (the script of sh/bash/zsh -c, eval, python -c)."""
import os, re


def strip_heredocs(t):
    return re.sub(r"<<-?\s*(['\"]?)(\w+)\1[^\n]*\n.*?\n\s*\2[ \t]*(?=\n|$)", "<<HEREDOC", t, flags=re.S)
def sub_end(t, i):
    """Where the substitution starting at t[i] ($( … ), <( … ), >( … ) or ` … `) ends, and its body's start.
    Quotes and substitutions inside it are read as the shell reads them: $(echo ")"; …) ends at the last )."""
    bt = t[i] == "`"
    b = i + (1 if bt else 2)
    j, depth, q = b, 1, None
    while j < len(t):
        ch = t[j]
        if q == "'":
            q = None if ch == "'" else q
        elif ch == "\\": j += 2; continue
        elif bt:
            if ch == "`": return j + 1, b
        elif t.startswith("$(", j) or ch == "`": j, _ = sub_end(t, j); continue
        elif q: q = None if ch == '"' else q
        elif ch in "'\"": q = ch
        elif ch == "(": depth += 1
        elif ch == ")":
            depth -= 1
            if not depth: return j + 1, b
        j += 1
    return len(t), b
def substitutions(t, pwd=False):
    """(t with each $( … ), <( … ), >( … ) and ` … ` outside single quotes made one word, ${SUBSTITUTION}
    (with pwd, $(pwd) as $PWD), and the scripts inside them): a command's words stay together
    for the checks, and the scripts are checked on their own. <( and >( count outside quotes only."""
    out, inner, q, i = [], [], None, 0
    while i < len(t):
        ch = t[i]
        if q == "'":
            q = None if ch == "'" else q
        elif ch == "\\" and i + 1 < len(t): out.append(t[i:i + 2]); i += 2; continue
        elif ch == "'" and not q: q = "'"
        elif ch == '"': q = None if q == '"' else '"'
        elif t.startswith("$(", i) or ch == "`" or (not q and ch in "<>" and t.startswith("(", i + 1)):
            j, b = sub_end(t, i)
            body = t[b:j - 1]
            inner.append(body)
            out.append("$PWD" if pwd and body.strip() == "pwd" else "${SUBSTITUTION}")
            i = j; continue
        out.append(ch); i += 1
    return "".join(out), inner
def segments(t):
    out, cur, q, i = [], [], None, 0
    while i < len(t):
        ch = t[i]
        if q:
            cur.append(ch)
            if ch == "\\" and q == '"' and i + 1 < len(t): cur.append(t[i + 1]); i += 1
            elif ch == q: q = None
        elif ch == "\\" and i + 1 < len(t): cur.append(ch); cur.append(t[i + 1]); i += 1
        elif ch in "'\"": q = ch; cur.append(ch)
        elif t.startswith("$(", i) or ch == "`" or (ch in "<>" and t.startswith("(", i + 1)):   # a substitution stays in its command
            j, _ = sub_end(t, i); cur.append(t[i:j]); i = j; continue
        elif ch == "&" and ((cur and cur[-1] in "<>") or t[i + 1:i + 2] == ">"): cur.append(ch)   # 2>&1, &>
        elif ch == "|" and cur and cur[-1] == ">": cur.append(ch)   # >|
        elif ch in ";&|()\n": out.append("".join(cur)); cur = []
        else: cur.append(ch)
        i += 1
    out.append("".join(cur))
    return [s for s in out if s.strip()]
def tokens(seg):
    """The words of a command segment, as the shell reads them, minus its redirections: an unquoted
    >, >>, <, <<, <<<, <>, >&, <&, &>, &>>, and zsh's >| >! >>| >>! &>| &>! &>>| &>>!, with an fd
    number before it (2>, unquoted) and its target (a word, or the fd after >& when it's the whole
    word: 2>&1, >&-), with or without a space before the target, are dropped; quoted text never is.
    A backslash-newline joins lines, as in the shell."""
    out, cur, has, lit, q, i, skip = [], [], False, True, None, 0, False   # lit: no quote or \ in the word
    def end():
        nonlocal cur, has, lit, skip
        if has:
            if skip: skip = False
            else: out.append("".join(cur))
        cur, has, lit = [], False, True
    while i < len(seg):
        ch = seg[i]
        if q:
            if ch == q: q = None
            elif ch == "\\" and q == '"' and seg[i + 1:i + 2] == "\n": i += 1
            elif ch == "\\" and q == '"' and seg[i + 1:i + 2] in ('"', "\\", "$", "`"): i += 1; cur.append(seg[i])
            else: cur.append(ch)
        elif ch == "\\" and seg[i + 1:i + 2] == "\n": i += 1   # a line continuation
        elif ch == "\\" and i + 1 < len(seg): i += 1; cur.append(seg[i]); has, lit = True, False
        elif ch in "'\"": q = ch; has, lit = True, False
        elif ch.isspace(): end()
        elif ch == "#" and not has: break   # a comment
        elif ch in "<>" or (ch == "&" and seg[i + 1:i + 2] == ">"):
            if has and lit and "".join(cur).isdigit(): cur, has = [], False   # the fd number: 2> ("2"> is a word)
            else: end()
            m = re.compile(r"&>>?[|!]?|>>[|!]?|<<<|<<-?|<>|[<>]&|>[|!]|[<>]").match(seg, i)
            i = m.end() - 1
            if m.group().endswith("&"):   # >&1, >&-: the target is an fd; >&file, >&1x: a word
                f = re.compile(r"(\d+-?|-)(?=[\s;&|()<>]|$)").match(seg, i + 1)
                if f: i = f.end() - 1
                else: skip = True
            else: skip = True
        else: cur.append(ch); has = True
        i += 1
    end()
    return out
PREFIX_TAKES = {"nice": {"-n"}, "env": {"-u", "-C", "-S"}, "xargs": {"-I", "-n", "-L", "-P", "-d", "-E", "-s"}, "time": set(), "command": set(), "exec": set(), "nohup": set()}


def command(toks):
    """(assignments {name: value}, words) of a segment's words: its leading NAME=value words and
    prefixes (env, time, nice, command, xargs, exec, nohup, with their options) taken off."""
    env = {}
    while True:
        while toks and re.match(r"^\w+=", toks[0]):
            k, v = toks[0].split("=", 1); env[k] = v; toks = toks[1:]
        if not (toks and os.path.basename(toks[0]) in PREFIX_TAKES):
            return env, toks
        takes = PREFIX_TAKES[os.path.basename(toks[0])]; toks = toks[1:]
        while toks and (toks[0].startswith("-") or re.match(r"^\w+=", toks[0])):
            if re.match(r"^\w+=", toks[0]): k, v = toks[0].split("=", 1); env[k] = v
            toks = toks[2:] if toks[0] in takes else toks[1:]
