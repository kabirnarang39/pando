# Pando for free: a production server at no cost

Everything below has a free tier that does not expire. Together they run a real Pando server
with HTTPS, off-site backups, monitoring, and the web console, for $0 a month until usage
grows past the free limits.

| Piece | Service (free tier) | What you get |
|---|---|---|
| Server | Oracle Cloud Always Free, `VM.Standard.A1.Flex` | 2 ARM cores, 12 GB RAM, 200 GB block storage, always on |
| HTTPS | Caddy on the server, Let's Encrypt | automatic certificates and renewal |
| DNS and DDoS protection (optional) | Cloudflare Free | proxies gRPC on port 443 once gRPC is enabled for the zone |
| Backups: history | Neon Free (Postgres) | 1 GB per project, off the machine |
| Backups: file contents | Cloudflare R2 Free | 10 GB, no egress fees |
| Web console and docs | GitHub Pages | global CDN; the console is static files |
| Builds and releases | GitHub Actions and GHCR | free for public repositories |
| Monitoring | Grafana Cloud Free | scrape `/metrics` (10,000 series) |

Why this is fast on a free machine: Pando lands changes with path-scoped conflict checks and
batched commits; on a laptop it lands 1,012 changes a second from 200 agents. Two ARM cores and
local SQLite comfortably serve a team and its agent fleet. Measure your own with
`cargo run --release --example land_load`.

## Steps

1. **Create the machine.** Oracle Cloud console: Compute, Instances, Create. Image: Ubuntu
   24.04 (aarch64). Shape: `VM.Standard.A1.Flex`, 2 OCPU, 12 GB. Paste `cloud-init.yaml` (with
   your values filled in) into *Advanced options, Cloud-init script*.
2. **Open ports 80 and 443** in the instance's VCN security list (Networking, Virtual Cloud
   Networks, your subnet, Security list: two ingress rules from `0.0.0.0/0`).
3. **Point DNS** at the instance's public IP (an `A` record for the name in the Caddyfile).
   With Cloudflare's proxy on, also turn on *Network, gRPC* for the zone.
4. **Backups:** create a Neon project and an R2 bucket with an API token (Object Read and
   Write), and put their values in `/etc/pando/backup.env`. Run one now with
   `sudo systemctl start pando-backup` and check `journalctl -u pando-backup`.
5. **Use it:** `pando clone https://pando.example.com work`, or open the
   [console](https://grovedock.com/console/), choose *Connection*, and enter
   the address and a token.

## Keep it free and safe

- **Oracle reclaims idle Always Free machines** (95th-percentile CPU, network and memory all
  under 20% for 7 days). A server people use is not idle; for a quiet one, upgrade the account
  to pay-as-you-go: Always Free resources stay free, and reclamation stops.
- **Restore** onto a new machine with the same `pandod copy`, in the other direction:
  `pandod copy --from-metadata "$BACKUP_METADATA" --from-objects "$BACKUP_OBJECTS" --to-metadata sqlite:///var/lib/pando/meta.db`.
- **Tokens:** `/etc/pando/pandod.env` holds them (mode 600). Without tokens, pandod refuses to
  serve beyond localhost.
- **When you outgrow it:** move metadata to Postgres and objects to R2 or S3 with `pandod copy`
  while serving, then run several `pandod` replicas behind a load balancer. Nothing else changes.
