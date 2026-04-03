# x402 Base Pulse

> Real-time payment protocol analytics for [Coinbase x402](https://github.com/coinbase/x402) on Base

Track every x402 payment settlement on Base. This Substreams detects when facilitators call `transferWithAuthorization` on USDC to settle [HTTP 402](https://docs.cdp.coinbase.com/x402) payments, extracting payer, recipient, amount, and facilitator data from each settlement.

**v3.0.0** — Now gates EIP-3009 settlements through the on-chain [FacilitatorRegistry](https://basescan.org/address/0x67C75c4FD5BbbF5f6286A1874fe2d7dF0024Ebe8), matching the [x402-subgraph](https://github.com/PaulieB14/x402-subgraph). Facilitator names, URLs, and active status are resolved from registry events.

---

## How It Works

The [x402 protocol](https://docs.cdp.coinbase.com/x402/core-concepts/how-it-works) enables internet-native payments using the HTTP 402 status code. When a client wants to access a paid resource:

1. Server responds with **HTTP 402** + payment requirements
2. Client signs an [EIP-3009](https://eips.ethereum.org/EIPS/eip-3009) authorization
3. Facilitator calls `transferWithAuthorization` on USDC to settle payment on-chain
4. USDC emits `AuthorizationUsed` + `Transfer` events
5. **This Substreams captures those events** and extracts settlement data
6. **FacilitatorRegistry** gates EIP-3009 settlements — only registered facilitators are indexed

## Modules

| Module | Kind | Description |
|--------|------|-------------|
| `map_facilitator_registry_events` | Map | Extracts `FacilitatorAdded` / `FacilitatorRemoved` events from the on-chain registry |
| `store_facilitator_registry` | Store | Maintains the set of registered facilitators with names and URLs |
| `map_x402_settlements` | Map | Pairs `AuthorizationUsed` + `Transfer` events, gated by facilitator registry |
| `store_payer_volume` | Store | Accumulates total USDC spent per payer |
| `store_payer_count` | Store | Counts payments per payer |
| `store_recipient_volume` | Store | Accumulates total USDC received per resource server |
| `store_recipient_count` | Store | Counts payments per recipient |
| `store_facilitator_volume` | Store | Accumulates total USDC volume per facilitator |
| `store_facilitator_count` | Store | Counts settlements per facilitator |
| `store_facilitator_gas` | Store | Tracks gas costs per facilitator |
| `store_first_seen` | Store | Records first-seen timestamp per payer, recipient, and facilitator |
| `map_payer_stats` | Map | Computes payer leaderboards and averages |
| `map_recipient_stats` | Map | Computes resource server revenue stats |
| `map_facilitator_stats` | Map | Computes facilitator economics with name, URL, and active status from registry |
| `db_out` | Map | Outputs `DatabaseChanges` for PostgreSQL sink |

## Contracts Indexed

| Contract | Address | Events |
|----------|---------|--------|
| USDC (Base) | `0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913` | `AuthorizationUsed`, `Transfer` |
| FacilitatorRegistry | `0x67C75c4FD5BbbF5f6286A1874fe2d7dF0024Ebe8` | `FacilitatorAdded`, `FacilitatorRemoved` |
| x402ExactPermit2Proxy | `0x4020615294c913F045dc10f0a5cdEbd86c280001` | `Settled`, `SettledWithPermit` |
| x402UptoPermit2Proxy | `0x4020633461b2895a48930Ff97eE8fCdE8E520002` | `Settled`, `SettledWithPermit` |

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
  x402-base-pulse-v3.0.0.spkg \
  -e base-mainnet.streamingfast.io:443
```

## SQL Output

### Tables
| Table | Key | Description |
|-------|-----|-------------|
| `settlements` | `tx_hash-log_index` | Every settlement with payer, recipient, amount, facilitator, gas |
| `payers` | `payer_address` | Aggregated spend and payment count per payer |
| `recipients` | `recipient_address` | Revenue and payment count per resource server |
| `facilitators` | `facilitator_address` | Name, URL, active status, volume settled, settlement count, total gas spent |

### Views
| View | Description |
|------|-------------|
| `daily_stats` | Daily protocol-wide volume, unique participants, gas |
| `top_payers` | Ranked by total USDC spent |
| `top_recipients` | Ranked by total USDC received |
| `facilitator_economics` | Name, active status, volume settled vs gas cost per facilitator |
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
- [x402-subgraph](https://github.com/PaulieB14/x402-subgraph) -- Companion subgraph with matching facilitator gating

## Network

- **Chain**: Base
- **Start Block**: 25,000,000 (settlements), 30,011,612 (FacilitatorRegistry)
- **Endpoint**: `base-mainnet.streamingfast.io:443`
