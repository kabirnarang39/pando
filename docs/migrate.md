# Migrate from GitHub

One command moves a repository from GitHub, GitLab, Bitbucket or any git host:

```sh
export PANDO_GITHUB_TOKEN=$(gh auth token)   # private repositories, and GitHub's rate limit
pando migrate https://github.com/acme/fleet --to https://pando.example.com/acme/fleet
pando migrate git@gitlab.com:acme/fleet.git --to https://pando.example.com/acme/fleet
pando migrate https://github.example.com/acme/fleet --to https://pando.example.com/acme/fleet \
  --github-api https://github.example.com/api/v3          # GitHub Enterprise Server
```

## What comes across

| | |
|---|---|
| **History** | every commit on every branch, with its author, message and date, and merges; the files at each branch tip are byte-identical to git's |
| **Pull requests** (GitHub) | each open pull request becomes a change request from a `github/pr-N` branch into its base, with its title, description and a link back |
| **Issues** (GitHub) | open and closed, oldest first, with every comment, labels, and closed as done or not planned |
| **The repository** | created if it doesn't exist, with its GitHub description |

Commit ids change (Pando addresses content its own way); each change keeps its git message
and author. Issues, pull requests and comments are posted by whoever runs the migration, each
naming its original GitHub author with a link to the original. Run it again later to bring
across new commits; history is imported incrementally.

## Keep the old host current while you move

`migrate` ends by printing the `pando mirror` command for the repository you came from:

```sh
pando mirror --to https://github.com/acme/fleet --server https://pando.example.com/acme/fleet --every 10
```

With the mirror running, CI, code search and issue links on the old host keep working while
people and agents move to Pando at their own pace. Stopping the mirror is the whole rollback.

## Tokens

- A GitHub token is needed for private repositories and to stay under GitHub's rate limit (60
  requests an hour without one). Give it in `PANDO_GITHUB_TOKEN` (or `GITHUB_TOKEN`), or in a file
  with `--github-token-file`. `--github-token` works but warns, since other users of the machine
  can see command lines. The token is sent only to the repository's own host and passed to git
  through its environment; source URLs are printed without credentials.
- `--no-issues` and `--no-pull-requests` skip those.
- The destination uses your Pando token (`PANDO_TOKEN`), which needs to be allowed to create
  repositories in the organisation (an organisation administrator, or a site administrator for a
  new organisation).
