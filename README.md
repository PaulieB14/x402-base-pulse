# x402 Base Pulse

> Real-time payment protocol analytics for [Coinbase x402](https://github.com/coinbase/x402) on Base

**Note:** The x402 proxy contract is currently active on Base Sepolia testnet (194+ settlements since Jan 2026) and has not yet been deployed to Base mainnet. This Substreams is production-ready and will begin indexing automatically once the proxy goes live on mainnet.

Track every x402 payment settlement on Base mainnet. Indexes the **x402ExactPermit2Proxy** contract, correlates USDC transfers, and computes per-actor analytics in real time.

---

## Modules

```
Layer 1 - Extraction
  map_x402_settlements .......... Proxy settlement events (Settled / SettledWithPermit)
  map_payment_transfers ......... USDC Transfer events in x402 transactions

Layer 2 - State Stores
  store_payer_volume ............ Total spend per payer
  store_payer_count ............. Payment count per payer
  store_recipient_volume ........ Revenue per resource server
  store_recipient_count ......... Payment count per recipient
  store_facilitator_volume ...... Volume per facilitator
  store_facilitator_count ....... Settlement count per facilitator
  store_facilitator_gas ......... Gas cost per facilitator

Layer 3 - Analytics
  map_payer_stats ............... Payer leaderboards & averages
  map_recipient_stats ........... Resource server revenue stats
  map_facilitator_stats ......... Facilitator economics

Layer 4 - SQL Sink
  db_out ........................ PostgreSQL output (5 tables + 5 views)
```

## Contracts Indexed

| Contract | Address | Events |
|----------|---------|--------|
| x402ExactPermit2Proxy | `0x4020615294c913F045dc10f0a5cdEbd86c280001` | Settled, SettledWithPermit |
| USDC (Base) | `0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913` | Transfer |

## Quick Start

```bash
# Stream x402 settlements from Base
substreams run x402-base-pulse map_x402_settlements \
  -e base-mainnet.streamingfast.io:443 \
  -s 25000000 -t +1000

# GUI mode
substreams gui x402-base-pulse map_x402_settlements \
  -e base-mainnet.streamingfast.io:443 \
  -s 25000000

# Sink to PostgreSQL
substreams-sink-sql run "psql://localhost/x402" \
  x402-base-pulse-v1.0.0.spkg \
  -e base-mainnet.streamingfast.io:443
```

## SQL Schema

### Tables
| Table | Primary Key | Description |
|-------|-------------|-------------|
| `settlements` | tx_hash + log_index | Every x402 payment on Base |
| `payers` | payer_address | Aggregated spend per payer |
| `recipients` | recipient_address | Revenue per resource server |
| `facilitators` | facilitator_address | Gas economics per facilitator |
| `daily_stats` | date | Protocol-wide daily aggregates |

### Views
| View | Description |
|------|-------------|
| `top_payers` | Leaderboard by total spend |
| `top_recipients` | Top resource servers by revenue |
| `facilitator_economics` | Gas cost analysis per facilitator |
| `whale_payments` | Payments > $100 USDC |
| `recent_settlements` | Live settlement feed |

## What is x402?

[x402](https://github.com/coinbase/x402) is Coinbase's open-standard payment protocol using the HTTP 402 status code for internet-native crypto payments. Clients pay for API resources via signed authorizations (EIP-3009 / Permit2), and facilitators handle on-chain settlement and gas sponsorship.

**Payment flow:**
1. Client requests a paid resource
2. Server responds with HTTP 402 + payment requirements
3. Client signs a payment authorization
4. Facilitator settles on-chain via the x402 proxy
5. Server delivers the resource

This Substreams indexes step 4 -- every on-chain settlement -- giving you full visibility into the x402 payment network on Base.

## Build from Source

```bash
cargo build --target wasm32-unknown-unknown --release
substreams pack substreams.yaml
```

## Network

- **Chain**: Base (EVM)
- **Start Block**: 25,000,000
- **Endpoint**: `base-mainnet.streamingfast.io:443`
