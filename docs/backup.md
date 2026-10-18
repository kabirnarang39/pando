# Backup and restore

Two commands cover it: `pandod copy` moves one repository between stores (a backup, a restore,
or a migration to another backend), and `pandod backup` snapshots a whole platform server while
it runs.

## Backups, restores and moving between backends

`pandod copy` copies a repository from one store to another. The same command makes a
backup, restores one, and moves a repository to another backend:

```sh
# Back up the production database to a SQLite file
pandod copy --from-metadata postgres://pando@db/pando --from-objects s3://pando-objects \
            --to-metadata sqlite:///backups/pando.db

# Move a single-node server from SQLite to Postgres and S3
pandod copy --from-metadata sqlite:///var/lib/pando/meta.db \
            --to-metadata postgres://pando@db/pando --to-objects s3://pando-objects
```

It is safe while the source is serving: branch pointers are read first and written to
the destination last, and everything they point to was stored before them, so the copy
is always consistent. Objects are copied as stored, so compacted history stays compact,
and whole objects are verified against their hashes on the way. Anything the destination
already holds is skipped, so running the same copy again only moves what is new: on
git/git's history (84,050 changesets, 330,000 objects) a first copy to SQLite takes 13 s
and a repeat 3.5 s. Run it on a schedule for incremental backups. System pointers such as
the compaction lease are not copied.

### Backing up a platform server

`pandod copy` moves one repository. A platform server also keeps a registry (accounts, tokens,
organisations, change requests, reviews, checks, issues, protections, webhooks) and one database
per repository. `pandod backup` snapshots all of them while the server runs:

```sh
pandod backup --platform sqlite:///var/lib/pando/platform.db \
  --repos-metadata 'sqlite:///var/lib/pando/repos/{repo}.db' \
  --repos-objects 'file:///var/lib/pando/objects/{repo_id}' \
  --to /backups/pando-$(date +%F)          # or s3://bucket/prefix, gs://..., az://...
```

The backup holds `platform.db`, `repos/OWNER--NAME.db` for every repository the registry lists,
`default.db` when `--metadata` is SQLite, and `manifest.json`, which records where each file
belongs and where objects live. Each database is copied with SQLite's `VACUUM INTO`, one
consistent transaction each. Objects stored inside the metadata (`--repos-objects metadata`)
are in the copy. Objects stored apart are only ever added, never changed, and are not copied:
back that store up with its provider's tools (bucket versioning or replication), and don't
delete objects from it. Repositories on Postgres are refused; use `pg_dump` for those. To an
object store, files go under `pando-backup-<time>/` with the provider's usual credential
environment variables.

**Restore** (the server must be stopped):

1. Stop `pandod`.
2. Copy `platform.db` to the `--platform` path.
3. For each entry in `manifest.json` → `repositories`, copy its `file` to the path in its
   `metadata` (without `sqlite://`); for `default`, likewise.
4. Delete any `-wal` and `-shm` files left next to the files you replaced.
5. Start `pandod`. Tokens, members, change requests and history are as they were at the backup.

For a point-in-time copy between backups, run [Litestream](https://litestream.io) next to the
server to stream `platform.db` and each repository database to object storage continuously;
take `pandod backup` snapshots as well, since they cover databases created after Litestream was
configured.
