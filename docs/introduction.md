<div class="pando-hero">
<svg class="pando-grove" viewBox="0 0 560 124" aria-hidden="true" focusable="false" fill="none" stroke-width="2" stroke-linecap="round"><path class="trunk" d="M14 118V51"/><circle class="tip" cx="14" cy="51" r="3.2"/><path class="trunk" d="M36 118V73"/><circle class="tip" cx="36" cy="73" r="3.2"/><path class="trunk" d="M58 118V42"/><circle class="tip" cx="58" cy="42" r="3.2"/><path class="trunk" d="M80 118V86"/><circle class="tip" cx="80" cy="86" r="3.2"/><path class="trunk" d="M102 118V83"/><circle class="tip" cx="102" cy="83" r="3.2"/><path class="trunk" d="M124 118V80"/><circle class="tip" cx="124" cy="80" r="3.2"/><path class="trunk" d="M146 118V46"/><circle class="tip" cx="146" cy="46" r="3.2"/><path class="trunk" d="M168 118V85"/><circle class="tip" cx="168" cy="85" r="3.2"/><path class="trunk" d="M190 118V28"/><circle class="tip" cx="190" cy="28" r="3.2"/><path class="trunk" d="M212 118V65"/><circle class="tip" cx="212" cy="65" r="3.2"/><path class="trunk" d="M234 118V88"/><circle class="tip" cx="234" cy="88" r="3.2"/><path class="trunk" d="M256 118V81"/><circle class="tip" cx="256" cy="81" r="3.2"/><path class="trunk" d="M278 118V37"/><circle class="tip" cx="278" cy="37" r="3.2"/><path class="trunk" d="M300 118V39"/><circle class="tip" cx="300" cy="39" r="3.2"/><path class="trunk" d="M322 118V84"/><circle class="tip" cx="322" cy="84" r="3.2"/><path class="trunk" d="M344 118V62"/><circle class="tip" cx="344" cy="62" r="3.2"/><path class="trunk" d="M366 118V81"/><circle class="tip" cx="366" cy="81" r="3.2"/><path class="trunk" d="M388 118V38"/><circle class="tip" cx="388" cy="38" r="3.2"/><path class="trunk" d="M410 118V85"/><circle class="tip" cx="410" cy="85" r="3.2"/><path class="trunk" d="M432 118V77"/><circle class="tip" cx="432" cy="77" r="3.2"/><path class="trunk" d="M454 118V64"/><circle class="tip" cx="454" cy="64" r="3.2"/><path class="trunk" d="M476 118V85"/><circle class="tip" cx="476" cy="85" r="3.2"/><path class="trunk" d="M498 118V42"/><circle class="tip" cx="498" cy="42" r="3.2"/><path class="trunk" d="M520 118V86"/><circle class="tip" cx="520" cy="86" r="3.2"/><path class="trunk" d="M542 118V64"/><circle class="tip" cx="542" cy="64" r="3.2"/><path class="root" d="M4 118H556" stroke-width="2.5"/></svg>

<h1 class="pando-hero-title">One root. Thousands of agents.</h1>

<p class="pando-lede">Pando is an open protocol and reference server for source control at agent
scale. Commits, rebases and history run on the server, so hundreds of agents land on one branch
at once with no merge queue, a clone fetches only the files you ask for, and CI reads files
without checking anything out. It runs beside GitHub, or instead of it.</p>
</div>

## Start here

<ul class="pando-paths">
<li><a href="getting-started.html"><span class="pando-path-title">Try it in 5 minutes</span><span class="pando-path-text">Install, run a server on your laptop, import a git repository and land your first change.</span></a></li>
<li><a href="agents.html"><span class="pando-path-title">Run agents on it</span><span class="pando-path-text">Give each agent a URL and a token over MCP. It reads, writes and lands without a clone.</span></a></li>
<li><a href="mirror.html"><span class="pando-path-title">Beside GitHub</span><span class="pando-path-text">Mirror every landed change to GitHub, GitLab or Bitbucket, or move history, pull requests and issues across in one command.</span></a></li>
<li><a href="deploy.html"><span class="pando-path-title">Self-host</span><span class="pando-path-text">One binary: SQLite on one machine, or Postgres and S3 behind caches. Sign-in, backups, monitoring.</span></a></li>
<li><a href="reference/protocol.html"><span class="pando-path-title">Protocol reference</span><span class="pando-path-text">Every gRPC call in <code>pando.v1</code>, for clients in any language.</span></a></li>
<li><a href="limits.html"><span class="pando-path-title">Pando Cloud</span><span class="pando-path-text">A hosted Pando is coming. Until then, the same server runs on your own machines, with no feature gates.</span></a></li>
</ul>

Also: [migrate from GitHub](migrate.md), the [CLI reference](reference/cli.md), the
[server reference](reference/server.md), and the [live console demo](https://kabirnarang39.github.io/pando/console/),
which simulates an organisation of agents in your browser.

## How it differs from git

| | git | Pando |
|---|---|---|
| Clone | downloads all history | downloads one tree, lazily, only the paths you ask for |
| Commit | built locally, pushed as packs | the server builds it from the list of changed files |
| Concurrent pushes to `main` | one wins, the rest fetch, rebase and retry | the server rebases changes that touch different files and lands them all ([how](landing.md)) |
| Reading a file in CI | clone first | one call: `pando cat main path` |
| `log`, `blame`, `diff` | need local history | server APIs; ancestry in O(log n) |
| Integrity | SHA-1 | BLAKE3, verified at every cache tier |

Where git wins: working offline with full history. Pando needs the server for history,
commits and landing. The [benchmarks](comparisons/git-vs-pando.md) give the measured numbers,
the method and the limits.

Plain git keeps working against a Pando server through `git-remote-pando`; see
[Coming from git](git-users.md).
