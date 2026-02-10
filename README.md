# x402 Base Pulse

> Real-time payment protocol analytics for [Coinbase x402](https://github.com/coinbase/x402) on Base

Track every x402 payment settlement on Base. This Substreams detects when facilitators call `transferWithAuthorization` on USDC to settle [HTTP 402](https://docs.cdp.coinbase.com/x402) payments, extracting payer, recipient, amount, and facilitator data from each settlement.

---

## How It Works

The [x402 protocol](https://docs.cdp.coinbase.com/x402/core-concepts/how-it-works) enables internet-native payments using the HTTP 402 status code. When a client wants to access a paid resource:

1. Server responds with **HTTP 402** + payment requirements
2. Client signs an [EIP-3009](https://eips.ethereum.org/EIPS/eip-3009) authorization
3. Facilitator calls `transferWithAuthorization` on USDC to settle payment on-chain
4. USDC emits `AuthorizationUsed` + `Transfer` events
5. **This Substreams captures those events** and extracts settlement data

## Modules

| Module | Kind | Description |
|--------|------|-------------|
| `map_x402_settlements` | Map | Pairs `AuthorizationUsed` + `Transfer` events on USDC to extract settlement details |
| `store_payer_volume` | Store | Accumulates total USDC spent per payer |
| `store_payer_count` | Store | Counts payments per payer |
| `store_recipient_volume` | Store | Accumulates total USDC received per resource server |
| `store_recipient_count` | Store | Counts payments per recipient |
| `store_facilitator_volume` | Store | Accumulates total USDC volume per facilitator |
| `store_facilitator_count` | Store | Counts settlements per facilitator |
| `store_facilitator_gas` | Store | Tracks gas costs per facilitator |
| `map_payer_stats` | Map | Computes payer leaderboards and averages |
| `map_recipient_stats` | Map | Computes resource server revenue stats |
| `map_facilitator_stats` | Map | Computes facilitator economics (volume vs gas cost) |
| `db_out` | Map | Outputs `DatabaseChanges` for PostgreSQL sink |

## Contract Indexed

| Contract | Address | Events |
|----------|---------|--------|
| USDC (Base) | `0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913` | `AuthorizationUsed`, `Transfer` |

## Quick Start

```bash
# Stream settlements
substreams run x402-base-pulse map_x402_settlements \
  -e base-mainnet.streamingfast.io:443 \
  -s 29000000 -t +1000

# GUI mode
substreams gui x402-base-pulse map_x402_settlements \
  -e base-mainnet.streamingfast.io:443 \
  -s 29000000

# Sink to PostgreSQL
substreams-sink-sql run "psql://localhost/x402" \
  x402-base-pulse-v2.0.1.spkg \
  -e base-mainnet.streamingfast.io:443
```

## SQL Output

### Tables
| Table | Key | Description |
|-------|-----|-------------|
| `settlements` | `tx_hash-log_index` | Every settlement with payer, recipient, amount, facilitator, gas |
| `payers` | `payer_address` | Aggregated spend and payment count per payer |
| `recipients` | `recipient_address` | Revenue and payment count per resource server |
| `facilitators` | `facilitator_address` | Volume settled, settlement count, total gas spent |
| `daily_stats` | `date` | Daily protocol-wide volume, participants, gas |

### Views
| View | Description |
|------|-------------|
| `top_payers` | Ranked by total USDC spent |
| `top_recipients` | Ranked by total USDC received |
| `facilitator_economics` | Volume settled vs gas cost per facilitator |
| `whale_payments` | Payments > $100 USDC |
| `recent_settlements` | Latest 100 settlements |

## Build

```bash
cargo build --target wasm32-unknown-unknown --release
substreams pack substreams.yaml
```

## References

- [x402 Protocol](https://docs.cdp.coinbase.com/x402) -- Coinbase's HTTP 402 payment standard
- [How It Works](https://docs.cdp.coinbase.com/x402/core-concepts/how-it-works) -- Settlement flow
- [Network Support](https://docs.cdp.coinbase.com/x402/network-support) -- Supported tokens and chains
- [EIP-3009](https://eips.ethereum.org/EIPS/eip-3009) -- Transfer With Authorization
- [x402 Source](https://github.com/coinbase/x402) -- Protocol implementation

## Network

- **Chain**: Base
- **Start Block**: 25,000,000
- **Endpoint**: `base-mainnet.streamingfast.io:443`
