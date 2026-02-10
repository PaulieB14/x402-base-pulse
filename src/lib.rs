//! x402 Base Pulse - Substreams v1.0.0
//!
//! Real-time analytics for the Coinbase x402 payment protocol on Base.
//!
//! Tracks every x402 settlement through the x402ExactPermit2Proxy contract,
//! correlates with USDC transfers, and computes payer, recipient, and
//! facilitator statistics.
//!
//! Module layers:
//! - Layer 1: Event extraction (map_x402_settlements, map_payment_transfers)
//! - Layer 2: State stores (payer/recipient/facilitator volume, counts, gas)
//! - Layer 3: Analytics (map_payer_stats, map_recipient_stats, map_facilitator_stats)
//! - Layer 4: SQL sink (db_out)

mod abi;
mod pb;

use abi::{decode_erc20_transfer, decode_proxy_event, format_address};
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
// =============================================

/// x402ExactPermit2Proxy - deterministic across all EVM chains via CREATE2
const X402_PROXY: [u8; 20] = hex!("4020615294c913F045dc10f0a5cdEbd86c280001");

/// USDC on Base mainnet
const USDC: [u8; 20] = hex!("833589fCD6eDb6E08f4c7C32D4f71b54bdA02913");

/// ERC-20 Transfer event signature
const TRANSFER_SIG: [u8; 32] =
    hex!("ddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef");

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

/// Extract x402 settlements by finding all transactions that emit events
/// from the x402ExactPermit2Proxy contract, then correlating with USDC
/// Transfer events in the same transaction.
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

        // Check if this transaction has any logs from the x402 proxy
        let has_proxy_event = receipt.logs.iter().any(|log| log.address == X402_PROXY);

        if !has_proxy_event {
            continue;
        }

        let facilitator = format_address(&trx.from);
        let gas_used = trx.gas_used.to_string();
        let gas_price = trx
            .gas_price
            .as_ref()
            .map(|p| proto_bigint_to_string(p))
            .unwrap_or_else(|| "0".to_string());

        // Collect USDC Transfer events from this transaction
        let usdc_transfers: Vec<_> = receipt
            .logs
            .iter()
            .filter(|log| {
                log.address == USDC
                    && log.topics.len() >= 3
                    && log.topics[0] == TRANSFER_SIG
            })
            .filter_map(|log| decode_erc20_transfer(log).map(|t| (log.index, t)))
            .collect();

        // Process each proxy event
        for log in &receipt.logs {
            if log.address != X402_PROXY {
                continue;
            }

            let proxy_event = decode_proxy_event(log);

            // Try to get payment details from the proxy event first,
            // then fall back to USDC transfer correlation
            let (payer, recipient, token, amount, settlement_type) =
                if let Some(ref evt) = proxy_event {
                    let payer = evt
                        .payer
                        .as_ref()
                        .map(|b| format_address(b))
                        .unwrap_or_default();
                    let recipient = evt
                        .recipient
                        .as_ref()
                        .map(|b| format_address(b))
                        .unwrap_or_default();
                    let token = evt
                        .token
                        .as_ref()
                        .map(|b| format_address(b))
                        .unwrap_or_default();
                    let amount = evt.amount.clone().unwrap_or_default();
                    let st = evt.settlement_type.clone();
                    (payer, recipient, token, amount, st)
                } else {
                    (
                        String::new(),
                        String::new(),
                        String::new(),
                        String::new(),
                        "unknown".to_string(),
                    )
                };

            // If proxy decode didn't yield payment details, use the USDC transfer
            let (final_payer, final_recipient, final_token, final_amount) = if !payer.is_empty()
                && payer != ZERO_ADDR
                && !amount.is_empty()
                && amount != "0"
            {
                (payer, recipient, token, amount)
            } else if let Some((_, ref transfer)) = usdc_transfers.first() {
                (
                    format_address(&transfer.from),
                    format_address(&transfer.to),
                    format_address(&USDC),
                    transfer.amount.clone(),
                )
            } else {
                (
                    facilitator.clone(),
                    String::new(),
                    format_address(&USDC),
                    "0".to_string(),
                )
            };

            let settlement = x402::Settlement {
                id: format!("{}-{}", Hex(&trx.hash).to_string(), log.index),
                tx_hash: Hex(&trx.hash).to_string(),
                log_index: log.index,
                block_number: blk.number,
                timestamp: Some(blk.timestamp().clone()),
                payer: final_payer,
                recipient: final_recipient,
                token: final_token,
                amount: final_amount,
                settlement_type,
                facilitator: facilitator.clone(),
                gas_used: gas_used.clone(),
                gas_price: gas_price.clone(),
                proxy_event_sig: proxy_event
                    .as_ref()
                    .map(|e| e.event_sig.clone())
                    .unwrap_or_default(),
                proxy_event_data: proxy_event
                    .as_ref()
                    .map(|e| e.raw_data.clone())
                    .unwrap_or_default(),
            };

            settlements.settlements.push(settlement);
        }
    }

    Ok(settlements)
}

/// Extract USDC Transfer events that occur in transactions involving the
/// x402 proxy. This gives us the raw payment flow data.
#[substreams::handlers::map]
fn map_payment_transfers(
    blk: eth::Block,
) -> Result<x402::PaymentTransfers, substreams::errors::Error> {
    let mut transfers = x402::PaymentTransfers {
        block_number: blk.number,
        ..Default::default()
    };

    for trx in blk.transaction_traces.iter() {
        let receipt = match trx.receipt.as_ref() {
            Some(r) => r,
            None => continue,
        };

        let has_proxy = receipt.logs.iter().any(|log| log.address == X402_PROXY);

        for log in &receipt.logs {
            if log.address != USDC {
                continue;
            }
            if log.topics.len() < 3 || log.topics[0] != TRANSFER_SIG {
                continue;
            }

            if let Some(decoded) = decode_erc20_transfer(log) {
                let from = format_address(&decoded.from);
                let to = format_address(&decoded.to);

                if from == ZERO_ADDR && to == ZERO_ADDR {
                    continue;
                }

                transfers.transfers.push(x402::PaymentTransfer {
                    tx_hash: Hex(&trx.hash).to_string(),
                    log_index: log.index,
                    block_number: blk.number,
                    timestamp: Some(blk.timestamp().clone()),
                    from_address: from,
                    to_address: to,
                    amount: decoded.amount,
                    token: format_address(&USDC),
                    facilitator: format_address(&trx.from),
                    is_x402_related: has_proxy,
                });
            }
        }
    }

    Ok(transfers)
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
            .set("proxy_event_sig", &s.proxy_event_sig)
            .set("proxy_event_data", &s.proxy_event_data);
    }

    // Upsert payer stats (create_row - SQL sink handles ON CONFLICT)
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
