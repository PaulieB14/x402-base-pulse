# x402 Base Pulse

> Real-time payment protocol analytics for [Coinbase x402](https://github.com/coinbase/x402) on Base

Track every x402 payment settlement on Base mainnet. Detects EIP-3009 `transferWithAuthorization` calls on USDC -- the primary settlement mechanism used by x402 facilitators -- and computes per-actor analytics in real time.

---

## How x402 Settlement Works

Per the [x402 protocol docs](https://docs.cdp.coinbase.com/x402/core-concepts/how-it-works):

1. Client requests a paid resource
2. Server responds with **HTTP 402** + payment requirements
3. Client signs an EIP-3009 payment authorization
4. **Facilitator settles on-chain** by calling `transferWithAuthorization` on USDC
5. Server delivers the resource

This Substreams indexes **step 4** -- every on-chain settlement -- by detecting `AuthorizationUsed` events on the USDC contract, giving full visibility into the x402 payment network on Base.

## Detection Methods

| Method | Mechanism | Status |
|--------|-----------|--------|
| **EIP-3009** (primary) | `AuthorizationUsed` events on USDC from `transferWithAuthorization` | Active on mainnet |
| **Permit2 proxy** (secondary) | `Settled()` / `SettledWithPermit()` from x402ExactPermit2Proxy | Ready for mainnet deployment |

## Modules

```
Layer 1 - Extraction
  map_x402_settlements .......... EIP-3009 AuthorizationUsed + Transfer event pairs

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

## Contracts & Events

| Contract | Address | Events |
|----------|---------|--------|
| USDC (Base) | `0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913` | AuthorizationUsed, Transfer |
| x402ExactPermit2Proxy | `0x4020615294c913F045dc10f0a5cdEbd86c280001` | Settled, SettledWithPermit |
| x402UptoPermit2Proxy | `0x4020633461b2895a48930Ff97eE8fCdE8E520002` | Settled, SettledWithPermit |

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
  x402-base-pulse-v2.0.0.spkg \
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

## References

- [x402 Protocol Docs](https://docs.cdp.coinbase.com/x402)
- [x402 Network Support](https://docs.cdp.coinbase.com/x402/network-support) -- supported tokens & chains
- [x402 How It Works](https://docs.cdp.coinbase.com/x402/core-concepts/how-it-works) -- settlement flow
- [EIP-3009](https://eips.ethereum.org/EIPS/eip-3009) -- Transfer With Authorization standard
- [x402 Source Code](https://github.com/coinbase/x402) -- protocol implementation

## Build from Source

```bash
cargo build --target wasm32-unknown-unknown --release
substreams pack substreams.yaml
```

## Network

- **Chain**: Base (EVM)
- **Start Block**: 25,000,000
- **Endpoint**: `base-mainnet.streamingfast.io:443`
