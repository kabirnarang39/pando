# Coming from git

You can move to Pando one person at a time. git users and `pando` users work on the same
branches of the same server.

## Keep using git

With `git-remote-pando` on your `PATH` (the installer puts it there), git talks to a Pando
server directly:

```sh
git clone pando::http://127.0.0.1:7420 repo
git push          # lands on the server; if main moved, the server rebases instead of rejecting
git pull --rebase
```

Fetches are incremental. Set `PANDO_PUSH=strict` to get git's reject-if-behind behaviour
instead of a server-side rebase.

## Command map

| git | pando | Notes |
|---|---|---|
| `git clone URL` | `pando clone URL DIR` | add `--path services/api` to check out part of the repository |
| `git status` | `pando status` | alias `st` |
| `git add` + `git commit -m` | `pando commit -m MSG [PATHS]` | no staging area; pass paths to commit a subset |
| `git push` | `pando land` | alias `push`; the server rebases if `main` moved |
| `git pull` | `pando sync` | alias `pull` |
| `git switch -c x` / `git switch x` | `pando checkout -b x` / `pando checkout x` | |
| `git log -- path` | `pando log path` | answered by the server, no local history |
| `git diff` / `git diff A B` | `pando diff` / `pando diff A B` | |
| `git show REV` | `pando show REV` | |
| `git blame f` | `pando blame f` | |
| `git show REV:path` | `pando cat REV path` | works without a clone |
| `git ls-tree REV dir` | `pando ls REV [dir]` | works without a clone |
| `git branch` | `pando refs` | alias `branch` |
| `git merge-base A B` | `pando merge-base A B` | |
| (none) | `pando find PATTERN` | path search on the server |
| `git lfs lock f` / `git lfs unlock f` | `pando lock f` / `pando unlock f` | exclusive locks for files that cannot be merged |
| `git lfs locks` | `pando locks [dir]` | |
| (none) | `pando mount DIR --rev main` | a read-only view, files fetched on first read (Linux, macOS, Windows) |

## Moving history in and out

```sh
pando import-git ~/code/my-repo --server http://127.0.0.1:7420   # incremental; rerun any time
pando export-git ./mirror.git                                     # first-parent history, identical trees
```

## Keep GitHub, GitLab or Bitbucket up to date

`pando mirror` keeps a git repository current with a branch, so CI, code search, issue links
and anything else attached to your git host keep working while agents land in Pando:

```sh
pando mirror --to git@github.com:acme/fleet.git --every 5        # runs until stopped
pando mirror --to https://bitbucket.org/acme/fleet.git           # once, for cron or CI
```

- Every landed change becomes one git commit with its author, message and date. The same
  history always makes the same commits, so a mirror can be rebuilt from scratch at any time.
- It is incremental: each round pushes only what landed since the last one.
- It never force-pushes. If the destination branch has commits the mirror did not make, it
  stops and says so; protect the branch on your git host so only the mirror writes to it.
- Credentials are git's own: an SSH key, or a credential helper or deploy token. Tokens in
  the URL are never printed.
- The local copy lives in your cache directory (`--dir` to choose).

Submodules are not imported. Commits are imported with their authors, messages and dates,
so `pando log` and `pando blame` show the original history.

## What works differently

- **No staging area.** `pando commit` takes paths instead of `git add`.
- **No local history.** `log`, `blame` and `diff` between revisions ask the server. You
  need the server to be reachable; this is the main trade-off against git.
- **Landing replaces push.** If `main` moved, the server rebases your change when it
  touches different files. You only resolve conflicts that are real.
