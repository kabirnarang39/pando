# The web console

The console is a web app for watching and reviewing work on a Pando server: browse code with
blame and history, follow every change as it lands, see who (person or agent) landed what,
review change requests, and see which files are locked. It runs entirely in the browser and
talks to `pandod` directly over gRPC-Web; there is no backend of its own.

**Try it:** the [live demo](https://kabirnarang39.github.io/pando/console/) runs a simulated
organisation in your browser: four repositories, a dozen agents landing changes every few
seconds, change requests with reviews, checks and conflicts. Nothing leaves your machine.

## Connect it to your server

1. Allow the site that serves the console to call your server:

   ```sh
   PANDO_TOKENS="me:SECRET" pandod server --listen 0.0.0.0:7440 --web-origins https://console.example.com
   ```

   `--web-origins` (or `PANDO_WEB_ORIGINS`) takes a comma-separated list. Browser access is
   the `web` feature flag, on by default; turn it off with `PANDO_FLAGS=-web`.
2. Open the console, choose **Connection**, and enter the server's address and, if the server
   requires one, a token. The token stays in the tab unless you choose to remember it.

A link of the form `https://console.example.com/?server=https://pando.example.com` opens the
console already pointed at a server.

## Host it yourself

The console is static files. Build it once and serve `console/dist` from anywhere (an object
store, a CDN, nginx, GitHub Pages):

```sh
cd console && pnpm install && pnpm build
```

URLs use the fragment (`/#/server/repo/changes`), so no server-side routing is needed and deep
links work from any path.

## What works where

| Feature | `pandod` | `pandod --platform` | Coming next |
|---|---|---|---|
| Code, blame, file history, search by path | yes | yes | |
| Changes as they land, word-level diffs | yes | yes | |
| Locks, branch switching, fleet timeline | yes | yes | |
| Many repositories, grouped by owner | one repository | yes | |
| People, agents, teams, roles and tokens | | yes (3.2) | |
| Change requests, reviews, line comments, land gate, rebase | | yes (3.3) | |
| Inbox, activity, protected branches | | yes (3.3) | |

The console asks the server what it supports and shows only that: a server without hosting
shows its one repository, and a server without review hides reviews and the inbox. The demo shows
everything. The platform contract is
[`pando_platform.proto`](https://github.com/kabirnarang39/pando/blob/main/crates/pando-proto/proto/pando_platform.proto);
the plan is in [Platform](platform.md).

## Keyboard

| Keys | Does |
|---|---|
| ⌘K or Ctrl K, `/` | search files, repositories, reviews and actions |
| `t` | go to a file in this repository |
| `g` then `h`, `i`, `c`, `l`, `r`, `k` | home, inbox, code, changes, reviews, locks |
| `j`, `k` | next and previous item in a list |
| `n`, `p` | next and previous file in a review |
| `a` | review the change request |
| `b`, `h`, `u` | blame, file history, unified or split diff |
| `?` | all shortcuts |

## Performance and privacy

- The first screen needs 218 KB of gzipped JavaScript; each screen after that loads its own
  code on first visit (and is preloaded on hover). The build fails if the first screen grows
  past its budget.
- Syntax highlighting runs in a worker. Parts of a file far from the screen are plain text until
  they come near it, yet every line stays in the page, so the browser's find works: a
  50,000-line (1.5 MB) file opens in under a second in the end-to-end tests. Files over 1 MB
  show without colours, and say so.
- Fonts and icons are bundled: the console makes no request to any third party.
- Untrusted content (READMEs, descriptions, comments) is rendered without HTML: markdown is
  turned into elements, raw HTML shows as text, and only http(s), mailto and relative links
  are followed.
