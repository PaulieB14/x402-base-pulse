//! x402 Base Pulse - Substreams v2.0.0
//!
//! Real-time analytics for the Coinbase x402 payment protocol on Base.
//!
//! Detects x402 settlements through two mechanisms per the x402 protocol
//! docs (https://docs.cdp.coinbase.com/x402/core-concepts/how-it-works):
//!
//! 1. **EIP-3009 (primary)**: Facilitators settle payments by calling
//!    `transferWithAuthorization` on USDC (EIP-3009 compliant). Each call
//!    emits `AuthorizationUsed(address indexed authorizer, bytes32 indexed nonce)`
//!    paired with a `Transfer(address,address,uint256)` event.
//!
//! 2. **Permit2 proxy (secondary)**: `Settled()` / `SettledWithPermit()` events
//!    from the x402ExactPermit2Proxy contract (currently testnet only).
//!
//! Module layers:
//! - Layer 1: Event extraction (map_x402_settlements)
//! - Layer 2: State stores (payer/recipient/facilitator volume, counts, gas)
//! - Layer 3: Analytics (map_payer_stats, map_recipient_stats, map_facilitator_stats)
//! - Layer 4: SQL sink (db_out)

mod abi;
mod pb;

use abi::{
    decode_authorization_used, decode_erc20_transfer, format_address,
    is_settled_event, is_settled_with_permit_event,
};
use hex_literal::hex;
use pb::x402::v1 as x402;
use substreams::prelude::*;
use substreams::scalar::BigInt;
use substreams::store::{StoreAddBigInt, StoreAddInt64, StoreGet};
use substreams::Hex;
use substreams_database_change::pb::database::DatabaseChanges;
use substreams_database_change::tables::Tables;
use substreams_ethereum::pb::eth::v2 as eth;

// =============================================
// Contract addresses on Base mainnet
// Per: https://docs.cdp.coinbase.com/x402/network-support
// =============================================

/// USDC on Base mainnet - EIP-3009 compliant token
const USDC: [u8; 20] = hex!("833589fCD6eDb6E08f4c7C32D4f71b54bdA02913");

/// x402ExactPermit2Proxy - deterministic across all EVM chains via CREATE2
/// Secondary detection path (currently active on testnet only)
const X402_PROXY: [u8; 20] = hex!("4020615294c913F045dc10f0a5cdEbd86c280001");

/// x402UptoPermit2Proxy - secondary proxy for "upto" payment scheme
const X402_UPTO_PROXY: [u8; 20] = hex!("4020633461b2895a48930Ff97eE8fCdE8E520002");

// Null / zero address
const ZERO_ADDR: &str = "0x0000000000000000000000000000000000000000";

substreams_ethereum::init!();

/// Convert Unix timestamp seconds to PostgreSQL TIMESTAMP format
fn unix_to_timestamp(secs: i64) -> String {
    let days_since_epoch = secs / 86400;
    let time_of_day = secs % 86400;
    let hours = time_of_day / 3600;
    let minutes = (time_of_day % 3600) / 60;
    let seconds = time_of_day % 60;

    let mut days = days_since_epoch;
    let mut year = 1970i64;
    loop {
        let diy = if is_leap_year(year) { 366 } else { 365 };
        if days < diy {
            break;
        }
        days -= diy;
        year += 1;
    }

    let dim: [i64; 12] = if is_leap_year(year) {
        [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    };

    let mut month = 1;
    for &d in &dim {
        if days < d {
            break;
        }
        days -= d;
        month += 1;
    }
    let day = days + 1;

    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        year, month, day, hours, minutes, seconds
    )
}

fn is_leap_year(y: i64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || (y % 400 == 0)
}

/// Extract gas_price from a protobuf BigInt (big-endian signed bytes) as a string
fn proto_bigint_to_string(bi: &eth::BigInt) -> String {
    if bi.bytes.is_empty() {
        return "0".to_string();
    }
    let val = num_bigint::BigInt::from_signed_bytes_be(&bi.bytes);
    val.to_string()
}

// =============================================
// LAYER 1: Event Extraction
// =============================================

/// Extract x402 settlements by detecting EIP-3009 AuthorizationUsed events
/// on the USDC contract.
///
/// Per the x402 protocol (https://docs.cdp.coinbase.com/x402/core-concepts/how-it-works),
/// facilitators settle payments by calling `transferWithAuthorization` on USDC.
/// Each `AuthorizationUsed(address indexed authorizer, bytes32 indexed nonce)`
/// event is paired with its corresponding `Transfer(address,address,uint256)`
/// event to capture payer, recipient, and amount.
///
/// Also detects Permit2 proxy settlements (Settled / SettledWithPermit) from
/// the x402ExactPermit2Proxy contract for the newer settlement path.
#[substreams::handlers::map]
fn map_x402_settlements(blk: eth::Block) -> Result<x402::Settlements, substreams::errors::Error> {
    let mut settlements = x402::Settlements {
        block_number: blk.number,
        block_timestamp: Some(blk.timestamp().clone()),
        ..Default::default()
    };

    for trx in blk.transaction_traces.iter() {
        let receipt = match trx.receipt.as_ref() {
            Some(r) => r,
            None => continue,
        };

        // -----------------------------------------------
        // Path 1: EIP-3009 AuthorizationUsed on USDC
        // Facilitator calls transferWithAuthorization on USDC.
        // USDC emits AuthorizationUsed + Transfer events.
        // -----------------------------------------------
        let auth_events: Vec<_> = receipt
            .logs
            .iter()
            .filter(|log| log.address == USDC)
            .filter_map(|log| decode_authorization_used(log))
            .collect();

        if !auth_events.is_empty() {
            // Collect Transfer events from USDC in this transaction
            let transfer_events: Vec<_> = receipt
                .logs
                .iter()
                .filter(|log| log.address == USDC)
                .filter_map(|log| decode_erc20_transfer(log))
                .collect();

            let facilitator = format_address(&trx.from);
            let gas_used = trx.gas_used.to_string();
            let gas_price = trx
                .gas_price
                .as_ref()
                .map(|p| proto_bigint_to_string(p))
                .unwrap_or_else(|| "0".to_string());

            // Check if this tx also has proxy events (hybrid detection)
            let has_proxy_settled = receipt.logs.iter().any(|log| {
                (log.address == X402_PROXY || log.address == X402_UPTO_PROXY)
                    && (is_settled_event(log) || is_settled_with_permit_event(log))
            });

            for auth in &auth_events {
                // Find the corresponding Transfer event for this authorization.
                // In USDC's implementation, transferWithAuthorization emits
                // AuthorizationUsed then Transfer, so we look for a Transfer
                // where from == authorizer with log_index > auth.log_index.
                let transfer = transfer_events
                    .iter()
                    .filter(|t| t.from == auth.authorizer && t.log_index > auth.log_index)
                    .min_by_key(|t| t.log_index);

                let (payer, recipient, amount) = if let Some(t) = transfer {
                    (
                        format_address(&auth.authorizer),
                        format_address(&t.to),
                        t.amount.clone(),
                    )
                } else {
                    // AuthorizationUsed without a matching Transfer (shouldn't happen
                    // in normal USDC operation, but handle gracefully)
                    (format_address(&auth.authorizer), String::new(), "0".to_string())
                };

                let settlement_type = if has_proxy_settled {
                    "eip3009_proxy".to_string()
                } else {
                    "eip3009".to_string()
                };

                let nonce = Hex(&auth.nonce).to_string();

                settlements.settlements.push(x402::Settlement {
                    id: format!("{}-{}", Hex(&trx.hash).to_string(), auth.log_index),
                    tx_hash: Hex(&trx.hash).to_string(),
                    log_index: auth.log_index,
                    block_number: blk.number,
                    timestamp: Some(blk.timestamp().clone()),
                    payer,
                    recipient,
                    token: format_address(&USDC),
                    amount,
                    settlement_type,
                    facilitator: facilitator.clone(),
                    gas_used: gas_used.clone(),
                    gas_price: gas_price.clone(),
                    nonce,
                });
            }

            continue; // EIP-3009 path handled this tx
        }

        // -----------------------------------------------
        // Path 2: Permit2 proxy (Settled / SettledWithPermit)
        // When x402ExactPermit2Proxy deploys on mainnet, it emits
        // parameterless Settled() or SettledWithPermit() events.
        // We correlate with USDC Transfer events in the same tx.
        // -----------------------------------------------
        let proxy_events: Vec<_> = receipt
            .logs
            .iter()
            .filter(|log| {
                (log.address == X402_PROXY || log.address == X402_UPTO_PROXY)
                    && (is_settled_event(log) || is_settled_with_permit_event(log))
            })
            .collect();

        if proxy_events.is_empty() {
            continue;
        }

        // Collect USDC transfers for correlation
        let usdc_transfers: Vec<_> = receipt
            .logs
            .iter()
            .filter(|log| log.address == USDC)
            .filter_map(|log| decode_erc20_transfer(log))
            .collect();

        let facilitator = format_address(&trx.from);
        let gas_used = trx.gas_used.to_string();
        let gas_price = trx
            .gas_price
            .as_ref()
            .map(|p| proto_bigint_to_string(p))
            .unwrap_or_else(|| "0".to_string());

        for proxy_log in &proxy_events {
            let settlement_type = if is_settled_with_permit_event(proxy_log) {
                "settled_with_permit".to_string()
            } else {
                "settled".to_string()
            };

            // Get payment details from the closest USDC Transfer
            let (payer, recipient, amount) = if let Some(t) = usdc_transfers.first() {
                (
                    format_address(&t.from),
                    format_address(&t.to),
                    t.amount.clone(),
                )
            } else {
                (facilitator.clone(), String::new(), "0".to_string())
            };

            settlements.settlements.push(x402::Settlement {
                id: format!("{}-{}", Hex(&trx.hash).to_string(), proxy_log.index),
                tx_hash: Hex(&trx.hash).to_string(),
                log_index: proxy_log.index,
                block_number: blk.number,
                timestamp: Some(blk.timestamp().clone()),
                payer,
                recipient,
                token: format_address(&USDC),
                amount,
                settlement_type,
                facilitator: facilitator.clone(),
                gas_used: gas_used.clone(),
                gas_price: gas_price.clone(),
                nonce: String::new(),
            });
        }
    }

    Ok(settlements)
}

// =============================================
// LAYER 2: State Stores
// =============================================

/// Accumulate total payment volume per payer
#[substreams::handlers::store]
fn store_payer_volume(settlements: x402::Settlements, store: StoreAddBigInt) {
    for s in settlements.settlements {
        if s.payer.is_empty() || s.payer == ZERO_ADDR {
            continue;
        }
        let amount = BigInt::try_from(&s.amount).unwrap_or_else(|_| BigInt::zero());
        store.add(0, &s.payer.to_lowercase(), &amount);
    }
}

/// Count total payments per payer
#[substreams::handlers::store]
fn store_payer_count(settlements: x402::Settlements, store: StoreAddInt64) {
    for s in settlements.settlements {
        if s.payer.is_empty() || s.payer == ZERO_ADDR {
            continue;
        }
        store.add(0, &s.payer.to_lowercase(), 1);
    }
}

/// Accumulate total revenue per recipient (resource server)
#[substreams::handlers::store]
fn store_recipient_volume(settlements: x402::Settlements, store: StoreAddBigInt) {
    for s in settlements.settlements {
        if s.recipient.is_empty() || s.recipient == ZERO_ADDR {
            continue;
        }
        let amount = BigInt::try_from(&s.amount).unwrap_or_else(|_| BigInt::zero());
        store.add(0, &s.recipient.to_lowercase(), &amount);
    }
}

/// Count total payments per recipient
#[substreams::handlers::store]
fn store_recipient_count(settlements: x402::Settlements, store: StoreAddInt64) {
    for s in settlements.settlements {
        if s.recipient.is_empty() || s.recipient == ZERO_ADDR {
            continue;
        }
        store.add(0, &s.recipient.to_lowercase(), 1);
    }
}

/// Accumulate total volume settled per facilitator
#[substreams::handlers::store]
fn store_facilitator_volume(settlements: x402::Settlements, store: StoreAddBigInt) {
    for s in settlements.settlements {
        if s.facilitator.is_empty() {
            continue;
        }
        let amount = BigInt::try_from(&s.amount).unwrap_or_else(|_| BigInt::zero());
        store.add(0, &s.facilitator.to_lowercase(), &amount);
    }
}

/// Count total settlements per facilitator
#[substreams::handlers::store]
fn store_facilitator_count(settlements: x402::Settlements, store: StoreAddInt64) {
    for s in settlements.settlements {
        if s.facilitator.is_empty() {
            continue;
        }
        store.add(0, &s.facilitator.to_lowercase(), 1);
    }
}

/// Accumulate total gas cost per facilitator (gas_used * gas_price in wei)
#[substreams::handlers::store]
fn store_facilitator_gas(settlements: x402::Settlements, store: StoreAddBigInt) {
    for s in settlements.settlements {
        if s.facilitator.is_empty() {
            continue;
        }
        let gas_used = BigInt::try_from(&s.gas_used).unwrap_or_else(|_| BigInt::zero());
        let gas_price = BigInt::try_from(&s.gas_price).unwrap_or_else(|_| BigInt::zero());
        let gas_cost = gas_used * gas_price;
        store.add(0, &s.facilitator.to_lowercase(), &gas_cost);
    }
}

// =============================================
// LAYER 3: Analytics
// =============================================

/// Compute aggregated payer statistics
#[substreams::handlers::map]
fn map_payer_stats(
    settlements: x402::Settlements,
    volume_deltas: Deltas<DeltaBigInt>,
    count_store: StoreGetInt64,
) -> Result<x402::PayerStats, substreams::errors::Error> {
    let mut stats = x402::PayerStats {
        block_number: settlements.block_number,
        ..Default::default()
    };

    for delta in volume_deltas.deltas {
        let payer = delta.key.clone();
        let total_payments = count_store.get_last(&payer).unwrap_or(0) as u64;

        stats.stats.push(x402::PayerStat {
            payer_address: payer,
            total_spent: delta.new_value.to_string(),
            total_payments,
            first_payment_at: None,
            last_payment_at: settlements.block_timestamp.clone(),
        });
    }

    Ok(stats)
}

/// Compute aggregated recipient (resource server) statistics
#[substreams::handlers::map]
fn map_recipient_stats(
    settlements: x402::Settlements,
    volume_deltas: Deltas<DeltaBigInt>,
    count_store: StoreGetInt64,
) -> Result<x402::RecipientStats, substreams::errors::Error> {
    let mut stats = x402::RecipientStats {
        block_number: settlements.block_number,
        ..Default::default()
    };

    for delta in volume_deltas.deltas {
        let recipient = delta.key.clone();
        let total_payments = count_store.get_last(&recipient).unwrap_or(0) as u64;

        stats.stats.push(x402::RecipientStat {
            recipient_address: recipient,
            total_received: delta.new_value.to_string(),
            total_payments,
            first_payment_at: None,
            last_payment_at: settlements.block_timestamp.clone(),
        });
    }

    Ok(stats)
}

/// Compute facilitator economics
#[substreams::handlers::map]
fn map_facilitator_stats(
    settlements: x402::Settlements,
    volume_deltas: Deltas<DeltaBigInt>,
    count_store: StoreGetInt64,
    gas_store: StoreGetBigInt,
) -> Result<x402::FacilitatorStats, substreams::errors::Error> {
    let mut stats = x402::FacilitatorStats {
        block_number: settlements.block_number,
        ..Default::default()
    };

    for delta in volume_deltas.deltas {
        let facilitator = delta.key.clone();
        let total_settlements = count_store.get_last(&facilitator).unwrap_or(0) as u64;
        let total_gas = gas_store
            .get_last(&facilitator)
            .map(|v| v.to_string())
            .unwrap_or_else(|| "0".to_string());

        stats.stats.push(x402::FacilitatorStat {
            facilitator_address: facilitator,
            total_settlements,
            total_volume_settled: delta.new_value.to_string(),
            total_gas_spent: total_gas,
            first_settlement_at: None,
            last_settlement_at: settlements.block_timestamp.clone(),
        });
    }

    Ok(stats)
}

// =============================================
// LAYER 4: SQL Sink
// =============================================

/// Output database changes for PostgreSQL
#[substreams::handlers::map]
fn db_out(
    params: String,
    settlements: x402::Settlements,
    payer_stats: x402::PayerStats,
    recipient_stats: x402::RecipientStats,
    facilitator_stats: x402::FacilitatorStats,
) -> Result<DatabaseChanges, substreams::errors::Error> {
    let mut tables = Tables::new();

    // Parse min_amount param
    let min_amount: i64 = params
        .split('=')
        .nth(1)
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);

    // Insert settlements
    for s in settlements.settlements {
        let amount: i64 = s.amount.parse().unwrap_or(0);
        if amount < min_amount {
            continue;
        }

        let timestamp = s
            .timestamp
            .as_ref()
            .map(|t| unix_to_timestamp(t.seconds))
            .unwrap_or_else(|| "1970-01-01 00:00:00".to_string());

        tables
            .create_row("settlements", &s.id)
            .set("block_number", s.block_number)
            .set("block_timestamp", &timestamp)
            .set("tx_hash", &s.tx_hash)
            .set("log_index", s.log_index)
            .set("payer", &s.payer)
            .set("recipient", &s.recipient)
            .set("token", &s.token)
            .set("amount", &s.amount)
            .set("settlement_type", &s.settlement_type)
            .set("facilitator", &s.facilitator)
            .set("gas_used", &s.gas_used)
            .set("gas_price", &s.gas_price)
            .set("nonce", &s.nonce);
    }

    // Upsert payer stats
    for stat in payer_stats.stats {
        tables
            .create_row("payers", &stat.payer_address)
            .set("total_spent", stat.total_spent.as_str())
            .set("total_payments", stat.total_payments as i64);
    }

    // Upsert recipient stats
    for stat in recipient_stats.stats {
        tables
            .create_row("recipients", &stat.recipient_address)
            .set("total_received", stat.total_received.as_str())
            .set("total_payments", stat.total_payments as i64);
    }

    // Upsert facilitator stats
    for stat in facilitator_stats.stats {
        tables
            .create_row("facilitators", &stat.facilitator_address)
            .set("total_settlements", stat.total_settlements as i64)
            .set("total_volume_settled", stat.total_volume_settled.as_str())
            .set("total_gas_spent", stat.total_gas_spent.as_str());
    }

    Ok(tables.to_database_changes())
}
