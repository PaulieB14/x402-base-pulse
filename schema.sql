-- x402 Base Pulse - PostgreSQL Schema
-- Version: 2.0.0
--
-- Detects x402 settlements via EIP-3009 AuthorizationUsed events on USDC.
-- Per: https://docs.cdp.coinbase.com/x402/core-concepts/how-it-works
--
-- Run with: substreams-sink-sql setup "postgres://..." x402-base-pulse-v2.0.0.spkg

-------------------------------------------------
-- SETTLEMENTS: Every x402 payment on Base
-------------------------------------------------
CREATE TABLE IF NOT EXISTS settlements (
    id VARCHAR(128) PRIMARY KEY,              -- tx_hash-log_index
    block_number BIGINT NOT NULL,
    block_timestamp TIMESTAMP NOT NULL,
    tx_hash VARCHAR(66) NOT NULL,
    log_index INTEGER NOT NULL,

    -- Payment details
    payer VARCHAR(42) NOT NULL,               -- Who paid (EIP-3009 authorizer)
    recipient VARCHAR(42) NOT NULL,           -- Resource server (payTo)
    token VARCHAR(42) NOT NULL,               -- Token address (USDC)
    amount NUMERIC(38, 6) NOT NULL DEFAULT 0, -- Payment amount (atomic units)

    -- Settlement classification
    -- eip3009: facilitator called transferWithAuthorization on USDC
    -- eip3009_proxy: EIP-3009 + proxy Settled event in same tx
    -- settled: proxy Settled() event
    -- settled_with_permit: proxy SettledWithPermit() event
    settlement_type VARCHAR(32) NOT NULL,

    -- Facilitator info
    facilitator VARCHAR(42) NOT NULL,         -- tx.from - who submitted and paid gas
    gas_used NUMERIC(20, 0) NOT NULL DEFAULT 0,
    gas_price NUMERIC(30, 0) NOT NULL DEFAULT 0,

    -- EIP-3009 authorization nonce (hex-encoded bytes32)
    nonce VARCHAR(66),

    created_at TIMESTAMP DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_settlements_block ON settlements(block_number);
CREATE INDEX IF NOT EXISTS idx_settlements_payer ON settlements(payer);
CREATE INDEX IF NOT EXISTS idx_settlements_recipient ON settlements(recipient);
CREATE INDEX IF NOT EXISTS idx_settlements_facilitator ON settlements(facilitator);
CREATE INDEX IF NOT EXISTS idx_settlements_timestamp ON settlements(block_timestamp);
CREATE INDEX IF NOT EXISTS idx_settlements_type ON settlements(settlement_type);
CREATE INDEX IF NOT EXISTS idx_settlements_amount ON settlements(amount DESC);

-------------------------------------------------
-- PAYERS: Aggregated stats per payer address
-------------------------------------------------
CREATE TABLE IF NOT EXISTS payers (
    payer_address VARCHAR(42) PRIMARY KEY,

    -- Payment metrics
    total_spent NUMERIC(38, 6) NOT NULL DEFAULT 0,
    total_payments INTEGER NOT NULL DEFAULT 0,

    -- Timestamps
    first_payment_at TIMESTAMP,
    last_payment_at TIMESTAMP,
    updated_at TIMESTAMP DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_payers_spent ON payers(total_spent DESC);
CREATE INDEX IF NOT EXISTS idx_payers_count ON payers(total_payments DESC);

-------------------------------------------------
-- RECIPIENTS: Resource servers receiving x402 payments
-------------------------------------------------
CREATE TABLE IF NOT EXISTS recipients (
    recipient_address VARCHAR(42) PRIMARY KEY,

    -- Revenue metrics
    total_received NUMERIC(38, 6) NOT NULL DEFAULT 0,
    total_payments INTEGER NOT NULL DEFAULT 0,

    -- Timestamps
    first_payment_at TIMESTAMP,
    last_payment_at TIMESTAMP,
    updated_at TIMESTAMP DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_recipients_revenue ON recipients(total_received DESC);
CREATE INDEX IF NOT EXISTS idx_recipients_count ON recipients(total_payments DESC);

-------------------------------------------------
-- FACILITATORS: Gas economics per facilitator
-------------------------------------------------
CREATE TABLE IF NOT EXISTS facilitators (
    facilitator_address VARCHAR(42) PRIMARY KEY,

    -- Settlement metrics
    total_settlements INTEGER NOT NULL DEFAULT 0,
    total_volume_settled NUMERIC(38, 6) NOT NULL DEFAULT 0,

    -- Gas economics
    total_gas_spent NUMERIC(38, 0) NOT NULL DEFAULT 0,   -- Total gas cost in wei

    -- Timestamps
    first_settlement_at TIMESTAMP,
    last_settlement_at TIMESTAMP,
    updated_at TIMESTAMP DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_facilitators_volume ON facilitators(total_volume_settled DESC);
CREATE INDEX IF NOT EXISTS idx_facilitators_settlements ON facilitators(total_settlements DESC);
CREATE INDEX IF NOT EXISTS idx_facilitators_gas ON facilitators(total_gas_spent DESC);

-------------------------------------------------
-- DAILY_STATS: Daily protocol-wide aggregates
-------------------------------------------------
CREATE TABLE IF NOT EXISTS daily_stats (
    date DATE PRIMARY KEY,

    -- Volume
    total_volume NUMERIC(38, 6) NOT NULL DEFAULT 0,
    total_settlements INTEGER NOT NULL DEFAULT 0,

    -- Participants
    unique_payers INTEGER NOT NULL DEFAULT 0,
    unique_recipients INTEGER NOT NULL DEFAULT 0,
    unique_facilitators INTEGER NOT NULL DEFAULT 0,

    -- Gas economics
    total_gas_spent NUMERIC(38, 0) NOT NULL DEFAULT 0,

    updated_at TIMESTAMP DEFAULT NOW()
);

-------------------------------------------------
-- VIEWS: Protocol analytics dashboards
-------------------------------------------------

-- Top payers by total spend
CREATE OR REPLACE VIEW top_payers AS
SELECT
    payer_address,
    total_spent,
    total_payments,
    CASE WHEN total_payments > 0
        THEN ROUND(total_spent / total_payments, 6)
        ELSE 0
    END AS avg_payment_size,
    RANK() OVER (ORDER BY total_spent DESC) AS rank
FROM payers
WHERE total_payments >= 1
ORDER BY total_spent DESC
LIMIT 100;

-- Top resource servers by revenue
CREATE OR REPLACE VIEW top_recipients AS
SELECT
    recipient_address,
    total_received,
    total_payments,
    CASE WHEN total_payments > 0
        THEN ROUND(total_received / total_payments, 6)
        ELSE 0
    END AS avg_payment_received,
    RANK() OVER (ORDER BY total_received DESC) AS rank
FROM recipients
WHERE total_payments >= 1
ORDER BY total_received DESC
LIMIT 100;

-- Facilitator economics leaderboard
CREATE OR REPLACE VIEW facilitator_economics AS
SELECT
    facilitator_address,
    total_settlements,
    total_volume_settled,
    total_gas_spent,
    CASE WHEN total_settlements > 0
        THEN ROUND(total_volume_settled / total_settlements, 6)
        ELSE 0
    END AS avg_settlement_size,
    RANK() OVER (ORDER BY total_volume_settled DESC) AS rank
FROM facilitators
WHERE total_settlements >= 1
ORDER BY total_volume_settled DESC
LIMIT 100;

-- Large payments (whale payments > $100 USDC = 100_000_000 atomic)
CREATE OR REPLACE VIEW whale_payments AS
SELECT
    s.*,
    p.total_spent AS payer_total_spent,
    p.total_payments AS payer_total_payments
FROM settlements s
LEFT JOIN payers p ON s.payer = p.payer_address
WHERE s.amount >= 100000000
ORDER BY s.block_timestamp DESC
LIMIT 500;

-- Recent settlements feed
CREATE OR REPLACE VIEW recent_settlements AS
SELECT
    id,
    block_number,
    block_timestamp,
    tx_hash,
    payer,
    recipient,
    amount,
    settlement_type,
    facilitator,
    gas_used,
    nonce
FROM settlements
ORDER BY block_number DESC, log_index DESC
LIMIT 100;
