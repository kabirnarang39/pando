// The repositories the demo starts from: a product monorepo of the kind agent fleets work in,
// and three smaller ones beside it.

export interface RepoSeed {
  name: string;
  description: string;
  topics: string[];
  files: Record<string, string>;
  /** Files that cannot be merged, with their sizes. */
  binaries: Record<string, number>;
  people: string[];
  seed: number;
  importer: string;
  importMessage: string;
  historyHours: number;
  /** Mean time between seeded landings, and between live ones. */
  paceMs: number;
  livePaceMs: number;
  /** Extra branches, as "how many changes behind main". */
  branches: Record<string, number>;
  locks: Record<string, { owner: string; minutesAgo: number }>;
}

const seedFiles: Record<string, string> = {
  "README.md": `# Fleet

The monorepo behind Fleet: billing, search, the web app and the infrastructure they run on.

| Directory | Owner | What lives there |
|---|---|---|
| services/billing | payments | invoicing, tax and refunds (Rust) |
| services/search | discovery | the product index and query API (Go) |
| web/app | storefront | the customer web app (TypeScript) |
| infra | platform | Terraform for every environment |

Every change lands on \`main\` through Pando. Agents land small changes continuously;
people review them in the console.
`,
  "services/billing/Cargo.toml": `[package]
name = "billing"
version = "0.18.2"
edition = "2024"

[dependencies]
rust_decimal = "1.37"
serde = { version = "1", features = ["derive"] }
thiserror = "2"
tokio = { version = "1", features = ["rt-multi-thread", "macros"] }
`,
  "services/billing/src/lib.rs": `//! Invoicing, tax and refunds.

pub mod invoice;
pub mod refund;
pub mod tax;

pub use invoice::{Invoice, LineItem};
pub use tax::TaxRegion;

/// How long an unpaid invoice stays open before it is sent to collections.
pub const PAYMENT_TERMS_DAYS: u32 = 30;

/// Retries for a payment provider that answered with a temporary error.
pub const PROVIDER_RETRIES: u32 = 3;
`,
  "services/billing/src/invoice.rs": `use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use crate::tax::TaxRegion;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LineItem {
    pub sku: String,
    pub quantity: u32,
    pub unit_price: Decimal,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Invoice {
    pub id: String,
    pub customer: String,
    pub region: TaxRegion,
    pub items: Vec<LineItem>,
}

impl Invoice {
    pub fn subtotal(&self) -> Decimal {
        self.items.iter().map(|i| i.unit_price * Decimal::from(i.quantity)).sum()
    }

    pub fn tax(&self) -> Decimal {
        (self.subtotal() * self.region.rate()).round_dp(2)
    }

    pub fn total(&self) -> Decimal {
        self.subtotal() + self.tax()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn total_includes_tax() {
        let invoice = Invoice {
            id: "inv_1".into(),
            customer: "c_1".into(),
            region: TaxRegion::Germany,
            items: vec![LineItem { sku: "seat".into(), quantity: 2, unit_price: Decimal::new(1000, 2) }],
        };
        assert_eq!(invoice.total(), Decimal::new(2380, 2));
    }
}
`,
  "services/billing/src/tax.rs": `use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaxRegion {
    Germany,
    France,
    India,
    UnitedKingdom,
    California,
}

impl TaxRegion {
    /// Standard VAT or sales tax rate for the region.
    pub fn rate(self) -> Decimal {
        match self {
            TaxRegion::Germany => Decimal::new(19, 2),
            TaxRegion::France => Decimal::new(20, 2),
            TaxRegion::India => Decimal::new(18, 2),
            TaxRegion::UnitedKingdom => Decimal::new(20, 2),
            TaxRegion::California => Decimal::new(725, 4),
        }
    }
}
`,
  "services/billing/src/refund.rs": `use rust_decimal::Decimal;
use thiserror::Error;

use crate::Invoice;

#[derive(Debug, Error)]
pub enum RefundError {
    #[error("refund of {requested} exceeds the invoice total {total}")]
    TooLarge { requested: Decimal, total: Decimal },
    #[error("invoice {0} was already refunded")]
    AlreadyRefunded(String),
}

/// Refunds at most the invoice total, once.
pub fn refund(invoice: &Invoice, amount: Decimal, refunded: bool) -> Result<Decimal, RefundError> {
    if refunded {
        return Err(RefundError::AlreadyRefunded(invoice.id.clone()));
    }
    let total = invoice.total();
    if amount > total {
        return Err(RefundError::TooLarge { requested: amount, total });
    }
    Ok(amount)
}
`,
  "services/search/go.mod": `module fleet/search

go 1.25

require github.com/blevesearch/bleve/v2 v2.5.3
`,
  "services/search/index.go": `package search

import (
	"context"
	"time"
)

// Document is one product as the index stores it.
type Document struct {
	ID       string
	Title    string
	Brand    string
	Price    int64
	InStock  bool
	Updated  time.Time
}

// Index keeps the product catalog searchable.
type Index struct {
	shards    int
	batchSize int
}

// NewIndex creates an index split into shards.
func NewIndex(shards int) *Index {
	return &Index{shards: shards, batchSize: 500}
}

// Upsert adds or replaces documents in batches.
func (ix *Index) Upsert(ctx context.Context, docs []Document) error {
	for start := 0; start < len(docs); start += ix.batchSize {
		end := min(start+ix.batchSize, len(docs))
		if err := ix.write(ctx, docs[start:end]); err != nil {
			return err
		}
	}
	return nil
}

func (ix *Index) write(ctx context.Context, batch []Document) error {
	_ = batch
	return ctx.Err()
}
`,
  "services/search/query.go": `package search

import "strings"

// Query is a parsed search request.
type Query struct {
	Terms    []string
	InStock  bool
	MaxPrice int64
	Limit    int
}

// DefaultLimit is how many results a query returns when it does not say.
const DefaultLimit = 24

// Parse turns the search box text into a query.
func Parse(text string) Query {
	q := Query{Limit: DefaultLimit}
	for _, word := range strings.Fields(strings.ToLower(text)) {
		switch {
		case word == "in-stock":
			q.InStock = true
		default:
			q.Terms = append(q.Terms, word)
		}
	}
	return q
}
`,
  "web/app/package.json": `{
  "name": "@fleet/web",
  "private": true,
  "scripts": { "dev": "vite", "build": "vite build", "test": "vitest run" },
  "dependencies": { "react": "19.3.0", "react-dom": "19.3.0" }
}
`,
  "web/app/src/App.tsx": `import { Checkout } from "./routes/Checkout";
import { SearchPage } from "./routes/Search";

export function App({ path }: { path: string }) {
  if (path.startsWith("/checkout")) return <Checkout />;
  return <SearchPage />;
}
`,
  "web/app/src/routes/Checkout.tsx": `import { useState } from "react";
import { Button } from "../components/Button";
import { formatMoney } from "../money";

const SHIPPING_FREE_FROM = 5000;

export function Checkout() {
  const [items] = useState([{ sku: "seat", name: "Team seat", price: 1000, quantity: 2 }]);
  const subtotal = items.reduce((sum, i) => sum + i.price * i.quantity, 0);
  const shipping = subtotal >= SHIPPING_FREE_FROM ? 0 : 499;

  return (
    <main className="checkout">
      <h1>Checkout</h1>
      <ul>
        {items.map((i) => (
          <li key={i.sku}>
            {i.quantity} × {i.name} <span>{formatMoney(i.price * i.quantity)}</span>
          </li>
        ))}
      </ul>
      <p>Shipping {shipping === 0 ? "free" : formatMoney(shipping)}</p>
      <Button kind="primary">Pay {formatMoney(subtotal + shipping)}</Button>
    </main>
  );
}
`,
  "web/app/src/routes/Search.tsx": `import { useDeferredValue, useState } from "react";
import { useSearch } from "../api";

export function SearchPage() {
  const [text, setText] = useState("");
  const query = useDeferredValue(text);
  const results = useSearch(query);

  return (
    <main>
      <input aria-label="Search products" value={text} onChange={(e) => setText(e.target.value)} />
      <p>{results.length} results</p>
      <ul>
        {results.map((r) => (
          <li key={r.id}>{r.title}</li>
        ))}
      </ul>
    </main>
  );
}
`,
  "web/app/src/components/Button.tsx": `import type { ButtonHTMLAttributes } from "react";

type Props = ButtonHTMLAttributes<HTMLButtonElement> & { kind?: "primary" | "quiet" };

export function Button({ kind = "quiet", className = "", ...rest }: Props) {
  return <button className={\`button button-\${kind} \${className}\`} {...rest} />;
}
`,
  "web/app/src/money.ts": `const formatter = new Intl.NumberFormat("en-US", { style: "currency", currency: "USD" });

/** Formats an amount in cents. */
export function formatMoney(cents: number): string {
  return formatter.format(cents / 100);
}
`,
  "infra/main.tf": `terraform {
  required_version = ">= 1.9"
}

module "billing" {
  source        = "./modules/service"
  name          = "billing"
  replicas      = 3
  cpu           = 1000
  memory_mb     = 2048
}

module "search" {
  source        = "./modules/service"
  name          = "search"
  replicas      = 6
  cpu           = 2000
  memory_mb     = 8192
}
`,
  "infra/variables.tf": `variable "environment" {
  type        = string
  description = "production, staging or a preview name"
}

variable "region" {
  type    = string
  default = "eu-central-1"
}
`,
  "tools/ci/pipeline.yaml": `steps:
  - name: test billing
    run: cargo test -p billing
    timeout_minutes: 10
  - name: test search
    run: go test ./services/search/...
    timeout_minutes: 8
  - name: build web
    run: pnpm --dir web/app build
    timeout_minutes: 6
`,
  "docs/runbooks/oncall.md": `# On-call

## Billing is slow

1. Check the provider dashboard for an incident.
2. Look at retry counts: more than 3 retries per payment means the provider is degraded.
3. Page payments if invoices stay unpaid for more than 30 minutes.

## Search returns nothing

1. Check the index shard health page.
2. Rebuild a shard with \`search reindex --shard N\`.
`,
};

/** Files that cannot be merged, so they are locked while someone edits them. */
const binaryFiles: Record<string, number> = {
  "design/checkout.fig": 48_213,
  "assets/brand/hero.psd": 291_840,
};

const people = [
  "agent/refactor-07",
  "agent/tests-03",
  "agent/docs-01",
  "agent/deps-02",
  "agent/perf-05",
  "agent/migrate-04",
  "agent/lint-09",
  "agent/security-06",
  "kabir",
];

export const fleetSeed: RepoSeed = {
  name: "fleet",
  description: "The product monorepo: billing, search, the web app and the infrastructure under them.",
  topics: ["monorepo", "rust", "go", "typescript"],
  files: seedFiles,
  binaries: binaryFiles,
  people,
  seed: 20261017,
  importer: "kabir",
  importMessage: "Import the Fleet monorepo",
  historyHours: 6,
  paceMs: 22_000,
  livePaceMs: 2200,
  branches: { "release/2.4": 40, "agent/perf-05/batching": 3 },
  locks: {
    "design/checkout.fig": { owner: "kabir", minutesAgo: 47 },
    "assets/brand/hero.psd": { owner: "agent/docs-01", minutesAgo: 6 },
  },
};

export const webSeed: RepoSeed = {
  name: "storefront",
  description: "The customer web app and its design system.",
  topics: ["typescript", "react", "design-system"],
  files: {
    "README.md": "# Storefront\n\nThe customer web app. `pnpm dev` runs it against staging.\n",
    "package.json": '{\n  "name": "@lumen/storefront",\n  "private": true,\n  "scripts": { "dev": "vite", "test": "vitest run" }\n}\n',
    "src/cart.ts": `export interface Line {\n  sku: string;\n  quantity: number;\n  cents: number;\n}\n\nexport const MAX_QUANTITY = 20;\nexport const FREE_SHIPPING_CENTS = 5000;\n\nexport function subtotal(lines: Line[]): number {\n  return lines.reduce((sum, l) => sum + l.cents * Math.min(l.quantity, MAX_QUANTITY), 0);\n}\n\nexport function shipping(lines: Line[]): number {\n  return subtotal(lines) >= FREE_SHIPPING_CENTS ? 0 : 499;\n}\n`,
    "src/search.ts": `export const DEBOUNCE_MS = 120;\nexport const PAGE_SIZE = 24;\n\nexport function normalise(query: string): string {\n  return query.trim().toLowerCase().replace(/\\s+/g, " ");\n}\n`,
    "src/tokens.ts": `export const radius = { sm: 4, md: 8, lg: 12 };\nexport const space = { xs: 4, sm: 8, md: 16, lg: 24 };\nexport const motionMs = 160;\n`,
    "docs/accessibility.md": "# Accessibility\n\nEvery control is reachable by keyboard.\n",
  },
  binaries: { "design/storefront.fig": 88_120 },
  people: ["agent/ui-11", "agent/tests-03", "agent/a11y-02", "agent/deps-02", "mira"],
  seed: 7311,
  importer: "mira",
  importMessage: "Import the storefront",
  historyHours: 5,
  paceMs: 70_000,
  livePaceMs: 9000,
  branches: {},
  locks: { "design/storefront.fig": { owner: "mira", minutesAgo: 130 } },
};

export const infraSeed: RepoSeed = {
  name: "infra",
  description: "Terraform and deploy pipelines for every environment.",
  topics: ["terraform", "platform"],
  files: {
    "README.md": "# Infra\n\nEvery environment, as code. Changes to production need two approvals.\n",
    "environments/production.tf": `module "cluster" {\n  source       = "../modules/cluster"\n  nodes        = 24\n  node_cpu     = 16\n  node_memory  = 64\n}\n`,
    "environments/staging.tf": `module "cluster" {\n  source       = "../modules/cluster"\n  nodes        = 6\n  node_cpu     = 8\n  node_memory  = 32\n}\n`,
    "pipelines/deploy.yaml": "steps:\n  - name: plan\n    timeout_minutes: 10\n  - name: apply\n    timeout_minutes: 30\n",
    "docs/runbooks/rollback.md": "# Rolling back\n\n1. Find the last good change.\n2. Land a revert.\n",
  },
  binaries: {},
  people: ["agent/infra-08", "agent/cost-04", "dev"],
  seed: 4242,
  importer: "dev",
  importMessage: "Import infrastructure",
  historyHours: 8,
  paceMs: 300_000,
  livePaceMs: 30_000,
  branches: {},
  locks: {},
};

export const mlSeed: RepoSeed = {
  name: "ranking",
  description: "Search ranking models and the evaluation harness.",
  topics: ["python", "ml"],
  files: {
    "README.md": "# Ranking\n\nTraining and evaluation for search ranking.\n",
    "ranking/train.py": "BATCH_SIZE = 256\nEPOCHS = 12\nLEARNING_RATE_BPS = 30\n\n\ndef train(data):\n    for epoch in range(EPOCHS):\n        yield epoch\n",
    "ranking/evaluate.py": "TOP_K = 24\n\n\ndef ndcg(results, k=TOP_K):\n    return sum(1 / (i + 2) for i, r in enumerate(results[:k]) if r)\n",
    "docs/metrics.md": "# Metrics\n\nWe report NDCG at 24.\n",
  },
  binaries: { "models/ranker-v7.onnx": 412_000 },
  people: ["agent/train-01", "agent/eval-02", "sana"],
  seed: 9001,
  importer: "sana",
  importMessage: "Import ranking",
  historyHours: 7,
  paceMs: 160_000,
  livePaceMs: 20_000,
  branches: { "experiment/longer-context": 6 },
  locks: { "models/ranker-v7.onnx": { owner: "agent/train-01", minutesAgo: 18 } },
};
