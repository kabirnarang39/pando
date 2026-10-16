# AI agents and SDKs

## MCP

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
