CREATE TABLE IF NOT EXISTS subscriptions (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  channel_type TEXT NOT NULL DEFAULT 'email',  -- 'email' | 'slack' | 'discord'
  destination TEXT NOT NULL,                   -- email address or webhook URL
  pool_id TEXT NOT NULL,
  asset_symbol TEXT NOT NULL,
  leverage_bracket REAL NOT NULL,
  verified INTEGER DEFAULT 0,
  verify_token TEXT,
  unsub_token TEXT,
  created_at TEXT DEFAULT (datetime('now')),
  last_alerted_at TEXT,
  UNIQUE(destination, pool_id, asset_symbol, leverage_bracket)
);

CREATE INDEX IF NOT EXISTS idx_subs_pool_asset_lev
  ON subscriptions(pool_id, asset_symbol, leverage_bracket);

-- Migration for existing deployments (safe to run multiple times):
-- ALTER TABLE subscriptions ADD COLUMN channel_type TEXT NOT NULL DEFAULT 'email';
-- ALTER TABLE subscriptions ADD COLUMN destination TEXT;
-- UPDATE subscriptions SET destination = email WHERE destination IS NULL;
