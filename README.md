# x402 Base Pulse

Real-time payment protocol analytics for [Coinbase x402](https://github.com/coinbase/x402) on Base.

## What It Tracks

Every x402 payment settlement on Base mainnet by indexing the **x402ExactPermit2Proxy** contract (`0x4020615294c913F045dc10f0a5cdEbd86c280001`), correlating USDC transfers, and computing real-time analytics.

### Contracts Indexed
- **x402ExactPermit2Proxy**: `0x4020615294c913F045dc10f0a5cdEbd86c280001` (Settled / SettledWithPermit events)
- **USDC on Base**: `0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913` (Transfer events in x402 transactions)

### Data Tracked
- **Settlements**: Every x402 payment with payer, recipient, amount, facilitator, gas costs
- **Payer Stats**: Total spent, payment count per payer address
- **Recipient Stats**: Revenue per resource server address
- **Facilitator Stats**: Volume processed, gas economics per facilitator

## Architecture

| Layer | Modules | Purpose |
|-------|---------|---------|
| 1 - Extraction | `map_x402_settlements`, `map_payment_transfers` | Parse proxy events + USDC transfers |
| 2 - Stores | 7 state stores | Accumulate volume, counts, gas per actor |
| 3 - Analytics | `map_payer_stats`, `map_recipient_stats`, `map_facilitator_stats` | Computed aggregations |
| 4 - SQL Sink | `db_out` | PostgreSQL output with views |

## Quick Start

```bash
# Stream settlements
substreams run substreams.yaml map_x402_settlements \
  -e base-mainnet.substreams.pinax.network:443 \
  -s 25000000 -t +1000

# SQL sink to Postgres
substreams-sink-sql run "psql://localhost/x402" \
  x402-base-pulse-v1.0.0.spkg \
  -e base-mainnet.substreams.pinax.network:443
```

## SQL Views

- `top_payers` - Leaderboard by total spend
- `top_recipients` - Top resource servers by revenue
- `facilitator_economics` - Gas cost analysis per facilitator
- `whale_payments` - Payments > $100 USDC
- `recent_settlements` - Live settlement feed

## About x402

x402 is Coinbase's open-standard payment protocol that uses the HTTP 402 status code to enable internet-native cryptocurrency payments. Clients pay for API resources via signed authorizations (EIP-3009/Permit2), and facilitators handle on-chain settlement and gas sponsorship.
