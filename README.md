# x402 Base Pulse

> Real-time payment protocol analytics for [Coinbase x402](https://github.com/coinbase/x402) on Base

Track every x402 payment settlement on Base. This Substreams detects when facilitators call `transferWithAuthorization` on USDC to settle [HTTP 402](https://docs.cdp.coinbase.com/x402) payments, extracting payer, recipient, amount, and facilitator data from each settlement.

**v3.3.0** — Real **batch-settlement** indexing. x402's `batch-settlement` scheme uses on-chain payment channels: clients deposit into escrow, sign per-request vouchers, and servers claim them in batches. This version indexes the [`x402BatchSettlement`](https://basescan.org/address/0x4020074e9dF2ce1deE5A9C1b5c3f541D02a10003) contract's `Claimed` events (the actual per-voucher settled value), recovers **payer/receiver from the claim call's embedded `ChannelConfig`**, and tags each with its **`channel_id`**. Batch settlements land as `settlement_type="batch_claim"`, `scheme="batch"`. (416 batch txs on Base as of 2026-07-28.) The block filter is extended to the batch contract so channel activity is captured alongside EIP-3009.

**v3.2.0** — Enriches every settlement with the EIP-3009 **validity window** (`valid_after` / `valid_before`, decoded from the `transferWithAuthorization` call — a real fraud/anomaly signal) and a **`scheme`** field (`exact` / `batch`). **Fixes the Permit2 proxy address**: the previous `0x4020615…` / `0x4020633…` addresses carry **no contract code** on Base; the live proxy is [`0x402085c2…0001`](https://basescan.org/address/0x402085c248eea27d92e8b30b2c58ed07f9e20001). Adds forward-looking **batch-settlement** capture (`x402BatchSettlement` `Settled` sweeps → `scheme="batch"`; recipient/amount/token exact, payer approximate pending channel-level attribution in v3.3).

**v3.1.0** — Adds a static facilitator allowlist (112 addresses across 29 operators) sourced from [Merit Systems' x402scan `facilitators` package](https://github.com/Merit-Systems/x402scan/tree/main/packages/external/facilitators). The gate now accepts a settlement if `tx.from` is **either** on-chain-registered via `FacilitatorRegistry` **or** present in the published allowlist. In practice the on-chain registry is sparsely populated (only Meridian as of 2026-05), so without the allowlist virtually no real x402 activity would be indexed. Matches gating used by x402scan + [x402-omnigraph subgraph](https://github.com/PaulieB14/x402-omnigraph).

**v3.0.0** — Gated EIP-3009 settlements through the on-chain [FacilitatorRegistry](https://basescan.org/address/0x67C75c4FD5BbbF5f6286A1874fe2d7dF0024Ebe8) only. Facilitator names, URLs, and active status resolved from registry events. *Deprecated: produces empty output in practice — use v3.1.0.*

---

## How It Works

The [x402 protocol](https://docs.cdp.coinbase.com/x402/core-concepts/how-it-works) enables internet-native payments using the HTTP 402 status code. When a client wants to access a paid resource:

1. Server responds with **HTTP 402** + payment requirements
2. Client signs an [EIP-3009](https://eips.ethereum.org/EIPS/eip-3009) authorization
3. Facilitator calls `transferWithAuthorization` on USDC to settle payment on-chain
4. USDC emits `AuthorizationUsed` + `Transfer` events
5. **This Substreams captures those events** and extracts settlement data
6. **Facilitator gate** — settlements are kept only if `tx.from` is either on-chain-registered via `FacilitatorRegistry` or on the published static allowlist (x402scan's `facilitators` npm package, baked into the WASM at build time)

## Modules

| Module | Kind | Description |
|--------|------|-------------|
| `map_facilitator_registry_events` | Map | Extracts `FacilitatorAdded` / `FacilitatorRemoved` events from the on-chain registry |
| `store_facilitator_registry` | Store | Maintains the set of registered facilitators with names and URLs |
| `map_x402_settlements` | Map | EIP-3009 (`AuthorizationUsed`+`Transfer`, + validity window from the call), Permit2 proxy (`Settled`/`SettledWithPermit`), and batch-settlement `Claimed` vouchers (payer/receiver from the claim call); gates by FacilitatorRegistry **OR** static allowlist |
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
| USDC (Base) | `0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913` | `AuthorizationUsed`, `Transfer` (+ `transferWithAuthorization` call for validity window) |
| FacilitatorRegistry | `0x67C75c4FD5BbbF5f6286A1874fe2d7dF0024Ebe8` | `FacilitatorAdded`, `FacilitatorRemoved` |
| x402 Permit2 Proxy | `0x402085c248eea27d92e8b30b2c58ed07f9e20001` (live; old `…615`/`…633` have no code) | `Settled`, `SettledWithPermit` |
| x402BatchSettlement | `0x4020074e9dF2ce1deE5A9C1b5c3f541D02a10003` | `Claimed(channelId, sender, claimAmount, newTotalClaimed)` |

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
  x402-base-pulse-v3.3.0.spkg \
  -e base-mainnet.streamingfast.io:443
```

## SQL Output

### Tables
| Table | Key | Description |
|-------|-----|-------------|
| `settlements` | `tx_hash-log_index` | Every settlement: payer, recipient, amount, facilitator, gas, **`scheme`**, **validity window** (`valid_after`/`valid_before`), nonce, **`channel_id`** (batch) |
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
