# Agents and MCP

## Remote MCP: any agent, no checkout

`pando mcp --http` serves a whole Pando server to agents anywhere over the
[Model Context Protocol](https://modelcontextprotocol.io) (the stateless 2026-07-28 version). An
agent needs a URL and a token, nothing else: it reads, searches, writes and lands files, opens
and reviews change requests, reports CI checks and reads its inbox, without cloning anything.

```sh
pando mcp --http 0.0.0.0:7430 --server https://pando.example.com --allowed-host mcp.example.com
pando token create claude --as agent/claude-01 --repo lumen/fleet --role write   # one token per agent
```

Connect an agent with the URL and its token:

```sh
claude mcp add --transport http pando https://mcp.example.com/mcp \
  --header "Authorization: Bearer $PANDO_TOKEN"                  # Claude Code
```

Claude Desktop, Cursor, ChatGPT, Gemini CLI and other clients that speak MCP over HTTP take
the same URL and header.

- **The token decides everything.** The gateway passes each request's token to the server and
  holds no credentials of its own, so an agent can do exactly what its token allows: limited to
  some repositories, a highest role, an expiry, every action in the audit log.
- **Protected branches still hold.** When an agent writes to one, the change goes to a new
  `review/` branch and a change request opens, asking the person the agent answers to.
- **Stateless:** run as many gateways as you like behind a load balancer; nothing is shared.

| Tool | What it does |
|---|---|
| `pando_repositories` | repositories you can read, with today's activity |
| `pando_read_file`, `pando_list_directory`, `pando_find_files` | read code at any branch or change |
| `pando_history`, `pando_blame`, `pando_diff` | who changed what, and when |
| `pando_write` | write or delete files and land them as one change (or open a change request on a protected branch) |
| `pando_change_requests`, `pando_change_request` | change requests and what blocks each |
| `pando_open_change_request`, `pando_comment`, `pando_review` | propose, discuss, approve or request changes |
| `pando_land_change_request` | land once nothing blocks it |
| `pando_report_check` | report a CI result on a change request |
| `pando_issues`, `pando_issue`, `pando_open_issue`, `pando_comment_on_issue`, `pando_update_issue` | work to do: read, open, discuss, close, assign |
| `pando_inbox` | what needs you: assigned issues, requested reviews, changes requested on yours |

### The agent loop

1. Someone assigns an issue to an agent (`pando issue assign 12 agent/fix-01`, or in the
   console). An `issue` webhook can wake the agent's runner at once.
2. The agent calls `pando_inbox`, reads the issue with `pando_issue`, and writes the fix on a
   branch with `pando_write`.
3. It opens a change request saying "Fixes #12" with `pando_open_change_request`; the person it
   answers to is asked to review.
4. After approval and passing checks it lands with `pando_land_change_request`, and the issue
   closes itself.

## Local MCP, in a workspace

`pando mcp` serves Pando as [Model Context Protocol](https://modelcontextprotocol.io)
tools over stdio, so coding agents read, commit and land without shelling out:

```sh
claude mcp add pando -- pando mcp      # Claude Code; other MCP clients work the same way
```

Run it inside a workspace. The tools:

| Tool | What it does |
|---|---|
| `pando_cat` | read a file at any revision |
| `pando_ls` | list a directory at any revision |
| `pando_find` | find files whose path contains a string |
| `pando_log` | history, optionally for one path |
| `pando_blame` | which change last touched each line |
| `pando_diff` | diff between two revisions |
| `pando_status` | uncommitted changes in the workspace |
| `pando_commit` | create a changeset from workspace changes |
| `pando_land` | land committed work on the branch |
| `pando_sync` | update the workspace to the branch head |

## Many agents on one branch

Agents can all land on `main` at once. The server rebases changes that touch different
files and lands them in batches, so an agent never has to pull, rebase and retry. A change
that really conflicts is refused with the conflicting paths, and the branch is untouched.
On one laptop, 1,000 agents landing at once sustain about 500 changes a second; see
[Benchmarks](comparisons/git-vs-pando.md).

## SDKs

Read files, list directories, search and inspect history from your own tools, without a
clone. Every client verifies content against its BLAKE3 id.

| Language | Package | Example |
|---|---|---|
| Python | [`sdk/python`](https://github.com/kabirnarang39/pando/tree/main/sdk/python) | `Client(url).read_file("src/lib.rs")` |
| TypeScript | [`sdk/typescript`](https://github.com/kabirnarang39/pando/tree/main/sdk/typescript) | `await new Client(url).readFile("src/lib.rs")` |
| Go | [`sdk/go`](https://github.com/kabirnarang39/pando/tree/main/sdk/go) | `c.ReadFile(ctx, "main", "src/lib.rs")` |

For any other language, generate a client from
[`pando.proto`](https://github.com/kabirnarang39/pando/blob/main/crates/pando-proto/proto/pando.proto).
Object encodings are in
[`pando_objects.proto`](https://github.com/kabirnarang39/pando/blob/main/crates/pando-proto/proto/pando_objects.proto).
